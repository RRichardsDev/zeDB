//! One module per command group, over the few openers they all share.

use std::path::{Path, PathBuf};

use zedb_core::repo::MigrationRepo;

pub mod check;
pub mod engine;
pub mod fleet;
pub mod mcp;
pub mod repo;
pub mod verify;

pub fn open_repo(root: &Path) -> Result<MigrationRepo, String> {
    MigrationRepo::open(root).map_err(|error| error.to_string())
}

/// The CLI is synchronous on the outside and the driver is async
/// underneath, so a command that talks to a server owns a runtime for the
/// length of the call.
pub fn runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Runtime::new().map_err(|error| error.to_string())
}

/// Replay-backed commands need the exact pinned build rather than whatever
/// ClickHouse happens to be on PATH, so a missing cache is an error with
/// the fix in it rather than a fallback.
pub fn pinned_binary(repo: &MigrationRepo) -> Result<PathBuf, String> {
    let version = &repo.config.engine.version;
    // Honor the fallback `zedb pin` may have resolved and recorded for
    // a version with no reviewed artifact; otherwise pin says "pinned"
    // and the next command demands pinning again, forever.
    zedb_ch::cached_binary_or_fallback(version)
        .ok_or_else(|| format!("pinned ClickHouse {version} is not cached; run `zedb pin` first"))
}

pub fn terminal_field(text: &str) -> String {
    escape_terminal(text, false)
}

pub fn terminal_text(text: &str) -> String {
    escape_terminal(text, true)
}

fn escape_terminal(text: &str, preserve_layout: bool) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        if character.is_control() && !(preserve_layout && (character == '\n' || character == '\t'))
        {
            escaped.extend(character.escape_default());
        } else {
            escaped.push(character);
        }
    }
    escaped
}

#[cfg(test)]
mod security_tests {
    use super::*;

    #[test]
    fn terminal_escaping_blocks_controls_without_flattening_text_layout() {
        assert_eq!(terminal_field("db\n\u{1b}[2J"), "db\\n\\u{1b}[2J");
        assert_eq!(terminal_text("line\n\t\u{1b}[2J"), "line\n\t\\u{1b}[2J");
    }
}
