//! Agent discovery: what is actually installed on this machine, and in
//! what state.
//!
//! GUI apps launch with a skinny PATH (no shell profile), so lookup
//! searches the real PATH plus the standard install locations: homebrew,
//! /usr/local, ~/.local/bin, and nvm's per-version bin directories.
//! Commands are resolved to absolute paths at discovery time so spawns
//! work identically from Finder and a terminal.

use std::path::{Path, PathBuf};

/// How usable an agent is right now, with a hint a human can act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Availability {
    Ready,
    /// Installed, but its auth marker is absent; the hint names the
    /// login command. Still launchable: the heuristic can be stale.
    NeedsLogin {
        hint: String,
    },
    /// Not installed; the hint says how to get it.
    Missing {
        hint: String,
    },
}

/// One launchable (or explainable) agent.
#[derive(Debug, Clone)]
pub struct DiscoveredAgent {
    /// Stable identifier ("claude-code", "codex", or "custom").
    pub id: String,
    pub name: String,
    /// Absolute program path when resolved, the raw command otherwise.
    pub command: String,
    pub args: Vec<String>,
    pub availability: Availability,
}

/// Directories searched beyond PATH, in order.
fn fallback_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
    ];
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join(".local/bin"));
        dirs.push(home.join("bin"));
        // nvm installs node/npx per version; take any.
        if let Ok(entries) = std::fs::read_dir(home.join(".nvm/versions/node")) {
            for entry in entries.flatten() {
                dirs.push(entry.path().join("bin"));
            }
        }
    }
    dirs
}

/// Find `name` in the given directories; first hit wins.
pub fn find_executable_in(name: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    for dir in dirs {
        let candidate = dir.join(name);
        if is_executable(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.is_file()
        && std::fs::metadata(path)
            .map(|meta| meta.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
}

/// PATH plus the standard install locations.
pub fn find_executable(name: &str) -> Option<PathBuf> {
    // An explicit path is taken at face value.
    if name.contains('/') {
        let path = PathBuf::from(name);
        return is_executable(&path).then_some(path);
    }
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    dirs.extend(fallback_dirs());
    find_executable_in(name, &dirs)
}

/// The built-in agents, resolved against this machine.
pub fn discover_known() -> Vec<DiscoveredAgent> {
    let npx = find_executable("npx");
    let home = dirs::home_dir().unwrap_or_default();
    let mut agents = Vec::new();

    // Claude Code: the ACP adapter runs via npx; auth comes from the
    // Claude Code CLI's own login state.
    let claude_installed = find_executable("claude").is_some() || home.join(".claude").is_dir();
    let claude_logged_in =
        home.join(".claude.json").is_file() || home.join(".claude/.credentials.json").is_file();
    agents.push(DiscoveredAgent {
        id: "claude-code".into(),
        name: "Claude Code".into(),
        command: npx
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "npx".into()),
        args: vec!["-y".into(), "@agentclientprotocol/claude-agent-acp".into()],
        availability: if npx.is_none() {
            Availability::Missing {
                hint: "needs Node (npx not found); install Node.js".into(),
            }
        } else if !claude_installed {
            Availability::Missing {
                hint: "install Claude Code, then run `claude` to log in".into(),
            }
        } else if !claude_logged_in {
            Availability::NeedsLogin {
                hint: "run `claude` in a terminal to log in".into(),
            }
        } else {
            Availability::Ready
        },
    });

    // Codex: adapter via npx; auth in ~/.codex.
    let codex_installed = find_executable("codex").is_some() || home.join(".codex").is_dir();
    let codex_logged_in = home.join(".codex/auth.json").is_file();
    agents.push(DiscoveredAgent {
        id: "codex".into(),
        name: "Codex".into(),
        command: npx
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "npx".into()),
        args: vec!["-y".into(), "@zed-industries/codex-acp".into()],
        availability: if npx.is_none() {
            Availability::Missing {
                hint: "needs Node (npx not found); install Node.js".into(),
            }
        } else if !codex_installed {
            Availability::Missing {
                hint: "install Codex (brew install codex), then log in".into(),
            }
        } else if !codex_logged_in {
            Availability::NeedsLogin {
                hint: "run `codex` in a terminal to log in".into(),
            }
        } else {
            Availability::Ready
        },
    });

    agents
}

/// Resolve a user-configured agent: name plus a command line.
pub fn resolve_custom(name: &str, command: &str, args: &[String]) -> DiscoveredAgent {
    let resolved = find_executable(command);
    DiscoveredAgent {
        id: "custom".into(),
        name: name.to_string(),
        command: resolved
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| command.to_string()),
        args: args.to_vec(),
        availability: if resolved.is_some() {
            Availability::Ready
        } else {
            Availability::Missing {
                hint: format!("{command} not found on PATH or standard locations"),
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_in_given_dirs_only_when_executable() {
        let dir = tempfile::tempdir().unwrap();
        let tool = dir.path().join("mytool");
        std::fs::write(&tool, "#!/bin/sh\n").unwrap();
        // Not executable yet.
        assert!(find_executable_in("mytool", &[dir.path().to_path_buf()]).is_none());
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            find_executable_in("mytool", &[dir.path().to_path_buf()]),
            Some(tool)
        );
    }

    #[test]
    fn find_executable_resolves_common_binaries_and_paths() {
        // sh exists everywhere.
        let sh = find_executable("sh").expect("sh on PATH");
        assert!(sh.is_absolute());
        // Explicit paths are taken as-is.
        assert_eq!(find_executable("/bin/sh"), Some(PathBuf::from("/bin/sh")));
        assert!(find_executable("/no/such/binary").is_none());
        assert!(find_executable("definitely-not-a-real-tool-xyz").is_none());
    }

    #[test]
    fn known_agents_always_enumerate() {
        let agents = discover_known();
        assert_eq!(agents.len(), 2);
        assert!(agents.iter().any(|agent| agent.id == "claude-code"));
        assert!(agents.iter().any(|agent| agent.id == "codex"));
    }

    #[test]
    fn custom_agents_resolve_or_explain() {
        let ready = resolve_custom("My Agent", "sh", &["-c".into(), "true".into()]);
        assert_eq!(ready.availability, Availability::Ready);
        assert!(ready.command.ends_with("/sh"));
        let missing = resolve_custom("Ghost", "no-such-agent-cmd", &[]);
        assert!(matches!(missing.availability, Availability::Missing { .. }));
    }
}
