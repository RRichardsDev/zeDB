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
}
