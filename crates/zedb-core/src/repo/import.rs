//! One-time conversion of an analytics-clickhouse-ddl repo into format 1
//! (docs/PHASE-1.md M8). The migration chain shape is unchanged between
//! generations, so migrations copy verbatim; the conversion supplies what
//! the ancestor kept in code: zedb.toml with declared scopes, parameters
//! (with the ancestor's dummy and sentinel values), shared bootstrap
//! databases, and the pinned engine version parsed from pin.py.

use std::path::{Path, PathBuf};

use super::RepoError;

#[derive(Debug)]
pub struct ImportReport {
    pub destination: PathBuf,
    pub migrations: usize,
    pub engine_version: String,
    pub exclusion_groups: usize,
}

/// Parameters the ancestor tooling built in, with its DUMMY_PARAMS and
/// SENTINEL_PARAMS values carried over so checks and regen render exactly
/// as the ancestor did. Offsets have no runtime default on purpose: the
/// ancestor inherited them from the live schema, which format 1 replaces
/// with explicit --param until inheritance rules land.
const ANCESTOR_PARAMS: &[(&str, &str, &str, &str)] = &[
    // (name, dummy, sentinel, description)
    (
        "password",
        "rendertest_password",
        "zz_sentinel_password",
        "per-database user password",
    ),
    (
        "refresh_offset",
        "42",
        "739417",
        "legacy minute count for the activity refresh stagger",
    ),
    (
        "refresh_offset_expr",
        "1 HOUR 42 MINUTE",
        "2 HOUR 53 MINUTE",
        "per-database activity refresh stagger",
    ),
    (
        "attribution_offset",
        "42",
        "739421",
        "legacy minute count for the attribution refresh stagger",
    ),
    (
        "attribution_offset_expr",
        "2 HOUR 4 MINUTE",
        "9 HOUR 47 MINUTE",
        "per-database attribution refresh stagger",
    ),
    (
        "attribution_window_days",
        "7",
        "739451",
        "attribution lookback window in days",
    ),
];

fn copy_tree(from: &Path, to: &Path) -> Result<(), RepoError> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// Parse `CH_VERSION = "..."` from the ancestor's pin.py.
fn pinned_version(ancestor: &Path) -> Result<String, RepoError> {
    let pin = ancestor.join("clickhouse_ddl/pin.py");
    let text = std::fs::read_to_string(&pin).map_err(|error| RepoError::Config {
        path: pin.clone(),
        message: format!("cannot read the ancestor's pinned version: {error}"),
    })?;
    text.lines()
        .find_map(|line| {
            let rest = line.trim().strip_prefix("CH_VERSION")?.trim_start();
            let rest = rest.strip_prefix('=')?.trim();
            Some(rest.trim_matches('"').to_string())
        })
        .ok_or_else(|| RepoError::Config {
            path: pin,
            message: "no CH_VERSION assignment found".into(),
        })
}

/// Convert `ancestor` into a fresh format-1 repo at `destination`.
///
/// current-state/ is deliberately not copied: it is a generated artifact
/// and `zedb regen` derives it under this tooling's own naming, with
/// `zedb check equivalence` proving the result matches the chain.
pub fn import_repo(ancestor: &Path, destination: &Path) -> Result<ImportReport, RepoError> {
    if destination.join("zedb.toml").exists() {
        return Err(RepoError::AlreadyARepo(destination.to_path_buf()));
    }
    let migrations_from = ancestor.join("migrations");
    if !migrations_from.is_dir() {
        return Err(RepoError::Config {
            path: ancestor.to_path_buf(),
            message: "no migrations/ directory; is this an analytics-clickhouse-ddl repo?".into(),
        });
    }
    let engine_version = pinned_version(ancestor)?;

    std::fs::create_dir_all(destination)?;
    copy_tree(&migrations_from, &destination.join("migrations"))?;
    std::fs::create_dir_all(destination.join("current-state"))?;

    let exclusions_from = ancestor.join("exceptions.toml");
    let mut exclusion_groups = 0;
    if exclusions_from.is_file() {
        let text = std::fs::read_to_string(&exclusions_from)?;
        exclusion_groups = text
            .lines()
            .filter(|line| line.trim_start().starts_with("[groups."))
            .count();
        std::fs::write(destination.join("exclusions.toml"), text)?;
    }

    let mut params = String::new();
    for (name, dummy, sentinel, description) in ANCESTOR_PARAMS {
        params.push_str(&format!(
            "{name} = {{ dummy = \"{dummy}\", sentinel = \"{sentinel}\", description = \"{description}\" }}\n"
        ));
    }
    let config = format!(
        r#"# Imported from analytics-clickhouse-ddl by `zedb import`.
format = 1

[engine]
kind = "clickhouse"
version = "{engine_version}"

[tracking]
database = "default"
cluster_param = "cluster"

[replay]
# Created by cluster bootstrap, not the chain; replays pre-create them.
shared_databases = ["org_to_slug_mappings", "RefreshableViews"]

[scopes]
global = {{ }}
org = {{ param = "db" }}

[params]
{params}"#
    );
    std::fs::write(destination.join("zedb.toml"), config)?;

    let repo = super::MigrationRepo::open_root(destination)?;
    Ok(ImportReport {
        destination: destination.to_path_buf(),
        migrations: repo.migrations.len(),
        engine_version,
        exclusion_groups,
    })
}
