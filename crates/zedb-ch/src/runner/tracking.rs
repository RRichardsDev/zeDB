use super::*;

impl Runner<'_> {
    pub async fn ensure_tracking(&self) -> Result<(), RunnerError> {
        self.validate_sql_identifiers()?;
        if self.options.dry_run {
            return Ok(());
        }
        let tracking = &self.repo.config.tracking;
        let clustered = !self.options.no_cluster
            && tracking.cluster_param.is_some()
            && self.options.cluster.is_some();
        let (on_cluster, engine_meta, engine_rows) = if clustered {
            let cluster = self.options.cluster.as_deref().expect("checked above");
            (
                format!(" ON CLUSTER {}", backtick_identifier(cluster)),
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
                 repo LowCardinality(String) DEFAULT 'default', \
                 db String, migration UInt32, action LowCardinality(String), \
                 status LowCardinality(String), error Nullable(String), \
                 recorded_at DateTime64(3) DEFAULT now64(3), \
                 duration_secs Decimal(9, 2) DEFAULT 0, run_id UUID, \
                 params Map(String, String)) \
                 ENGINE = {engine_rows} ORDER BY (repo, db, migration)"
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

    pub(super) async fn last_states(
        &self,
        database: &str,
    ) -> Result<Vec<(u32, String, String)>, RunnerError> {
        self.validate_sql_identifiers()?;
        let sql = format!(
            "SELECT migration, \
             argMax(action, (recorded_at, run_id)) AS last_action, \
             argMax(status, (recorded_at, run_id)) AS last_status \
             FROM {} WHERE repo = {} AND db = {} AND status != 'started' GROUP BY migration",
            self.tracking_table(),
            quote(&self.tracking_repo()),
            quote(database)
        );
        let result = match self.client.query(&sql).await {
            Ok(result) => result,
            // A missing tracking table or database (code 60 / 81: a
            // server where nothing was ever applied, e.g. a fresh
            // Cloud service) means exactly that: nothing applied. The
            // error must NAME the tracking store: anything else (a
            // transient failure that merely resembles these codes)
            // surfaces instead of silently reading as empty history.
            Err(error)
                if {
                    let text = error.to_string();
                    let missing_kind = text.contains("UNKNOWN_TABLE")
                        || text.contains("UNKNOWN_DATABASE")
                        || text.contains("(code 60)")
                        || text.contains("(code 81)");
                    missing_kind && text.contains(&self.repo.config.tracking.database)
                } =>
            {
                return Ok(Vec::new());
            }
            Err(error) => return Err(RunnerError::Server(error.to_string())),
        };
        if result.rows.is_empty() && std::env::var_os("ZEDB_TRACKING_DEBUG").is_some() {
            // Diagnostic for the flaky empty-read: what does this
            // server actually hold, and which server is it?
            let total = self
                .client
                .query(&format!(
                    "SELECT repo, db, migration, action, status FROM {} ORDER BY recorded_at LIMIT 12",
                    self.tracking_table()
                ))
                .await
                .map(|r| format!("{:?}", r.rows))
                .unwrap_or_else(|e| format!("err: {e}"));
            let path = self
                .client
                .query("SELECT path FROM system.disks WHERE name='default'")
                .await
                .map(|r| format!("{:?}", r.rows))
                .unwrap_or_else(|e| format!("err: {e}"));
            eprintln!(
                "[tracking-debug] empty last_states for db={} client_url={} table_total={} server_path={}",
                terminal_field(database),
                terminal_field(&self.options.server.url),
                terminal_field(&total),
                terminal_field(&path)
            );
        }
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
    pub(super) async fn system_executors(
        &self,
        admin_routed: bool,
    ) -> Result<Vec<ChClient>, RunnerError> {
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
}
