use super::*;

impl McpServer {
    pub(super) fn schema_cache_available(&self) -> bool {
        self.schema_cache_path
            .as_deref()
            .is_some_and(|path| path.is_file())
    }

    /// Fresh snapshot plus a human freshness stamp, read from disk.
    fn schema_snapshot(&self) -> Result<(std::sync::Arc<SchemaSnapshot>, String), String> {
        let path = self
            .schema_cache_path
            .as_deref()
            .ok_or("no schema cache configured; use the live tools")?;
        let cache = crate::schema_cache::SchemaCache::open(path)
            .map_err(|error| format!("schema cache unreadable: {error}"))?;
        let snapshot = cache.snapshot();
        if snapshot.databases.is_empty() {
            return Err("schema cache is empty; use the live tools".into());
        }
        let refreshed =
            chrono::DateTime::<chrono::Utc>::from_timestamp_millis(snapshot.refreshed_at_ms as i64)
                .map(|time| time.format("%Y-%m-%d %H:%M UTC").to_string())
                .unwrap_or_else(|| "unknown".into());
        Ok((snapshot, format!("cached as of {refreshed}")))
    }

    pub(super) fn tool_schema_search(&self, arguments: &Value) -> Result<String, String> {
        let query = arguments
            .get("query")
            .and_then(Value::as_str)
            .ok_or("schema_search needs a query")?;
        let (snapshot, freshness) = self.schema_snapshot()?;
        let (hits, total) = schema_intelligence::schema_search(&snapshot, query, 50);
        if hits.is_empty() {
            return Ok(format!(
                "no cached schema names match {query:?} ({freshness}); \
                 columns only match in databases that have been browsed"
            ));
        }
        let mut lines: Vec<String> = hits
            .iter()
            .map(|hit| format!("{} ({})", hit.path, hit.detail))
            .collect();
        if total > lines.len() {
            lines.push(format!("... {} more matches", total - lines.len()));
        }
        lines.push(freshness);
        Ok(lines.join("\n"))
    }

    pub(super) fn tool_lint_sql(&self, arguments: &Value) -> Result<String, String> {
        let sql = arguments
            .get("sql")
            .and_then(Value::as_str)
            .ok_or("lint_sql needs sql")?;
        let database = arguments
            .get("database")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                self.config
                    .as_ref()
                    .and_then(|config| config.database.clone())
            });
        let (snapshot, freshness) = self.schema_snapshot()?;
        let issues = schema_intelligence::analyze_sql(&snapshot, database.as_deref(), sql);
        if issues.is_empty() {
            return Ok(format!(
                "no issues found against the cached schema ({freshness}). \
                 The check is conservative: ambiguous or uncached names \
                 pass silently, so confirm with describe before DDL."
            ));
        }
        let mut lines: Vec<String> = issues
            .iter()
            .map(|issue| {
                let line = sql[..issue.range.start]
                    .bytes()
                    .filter(|byte| *byte == b'\n')
                    .count()
                    + 1;
                format!("line {line}: {}", issue.message)
            })
            .collect();
        lines.push(freshness);
        Ok(lines.join("\n"))
    }

    pub(super) async fn tool_fleet_status(&self) -> Result<String, String> {
        let repo = self.effective_repo().await?;
        let runner = self.runner_for(&repo)?;
        let resolved = runner
            .resolve_targets(&Targets::All)
            .await
            .map_err(|error| error.to_string())?;
        let statuses = runner
            .status(&Targets::Databases(resolved.databases))
            .await
            .map_err(|error| error.to_string())?;
        let mut out = String::new();
        for status in statuses {
            let head = status
                .head
                .map(|head| format!("{head:05}"))
                .unwrap_or_else(|| "none".into());
            out.push_str(&format!(
                "{}: head {head}, pending [{}], customised [{}], failed [{}]\n",
                status.database,
                join_numbers(&status.pending),
                join_numbers(&status.customised),
                status
                    .failed
                    .iter()
                    .map(|(number, error)| format!("{number:05}: {error}"))
                    .collect::<Vec<_>>()
                    .join("; "),
            ));
        }
        for (database, group) in resolved.skipped {
            out.push_str(&format!("{database}: excluded (group {group})\n"));
        }
        Ok(out)
    }

    pub(super) async fn tool_list_migrations(&self) -> Result<String, String> {
        let repo = self.effective_repo().await?;
        let mut out = String::new();
        for migration in &repo.migrations {
            out.push_str(&format!(
                "{:05}  {}/{:02}  {}  {}{}\n",
                migration.number,
                migration.year,
                migration.month,
                migration
                    .rollback_class
                    .map(|class| class.as_str())
                    .unwrap_or("irreversible (no rollback)"),
                if migration.targeted.is_some() {
                    "targeted  "
                } else {
                    ""
                },
                migration.headline().unwrap_or_default(),
            ));
        }
        out.push_str(&format!(
            "\nTemplating: ${{db}} is the target database, ${{cluster}} the cluster; \
             declared params: {}\n",
            repo.config
                .params
                .keys()
                .cloned()
                .collect::<Vec<_>>()
                .join(", "),
        ));
        Ok(out)
    }

    pub(super) async fn tool_migration_sql(&self, arguments: &Value) -> Result<String, String> {
        let repo = self.effective_repo().await?;
        let number = arguments
            .get("number")
            .and_then(Value::as_u64)
            .ok_or("number argument required")? as u32;
        let migration = repo
            .migration(number)
            .ok_or_else(|| format!("no migration {number:05}"))?;
        let upgrade = migration.upgrade_sql().map_err(|error| error.to_string())?;
        let rollback = migration
            .rollback_sql()
            .map_err(|error| error.to_string())?;
        Ok(match rollback {
            Some(rollback) => {
                format!("-- upgrade.sql\n{upgrade}\n-- rollback.sql\n{rollback}")
            }
            None => format!("-- upgrade.sql\n{upgrade}\n-- no rollback.sql (irreversible)"),
        })
    }

    pub(super) async fn tool_dry_run(&self, arguments: &Value) -> Result<String, String> {
        let repo = self.effective_repo().await?;
        let database = required_str(arguments, "database")?;
        let runner = self.runner_for(&repo)?;
        let applied = runner
            .applied_migrations(database)
            .await
            .map_err(|error| error.to_string())?;
        let mut params: BTreeMap<String, String> = BTreeMap::new();
        for (name, config) in &repo.config.params {
            if let Some(default) = &config.default {
                params.insert(name.clone(), default.clone());
            }
        }
        params.insert("db".into(), database.to_string());
        let mut out = String::new();
        for migration in &repo.migrations {
            if migration.targeted.is_some() || applied.contains(&migration.number) {
                continue;
            }
            let sql = migration.upgrade_sql().map_err(|error| error.to_string())?;
            let mut rendered = sql;
            for (name, value) in &params {
                rendered = rendered.replace(&format!("${{{name}}}"), value);
            }
            out.push_str(&format!(
                "-- migration {:05} (pending; unresolved ${{...}} left visible)\n{}\n",
                migration.number,
                rendered.trim_end()
            ));
        }
        if out.is_empty() {
            out = format!("{database} is up to date; nothing would run\n");
        }
        Ok(out)
    }

    pub(super) async fn tool_drift(&self, arguments: &Value) -> Result<String, String> {
        let repo = self.effective_repo().await?;
        let database = required_str(arguments, "database")?;
        let runner = self.runner_for(&repo)?;
        let binary = crate::ensure_binary(&repo.config.engine.version)
            .await
            .map_err(|error| error.to_string())?;
        let verifier = Verifier::new(&repo, &runner, binary);
        let drifts = verifier
            .verify(&Targets::Databases(vec![database.to_string()]))
            .await
            .map_err(|error| error.to_string())?;
        let mut out = String::new();
        for drift in drifts {
            if drift.findings.is_empty() {
                out.push_str(&format!("{}: no drift\n", drift.database));
            } else {
                out.push_str(&format!("{}:\n", drift.database));
                for finding in drift.findings {
                    out.push_str(&format!("  {finding}\n"));
                }
            }
        }
        Ok(out)
    }

    pub(super) async fn tool_list_databases(&self) -> Result<String, String> {
        self.run_capped("SELECT name FROM system.databases ORDER BY name")
            .await
    }

    pub(super) async fn tool_list_tables(&self, arguments: &Value) -> Result<String, String> {
        let database = required_str(arguments, "database")?;
        self.run_capped(&format!(
            "SELECT name, engine, total_rows FROM system.tables \
             WHERE database = {} ORDER BY name",
            sql_quote(database)
        ))
        .await
    }

    pub(super) async fn tool_describe(&self, arguments: &Value) -> Result<String, String> {
        let database = required_str(arguments, "database")?;
        let table = required_str(arguments, "table")?;
        self.run_capped(&format!(
            "SELECT name, type, default_expression, comment FROM system.columns \
             WHERE database = {} AND table = {} ORDER BY position",
            sql_quote(database),
            sql_quote(table)
        ))
        .await
    }

    pub(super) async fn tool_run_query(&self, arguments: &Value) -> Result<String, String> {
        let sql = required_str(arguments, "sql")?;
        self.run_capped(sql).await
    }

    async fn run_capped(&self, sql: &str) -> Result<String, String> {
        let client = self.client()?;
        let result = client
            .query_guarded(
                sql,
                self.caps.max_execution_time_secs,
                self.caps.max_result_rows,
                self.caps.max_bytes_to_read,
            )
            .await
            .map_err(|error| error.to_string())?;
        let mut out = String::new();
        out.push_str(
            &result
                .columns
                .iter()
                .map(|column| column.name.clone())
                .collect::<Vec<_>>()
                .join("\t"),
        );
        out.push('\n');
        for row in &result.rows {
            out.push_str(
                &row.iter()
                    .map(|value| value.to_string())
                    .collect::<Vec<_>>()
                    .join("\t"),
            );
            out.push('\n');
        }
        if result.rows.len() as u64 >= self.caps.max_result_rows {
            out.push_str(&format!(
                "(capped at {} rows; narrow the query for more)\n",
                self.caps.max_result_rows
            ));
        }
        Ok(out)
    }
}
