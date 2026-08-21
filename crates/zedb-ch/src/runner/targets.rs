use super::*;

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

    pub(super) fn tracking_table(&self) -> String {
        format!("{}.zedb_migrations", self.repo.config.tracking.database)
    }

    /// This repo's identity in the tracking rows; several repos can
    /// share one tracking database without reading each other's rows.
    pub(super) fn tracking_repo(&self) -> String {
        self.repo.config.tracking.repo.clone().unwrap_or_else(|| {
            self.repo
                .root
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| "default".into())
        })
    }

    pub(super) fn require_write(&self, action: &str) -> Result<(), RunnerError> {
        self.validate_sql_identifiers()?;
        if self.options.server.read_only || !self.options.write {
            return Err(RunnerError::Refused(format!(
                "{action} mutates the server and this connection is read-only; \
                 re-run with --write to consent"
            )));
        }
        Ok(())
    }

    /// The tracking database is interpolated into SQL as a bare
    /// identifier, so it must be a plain identifier chunk. The cluster
    /// name is NOT validated here: where the runner itself interpolates
    /// it (tracking DDL) it is backtick-quoted, and hyphenated cluster
    /// names are legal and common, so read paths must not reject them.
    pub(super) fn validate_sql_identifiers(&self) -> Result<(), RunnerError> {
        validate_identifier(&self.repo.config.tracking.database, "tracking database")
    }

    /// The fleet chain: every non-targeted migration in order.
    pub(super) fn fleet(&self) -> Vec<&Migration> {
        self.repo
            .migrations
            .iter()
            .filter(|migration| migration.targeted.is_none())
            .collect()
    }

    /// Resolve targets without side effects; `All` reports skips.
    pub async fn resolve_targets(&self, targets: &Targets) -> Result<ResolvedTargets, RunnerError> {
        self.validate_sql_identifiers()?;
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
                // Without a configured registry, the fleet is every
                // non-system database except the tracking database
                // itself; [fleet].registry_query narrows it.
                let default_query;
                let query = match self.repo.config.fleet.registry_query.as_deref() {
                    Some(query) => query,
                    None => {
                        default_query = format!(
                            "SELECT name FROM system.databases WHERE name NOT IN \
                             ('system', 'information_schema', 'INFORMATION_SCHEMA', \
                             'default', '{}') \
                             ORDER BY name",
                            self.repo.config.tracking.database
                        );
                        &default_query
                    }
                };
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
                // The tracking database is never a migration target,
                // however the registry is written.
                databases.retain(|database| *database != self.repo.config.tracking.database);
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
                "skipping excluded database {} (group {}); target it with --db/--group",
                terminal_field(database),
                terminal_field(group)
            );
        }
        Ok(resolved.databases)
    }
}
