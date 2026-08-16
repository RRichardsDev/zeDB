//! Read-only git awareness for migration repos.
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

/// The executable git invokes as GIT_ASKPASS for HTTPS remotes on
/// known hosts: the app binary in a hidden answer mode that reads the
/// token from the Keychain. Unset (the CLI, tests), git behaves
/// exactly as before: the user's own auth.
static AUTH_BROKER: std::sync::OnceLock<Option<std::path::PathBuf>> = std::sync::OnceLock::new();

pub fn set_auth_broker(exe: Option<std::path::PathBuf>) {
    let _ = AUTH_BROKER.set(exe);
}

/// Env overrides that route an HTTPS github.com/gitlab.com remote's
/// credentials through the broker; empty for everything else (SSH
/// stays the user's own git).
fn auth_envs(url: &str) -> Vec<(String, String)> {
    let Some(Some(broker)) = AUTH_BROKER.get() else {
        return Vec::new();
    };
    if !url.starts_with("https://") {
        return Vec::new();
    }
    let host = url
        .trim_start_matches("https://")
        .split(['/', '@'])
        .next()
        .unwrap_or_default();
    if host != "github.com" && host != "gitlab.com" {
        return Vec::new();
    }
    vec![
        ("GIT_ASKPASS".into(), broker.display().to_string()),
        ("ZEDB_GIT_ASKPASS".into(), "1".into()),
        ("ZEDB_GIT_HOST".into(), host.to_string()),
        ("GIT_TERMINAL_PROMPT".into(), "0".into()),
    ]
}

/// The auth envs for a checkout, from its origin URL.
fn auth_envs_for_root(root: &Path) -> Vec<(String, String)> {
    let url = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["config", "--get", "remote.origin.url"])
        .output()
        .ok()
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_default();
    auth_envs(&url)
}

fn run_git(root: &Path, args: &[&str]) -> Result<String, String> {
    run_git_env(root, args, &[])
}

fn run_git_env(root: &Path, args: &[&str], envs: &[(String, String)]) -> Result<String, String> {
    let mut command = Command::new("git");
    command.arg("-C").arg(root);
    if !envs.is_empty() {
        // Configured credential helpers (macOS ships osxkeychain)
        // outrank GIT_ASKPASS and answer with whatever account they
        // stored last; clear the helper list so the broker is the
        // only voice for this run.
        command.args(["-c", "credential.helper="]);
    }
    let output = command
        .args(args)
        .envs(
            envs.iter()
                .map(|(key, value)| (key.as_str(), value.as_str())),
        )
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
    run_git_env(root, &["push"], &auth_envs_for_root(root))
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
        run_git_env(
            root,
            &["push", "-u", "origin", "HEAD"],
            &auth_envs_for_root(root),
        )
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
    run_git_env(root, &["pull", "--ff-only"], &auth_envs_for_root(root))
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
/// Boil git's stderr down to the lines that say something: the
/// `remote:` banner noise (empty lines, ==== rules) drops, prefixes
/// strip, and ERROR/fatal lines lead.
pub fn summarize_git_error(raw: &str) -> String {
    let mut meaningful: Vec<String> = Vec::new();
    for line in raw.lines() {
        let line = line.trim();
        let line = line.strip_prefix("remote:").unwrap_or(line).trim();
        if line.is_empty() || line.chars().all(|c| c == '=' || c == '-') {
            continue;
        }
        if line.starts_with("Cloning into") || line.starts_with("clone failed") {
            continue;
        }
        meaningful.push(line.to_string());
    }
    let lead = meaningful
        .iter()
        .position(|line| line.contains("ERROR") || line.contains("fatal:"));
    let summary = match lead {
        Some(index) => meaningful[index..].join(" "),
        None => meaningful.join(" "),
    };
    let mut summary = summary.trim().to_string();
    if summary.len() > 240 {
        let mut cut = 240;
        while !summary.is_char_boundary(cut) {
            cut -= 1;
        }
        summary.truncate(cut);
        summary.push('\u{2026}');
    }
    if summary.is_empty() {
        raw.trim().to_string()
    } else {
        summary
    }
}

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
    // A just-created repository 404s while the host provisions it
    // (GitLab reliably, GitHub occasionally). Not-found clones retry
    // optimistically with exponential backoff, up to ~10s in total;
    // real not-founds surface after the last attempt. Runs on
    // blocking pools, so the sleeps stall no UI.
    let mut delay = std::time::Duration::from_millis(500);
    let mut waited = std::time::Duration::ZERO;
    let budget = std::time::Duration::from_secs(10);
    loop {
        match clone_once(url, dest) {
            Ok(()) => return Ok(()),
            Err(error) => {
                let transient = error.contains("not found")
                    || error.contains("could not be found")
                    || error.contains("could not read");
                if !transient || waited >= budget {
                    return Err(error);
                }
                std::thread::sleep(delay);
                waited += delay;
                delay = (delay * 2).min(budget - waited.min(budget));
                if delay.is_zero() {
                    delay = std::time::Duration::from_millis(500);
                }
            }
        }
    }
}

/// Append one debug line for git auth troubleshooting:
/// $data/zedb/git-debug.log. Cheap, best-effort, no secrets.
pub fn git_debug(line: &str) {
    let Some(dir) = dirs::data_local_dir().map(|dir| dir.join("zedb")) else {
        return;
    };
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("git-debug.log"))
    {
        use std::io::Write;
        let _ = writeln!(file, "{line}");
    }
}

fn clone_once(url: &str, dest: &Path) -> Result<(), String> {
    let envs = auth_envs(url);
    git_debug(&format!(
        "clone_once url={url} env_keys={:?} broker={:?}",
        envs.iter().map(|(key, _)| key.as_str()).collect::<Vec<_>>(),
        AUTH_BROKER.get(),
    ));
    let mut command = Command::new("git");
    if !envs.is_empty() {
        // See run_git_env: the broker must outrank osxkeychain.
        command.args(["-c", "credential.helper="]);
    }
    let output = command
        .arg("clone")
        .arg(url)
        .arg(dest)
        .envs(
            envs.iter()
                .map(|(key, value)| (key.as_str(), value.as_str())),
        )
        .output()
        .map_err(|error| format!("could not run git: {error}"))?;
    git_debug(&format!(
        "clone_once status={:?} stderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr).trim()
    ));
    if output.status.success() {
        Ok(())
    } else {
        // A failed attempt may leave a partial directory behind;
        // clear it so the retry starts clean.
        if dest.exists() {
            let _ = std::fs::remove_dir_all(dest);
        }
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
mod tests_auth {
    use super::*;

    #[test]
    fn broker_envs_only_for_https_known_hosts() {
        set_auth_broker(Some(std::path::PathBuf::from("/usr/local/bin/zedb")));
        assert_eq!(auth_envs("git@github.com:me/repo.git"), Vec::new());
        assert_eq!(auth_envs("https://example.com/me/repo.git"), Vec::new());
        let envs = auth_envs("https://gitlab.com/me/repo.git");
        assert!(envs
            .iter()
            .any(|(key, value)| key == "ZEDB_GIT_HOST" && value == "gitlab.com"));
        assert!(envs.iter().any(|(key, _)| key == "GIT_ASKPASS"));
    }
}

#[cfg(test)]
mod tests_git_error {
    use super::summarize_git_error;

    #[test]
    fn strips_remote_banner_noise() {
        let raw = "Cloning into '/tmp/x'...\nremote:\nremote: ========================\nremote:\nremote: ERROR: The project you were looking for could not be found or you don't have permission to view it.\nremote:\nfatal: Could not read from remote repository.";
        let summary = summarize_git_error(raw);
        assert!(summary.starts_with("ERROR: The project"));
        assert!(!summary.contains("===="));
        assert!(!summary.contains("Cloning into"));
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
