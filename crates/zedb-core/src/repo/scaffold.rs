use std::path::{Path, PathBuf};

use chrono::Datelike;

use super::chain::new_migration_directory;
use super::{MigrationRepo, RepoError};

const CONFIG_TEMPLATE: &str = r#"format = 1

[engine]
kind = "clickhouse"
# Pin to the version your servers run; discovered versions are recorded here.
version = "25.3.2.1"

[tracking]
database = "default"

[scopes]
global = { }
db = { param = "db" }

[params]
# Declare template parameters usable as ${name} in migrations, e.g.:
# ttl_days = { default = "30", description = "raw event retention window" }
"#;

/// Create an empty format-1 repo at `path`.
pub fn init_repo(path: &Path) -> Result<PathBuf, RepoError> {
    let config = path.join("zedb.toml");
    if config.exists() {
        return Err(RepoError::AlreadyARepo(path.to_path_buf()));
    }
    std::fs::create_dir_all(path.join("migrations"))?;
    std::fs::create_dir_all(path.join("current-state"))?;
    std::fs::write(&config, CONFIG_TEMPLATE)?;
    Ok(config)
}

pub struct ScaffoldOptions {
    pub description: String,
    pub targeted: bool,
    /// Scaffold a rollback.sql template (declared clean until edited).
    pub with_rollback: bool,
}

/// Scaffold the next migration in the chain; returns its directory.
pub fn scaffold_migration(
    repo: &MigrationRepo,
    options: &ScaffoldOptions,
) -> Result<PathBuf, RepoError> {
    let number = repo.next_number();
    let today = chrono::Local::now().date_naive();
    let directory = new_migration_directory(
        &repo.root.join("migrations"),
        number,
        today.year() as u16,
        today.month() as u8,
    );
    std::fs::create_dir_all(&directory)?;
    std::fs::write(
        directory.join("upgrade.sql"),
        format!(
            "-- migration {number:05}: {}\n\n-- Write plain SQL; statements are split on `;`.\n",
            options.description
        ),
    )?;
    if options.with_rollback {
        std::fs::write(
            directory.join("rollback.sql"),
            "-- rollback-class: clean\n\n-- Revert every object upgrade.sql creates or changes.\n",
        )?;
    }
    if options.targeted {
        std::fs::write(
            directory.join("targeted.toml"),
            "# Opt-in migration: applied per database with `zedb apply`.\n# allow_list = [\"db_name\"]\n",
        )?;
    }
    Ok(directory)
}
