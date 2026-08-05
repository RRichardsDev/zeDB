//! Live execution against real servers (docs/PHASE-1.md M5): tracking
//! bootstrap, status, upgrade, rollback with class enforcement, stamp,
//! and targeted apply. Ported from the ancestor's runner.py.
//!
//! Safety is architecture: mutating entry points refuse read-only
//! connections, every run records to the tracking table and a local audit
//! log, structural rollbacks warn, irreversible ones require explicit
//! acknowledgement, and rollbacks only peel from the top of the chain.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::time::Instant;

use zedb_core::repo::{placeholders, render, Migration, MigrationRepo, RollbackClass};

use crate::replay::{decluster, is_access_control, split_statements};
use crate::{ChClient, ChConfig};

#[derive(Debug, thiserror::Error)]
pub enum RunnerError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Server(String),
    #[error("{0}")]
    Repo(String),
    #[error("{0}")]
    Refused(String),
}

pub struct RunnerOptions {
    pub server: ChConfig,
    pub cluster: Option<String>,
    /// Render for a single node: `ON CLUSTER` dropped, Replicated engines
    /// declustered.
    pub no_cluster: bool,
    /// Explicit consent to mutate; without it every mutating entry point
    /// refuses.
    pub write: bool,
    pub dry_run: bool,
    pub overrides: BTreeMap<String, String>,
}

pub struct Runner<'a> {
    repo: &'a MigrationRepo,
    client: ChClient,
    options: RunnerOptions,
    run_id: String,
}

/// Which databases an operation targets; `All` discovers from the server
/// and skips exclusion groups out loud.
pub enum Targets {
    Databases(Vec<String>),
    Group(String),
    All,
}

/// The outcome of resolving `Targets`: what runs, and what `All` skipped
/// (database, exclusion group) so callers can say so out loud.
pub struct ResolvedTargets {
    pub databases: Vec<String>,
    pub skipped: Vec<(String, String)>,
}

fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\\', "\\\\").replace('\'', "\\'"))
}

fn new_run_id() -> String {
    // v4-shaped from OS randomness; no uuid dependency needed.
    let mut bytes = [0u8; 16];
    getrandom(&mut bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let h = |range: std::ops::Range<usize>| {
        bytes[range]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    };
    format!(
        "{}-{}-{}-{}-{}",
        h(0..4),
        h(4..6),
        h(6..8),
        h(8..10),
        h(10..16)
    )
}

fn getrandom(buffer: &mut [u8]) {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    // RandomState seeds from OS entropy; fold hashes of a counter into
    // the buffer. Not cryptographic, but run ids only need uniqueness.
    let state = RandomState::new();
    for (index, chunk) in buffer.chunks_mut(8).enumerate() {
        let mut hasher = state.build_hasher();
        hasher.write_usize(index);
        let hash = hasher.finish().to_le_bytes();
        for (target, source) in chunk.iter_mut().zip(hash.iter()) {
            *target = *source;
        }
    }
}

impl<'a> Runner<'a> {
    pub fn new(repo: &'a MigrationRepo, options: RunnerOptions) -> Self {
        let client = ChClient::new(options.server.clone());
        Self {
            repo,
            client,
            options,
            run_id: new_run_id(),
        }
    }

    /// The underlying client, for read-only companions like verify.
    pub fn client(&self) -> &ChClient {
        &self.client
    }

    fn tracking_table(&self) -> String {
        format!("{}.zedb_migrations", self.repo.config.tracking.database)
    }

    fn require_write(&self, action: &str) -> Result<(), RunnerError> {
        if self.options.server.read_only || !self.options.write {
            return Err(RunnerError::Refused(format!(
                "{action} mutates the server and this connection is read-only; \
                 re-run with --write to consent"
            )));
        }
        Ok(())
    }

    /// The fleet chain: every non-targeted migration in order.
    fn fleet(&self) -> Vec<&Migration> {
        self.repo
            .migrations
            .iter()
            .filter(|migration| migration.targeted.is_none())
            .collect()
    }

    /// Resolve targets without side effects; `All` reports skips.
    pub async fn resolve_targets(&self, targets: &Targets) -> Result<ResolvedTargets, RunnerError> {
        match targets {
            Targets::Databases(databases) => {
                let mut unique = Vec::new();
                for database in databases {
                    if !unique.contains(database) {
                        unique.push(database.clone());
                    }
                }
                Ok(ResolvedTargets {
                    databases: unique,
                    skipped: Vec::new(),
                })
            }
            Targets::Group(name) => {
                let group = self.repo.exclusions.groups.get(name).ok_or_else(|| {
                    let known: Vec<&String> = self.repo.exclusions.groups.keys().collect();
                    RunnerError::Repo(format!(
                        "unknown group {name:?}; exclusions.toml defines: {known:?}"
                    ))
                })?;
                Ok(ResolvedTargets {
                    databases: group.databases.clone(),
                    skipped: Vec::new(),
                })
            }
            Targets::All => {
                let query = self
                    .repo
                    .config
                    .fleet
                    .registry_query
                    .as_deref()
                    .ok_or_else(|| {
                        RunnerError::Repo(
                            "--all needs [fleet].registry_query in zedb.toml to discover databases"
                                .into(),
                        )
                    })?;
                let result = self
                    .client
                    .query(query)
                    .await
                    .map_err(|error| RunnerError::Server(error.to_string()))?;
                let mut databases: Vec<String> = result
                    .rows
                    .iter()
                    .filter_map(|row| row.first().map(|value| value.to_string()))
                    .collect();
                databases.sort();
                databases.dedup();
                let skipped: Vec<(String, String)> = databases
                    .iter()
                    .filter_map(|database| {
                        self.repo
                            .exclusions
                            .excluded()
                            .find(|(excluded, _)| excluded == database)
                            .map(|(_, group)| (database.clone(), group.to_string()))
                    })
                    .collect();
                databases.retain(|database| !self.repo.exclusions.is_excluded(database));
                if databases.is_empty() {
                    return Err(RunnerError::Repo("no databases discovered".into()));
                }
                Ok(ResolvedTargets { databases, skipped })
            }
        }
    }

    /// Resolve targets and announce anything `All` skipped.
    pub async fn target_databases(&self, targets: &Targets) -> Result<Vec<String>, RunnerError> {
        let resolved = self.resolve_targets(targets).await?;
        for (database, group) in &resolved.skipped {
            eprintln!(
                "skipping excluded database {database} (group {group}); target it with --db/--group"
            );
        }
        Ok(resolved.databases)
    }

    pub async fn ensure_tracking(&self) -> Result<(), RunnerError> {
        let tracking = &self.repo.config.tracking;
        let clustered = !self.options.no_cluster
            && tracking.cluster_param.is_some()
            && self.options.cluster.is_some();
        let (on_cluster, engine_meta, engine_rows) = if clustered {
            let cluster = self.options.cluster.as_deref().expect("checked above");
            (
                format!(" ON CLUSTER {cluster}"),
                "ReplicatedMergeTree('/clickhouse/tables/{uuid}/{shard}', '{replica}')".to_string(),
                "ReplicatedMergeTree('/clickhouse/tables/{uuid}/{shard}', '{replica}')".to_string(),
            )
        } else {
            (String::new(), "MergeTree".into(), "MergeTree".into())
        };
        let database = &tracking.database;
        let statements = [
            format!("CREATE DATABASE IF NOT EXISTS {database}{on_cluster}"),
            format!(
                "CREATE TABLE IF NOT EXISTS {database}.zedb_meta{on_cluster} (\
                 key LowCardinality(String), value String, \
                 recorded_at DateTime64(3) DEFAULT now64(3)) \
                 ENGINE = {engine_meta} ORDER BY key"
            ),
            format!(
                "CREATE TABLE IF NOT EXISTS {database}.zedb_migrations{on_cluster} (\
                 db String, migration UInt32, action LowCardinality(String), \
                 status LowCardinality(String), error Nullable(String), \
                 recorded_at DateTime64(3) DEFAULT now64(3), \
                 duration_secs Decimal(9, 2) DEFAULT 0, run_id UUID, \
                 params Map(String, String)) \
                 ENGINE = {engine_rows} ORDER BY (db, migration)"
            ),
        ];
        for statement in statements {
            self.client
                .execute(&statement)
                .await
                .map_err(|error| RunnerError::Server(error.to_string()))?;
        }
        let seeded = self
            .client
            .query(&format!(
                "SELECT count() FROM {database}.zedb_meta WHERE key = 'tracking_version'"
            ))
            .await
            .map_err(|error| RunnerError::Server(error.to_string()))?;
        let count = seeded
            .rows
            .first()
            .and_then(|row| row.first())
            .map(|value| value.to_string())
            .unwrap_or_default();
        if count == "0" {
            self.client
                .execute(&format!(
                    "INSERT INTO {database}.zedb_meta (key, value) \
                     VALUES ('tracking_version', '1'), ('format', '1')"
                ))
                .await
                .map_err(|error| RunnerError::Server(error.to_string()))?;
        }
        Ok(())
    }

    async fn last_states(&self, database: &str) -> Result<Vec<(u32, String, String)>, RunnerError> {
        let sql = format!(
            "SELECT migration, \
             argMax(action, (recorded_at, run_id)) AS last_action, \
             argMax(status, (recorded_at, run_id)) AS last_status \
             FROM {} WHERE db = {} AND status != 'started' GROUP BY migration",
            self.tracking_table(),
            quote(database)
        );
        let result = match self.client.query(&sql).await {
            Ok(result) => result,
            // A missing tracking table means nothing was ever applied.
            Err(error) if error.to_string().contains("UNKNOWN_TABLE") => {
                return Ok(Vec::new());
            }
            Err(error) => return Err(RunnerError::Server(error.to_string())),
        };
        Ok(result
            .rows
            .iter()
            .filter_map(|row| {
                let migration: u32 = row.first()?.to_string().parse().ok()?;
                Some((migration, row.get(1)?.to_string(), row.get(2)?.to_string()))
            })
            .collect())
    }

    /// Fleet migrations currently applied per the tracking table.
    pub async fn applied_migrations(&self, database: &str) -> Result<BTreeSet<u32>, RunnerError> {
        Ok(self
            .last_states(database)
            .await?
            .into_iter()
            .filter(|(_, action, status)| {
                status == "success" && (action == "upgrade" || action == "stamp")
            })
            .map(|(migration, _, _)| migration)
            .collect())
    }

    /// Targeted customisations currently applied (action `apply`).
    pub async fn applied_customisations(
        &self,
        database: &str,
    ) -> Result<BTreeSet<u32>, RunnerError> {
        Ok(self
            .last_states(database)
            .await?
            .into_iter()
            .filter(|(_, action, status)| status == "success" && action == "apply")
            .map(|(migration, _, _)| migration)
            .collect())
    }

    fn resolve_params(
        &self,
        database: &str,
        sql: &str,
    ) -> Result<BTreeMap<String, String>, RunnerError> {
        let needed = placeholders(sql);
        let mut params: BTreeMap<String, String> = BTreeMap::new();
        params.insert("db".into(), database.to_string());
        match (&self.options.cluster, self.options.no_cluster) {
            (Some(cluster), _) => {
                params.insert("cluster".into(), cluster.clone());
            }
            (None, true) => {
                // Rendered, then stripped again by decluster.
                params.insert("cluster".into(), "unused".into());
            }
            (None, false) => {}
        }
        for (name, config) in &self.repo.config.params {
            if let Some(default) = &config.default {
                params.insert(name.clone(), default.clone());
            }
        }
        for (name, value) in &self.options.overrides {
            params.insert(name.clone(), value.clone());
        }
        let unresolved: Vec<&String> = needed
            .iter()
            .filter(|name| !params.contains_key(*name))
            .collect();
        if let Some(missing) = unresolved.first() {
            if *missing == "cluster" {
                return Err(RunnerError::Repo(format!(
                    "{database}: migration needs ${{cluster}}; pass --cluster (or --no-cluster)"
                )));
            }
            return Err(RunnerError::Repo(format!(
                "{database}: cannot resolve template parameter(s) {unresolved:?}; pass --param name=value"
            )));
        }
        Ok(params)
    }

    /// Strip rendered secrets from text that outlives the run.
    fn redact(&self, text: &str, params: &BTreeMap<String, String>) -> String {
        let mut redacted = text.to_string();
        for (name, value) in params {
            if name.to_lowercase().contains("password") && !value.is_empty() {
                redacted = redacted.replace(value.as_str(), "[redacted]");
            }
        }
        redacted
    }

    #[allow(clippy::too_many_arguments)]
    async fn record(
        &self,
        database: &str,
        migration: u32,
        action: &str,
        status: &str,
        error: Option<&str>,
        duration_secs: f64,
        params: &BTreeMap<String, String>,
    ) -> Result<(), RunnerError> {
        let stored_params: Vec<String> = params
            .iter()
            .filter(|(name, _)| !name.to_lowercase().contains("password"))
            .map(|(name, value)| format!("{}: {}", quote(name), quote(value)))
            .collect();
        let error_sql = match error {
            Some(message) => quote(message),
            None => "NULL".into(),
        };
        let sql = format!(
            "INSERT INTO {} (db, migration, action, status, error, duration_secs, run_id, params) \
             VALUES ({}, {migration}, {}, {}, {error_sql}, {duration_secs:.2}, {}, {{{}}})",
            self.tracking_table(),
            quote(database),
            quote(action),
            quote(status),
            quote(&self.run_id),
            stored_params.join(", ")
        );
        self.client
            .execute(&sql)
            .await
            .map_err(|error| RunnerError::Server(error.to_string()))?;
        self.audit(database, migration, action, status, error);
        Ok(())
    }

    /// Append to the local audit log; failures are reported, never fatal.
    fn audit(
        &self,
        database: &str,
        migration: u32,
        action: &str,
        status: &str,
        error: Option<&str>,
    ) {
        let line = format!(
            "{{\"time\":{:?},\"server\":{:?},\"db\":{:?},\"migration\":{migration},\"action\":{:?},\"status\":{:?},\"run_id\":{:?},\"error\":{:?}}}\n",
            chrono::Utc::now().to_rfc3339(),
            self.options.server.url,
            database,
            action,
            status,
            self.run_id,
            error.unwrap_or_default(),
        );
        let result = (|| -> std::io::Result<()> {
            let dir = dirs::data_local_dir()
                .unwrap_or_else(std::env::temp_dir)
                .join("zedb");
            std::fs::create_dir_all(&dir)?;
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join("audit.jsonl"))?;
            file.write_all(line.as_bytes())
        })();
        if let Err(error) = result {
            eprintln!("warning: could not write audit log: {error}");
        }
    }

    async fn apply_sql(
        &self,
        database: &str,
        migration: &Migration,
        action: &str,
        sql: &str,
    ) -> Result<(), RunnerError> {
        let params = self.resolve_params(database, sql)?;
        let rendered =
            render(sql, &params).map_err(|error| RunnerError::Repo(error.to_string()))?;
        let rendered = if self.options.no_cluster {
            decluster(&rendered)
        } else {
            rendered
        };
        let statements: Vec<String> = split_statements(&rendered)
            .into_iter()
            .filter(|statement| {
                statement
                    .lines()
                    .any(|line| !line.trim().is_empty() && !line.trim().starts_with("--"))
            })
            .collect();

        if self.options.dry_run {
            println!(
                "-- {database}: would {action} {:05} ({} statements)",
                migration.number,
                statements.len()
            );
            return Ok(());
        }

        self.record(
            database,
            migration.number,
            action,
            "started",
            None,
            0.0,
            &params,
        )
        .await?;
        let start = Instant::now();
        for (index, statement) in statements.iter().enumerate() {
            if let Err(error) = self.client.execute(statement.trim()).await {
                let first_sql = statement
                    .lines()
                    .find(|line| !line.trim().is_empty() && !line.trim().starts_with("--"))
                    .unwrap_or("?")
                    .trim();
                let message = self.redact(
                    &format!(
                        "statement {}/{} [{:.120}]: {error}",
                        index + 1,
                        statements.len(),
                        first_sql
                    ),
                    &params,
                );
                self.record(
                    database,
                    migration.number,
                    action,
                    "failed",
                    Some(&message),
                    start.elapsed().as_secs_f64(),
                    &params,
                )
                .await?;
                return Err(RunnerError::Server(format!(
                    "{database}: {action} {:05} failed: {message}",
                    migration.number
                )));
            }
        }
        self.record(
            database,
            migration.number,
            action,
            "success",
            None,
            start.elapsed().as_secs_f64(),
            &params,
        )
        .await?;
        println!(
            "{database}: {action} {:05} ok ({:.2}s)",
            migration.number,
            start.elapsed().as_secs_f64()
        );
        Ok(())
    }

    pub async fn upgrade(
        &self,
        targets: &Targets,
        ceiling: Option<u32>,
    ) -> Result<(), RunnerError> {
        self.require_write("upgrade")?;
        let fleet = self.fleet();
        if let Some(ceiling) = ceiling {
            if !fleet.iter().any(|migration| migration.number == ceiling) {
                return Err(RunnerError::Repo(format!("no migration {ceiling:05}")));
            }
        }
        self.ensure_tracking().await?;
        for database in self.target_databases(targets).await? {
            // `apply` rows count too: a graduated customisation already ran
            // on databases that opted in.
            let mut applied = self.applied_migrations(&database).await?;
            applied.extend(self.applied_customisations(&database).await?);
            let pending: Vec<&&Migration> = fleet
                .iter()
                .filter(|migration| {
                    !applied.contains(&migration.number)
                        && ceiling.is_none_or(|ceiling| migration.number <= ceiling)
                })
                .collect();
            if pending.is_empty() {
                println!("{database}: up to date ({} applied)", applied.len());
                continue;
            }
            for migration in pending {
                let sql = migration
                    .upgrade_sql()
                    .map_err(|error| RunnerError::Repo(error.to_string()))?;
                self.apply_sql(&database, migration, "upgrade", &sql)
                    .await?;
            }
        }
        Ok(())
    }

    fn gate(
        &self,
        migration: &Migration,
        acknowledge_irreversible: bool,
    ) -> Result<(), RunnerError> {
        if migration.number == 0 {
            return Ok(());
        }
        match migration.rollback_class {
            None => Err(RunnerError::Repo(format!(
                "{:05} has no rollback.sql",
                migration.number
            ))),
            Some(RollbackClass::Irreversible) if !acknowledge_irreversible => {
                Err(RunnerError::Refused(format!(
                    "{:05} declares its rollback irreversible; running it does not \
                     restore the previous state. Re-run with --irreversible to \
                     execute it anyway.",
                    migration.number
                )))
            }
            Some(_) => Ok(()),
        }
    }

    async fn roll(&self, database: &str, migration: &Migration) -> Result<(), RunnerError> {
        match migration.rollback_class {
            Some(RollbackClass::Structural) => println!(
                "{database}: WARNING: {:05} rollback is structural: schema is \
                 restored but newer data may be lost",
                migration.number
            ),
            Some(RollbackClass::Irreversible) => println!(
                "{database}: WARNING: {:05} rollback is declared IRREVERSIBLE and \
                 does not restore the previous state",
                migration.number
            ),
            _ => {}
        }
        let sql = migration
            .rollback_sql()
            .map_err(|error| RunnerError::Repo(error.to_string()))?
            .ok_or_else(|| {
                RunnerError::Repo(format!("{:05} has no rollback.sql", migration.number))
            })?;
        self.apply_sql(database, migration, "rollback", &sql).await
    }

    pub async fn rollback_to(
        &self,
        targets: &Targets,
        floor: u32,
        acknowledge_irreversible: bool,
    ) -> Result<(), RunnerError> {
        self.require_write("rollback")?;
        let fleet: BTreeMap<u32, &Migration> = self
            .fleet()
            .into_iter()
            .map(|migration| (migration.number, migration))
            .collect();
        if !fleet.contains_key(&floor) {
            return Err(RunnerError::Repo(format!("no migration {floor:05}")));
        }
        self.ensure_tracking().await?;
        for database in self.target_databases(targets).await? {
            let applied = self.applied_migrations(&database).await?;
            let todo: Vec<u32> = applied
                .iter()
                .rev()
                .copied()
                .filter(|number| *number > floor && fleet.contains_key(number))
                .collect();
            if todo.is_empty() {
                println!("{database}: already at or below {floor:05}");
                continue;
            }
            // Refuse before touching anything if the walk cannot complete.
            for number in &todo {
                self.gate(fleet[number], acknowledge_irreversible)?;
            }
            for number in todo {
                self.roll(&database, fleet[&number]).await?;
            }
        }
        Ok(())
    }

    pub async fn rollback_one(
        &self,
        targets: &Targets,
        number: u32,
        acknowledge_irreversible: bool,
        acknowledge_targeted: bool,
    ) -> Result<(), RunnerError> {
        self.require_write("rollback")?;
        let migration = self
            .repo
            .migration(number)
            .ok_or_else(|| RunnerError::Repo(format!("no migration {number:05}")))?;
        self.gate(migration, acknowledge_irreversible)?;
        self.ensure_tracking().await?;

        if migration.targeted.is_some() {
            // Off-chain, but removing a deliberate customisation deserves
            // friction.
            if !acknowledge_targeted {
                return Err(RunnerError::Refused(format!(
                    "{number:05} is a targeted customisation; someone applied it on \
                     purpose. Re-run with --targeted to confirm removing it."
                )));
            }
            for database in self.target_databases(targets).await? {
                if !self
                    .applied_customisations(&database)
                    .await?
                    .contains(&number)
                {
                    println!("{database}: customisation {number:05} not applied, skipping");
                    continue;
                }
                println!("{database}: WARNING: removing customisation {number:05}");
                self.roll(&database, migration).await?;
            }
            return Ok(());
        }

        for database in self.target_databases(targets).await? {
            let applied = self.applied_migrations(&database).await?;
            if !applied.contains(&number) {
                println!("{database}: {number:05} not applied, skipping");
                continue;
            }
            let top = *applied.iter().next_back().expect("applied is non-empty");
            if number != top {
                return Err(RunnerError::Refused(format!(
                    "{database}: {number:05} is not the latest applied ({top:05} is); \
                     rollbacks only peel from the top"
                )));
            }
            self.roll(&database, migration).await?;
        }
        Ok(())
    }

    /// Record databases as having the fleet schema through `number`
    /// without executing anything (adopting existing databases).
    pub async fn stamp(&self, targets: &Targets, number: u32) -> Result<(), RunnerError> {
        self.require_write("stamp")?;
        let fleet = self.fleet();
        if !fleet.iter().any(|migration| migration.number == number) {
            return Err(RunnerError::Repo(format!("no migration {number:05}")));
        }
        self.ensure_tracking().await?;
        for database in self.target_databases(targets).await? {
            let applied = self.applied_migrations(&database).await?;
            for migration in &fleet {
                if migration.number <= number && !applied.contains(&migration.number) {
                    self.record(
                        &database,
                        migration.number,
                        "stamp",
                        "success",
                        None,
                        0.0,
                        &BTreeMap::new(),
                    )
                    .await?;
                }
            }
            println!("{database}: stamped through {number:05}");
        }
        Ok(())
    }

    /// Map ancestor tracking rows (`default.schema_migrations`) into the
    /// format-1 tables, preserving recorded_at ordering so argMax-latest
    /// state carries over; returns how many rows moved. Idempotent-ish:
    /// refuses when zedb_migrations already has rows.
    pub async fn import_tracking(&self, source_table: &str) -> Result<u64, RunnerError> {
        self.require_write("import-tracking")?;
        self.ensure_tracking().await?;
        let existing = self
            .client
            .query(&format!("SELECT count() FROM {}", self.tracking_table()))
            .await
            .map_err(|error| RunnerError::Server(error.to_string()))?;
        let count = existing
            .rows
            .first()
            .and_then(|row| row.first())
            .map(|value| value.to_string())
            .unwrap_or_default();
        if count != "0" {
            return Err(RunnerError::Refused(format!(
                "{} already has {count} row(s); refusing to import on top",
                self.tracking_table()
            )));
        }
        self.client
            .execute(&format!(
                "INSERT INTO {} (db, migration, action, status, error, recorded_at,                  duration_secs, run_id, params)                  SELECT db, migration, action, status, error, recorded_at,                  duration_secs, run_id, map() FROM {source_table}",
                self.tracking_table()
            ))
            .await
            .map_err(|error| RunnerError::Server(error.to_string()))?;
        let imported = self
            .client
            .query(&format!("SELECT count() FROM {}", self.tracking_table()))
            .await
            .map_err(|error| RunnerError::Server(error.to_string()))?;
        let imported: u64 = imported
            .rows
            .first()
            .and_then(|row| row.first())
            .and_then(|value| value.to_string().parse().ok())
            .unwrap_or(0);
        self.audit("*", 0, "import-tracking", "success", None);
        Ok(imported)
    }

    /// Apply one targeted migration to specific databases.
    pub async fn apply_targeted(&self, targets: &Targets, number: u32) -> Result<(), RunnerError> {
        self.require_write("apply")?;
        let migration = self
            .repo
            .migration(number)
            .ok_or_else(|| RunnerError::Repo(format!("no migration {number:05}")))?;
        let Some(allow_list) = &migration.targeted else {
            return Err(RunnerError::Repo(format!(
                "{number:05} is not targeted; fleet migrations run via upgrade"
            )));
        };
        self.ensure_tracking().await?;
        for database in self.target_databases(targets).await? {
            if !allow_list.is_empty() && !allow_list.contains(&database) {
                return Err(RunnerError::Refused(format!(
                    "{database}: {number:05} restricts targets to {allow_list:?}"
                )));
            }
            if self
                .applied_customisations(&database)
                .await?
                .contains(&number)
            {
                println!("{database}: customisation {number:05} already applied, skipping");
                continue;
            }
            let sql = migration
                .upgrade_sql()
                .map_err(|error| RunnerError::Repo(error.to_string()))?;
            self.apply_sql(&database, migration, "apply", &sql).await?;
        }
        Ok(())
    }
}

pub struct DatabaseStatus {
    pub database: String,
    pub head: Option<u32>,
    pub latest: u32,
    pub pending: Vec<u32>,
    pub customised: Vec<u32>,
    pub failed: Vec<(u32, String)>,
}

impl Runner<'_> {
    /// Per-database chain position; read-only (no tracking bootstrap).
    pub async fn status(&self, targets: &Targets) -> Result<Vec<DatabaseStatus>, RunnerError> {
        let fleet = self.fleet();
        let latest = fleet.last().map(|migration| migration.number).unwrap_or(0);
        let mut statuses = Vec::new();
        for database in self.target_databases(targets).await? {
            let states = self.last_states(&database).await?;
            let applied: BTreeSet<u32> = states
                .iter()
                .filter(|(_, action, status)| {
                    status == "success" && (action == "upgrade" || action == "stamp")
                })
                .map(|(migration, _, _)| *migration)
                .collect();
            let customised: Vec<u32> = states
                .iter()
                .filter(|(_, action, status)| status == "success" && action == "apply")
                .map(|(migration, _, _)| *migration)
                .collect();
            let failed: Vec<(u32, String)> = states
                .iter()
                .filter(|(_, _, status)| status == "failed")
                .map(|(migration, action, _)| (*migration, action.clone()))
                .collect();
            let pending: Vec<u32> = fleet
                .iter()
                .filter(|migration| {
                    !applied.contains(&migration.number) && !customised.contains(&migration.number)
                })
                .map(|migration| migration.number)
                .collect();
            statuses.push(DatabaseStatus {
                database,
                head: applied.iter().next_back().copied(),
                latest,
                pending,
                customised,
                failed,
            });
        }
        Ok(statuses)
    }
}

/// Statements that would be routed through admin credentials in the
/// ancestor tooling. Format v1 does not implement admin routing yet; the
/// helper exists so callers can warn.
pub fn needs_admin(statement: &str) -> bool {
    is_access_control(statement)
}
