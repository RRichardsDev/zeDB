use super::*;

impl Runner<'_> {
    pub(super) fn resolve_params(
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
    pub(super) fn redact(&self, text: &str, params: &BTreeMap<String, String>) -> String {
        redact_params(text, params)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn record(
        &self,
        database: &str,
        migration: u32,
        action: &str,
        status: &str,
        error: Option<&str>,
        duration_secs: f64,
        params: &BTreeMap<String, String>,
    ) -> Result<(), RunnerError> {
        // Resolved template values may be credentials regardless of their
        // names. Tracking records the run outcome, never parameter values.
        let _ = params;
        let error_sql = match error {
            Some(message) => quote(message),
            None => "NULL".into(),
        };
        let sql = format!(
            "INSERT INTO {} (repo, db, migration, action, status, error, duration_secs, run_id, params) \
             VALUES ({}, {}, {migration}, {}, {}, {error_sql}, {duration_secs:.2}, {}, {{{}}})",
            self.tracking_table(),
            quote(&self.tracking_repo()),
            quote(database),
            quote(action),
            quote(status),
            quote(&self.run_id),
            ""
        );
        self.client
            .execute(&sql)
            .await
            .map_err(|error| RunnerError::Server(error.to_string()))?;
        self.audit(database, migration, action, status, error);
        Ok(())
    }

    /// Append to the local audit log; failures are reported, never fatal.
    pub(super) fn audit(
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
            audit_endpoint(&self.options.server.url),
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
            let metadata = std::fs::symlink_metadata(&dir)?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(std::io::Error::other(
                    "audit directory is not a real directory",
                ));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
            }
            append_audit_line(&dir.join("audit.jsonl"), line.as_bytes())
        })();
        if let Err(error) = result {
            eprintln!("warning: could not write audit log: {error}");
        }
    }

    pub(super) async fn apply_sql(
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
}

fn redact_params(text: &str, params: &BTreeMap<String, String>) -> String {
    let mut redacted = text.to_string();
    for (name, value) in params {
        if name != "db" && name != "cluster" && !value.is_empty() {
            redacted = redacted.replace(value.as_str(), "[redacted]");
        }
    }
    redacted
}

fn audit_endpoint(endpoint: &str) -> String {
    let Ok(url) = reqwest::Url::parse(endpoint) else {
        return "[invalid endpoint]".into();
    };
    let Some(host) = url.host_str() else {
        return "[invalid endpoint]".into();
    };
    match url.port() {
        Some(port) => format!("{}://{host}:{port}", url.scheme()),
        None => format!("{}://{host}", url.scheme()),
    }
}

fn append_audit_line(path: &std::path::Path, line: &[u8]) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options
            .mode(0o600)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    }
    let mut file = options.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(line)
}

#[cfg(test)]
mod security_tests {
    use super::*;

    #[test]
    fn redacts_all_custom_parameter_values() {
        let params = BTreeMap::from([
            ("db".into(), "analytics".into()),
            ("cluster".into(), "main".into()),
            ("api_key".into(), "key-123".into()),
            ("signed_url".into(), "token-456".into()),
        ]);
        let redacted = redact_params("analytics main failed with key-123 at token-456", &params);
        assert_eq!(
            redacted,
            "analytics main failed with [redacted] at [redacted]"
        );
    }

    #[test]
    fn audit_endpoint_discards_credentials_paths_and_queries() {
        assert_eq!(
            audit_endpoint("https://user:pass@db.example.com:8443/signed/path?token=secret"),
            "https://db.example.com:8443"
        );
    }

    #[cfg(unix)]
    #[test]
    fn audit_file_is_restricted_even_when_it_already_exists() {
        use std::os::unix::fs::PermissionsExt as _;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("audit.jsonl");
        std::fs::write(&path, b"old\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        append_audit_line(&path, b"new\n").unwrap();
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[cfg(unix)]
    #[test]
    fn audit_file_refuses_symlink_targets() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let outside = directory.path().join("outside");
        let audit = directory.path().join("audit.jsonl");
        std::fs::write(&outside, b"keep\n").unwrap();
        symlink(&outside, &audit).unwrap();

        assert!(append_audit_line(&audit, b"new\n").is_err());
        assert_eq!(std::fs::read(outside).unwrap(), b"keep\n");
    }
}
