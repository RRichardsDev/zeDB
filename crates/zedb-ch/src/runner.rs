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

use crate::replay::{decluster, split_statements};
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
    /// Optional elevated connection: statements needing grants the
    /// migration user deliberately lacks (OPTIMIZE, TRUNCATE, structural
    /// ALTER, functions, SYSTEM, definers) route here, exactly as the
    /// ancestor tooling did.
    pub admin: Option<ChConfig>,
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
    admin: Option<ChClient>,
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
        let admin = options.admin.clone().map(ChClient::new);
        Self {
            repo,
            client,
            admin,
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

    /// One executor per replica of the configured cluster, for node-local
    /// SYSTEM statements. Replicas come from system.clusters on the
    /// connected node and are reached on the same HTTP port and
    /// credentials; the connected node keeps its existing client. A
    /// single-replica cluster returns empty, meaning "use the current
    /// executor, no extra connections".
    async fn system_executors(&self, admin_routed: bool) -> Result<Vec<ChClient>, RunnerError> {
        let Some(cluster) = &self.options.cluster else {
            return Ok(Vec::new());
        };
        let rows = self
            .client
            .query(&format!(
                "SELECT DISTINCT host_name FROM system.clusters WHERE cluster = {}",
                quote(cluster)
            ))
            .await
            .map_err(|error| RunnerError::Server(error.to_string()))?;
        let hosts: Vec<String> = rows
            .rows
            .iter()
            .filter_map(|row| row.first().map(|value| value.to_string()))
            .collect();
        if hosts.len() <= 1 {
            return Ok(Vec::new());
        }
        let base = if admin_routed {
            self.options
                .admin
                .as_ref()
                .expect("admin_routed implies admin")
        } else {
            &self.options.server
        };
        let connected_host = host_of(&base.url);
        Ok(hosts
            .iter()
            .map(|host| {
                let mut config = base.clone();
                if Some(host.as_str()) != connected_host.as_deref() {
                    config.url = replace_host(&base.url, host);
                }
                ChClient::new(config)
            })
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
        // SYSTEM statements (START/REFRESH VIEW, ...) take no ON CLUSTER
        // and act on the connected node only, but each replica keeps its
        // own refresh scheduler, so they must run on every replica of the
        // cluster. Executors are discovered once per run.
        let mut system_executors: Option<Vec<ChClient>> = None;
        for (index, statement) in statements.iter().enumerate() {
            let routed_admin = self.admin.is_some() && needs_admin(statement);
            let executor = match &self.admin {
                Some(admin) if routed_admin => admin,
                _ => &self.client,
            };
            let fan_out = is_system(statement) && !self.options.no_cluster;
            let result = if fan_out {
                if system_executors.is_none() {
                    system_executors = Some(self.system_executors(routed_admin).await?);
                }
                let mut outcome = Ok(());
                match system_executors.as_ref().expect("populated above") {
                    replicas if replicas.is_empty() => {
                        outcome = executor.execute(statement.trim()).await;
                    }
                    replicas => {
                        for replica in replicas {
                            outcome = replica.execute(statement.trim()).await;
                            if outcome.is_err() {
                                break;
                            }
                        }
                    }
                }
                outcome
            } else {
                executor.execute(statement.trim()).await
            };
            if let Err(error) = result {
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
        self.apply_targeted_inner(targets, number, true).await
    }

    /// The lifecycle check's smoke test: the allow list is fleet
    /// deployment policy about real databases, and the check's
    /// ephemeral database can never be on it, so the check bypasses it
    /// to keep coverage of restricted targeted migrations. Crate-only:
    /// no real apply path can reach this.
    pub(crate) async fn apply_targeted_for_check(
        &self,
        targets: &Targets,
        number: u32,
    ) -> Result<(), RunnerError> {
        self.apply_targeted_inner(targets, number, false).await
    }

    async fn apply_targeted_inner(
        &self,
        targets: &Targets,
        number: u32,
        enforce_allow_list: bool,
    ) -> Result<(), RunnerError> {
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
            if enforce_allow_list && !allow_list.is_empty() && !allow_list.contains(&database) {
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

use std::sync::LazyLock;

static NEEDS_ADMIN: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"(?i)^(OPTIMIZE|TRUNCATE|ALTER\s+TABLE|CREATE\s+FUNCTION|DROP\s+FUNCTION|SYSTEM)\b",
    )
    .expect("static regex")
});
static NEEDS_ADMIN_ANYWHERE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?i)\bDEFINER\b").expect("static regex"));

fn body_lines(statement: &str) -> Vec<&str> {
    statement
        .lines()
        .filter(|line| !line.trim().is_empty() && !line.trim().starts_with("--"))
        .collect()
}

/// Statements needing elevated grants, judged on the statement body.
/// Definers route to admin as belt and braces (the migration user may
/// hold SET DEFINER, but admin is the reliable executor).
pub fn needs_admin(statement: &str) -> bool {
    let lines = body_lines(statement);
    let Some(first) = lines.first() else {
        return false;
    };
    NEEDS_ADMIN.is_match(first.trim())
        || NEEDS_ADMIN_ANYWHERE.is_match(&lines.join(
            "
",
        ))
}

/// SYSTEM statements act on the connected node only (no ON CLUSTER on
/// the target servers), so the runner fans them out per replica.
pub fn is_system(statement: &str) -> bool {
    body_lines(statement).first().is_some_and(|first| {
        let trimmed = first.trim();
        trimmed.len() >= 6
            && trimmed[..6].eq_ignore_ascii_case("SYSTEM")
            && !trimmed
                .as_bytes()
                .get(6)
                .is_some_and(|c| c.is_ascii_alphanumeric())
    })
}

/// The host part of an `http://host:port` URL.
fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let authority = rest.split('/').next().unwrap_or(rest);
    Some(
        authority
            .rsplit_once(':')
            .map(|(host, _)| host)
            .unwrap_or(authority)
            .to_string(),
    )
}

/// Swap the host in an `http://host:port` URL, keeping scheme and port.
fn replace_host(url: &str, host: &str) -> String {
    let (scheme, rest) = url.split_once("://").unwrap_or(("http", url));
    let (authority, path) = rest
        .split_once('/')
        .map(|(a, p)| (a, format!("/{p}")))
        .unwrap_or((rest, String::new()));
    let port = authority
        .rsplit_once(':')
        .map(|(_, port)| format!(":{port}"))
        .unwrap_or_default();
    format!("{scheme}://{host}{port}{path}")
}

/// Statements the migration user is genuinely *refused* (not merely
/// routed): what proves admin routing is load-bearing.
pub fn refused_without_admin(statement: &str) -> bool {
    body_lines(statement)
        .first()
        .is_some_and(|first| NEEDS_ADMIN.is_match(first.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_statements_are_classified_by_first_body_line() {
        assert!(is_system("-- restart the scheduler\nSYSTEM START VIEW x"));
        assert!(is_system("SYSTEM REFRESH VIEW RefreshableViews.db_X"));
        assert!(!is_system(
            "CREATE TABLE system_log (x UInt8) ENGINE = Memory"
        ));
        assert!(!is_system("SELECT * FROM system.tables"));
    }

    #[test]
    fn replica_urls_keep_scheme_and_port() {
        assert_eq!(
            host_of("http://localhost:8123").as_deref(),
            Some("localhost")
        );
        assert_eq!(
            replace_host("http://localhost:8123", "clickhouse-2"),
            "http://clickhouse-2:8123"
        );
        assert_eq!(
            replace_host("http://10.0.0.1:8443/path", "node-b"),
            "http://node-b:8443/path"
        );
    }
}
