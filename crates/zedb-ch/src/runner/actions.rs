use super::*;

impl Runner<'_> {
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
                println!(
                    "{}: up to date ({} applied)",
                    terminal_field(&database),
                    applied.len()
                );
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
                "{}: WARNING: {:05} rollback is structural: schema is \
                 restored but newer data may be lost",
                terminal_field(database),
                migration.number
            ),
            Some(RollbackClass::Irreversible) => println!(
                "{}: WARNING: {:05} rollback is declared IRREVERSIBLE and \
                 does not restore the previous state",
                terminal_field(database),
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
                println!(
                    "{}: already at or below {floor:05}",
                    terminal_field(&database)
                );
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
                    println!(
                        "{}: customisation {number:05} not applied, skipping",
                        terminal_field(&database)
                    );
                    continue;
                }
                println!(
                    "{}: WARNING: removing customisation {number:05}",
                    terminal_field(&database)
                );
                self.roll(&database, migration).await?;
            }
            return Ok(());
        }

        for database in self.target_databases(targets).await? {
            let applied = self.applied_migrations(&database).await?;
            if !applied.contains(&number) {
                println!(
                    "{}: {number:05} not applied, skipping",
                    terminal_field(&database)
                );
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
            if self.options.dry_run {
                println!(
                    "{}: would stamp through {number:05}",
                    terminal_field(&database)
                );
                continue;
            }
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
            println!("{}: stamped through {number:05}", terminal_field(&database));
        }
        Ok(())
    }

    /// Map ancestor tracking rows (`default.schema_migrations`) into the
    /// format-1 tables, preserving recorded_at ordering so argMax-latest
    /// state carries over; returns how many rows moved. Idempotent-ish:
    /// refuses when zedb_migrations already has rows.
    pub async fn import_tracking(&self, source_table: &str) -> Result<u64, RunnerError> {
        self.require_write("import-tracking")?;
        validate_qualified_table(source_table)?;
        if self.options.dry_run {
            // Dry runs deliberately make no network request (tested), so
            // the already-has-rows refusal below is NOT evaluated here;
            // callers must present this as unchecked, not as a promise.
            return Ok(0);
        }
        self.ensure_tracking().await?;
        let existing = self
            .client
            .query(&format!(
                "SELECT count() FROM {} WHERE repo = {}",
                self.tracking_table(),
                quote(&self.tracking_repo())
            ))
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
                "INSERT INTO {} (repo, db, migration, action, status, error, recorded_at,                  duration_secs, run_id, params)                  SELECT {} AS repo, db, migration, action, status, error, recorded_at,                  duration_secs, run_id, map() FROM {source_table}",
                self.tracking_table(),
                quote(&self.tracking_repo())
            ))
            .await
            .map_err(|error| RunnerError::Server(error.to_string()))?;
        let imported = self
            .client
            .query(&format!(
                "SELECT count() FROM {} WHERE repo = {}",
                self.tracking_table(),
                quote(&self.tracking_repo())
            ))
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
                println!(
                    "{}: customisation {number:05} already applied, skipping",
                    terminal_field(&database)
                );
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
