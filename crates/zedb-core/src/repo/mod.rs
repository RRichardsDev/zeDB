//! Migration repo model: the on-disk format defined in docs/FORMAT.md.

mod chain;
mod config;
mod exclusions;
mod import;
mod scaffold;
mod template;

use std::path::{Path, PathBuf};

pub use chain::{discover_chain, Migration, RollbackClass};
pub use config::{ParamConfig, ReplayConfig, RepoConfig, ScopeConfig};
pub use exclusions::{ExclusionGroup, Exclusions};
pub use import::{import_repo, ImportReport};
pub use scaffold::{
    init_repo, init_repo_if_empty, is_effectively_empty, scaffold_migration, ScaffoldOptions,
};
pub use template::{placeholders, render, undeclared_placeholders, BUILTIN_PARAMS};

#[derive(Debug, thiserror::Error)]
pub enum RepoError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{path}: {message}")]
    Config { path: PathBuf, message: String },
    #[error("{0}")]
    Layout(String),
    #[error("no zedb.toml found in {0} or any parent directory")]
    NotARepo(PathBuf),
    #[error("{0} already contains zedb.toml")]
    AlreadyARepo(PathBuf),
    #[error("template parameter ${{{name}}} must be a plain identifier, got {value:?}")]
    InvalidIdentifier { name: String, value: String },
    #[error("missing value for template parameter ${{{0}}}")]
    MissingParam(String),
}

/// A parsed migration repo: config, ordered chain, and exclusions.
#[derive(Debug)]
pub struct MigrationRepo {
    pub root: PathBuf,
    pub config: RepoConfig,
    pub migrations: Vec<Migration>,
    pub exclusions: Exclusions,
}

impl MigrationRepo {
    /// Open the repo containing `path`, walking up to find `zedb.toml`.
    pub fn open(path: &Path) -> Result<Self, RepoError> {
        let start = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()?.join(path)
        };
        let mut candidate = Some(start.as_path());
        while let Some(dir) = candidate {
            if dir.join("zedb.toml").is_file() {
                return Self::open_root(dir);
            }
            candidate = dir.parent();
        }
        Err(RepoError::NotARepo(start))
    }

    /// Open a repo rooted exactly at `root`.
    pub fn open_root(root: &Path) -> Result<Self, RepoError> {
        let config = RepoConfig::load(&root.join("zedb.toml"))?;
        let migrations = discover_chain(&root.join("migrations"), &config)?;
        let exclusions = Exclusions::load(&root.join("exclusions.toml"))?;
        Ok(Self {
            root: root.to_path_buf(),
            config,
            migrations,
            exclusions,
        })
    }

    /// The migration number a newly scaffolded migration should use.
    pub fn next_number(&self) -> u32 {
        match self.migrations.last() {
            Some(migration) => migration.number + 100,
            None => 0,
        }
    }

    pub fn migration(&self, number: u32) -> Option<&Migration> {
        self.migrations
            .iter()
            .find(|migration| migration.number == number)
    }
}
