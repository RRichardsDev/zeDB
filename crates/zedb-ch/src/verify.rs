//! `zedb verify`: diff each database's live schema against the expected
//! state for its applied chain position.
//!
//! The expectation is derived by replaying exactly the migrations the
//! tracking table says are applied (fleet and targeted) through the
//! pinned `clickhouse local`; the live side is `system.tables`. Both are
//! normalized the same way before comparing: declustered, definers
//! stripped, whitespace collapsed, and the database name substituted back
//! to `${db}`, so per-database values and grant-model details are not
//! what the diff is about.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::LazyLock;

use regex::Regex;
use zedb_core::repo::MigrationRepo;

use crate::checks::dummy_params;
use crate::replay::{decluster, LocalReplay, ReplaySide};
use crate::runner::{quote, Runner, RunnerError, Targets};

#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    #[error("{0}")]
    Runner(#[from] RunnerError),
    #[error("replaying the expectation failed:\n{0}")]
    Replay(String),
    #[error("{0}")]
    Repo(String),
}

pub struct DatabaseDrift {
    pub database: String,
    pub head: Option<u32>,
    /// Human-readable drift findings; empty means the database verifies
    /// clean at its chain position.
    pub findings: Vec<String>,
}

static DEFINER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(SQL SECURITY \w+|DEFINER = \S+)").expect("static regex"));
static EMPTY_ENGINE_ARGS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\w*MergeTree)\(\)").expect("static regex"));
static WHITESPACE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").expect("static regex"));

/// Normalize a create_table_query for live-vs-expected diffs: the live
/// side runs clustered engines while expectations are declustered, and
/// definers differ; none of that is drift.
pub(crate) fn diff_norm(create: &str, database: &str) -> String {
    let declustered = decluster(create);
    let no_args = EMPTY_ENGINE_ARGS.replace_all(&declustered, "$1");
    let no_definer = DEFINER.replace_all(&no_args, "");
    WHITESPACE
        .replace_all(&no_definer, " ")
        .replace(database, "${db}")
        .trim()
        .to_string()
}

const EXPECTED_SIDE: &str = "zz_verify_expected";

pub struct Verifier<'a> {
    repo: &'a MigrationRepo,
    runner: &'a Runner<'a>,
    replay: LocalReplay,
}

impl<'a> Verifier<'a> {
    pub fn new(repo: &'a MigrationRepo, runner: &'a Runner<'a>, binary: PathBuf) -> Self {
        Self {
            repo,
            runner,
            replay: LocalReplay::new(binary),
        }
    }

    pub async fn verify(&self, targets: &Targets) -> Result<Vec<DatabaseDrift>, VerifyError> {
        let mut drifts = Vec::new();
        for database in self.runner.target_databases(targets).await? {
            drifts.push(self.verify_database(&database).await?);
        }
        Ok(drifts)
    }

    async fn verify_database(&self, database: &str) -> Result<DatabaseDrift, VerifyError> {
        let applied = self.runner.applied_migrations(database).await?;
        let customised = self.runner.applied_customisations(database).await?;
        let head = applied.iter().next_back().copied();

        // The expectation is exactly what the tracking table says ran, in
        // chain order.
        let texts: Vec<String> = self
            .repo
            .migrations
            .iter()
            .filter(|migration| {
                applied.contains(&migration.number) || customised.contains(&migration.number)
            })
            .map(|migration| {
                migration
                    .upgrade_sql()
                    .map_err(|error| VerifyError::Repo(error.to_string()))
            })
            .collect::<Result<_, _>>()?;

        let expected: BTreeMap<String, String> = if texts.is_empty() {
            BTreeMap::new()
        } else {
            let params = dummy_params(self.repo);
            let dumps = self
                .replay
                .dump_objects(
                    &[ReplaySide {
                        name: EXPECTED_SIDE.into(),
                        texts,
                    }],
                    &params,
                    &self.repo.config.replay.shared_databases,
                )
                .map_err(|error| VerifyError::Replay(error.to_string()))?;
            dumps[EXPECTED_SIDE]
                .iter()
                .map(|(name, create)| {
                    let stripped = name.replace(&format!("{EXPECTED_SIDE}__"), "");
                    (
                        stripped.replace(EXPECTED_SIDE, "${db}"),
                        diff_norm(create, EXPECTED_SIDE),
                    )
                })
                // Objects in shared bootstrap databases are cluster-level,
                // not per-database state; comparing them per database is a
                // fleet-view concern, not drift.
                .filter(|(name, _)| name.starts_with("${db}."))
                .collect()
        };

        let live_rows = self
            .runner
            .client()
            .query(&format!(
                "SELECT name, create_table_query FROM system.tables \
                 WHERE database = {} AND name NOT LIKE '.%' ORDER BY name",
                quote(database)
            ))
            .await
            .map_err(|error| VerifyError::Runner(RunnerError::Server(error.to_string())))?;
        let live: BTreeMap<String, String> = live_rows
            .rows
            .iter()
            .filter_map(|row| {
                let name = row.first()?.to_string();
                let create = row.get(1)?.to_string();
                Some((format!("${{db}}.{name}"), diff_norm(&create, database)))
            })
            .collect();

        let mut findings = Vec::new();
        let mut names: Vec<&String> = expected.keys().chain(live.keys()).collect();
        names.sort();
        names.dedup();
        for name in names {
            match (expected.get(name), live.get(name)) {
                (Some(_), None) => findings.push(format!("missing: {name}")),
                (None, Some(_)) => {
                    findings.push(format!("unexpected (not from the chain): {name}"))
                }
                (Some(expected), Some(live)) if expected != live => findings.push(format!(
                    "drifted: {name}\n  expected: {expected}\n  live:     {live}"
                )),
                _ => {}
            }
        }

        Ok(DatabaseDrift {
            database: database.to_string(),
            head,
            findings,
        })
    }
}
