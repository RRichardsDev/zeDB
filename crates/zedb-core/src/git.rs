//! Read-only git awareness for migration repos.
//!
//! Shells out to the user's `git`, so auth and worktree semantics are
//! theirs. One exception is deliberate: a migration/settings repo can be a
//! shared checkout, and its `.git/config` can name a `core.fsmonitor` or
//! hooks program that git would run on ordinary commands. Because zeDB runs
//! `git status` automatically when a repo is opened or watched, every
//! invocation here disables fsmonitor and hooks and bounds its runtime, so
//! merely pointing zeDB at a repo cannot execute code it carries.

use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Local git operations (status, config, commit): fast, but bounded so a
/// wedged or malicious child cannot pin a blocking-pool thread forever.
const GIT_LOCAL_TIMEOUT: Duration = Duration::from_secs(30);
/// Network operations (clone, ls-remote, push, pull) get a longer budget.
const GIT_NETWORK_TIMEOUT: Duration = Duration::from_secs(120);
/// Cap captured git output; a repo with millions of entries cannot balloon
/// memory, and the child is killed by the timeout once its pipe backs up.
const MAX_GIT_OUTPUT: u64 = 16 * 1024 * 1024;

/// A git `Command` with the repo-local execution vectors neutralized:
/// fsmonitor and hooks (both nameable in a shared `.git/config`) are off,
/// and optional index locks are skipped so a read cannot fight a writer.
fn git_command(root: Option<&Path>) -> Command {
    let mut command = Command::new("git");
    if let Some(root) = root {
        command.arg("-C").arg(root);
    }
    command.args([
        "-c",
        "core.fsmonitor=false",
        "-c",
        "core.hooksPath=/dev/null",
        "-c",
        "commit.gpgSign=false",
        "-c",
        "tag.gpgSign=false",
        "-c",
        "credential.helper=",
        "-c",
        "core.gitProxy=none",
        "-c",
        "protocol.allow=never",
        "-c",
        "protocol.http.allow=always",
        "-c",
        "protocol.https.allow=always",
        "-c",
        "protocol.ssh.allow=always",
        "-c",
        "protocol.git.allow=always",
        "-c",
        "protocol.file.allow=always",
        "-c",
        "protocol.ext.allow=never",
        "-c",
        "core.sshCommand=ssh",
    ]);
    #[cfg(target_os = "macos")]
    command.args(["-c", "credential.helper=osxkeychain"]);
    command.env("GIT_OPTIONAL_LOCKS", "0");
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    command
}

#[cfg(unix)]
fn kill_process_group(child: &mut std::process::Child) {
    // Every Git command starts a fresh process group. Kill helpers as well as
    // Git itself so a remote helper cannot retain a captured pipe forever.
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.kill();
}

#[cfg(not(unix))]
fn kill_process_group(child: &mut std::process::Child) {
    let _ = child.kill();
}

/// Run `command` to completion within `timeout`, capturing bounded output.
/// stdout/stderr are drained on threads so a full pipe cannot deadlock the
/// wait, and the child is killed if it overruns.
fn run_capture(mut command: Command, timeout: Duration) -> std::io::Result<Output> {
    use std::io::Read as _;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn()?;
    let mut out_pipe = child.stdout.take().expect("stdout piped");
    let mut err_pipe = child.stderr.take().expect("stderr piped");
    let (out_tx, out_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = out_pipe
            .by_ref()
            .take(MAX_GIT_OUTPUT)
            .read_to_end(&mut buffer);
        let _ = out_tx.send(buffer);
    });
    let (err_tx, err_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = err_pipe
            .by_ref()
            .take(MAX_GIT_OUTPUT)
            .read_to_end(&mut buffer);
        let _ = err_tx.send(buffer);
    });
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            // Normal readers finish promptly once Git closes its pipes. If a
            // helper retained them, terminate the remaining command group,
            // then make one final bounded collection attempt.
            let mut stdout = out_rx.recv_timeout(Duration::from_secs(1)).ok();
            let mut stderr = err_rx.recv_timeout(Duration::from_secs(1)).ok();
            if stdout.is_none() || stderr.is_none() {
                kill_process_group(&mut child);
                if stdout.is_none() {
                    stdout = out_rx.recv_timeout(Duration::from_secs(1)).ok();
                }
                if stderr.is_none() {
                    stderr = err_rx.recv_timeout(Duration::from_secs(1)).ok();
                }
            }
            return Ok(Output {
                status,
                stdout: stdout.unwrap_or_default(),
                stderr: stderr.unwrap_or_default(),
            });
        }
        if Instant::now() >= deadline {
            kill_process_group(&mut child);
            let _ = child.wait();
            let _ = out_rx.recv_timeout(Duration::from_secs(1));
            let _ = err_rx.recv_timeout(Duration::from_secs(1));
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "git timed out",
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Drop control characters (including embedded newlines and terminal escape
/// sequences) from a git-reported branch or path before it reaches the UI
/// or a line-based parser.
fn strip_controls(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).collect()
}

/// Reject a URL that git would parse as an option (leading `-`), closing
/// argument-injection like `--upload-pack=<cmd>` masquerading as a remote.
fn is_option_like(url: &str) -> bool {
    url.trim_start().starts_with('-')
}

/// Git transports zeDB permits for automatic pull and push. In particular,
/// helper syntax such as `ext::command` and unknown `scheme::` transports is
/// rejected even if hostile repo config enables it.
fn is_allowed_remote(url: &str) -> bool {
    let url = url.trim();
    if url.is_empty() || is_option_like(url) || url.contains("::") {
        return false;
    }
    if let Some((scheme, _)) = url.split_once("://") {
        return matches!(scheme, "http" | "https" | "ssh" | "git" | "file");
    }
    // SCP-style SSH and ordinary local paths are both supported.
    true
}

fn ensure_no_local_filter_programs(root: &Path) -> Result<(), String> {
    let mut command = git_command(Some(root));
    command.args([
        "config",
        "--local",
        "--name-only",
        "--get-regexp",
        r"^filter\..*\.(clean|smudge|process)$",
    ]);
    let output = run_capture(command, GIT_LOCAL_TIMEOUT)
        .map_err(|error| format!("could not inspect Git filter config: {error}"))?;
    // `git config --get-regexp` exits 1 for no matches.
    if output.status.success() && !output.stdout.is_empty() {
        return Err("refusing a repository with local clean/smudge filter programs".into());
    }
    if output.status.code() == Some(1) {
        return Ok(());
    }
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

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
    let mut command = git_command(Some(root));
    command.args(["status", "--porcelain=v2", "--branch"]);
    let output = run_capture(command, GIT_LOCAL_TIMEOUT).ok()?;
    if !output.status.success() {
        return None;
    }
    Some(parse_porcelain_v2(&String::from_utf8_lossy(&output.stdout)))
}

/// Paths with uncommitted changes (staged, unstaged, or untracked),
/// relative to the repo root. None outside a git work tree.
pub fn changed_paths(root: &Path) -> Option<Vec<String>> {
    let mut command = git_command(Some(root));
    command.args(["status", "--porcelain=v2"]);
    let output = run_capture(command, GIT_LOCAL_TIMEOUT).ok()?;
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
        // A path can carry control bytes; strip them before it reaches the
        // commit panel or any other display.
        if let Some(path) = path {
            paths.push(strip_controls(&path));
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
fn https_host(url: &str) -> Option<&str> {
    let authority = url.strip_prefix("https://")?.split('/').next()?;
    let host_port = authority.rsplit('@').next()?;
    if let Some(bracketed) = host_port.strip_prefix('[') {
        return bracketed.split(']').next();
    }
    Some(
        host_port
            .rsplit_once(':')
            .map(|(host, _)| host)
            .unwrap_or(host_port),
    )
}

fn auth_envs(url: &str) -> Vec<(String, String)> {
    let Some(Some(broker)) = AUTH_BROKER.get() else {
        return Vec::new();
    };
    let Some(host) = https_host(url) else {
        return Vec::new();
    };
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

fn current_remote(root: &Path) -> String {
    let branch = run_git(root, &["symbolic-ref", "--quiet", "--short", "HEAD"]).unwrap_or_default();
    if branch.is_empty() {
        return "origin".into();
    }
    run_git(
        root,
        &["config", "--get", &format!("branch.{branch}.remote")],
    )
    .ok()
    .filter(|remote| !remote.is_empty() && remote != ".")
    .unwrap_or_else(|| "origin".into())
}

/// Resolve the URL Git will actually use after pushurl and insteadOf rules.
/// This is the URL against which broker authority must be decided.
fn effective_remote_url(root: &Path, remote: &str, push: bool) -> Result<String, String> {
    let mut command = git_command(Some(root));
    command.args(["remote", "get-url"]);
    if push {
        command.arg("--push");
    }
    command.arg(remote);
    let output = run_capture(command, GIT_LOCAL_TIMEOUT)
        .map_err(|error| format!("could not inspect git remote: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !is_allowed_remote(&url) {
        return Err("refusing a Git remote with an executable or unsupported transport".into());
    }
    Ok(url)
}

fn auth_envs_for_remote(
    root: &Path,
    remote: &str,
    push: bool,
) -> Result<Vec<(String, String)>, String> {
    effective_remote_url(root, remote, push).map(|url| auth_envs(&url))
}

/// The auth envs for the checkout's current upstream remote.
fn auth_envs_for_root(root: &Path, push: bool) -> Result<Vec<(String, String)>, String> {
    auth_envs_for_remote(root, &current_remote(root), push)
}

/// The askpass executable must independently verify Git's actual prompt.
/// Environment selected from a preflight URL is only a hint because repo
/// config, redirects, or a race could otherwise route the prompt elsewhere.
pub fn askpass_prompt_matches_host(prompt: &str, expected_host: &str) -> bool {
    let Some((_, quoted)) = prompt.split_once("for '") else {
        return false;
    };
    let Some((url, _)) = quoted.split_once('\'') else {
        return false;
    };
    https_host(url) == Some(expected_host)
}

fn run_git(root: &Path, args: &[&str]) -> Result<String, String> {
    run_git_env(root, args, &[])
}

fn run_git_env(root: &Path, args: &[&str], envs: &[(String, String)]) -> Result<String, String> {
    let mut command = git_command(Some(root));
    if !envs.is_empty() {
        // Configured credential helpers (macOS ships osxkeychain)
        // outrank GIT_ASKPASS and answer with whatever account they
        // stored last; clear the helper list so the broker is the
        // only voice for this run.
        command.args(["-c", "credential.helper="]);
    }
    command.args(args).envs(
        envs.iter()
            .map(|(key, value)| (key.as_str(), value.as_str())),
    );
    let output = run_capture(command, GIT_NETWORK_TIMEOUT)
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
    ensure_no_local_filter_programs(root)?;
    for path in pathspecs {
        // `git add` executes clean filters selected by .gitattributes. A
        // shared checkout must not turn zeDB's automatic settings commit into
        // a repo-configured command execution primitive.
        let attribute = run_git(root, &["check-attr", "filter", "--", path])?;
        let value = attribute
            .rsplit(':')
            .next()
            .map(str::trim)
            .unwrap_or_default();
        if value != "unspecified" && value != "unset" {
            return Err(format!(
                "refusing to stage {path:?}: a Git clean filter is configured"
            ));
        }
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
    let envs = auth_envs_for_root(root, true)?;
    run_git_env(root, &["push"], &envs)
}

/// Whether `url` is a reachable git remote for this user's own git
/// auth (ssh keys, credential helpers). Never prompts: batch-mode ssh
/// and disabled terminal prompts make an auth wall read as "no".
pub fn remote_exists(url: &str) -> bool {
    if !is_allowed_remote(url) {
        return false;
    }
    let mut command = git_command(None);
    // `--` before the URL so a value like `--upload-pack=<cmd>` cannot be
    // parsed as an ls-remote option and run a command.
    command
        .args(["ls-remote", "--exit-code", "--", url, "HEAD"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_SSH_COMMAND", "ssh -oBatchMode=yes -oConnectTimeout=5");
    run_capture(command, GIT_NETWORK_TIMEOUT)
        .map(|output| output.status.success())
        .unwrap_or(false)
}

/// Push, setting the upstream first when the checkout has none (the
/// first push into a fresh clone of an empty remote).
pub fn push_setting_upstream(root: &Path) -> Result<String, String> {
    if has_upstream(root) {
        push(root)
    } else {
        let envs = auth_envs_for_remote(root, "origin", true)?;
        run_git_env(root, &["push", "-u", "origin", "HEAD"], &envs)
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
    ensure_no_local_filter_programs(root)?;
    let envs = auth_envs_for_root(root, false)?;
    run_git_env(root, &["pull", "--ff-only"], &envs)
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
    if !is_allowed_remote(url) {
        return Err("refusing an executable, option-like, or unsupported Git remote".into());
    }
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
    if crate::store::create_private_dir(&dir).is_err() {
        return;
    }
    let path = dir.join("git-debug.log");
    let truncate = std::fs::metadata(&path)
        .map(|metadata| metadata.len() > 1024 * 1024)
        .unwrap_or(false);
    let mut options = std::fs::OpenOptions::new();
    options.create(true).write(true);
    if truncate {
        options.truncate(true);
    } else {
        options.append(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    if let Ok(mut file) = options.open(&path) {
        use std::io::Write;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = file.set_permissions(std::fs::Permissions::from_mode(0o600));
        }
        let safe: String = line
            .chars()
            .filter(|character| !character.is_control())
            .collect();
        let _ = writeln!(file, "{safe}");
    }
}

fn redacted_remote(url: &str) -> String {
    let url = url.trim();
    let without_fragment = url.split(['?', '#']).next().unwrap_or(url);
    let Some((scheme, rest)) = without_fragment.split_once("://") else {
        return "[local-or-ssh-remote]".into();
    };
    let authority = rest
        .split_once('/')
        .map(|(authority, _)| authority)
        .unwrap_or(rest);
    let host = authority.rsplit('@').next().unwrap_or(authority);
    format!("{scheme}://{host}")
}

fn clone_once(url: &str, dest: &Path) -> Result<(), String> {
    let envs = auth_envs(url);
    git_debug(&format!(
        "clone_once url={} env_keys={:?} broker={:?}",
        redacted_remote(url),
        envs.iter().map(|(key, _)| key.as_str()).collect::<Vec<_>>(),
        AUTH_BROKER.get(),
    ));
    let mut command = git_command(None);
    if !envs.is_empty() {
        // See run_git_env: the broker must outrank osxkeychain.
        command.args(["-c", "credential.helper="]);
    }
    // `--` before the URL so a `-`-leading value cannot become a clone
    // option such as `--upload-pack`.
    command.arg("clone").arg("--").arg(url).arg(dest).envs(
        envs.iter()
            .map(|(key, value)| (key.as_str(), value.as_str())),
    );
    let output = run_capture(command, GIT_NETWORK_TIMEOUT)
        .map_err(|error| format!("could not run git: {error}"))?;
    git_debug(&format!(
        "clone_once status={:?} stderr_bytes={}",
        output.status.code(),
        output.stderr.len()
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
                // A branch name can carry control bytes; strip them before
                // it reaches a status chip or deploy warning.
                branch = Some(strip_controls(head));
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

    #[test]
    fn broker_refuses_userinfo_host_spoofing() {
        set_auth_broker(Some(std::path::PathBuf::from("/usr/local/bin/zedb")));
        // git connects to evil.com here (github.com is userinfo); the
        // broker token must not be offered to it.
        assert_eq!(auth_envs("https://github.com@evil.com/x.git"), Vec::new());
        assert_eq!(
            auth_envs("https://github.com:pass@evil.com/x.git"),
            Vec::new()
        );
        // A port on the real host is fine and still matches.
        let envs = auth_envs("https://github.com:443/me/repo.git");
        assert!(envs
            .iter()
            .any(|(key, value)| key == "ZEDB_GIT_HOST" && value == "github.com"));
    }

    #[test]
    fn askpass_checks_the_host_in_gits_actual_prompt() {
        assert!(askpass_prompt_matches_host(
            "Password for 'https://x-access-token@github.com': ",
            "github.com"
        ));
        assert!(!askpass_prompt_matches_host(
            "Password for 'https://evil.example': ",
            "github.com"
        ));
        assert!(!askpass_prompt_matches_host(
            "Password for 'https://github.com@evil.example': ",
            "github.com"
        ));
    }

    #[test]
    fn effective_push_url_controls_broker_selection() {
        let directory = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            assert!(Command::new("git")
                .arg("-C")
                .arg(directory.path())
                .args(args)
                .status()
                .unwrap()
                .success());
        };
        run(&["init", "-q", "-b", "main"]);
        run(&[
            "remote",
            "add",
            "origin",
            "https://github.com/acme/repo.git",
        ]);
        run(&[
            "config",
            "remote.origin.pushurl",
            "https://evil.example/acme/repo.git",
        ]);
        set_auth_broker(Some(std::path::PathBuf::from("/usr/local/bin/zedb")));

        let effective = effective_remote_url(directory.path(), "origin", true).unwrap();
        assert_eq!(effective, "https://evil.example/acme/repo.git");
        assert!(auth_envs_for_remote(directory.path(), "origin", true)
            .unwrap()
            .is_empty());
    }
}

#[cfg(test)]
mod tests_injection {
    use super::*;

    #[test]
    fn option_like_urls_are_refused() {
        assert!(is_option_like("--upload-pack=touch /tmp/x"));
        assert!(is_option_like("  -c"));
        assert!(!is_option_like("https://github.com/a/b.git"));
        assert!(!is_option_like("git@github.com:a/b.git"));
        assert!(!remote_exists("--upload-pack=touch /tmp/pwned"));
        assert!(clone_repo("--upload-pack=x", std::path::Path::new("/tmp/zedb-none")).is_err());
        assert!(!is_allowed_remote("ext::sh -c id"));
        assert!(!remote_exists("ext::sh -c id"));
        assert!(clone_repo("ext::sh -c id", std::path::Path::new("/tmp/zedb-none")).is_err());
    }

    #[test]
    fn control_characters_are_stripped_from_git_reported_text() {
        assert_eq!(strip_controls("main\u{1b}[2J"), "main[2J");
        assert_eq!(strip_controls("a\nb\tc"), "abc");
    }

    #[test]
    fn remote_debug_text_removes_credentials_and_queries() {
        assert_eq!(
            redacted_remote("https://user:secret@github.com/acme/repo.git?token=secret"),
            "https://github.com"
        );
        assert_eq!(
            redacted_remote("git@github.com:acme/repo.git"),
            "[local-or-ssh-remote]"
        );
    }

    #[cfg(unix)]
    #[test]
    fn capture_does_not_wait_for_a_helper_that_retains_its_pipes() {
        let mut command = git_command(None);
        command.args(["-c", "alias.hold=!sh -c '(sleep 30) &'", "hold"]);
        let started = Instant::now();
        let _ = run_capture(command, Duration::from_secs(2));
        assert!(started.elapsed() < Duration::from_secs(5));
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
    fn commit_refuses_repo_configured_clean_filters() {
        let dir = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            assert!(Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(args)
                .status()
                .unwrap()
                .success());
        };
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "filter.hostile.clean", "cat"]);
        std::fs::write(dir.path().join(".gitattributes"), "*.json filter=hostile\n").unwrap();
        std::fs::write(dir.path().join("settings.json"), "{}\n").unwrap();

        let error = commit_paths(dir.path(), &["settings.json".into()], "settings").unwrap_err();
        assert!(error.contains("filter"), "{error}");
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
