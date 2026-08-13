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
    zedb_ch::cached_binary(version)
        .ok_or_else(|| format!("pinned ClickHouse {version} is not cached; run `zedb pin` first"))
}
