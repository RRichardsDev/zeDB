//! Read-only git awareness for migration repos (docs/PHASE-3.md M0).
//!
//! Shells out to the user's `git`, so configuration, auth, and worktree
//! semantics are exactly theirs. Everything here observes; nothing here
//! mutates a repo.

use std::path::Path;
use std::process::Command;

/// A snapshot of a checkout's git state, from `git status --porcelain=v2`.
///
/// Ahead/behind counts compare against the local remote-tracking ref, so
/// they are as fresh as the user's last fetch; zeDB does not fetch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitStatus {
    /// The checked-out branch, or None when HEAD is detached.
    pub branch: Option<String>,
    /// Uncommitted entries: staged, unstaged, unmerged, and untracked.
    pub dirty: usize,
    /// (ahead, behind) relative to upstream; None when no upstream is set.
    pub ahead_behind: Option<(u32, u32)>,
}

impl GitStatus {
    /// One-line form for status chips: `main`, `main*`, `main* +1 -2`,
    /// `detached`.
    pub fn summary(&self) -> String {
        let mut out = self
            .branch
            .clone()
            .unwrap_or_else(|| "detached".to_string());
        if self.dirty > 0 {
            out.push('*');
        }
        if let Some((ahead, behind)) = self.ahead_behind {
            if ahead > 0 {
                out.push_str(&format!(" +{ahead}"));
            }
            if behind > 0 {
                out.push_str(&format!(" -{behind}"));
            }
        }
        out
    }

    /// True when deploying from this checkout deserves a warning.
    pub fn stale(&self) -> bool {
        !self.deploy_warnings().is_empty()
    }

    /// Why a deploy from this checkout is suspect: the migrations being
    /// applied may not be the migrations that were reviewed and pushed.
    pub fn deploy_warnings(&self) -> Vec<String> {
        let mut warnings = Vec::new();
        if self.dirty > 0 {
            warnings.push(format!("checkout has {} uncommitted change(s)", self.dirty));
        }
        if let Some((_, behind)) = self.ahead_behind {
            if behind > 0 {
                warnings.push(format!(
                    "checkout is behind its upstream by {behind} commit(s)"
                ));
            }
        }
        if self.branch.is_none() {
            warnings.push("checkout is on a detached HEAD".to_string());
        }
        warnings
    }
}

/// Read the git state of `root`. None when `root` is not inside a git
/// work tree or `git` itself is unavailable; callers treat that as
/// "nothing to show", not an error, because a migration repo does not
/// have to be a git checkout to be opened.
pub fn read_git_status(root: &Path) -> Option<GitStatus> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["status", "--porcelain=v2", "--branch"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(parse_porcelain_v2(&String::from_utf8_lossy(&output.stdout)))
}

/// Paths with uncommitted changes (staged, unstaged, or untracked),
/// relative to the repo root. None outside a git work tree.
pub fn changed_paths(root: &Path) -> Option<Vec<String>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["status", "--porcelain=v2"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut paths = Vec::new();
    for line in text.lines() {
        let path = if let Some(rest) = line.strip_prefix("? ") {
            Some(rest.to_string())
        } else if line.starts_with("1 ") || line.starts_with("u ") {
            // `1 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <path>` (unmerged
            // lines carry two extra hash fields before the path).
            let skip = if line.starts_with("1 ") { 8 } else { 10 };
            line.splitn(skip + 1, ' ').nth(skip).map(str::to_string)
        } else if line.starts_with("2 ") {
            // Renames: `... <Xscore> <path>\t<origPath>`.
            line.splitn(10, ' ')
                .nth(9)
                .and_then(|tail| tail.split('\t').next())
                .map(str::to_string)
        } else {
            None
        };
        if let Some(path) = path {
            paths.push(path);
        }
    }
    paths.sort();
    paths.dedup();
    Some(paths)
}

fn run_git(root: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|error| format!("could not run git: {error}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if output.status.success() {
        Ok(format!("{}{}", stdout.trim(), stderr.trim()))
    } else {
        // git explains itself well; hand its words to the user intact.
        Err(if stderr.trim().is_empty() {
            stdout.trim().to_string()
        } else {
            stderr.trim().to_string()
        })
    }
}

/// Stage exactly `pathspecs` and commit them with `message`; returns the
/// short commit hash. The commit names the pathspecs explicitly, so
/// anything else already staged stays out of it.
pub fn commit_paths(root: &Path, pathspecs: &[String], message: &str) -> Result<String, String> {
    if pathspecs.is_empty() {
        return Err("nothing to commit".into());
    }
    let mut add = vec!["add", "--"];
    add.extend(pathspecs.iter().map(String::as_str));
    run_git(root, &add)?;
    let mut commit = vec!["commit", "-m", message, "--"];
    commit.extend(pathspecs.iter().map(String::as_str));
    run_git(root, &commit)?;
    run_git(root, &["rev-parse", "--short", "HEAD"])
}

/// Push the current branch with the user's own git auth. Failures (no
/// upstream, auth, non-fast-forward) return git's message verbatim.
pub fn push(root: &Path) -> Result<String, String> {
    run_git(root, &["push"])
}

/// Whether `url` is a reachable git remote for this user's own git
/// auth (ssh keys, credential helpers). Never prompts: batch-mode ssh
/// and disabled terminal prompts make an auth wall read as "no".
pub fn remote_exists(url: &str) -> bool {
    Command::new("git")
        .args(["ls-remote", "--exit-code", url, "HEAD"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_SSH_COMMAND", "ssh -oBatchMode=yes -oConnectTimeout=5")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

/// Push, setting the upstream first when the checkout has none (the
/// first push into a fresh clone of an empty remote).
pub fn push_setting_upstream(root: &Path) -> Result<String, String> {
    if has_upstream(root) {
        push(root)
    } else {
        run_git(root, &["push", "-u", "origin", "HEAD"])
    }
}

/// Whether the checkout has an upstream ref to pull from. A clone of a
/// still-empty remote does not, and pulling it would only produce a
/// scary no-such-ref error for a perfectly normal state.
pub fn has_upstream(root: &Path) -> bool {
    run_git(root, &["rev-parse", "--verify", "--quiet", "@{u}"]).is_ok()
}

/// Pull the current branch, fast-forward only: zeDB never merges or
/// resolves conflicts. Diverged history comes back as git's own
/// message and the checkout is left exactly as it was.
pub fn pull(root: &Path) -> Result<String, String> {
    run_git(root, &["pull", "--ff-only"])
}

/// Does this look like a git remote URL rather than a local path?
pub fn is_remote_url(text: &str) -> bool {
    let text = text.trim();
    text.starts_with("git@")
        || text.starts_with("ssh://")
        || text.starts_with("git://")
        || ((text.starts_with("http://") || text.starts_with("https://"))
            && text.trim_end_matches('/').ends_with(".git"))
}

/// The checkout directory name a remote URL clones into: its last path
/// segment minus `.git`, sanitized to filesystem-safe characters.
pub fn clone_directory_name(url: &str) -> String {
    let tail = url
        .trim()
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .rsplit(['/', ':'])
        .next()
        .unwrap_or("repo");
    let name: String = tail
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '-'
            }
        })
        .collect();
    if name.is_empty() {
        "repo".into()
    } else {
        name
    }
}

/// Where zeDB keeps checkouts it cloned itself.
pub fn managed_repos_dir() -> Option<std::path::PathBuf> {
    dirs::data_local_dir().map(|dir| dir.join("zedb").join("repos"))
}

/// Clone `url` into `dest` with the user's own git (and therefore their
/// auth: ssh agent, credential helper). Git's message comes back
/// verbatim on failure.
pub fn clone_repo(url: &str, dest: &Path) -> Result<(), String> {
    if dest.exists() {
        return Err(format!("{} already exists", dest.display()));
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let output = Command::new("git")
        .arg("clone")
        .arg(url)
        .arg(dest)
        .output()
        .map_err(|error| format!("could not run git: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

fn parse_porcelain_v2(text: &str) -> GitStatus {
    let mut branch = None;
    let mut ahead_behind = None;
    let mut dirty = 0;
    for line in text.lines() {
        if let Some(head) = line.strip_prefix("# branch.head ") {
            if head != "(detached)" {
                branch = Some(head.to_string());
            }
        } else if let Some(ab) = line.strip_prefix("# branch.ab ") {
            let mut parts = ab.split_whitespace();
            let ahead = parts
                .next()
                .and_then(|part| part.strip_prefix('+'))
                .and_then(|n| n.parse().ok());
            let behind = parts
                .next()
                .and_then(|part| part.strip_prefix('-'))
                .and_then(|n| n.parse().ok());
            if let (Some(ahead), Some(behind)) = (ahead, behind) {
                ahead_behind = Some((ahead, behind));
            }
        } else if line.starts_with("1 ")
            || line.starts_with("2 ")
            || line.starts_with("u ")
            || line.starts_with("? ")
        {
            dirty += 1;
        }
    }
    GitStatus {
        branch,
        dirty,
        ahead_behind,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_clean_tracking_branch() {
        let status = parse_porcelain_v2(
            "# branch.oid abc\n# branch.head main\n# branch.upstream origin/main\n# branch.ab +0 -0\n",
        );
        assert_eq!(status.branch.as_deref(), Some("main"));
        assert_eq!(status.dirty, 0);
        assert_eq!(status.ahead_behind, Some((0, 0)));
        assert!(!status.stale());
        assert_eq!(status.summary(), "main");
    }

    #[test]
    fn parses_dirty_and_behind() {
        let status = parse_porcelain_v2(
            "# branch.oid abc\n# branch.head main\n# branch.upstream origin/main\n# branch.ab +1 -2\n1 .M N... 100644 100644 100644 abc def zedb.toml\n? new-file\n",
        );
        assert_eq!(status.dirty, 2);
        assert_eq!(status.ahead_behind, Some((1, 2)));
        assert!(status.stale());
        assert_eq!(status.summary(), "main* +1 -2");
        let warnings = status.deploy_warnings();
        assert_eq!(warnings.len(), 2);
        assert!(warnings[0].contains("2 uncommitted"));
        assert!(warnings[1].contains("behind its upstream by 2"));
    }

    #[test]
    fn parses_detached_without_upstream() {
        let status = parse_porcelain_v2("# branch.oid abc\n# branch.head (detached)\n");
        assert_eq!(status.branch, None);
        assert_eq!(status.ahead_behind, None);
        assert_eq!(status.summary(), "detached");
        assert!(status.stale());
    }

    #[test]
    fn reads_a_real_repo() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_git_status(dir.path()).is_none());
        let run = |args: &[&str]| {
            let ok = Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(args)
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?} failed");
        };
        run(&["init", "-b", "main"]);
        std::fs::write(dir.path().join("a.txt"), "hello").unwrap();
        let status = read_git_status(dir.path()).unwrap();
        assert_eq!(status.branch.as_deref(), Some("main"));
        assert_eq!(status.dirty, 1);
        assert_eq!(status.ahead_behind, None);
    }

    #[test]
    fn commits_exactly_the_named_paths() {
        let dir = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            let ok = Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(args)
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?} failed");
        };
        run(&["init", "-b", "main"]);
        run(&["config", "user.email", "test@example.com"]);
        run(&["config", "user.name", "Test"]);
        std::fs::create_dir_all(dir.path().join("migrations")).unwrap();
        std::fs::write(dir.path().join("migrations/00100.sql"), "SELECT 1;").unwrap();
        std::fs::write(dir.path().join("unrelated.txt"), "keep out").unwrap();

        let changed = changed_paths(dir.path()).unwrap();
        assert_eq!(changed, vec!["migrations/", "unrelated.txt"]);

        let hash = commit_paths(dir.path(), &["migrations".to_string()], "add 00100").unwrap();
        assert!(!hash.is_empty());
        let status = read_git_status(dir.path()).unwrap();
        assert_eq!(status.dirty, 1, "unrelated.txt must stay uncommitted");
        assert!(push(dir.path()).is_err(), "push without a remote must fail");
    }

    #[test]
    fn recognizes_remote_urls() {
        for url in [
            "git@github.com:acme/fleet-ddl.git",
            "ssh://git@host/x.git",
            "https://github.com/acme/fleet-ddl.git",
            "git://host/x",
        ] {
            assert!(is_remote_url(url), "{url}");
        }
        for path in [
            "~/code/repo",
            "/tmp/repo",
            "https://example.com/page",
            "repo",
        ] {
            assert!(!is_remote_url(path), "{path}");
        }
        assert_eq!(
            clone_directory_name("git@github.com:acme/fleet-ddl.git"),
            "fleet-ddl"
        );
        assert_eq!(clone_directory_name("https://host/team/x.y.git/"), "x.y");
    }

    #[test]
    fn clones_with_the_users_git() {
        let source = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            let ok = Command::new("git")
                .arg("-C")
                .arg(source.path())
                .args(args)
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?} failed");
        };
        run(&["init", "-b", "main"]);
        run(&["config", "user.email", "test@example.com"]);
        run(&["config", "user.name", "Test"]);
        std::fs::write(source.path().join("zedb.toml"), "format = 1\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-m", "seed"]);

        let target = tempfile::tempdir().unwrap();
        let dest = target.path().join("clone");
        clone_repo(source.path().to_str().unwrap(), &dest).unwrap();
        assert!(dest.join(".git").is_dir());
        assert!(dest.join("zedb.toml").is_file());
        assert!(
            clone_repo(source.path().to_str().unwrap(), &dest).is_err(),
            "cloning onto an existing directory must refuse"
        );
    }
}
