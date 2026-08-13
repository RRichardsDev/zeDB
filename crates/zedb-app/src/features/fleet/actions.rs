use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use gpui::{prelude::*, Context};
use zedb_ch::runner::{Runner, RunnerOptions, Targets};
use zedb_ch::verify::Verifier;
use zedb_core::repo::MigrationRepo;

use super::view::{DriftInfo, FleetAction, FleetRow};
use crate::components::text_input::TextInput;
use crate::{rt, save_preferences, Workspace};

impl Workspace {
    pub(crate) fn fleet_open_repo(&mut self, cx: &mut Context<Self>) {
        let path_text = self.fleet.repo_path.read(cx).text().trim().to_string();
        if path_text.is_empty() {
            self.fleet.repo_error =
                Some("Enter the path of a migration repo checkout, or a git URL to clone".into());
            cx.notify();
            return;
        }
        if zedb_core::git::is_remote_url(&path_text) {
            self.fleet_clone_repo(path_text, cx);
            return;
        }
        let expanded = match (path_text.strip_prefix("~/"), std::env::var_os("HOME")) {
            (Some(rest), Some(home)) => Path::new(&home).join(rest),
            _ => Path::new(&path_text).to_path_buf(),
        };
        self.fleet_open_local(expanded, path_text, cx);
    }
    /// Open a local checkout. `source` is what the user typed and what
    /// preferences remember: the git URL stays the visible alias for a
    /// managed clone; the checkout path is plumbing.
    fn fleet_open_local(
        &mut self,
        expanded: std::path::PathBuf,
        source: String,
        cx: &mut Context<Self>,
    ) {
        // An effectively empty directory (fresh clone, nothing beyond
        // git bookkeeping and a README) becomes a format-1 repo on the
        // spot; anything with real content is left alone and errors as
        // before.
        if let Ok(true) = zedb_core::repo::init_repo_if_empty(&expanded) {
            self.notice = Some(
                "Initialized an empty checkout as a format-1 migration repo; \
                 commit zedb.toml when ready"
                    .into(),
            );
            self.notice_warning = false;
            // Pin the engine to the server we are actually connected
            // to instead of the template's placeholder version.
            if let Some(connected) = &self.connection.connected {
                let config = connected.client_config.clone();
                let root = expanded.clone();
                let handle = rt::tokio()
                    .spawn(async move { zedb_ch::pin::discover_server_version(config).await });
                cx.spawn(async move |this, cx| {
                    let Ok(Ok(version)) = handle.await else {
                        return;
                    };
                    this.update(cx, |this, cx| {
                        if zedb_core::repo::RepoConfig::set_pinned_version(&root, &version).is_ok()
                        {
                            this.notice = Some(format!(
                                "Initialized the checkout as a format-1 migration repo, \
                                 pinned to ClickHouse {version} from the connected server; \
                                 commit zedb.toml when ready"
                            ));
                            this.notice_warning = false;
                            this.fleet_open_repo(cx);
                        }
                        cx.notify();
                    })
                    .ok();
                })
                .detach();
            }
        }
        match MigrationRepo::open(&expanded) {
            Ok(repo) => {
                self.fleet.git = zedb_core::git::read_git_status(&repo.root);
                self.fleet.repo = Some(Arc::new(repo));
                self.fleet.repo_error = None;
                self.fleet.editing_repo_path = false;
                self.fleet.rows.clear();
                self.fleet.fetched_at = None;
                self.preferences.fleet_repo = Some(source);
                if let Err(error) = save_preferences(&self.preferences) {
                    self.notice = Some(format!("Could not save preferences: {error}"));
                }
                self.fleet_refresh(cx);
            }
            Err(error) => {
                self.fleet.repo = None;
                self.fleet.git = None;
                self.fleet.rows.clear();
                self.fleet.repo_error = Some(error.to_string());
            }
        }
        cx.notify();
    }

    /// Clone a pasted git URL into the managed repos directory and open
    /// the checkout, with the user's own git doing the network and auth
    /// work. An existing clone of the same URL is opened as-is.
    fn fleet_clone_repo(&mut self, url: String, cx: &mut Context<Self>) {
        if self.fleet.cloning {
            return;
        }
        let Some(base) = zedb_core::git::managed_repos_dir() else {
            self.fleet.repo_error = Some("No data directory available for clones".into());
            cx.notify();
            return;
        };
        let dest = base.join(zedb_core::git::clone_directory_name(&url));
        if dest.join(".git").exists() {
            if !zedb_core::git::has_upstream(&dest) {
                // Nothing upstream to pull (the remote is still empty);
                // open the checkout without the pull noise.
                self.notice = Some(format!(
                    "Opened the existing checkout at {} (no upstream commits to pull yet)",
                    dest.display()
                ));
                self.notice_warning = false;
                self.fleet_open_local(dest, url, cx);
                return;
            }
            // Already cloned from a previous paste: pull (fast-forward
            // only), then open the checkout either way.
            self.fleet.cloning = true;
            self.fleet.repo_error = None;
            self.notice = Some(format!("Pulling {}...", dest.display()));
            self.notice_warning = false;
            cx.notify();
            let pull_dest = dest.clone();
            let handle = rt::tokio().spawn(async move {
                tokio::task::spawn_blocking(move || zedb_core::git::pull(&pull_dest))
                    .await
                    .map_err(|error| error.to_string())?
            });
            cx.spawn(async move |this, cx| {
                let result = handle.await;
                this.update(cx, |this, cx| {
                    this.fleet.cloning = false;
                    match result.map_err(|error| error.to_string()) {
                        Ok(Ok(output)) => {
                            this.notice = Some(format!("Pulled: {output}"));
                            this.notice_warning = false;
                        }
                        Ok(Err(error)) | Err(error) => {
                            // Opened anyway: a stale checkout still
                            // works read-only, and the git chip plus
                            // deploy warnings carry the staleness.
                            this.notice =
                                Some(format!("Pull failed, opened the checkout as-is: {error}"));
                            this.notice_warning = true;
                            this.notice_flash_id += 1;
                        }
                    }
                    this.fleet_open_local(dest, url, cx);
                    cx.notify();
                })
                .ok();
            })
            .detach();
            return;
        }
        self.fleet.cloning = true;
        self.fleet.repo_error = None;
        self.notice = Some(format!("Cloning {url}..."));
        self.notice_warning = false;
        cx.notify();

        let clone_url = url.clone();
        let clone_dest = dest.clone();
        let handle = rt::tokio().spawn(async move {
            tokio::task::spawn_blocking(move || zedb_core::git::clone_repo(&clone_url, &clone_dest))
                .await
                .map_err(|error| error.to_string())?
        });
        cx.spawn(async move |this, cx| {
            let result = handle.await;
            this.update(cx, |this, cx| {
                this.fleet.cloning = false;
                match result.map_err(|error| error.to_string()) {
                    Ok(Ok(())) => {
                        this.notice = Some(format!("Cloned {url} to {}", dest.display()));
                        this.notice_warning = false;
                        this.fleet_open_local(dest, url, cx);
                    }
                    Ok(Err(error)) | Err(error) => {
                        this.fleet.repo_error = Some(format!("clone failed: {error}"));
                        this.notice = None;
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Pull the open checkout (fast-forward only), then reopen it so
    /// the chain and matrix reflect what arrived.
    pub(crate) fn fleet_pull(&mut self, cx: &mut Context<Self>) {
        if self.fleet.cloning {
            return;
        }
        let Some(repo) = self.fleet.repo.clone() else {
            return;
        };
        self.fleet.cloning = true;
        self.notice = Some("Pulling...".into());
        self.notice_warning = false;
        cx.notify();
        let root = repo.root.clone();
        let handle = rt::tokio().spawn(async move {
            tokio::task::spawn_blocking(move || zedb_core::git::pull(&root))
                .await
                .map_err(|error| error.to_string())?
        });
        cx.spawn(async move |this, cx| {
            let result = handle.await;
            this.update(cx, |this, cx| {
                this.fleet.cloning = false;
                match result.map_err(|error| error.to_string()) {
                    Ok(Ok(output)) => {
                        this.notice = Some(format!("Pulled: {output}"));
                        this.notice_warning = false;
                    }
                    Ok(Err(error)) | Err(error) => {
                        this.notice = Some(format!("Pull failed: {error}"));
                        this.notice_warning = true;
                        this.notice_flash_id += 1;
                    }
                }
                this.fleet_open_repo(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn fleet_refresh(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.fleet.repo.clone() else {
            return;
        };
        let Some(connected) = &self.connection.connected else {
            self.fleet.fetch_error = Some("Connect to a cluster to load fleet status".into());
            cx.notify();
            return;
        };
        let config = connected.client_config.clone();
        self.fleet.loading = true;
        self.fleet.fetch_error = None;
        self.fleet.fetch_generation += 1;
        let generation = self.fleet.fetch_generation;
        cx.notify();

        let handle = rt::tokio().spawn(async move {
            let git = zedb_core::git::read_git_status(&repo.root);
            let runner = Runner::new(
                &repo,
                RunnerOptions {
                    server: config,
                    admin: None,
                    cluster: None,
                    no_cluster: true,
                    write: false,
                    dry_run: false,
                    overrides: BTreeMap::new(),
                },
            );
            let resolved = runner
                .resolve_targets(&Targets::All)
                .await
                .map_err(|error| error.to_string())?;
            let mut databases = resolved.databases.clone();
            let excluded: BTreeMap<String, String> = resolved.skipped.into_iter().collect();
            databases.extend(excluded.keys().cloned());
            databases.sort();
            databases.dedup();
            let clusters: Vec<String> = runner
                .client()
                .query("SELECT DISTINCT cluster FROM system.clusters ORDER BY cluster")
                .await
                .map(|result| {
                    result
                        .rows
                        .iter()
                        .filter_map(|row| row.first().map(|value| value.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            let statuses = runner
                .status(&Targets::Databases(databases))
                .await
                .map_err(|error| error.to_string())?;
            let rows: Vec<FleetRow> = statuses
                .into_iter()
                .map(|status| FleetRow {
                    excluded: excluded.get(&status.database).cloned(),
                    database: status.database,
                    head: status.head,
                    pending: status.pending,
                    customised: status.customised,
                    failed: status
                        .failed
                        .into_iter()
                        .map(|(number, _)| number)
                        .collect(),
                })
                .collect();
            Ok::<_, String>((rows, clusters, git))
        });

        cx.spawn(async move |this, cx| {
            let result = handle.await;
            this.update(cx, |this, cx| {
                if this.fleet.fetch_generation != generation {
                    return;
                }
                this.fleet.loading = false;
                match result {
                    Ok(Ok((rows, clusters, git))) => {
                        this.fleet.rows = rows;
                        this.fleet.clusters = clusters;
                        this.fleet.git = git;
                        this.fleet.fetched_at = Some(Instant::now());
                        // Status first (fast), then drift streams in
                        // behind it, one database at a time.
                        if this.fleet.drift_loading.is_empty() {
                            this.fleet_verify_all(cx);
                        }
                    }
                    Ok(Err(error)) => this.fleet.fetch_error = Some(error),
                    Err(error) => this.fleet.fetch_error = Some(error.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn fleet_tier(&self) -> zedb_core::EnvTier {
        self.connection
            .selected
            .and_then(|index| self.connection.connections.get(index))
            .map(|connection| connection.tier)
            .unwrap_or(zedb_core::EnvTier::Dev)
    }

    /// The rollback class the ladder must gate on for an action, when any.
    pub(crate) fn action_rollback_class(
        &self,
        action: &FleetAction,
    ) -> Option<Option<zedb_core::repo::RollbackClass>> {
        let repo = self.fleet.repo.as_ref()?;
        match action {
            FleetAction::Rollback { number, .. } | FleetAction::RemoveTargeted { number, .. } => {
                repo.migration(*number)
                    .map(|migration| migration.rollback_class)
            }
            _ => None,
        }
    }

    pub(crate) fn fleet_request_action(&mut self, action: FleetAction, cx: &mut Context<Self>) {
        if !self.fleet.write_unlocked {
            self.notice = Some("Unlock writes first: mutations need explicit consent".into());
            cx.notify();
            return;
        }
        self.fleet.pending_action = Some(action);
        self.fleet.ack_structural = false;
        self.fleet.action_progress.clear();
        self.fleet.action_running = false;
        self.fleet.action_result = None;
        // TextInput has no setter; a fresh entity is an empty input.
        self.fleet.confirm_input = cx.new(|cx| TextInput::new("", "type to confirm", false, cx));
        cx.observe(&self.fleet.confirm_input, |_, _, cx| cx.notify())
            .detach();
        cx.notify();
    }

    pub(crate) fn fleet_execute_action(&mut self, cx: &mut Context<Self>) {
        let Some(action) = self.fleet.pending_action.clone() else {
            return;
        };
        let Some(repo) = self.fleet.repo.clone() else {
            return;
        };
        let Some(connected) = &self.connection.connected else {
            return;
        };
        let cluster = self.fleet.selected_cluster.clone();
        let config = connected.client_config.clone();
        self.fleet.action_running = true;
        self.fleet.action_result = None;
        self.fleet.action_progress.clear();
        cx.notify();

        // The work, decomposed so the modal can go green step by step:
        // per pending migration for one database, per database for the
        // fleet. Keys match the dry-run labels' text before ':'.
        let upgrade_all_units: Vec<String> = self
            .fleet
            .rows
            .iter()
            .filter(|row| row.excluded.is_none() && !row.pending.is_empty())
            .map(|row| row.database.clone())
            .collect();
        let single_pending: Vec<u32> = match &action {
            FleetAction::UpgradeDatabase(database) => self
                .fleet
                .rows
                .iter()
                .find(|row| row.database == *database)
                .map(|row| {
                    let mut pending = row.pending.clone();
                    pending.sort_unstable();
                    pending
                })
                .unwrap_or_default(),
            _ => Vec::new(),
        };

        let (progress_tx, mut progress_rx) =
            tokio::sync::mpsc::unbounded_channel::<(String, f32)>();
        let handle = rt::tokio().spawn(async move {
            let no_cluster = cluster.is_none();
            let runner = Runner::new(
                &repo,
                RunnerOptions {
                    server: config,
                    admin: None,
                    cluster,
                    no_cluster,
                    write: true,
                    dry_run: false,
                    overrides: BTreeMap::new(),
                },
            );
            let result = match &action {
                FleetAction::UpgradeAll => {
                    let mut outcome = Ok(());
                    for database in upgrade_all_units {
                        let started = Instant::now();
                        match runner
                            .upgrade(&Targets::Databases(vec![database.clone()]), None)
                            .await
                        {
                            Ok(()) => {
                                let _ =
                                    progress_tx.send((database, started.elapsed().as_secs_f32()));
                            }
                            Err(error) => {
                                outcome = Err(error);
                                break;
                            }
                        }
                    }
                    outcome
                }
                FleetAction::UpgradeDatabase(database) => {
                    let targets = Targets::Databases(vec![database.clone()]);
                    let mut outcome = Ok(());
                    for number in single_pending {
                        let started = Instant::now();
                        match runner.upgrade(&targets, Some(number)).await {
                            Ok(()) => {
                                let _ = progress_tx.send((
                                    format!("migration {number:05}"),
                                    started.elapsed().as_secs_f32(),
                                ));
                            }
                            Err(error) => {
                                outcome = Err(error);
                                break;
                            }
                        }
                    }
                    outcome
                }
                FleetAction::Rollback { database, number } => {
                    let started = Instant::now();
                    runner
                        .rollback_one(
                            &Targets::Databases(vec![database.clone()]),
                            *number,
                            true,
                            false,
                        )
                        .await
                        .inspect(|()| {
                            let _ = progress_tx.send((
                                format!("rollback {number:05}"),
                                started.elapsed().as_secs_f32(),
                            ));
                        })
                }
                FleetAction::ApplyTargeted { database, number } => {
                    let started = Instant::now();
                    runner
                        .apply_targeted(&Targets::Databases(vec![database.clone()]), *number)
                        .await
                        .inspect(|()| {
                            let _ = progress_tx.send((
                                format!("apply {number:05}"),
                                started.elapsed().as_secs_f32(),
                            ));
                        })
                }
                FleetAction::RemoveTargeted { database, number } => {
                    let started = Instant::now();
                    runner
                        .rollback_one(
                            &Targets::Databases(vec![database.clone()]),
                            *number,
                            true,
                            true,
                        )
                        .await
                        .inspect(|()| {
                            let _ = progress_tx.send((
                                format!("rollback {number:05}"),
                                started.elapsed().as_secs_f32(),
                            ));
                        })
                }
            };
            result.map_err(|error| error.to_string())
        });

        cx.spawn(async move |this, cx| {
            while let Some((key, seconds)) = progress_rx.recv().await {
                this.update(cx, |this, cx| {
                    this.fleet.action_progress.insert(key, seconds);
                    cx.notify();
                })
                .ok();
            }
            let result = handle.await;
            this.update(cx, |this, cx| {
                this.fleet.action_running = false;
                this.fleet.action_result = Some(match result {
                    Ok(Ok(())) => Ok("Completed; tracking table and audit log updated.".into()),
                    Ok(Err(error)) => Err(error),
                    Err(error) => Err(error.to_string()),
                });
                this.fleet_refresh(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Verify the whole fleet in one background pass; results land in
    /// the same per-database drift map the matrix badges read.
    pub(crate) fn fleet_verify_all(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.fleet.repo.clone() else {
            return;
        };
        let Some(connected) = &self.connection.connected else {
            return;
        };
        let Some(binary) = zedb_ch::cached_binary(&repo.config.engine.version) else {
            self.fleet.drift_error = Some(format!(
                "pinned ClickHouse {} is not cached; run `zedb pin` first",
                repo.config.engine.version
            ));
            cx.notify();
            return;
        };
        let databases: Vec<String> = self
            .fleet
            .rows
            .iter()
            .filter(|row| row.excluded.is_none())
            .map(|row| row.database.clone())
            .collect();
        if databases.is_empty() {
            return;
        }
        for database in &databases {
            self.fleet.drift_loading.insert(database.clone());
        }
        self.fleet.drift_error = None;
        let config = connected.client_config.clone();
        cx.notify();

        // Stream results per database so badges appear as each verify
        // lands rather than in one batch at the end.
        let (results_tx, mut results_rx) =
            tokio::sync::mpsc::unbounded_channel::<(String, Result<Vec<String>, String>)>();
        let task_databases = databases.clone();
        rt::tokio().spawn(async move {
            let runner = Runner::new(
                &repo,
                RunnerOptions {
                    server: config,
                    admin: None,
                    cluster: None,
                    no_cluster: true,
                    write: false,
                    dry_run: false,
                    overrides: BTreeMap::new(),
                },
            );
            let verifier = Verifier::new(&repo, &runner, binary);
            for database in task_databases {
                let outcome = verifier
                    .verify(&Targets::Databases(vec![database.clone()]))
                    .await
                    .map(|drifts| {
                        drifts
                            .into_iter()
                            .next()
                            .map(|drift| drift.findings)
                            .unwrap_or_default()
                    })
                    .map_err(|error| error.to_string());
                if results_tx.send((database, outcome)).is_err() {
                    break;
                }
            }
        });
        cx.spawn(async move |this, cx| {
            while let Some((database, outcome)) = results_rx.recv().await {
                let live = this
                    .update(cx, |this, cx| {
                        this.fleet.drift_loading.remove(&database);
                        match outcome {
                            Ok(findings) => {
                                this.fleet.drift.insert(
                                    database,
                                    DriftInfo {
                                        findings,
                                        checked_at: Instant::now(),
                                    },
                                );
                            }
                            Err(error) => this.fleet.drift_error = Some(error),
                        }
                        cx.notify();
                    })
                    .is_ok();
                if !live {
                    break;
                }
            }
        })
        .detach();
    }

    pub(crate) fn fleet_verify(&mut self, database: String, cx: &mut Context<Self>) {
        let Some(repo) = self.fleet.repo.clone() else {
            return;
        };
        let Some(connected) = &self.connection.connected else {
            return;
        };
        let Some(binary) = zedb_ch::cached_binary(&repo.config.engine.version) else {
            self.fleet.drift_error = Some(format!(
                "pinned ClickHouse {} is not cached; run `zedb pin` first",
                repo.config.engine.version
            ));
            cx.notify();
            return;
        };
        if self.fleet.drift_loading.contains(&database) {
            return;
        }
        let config = connected.client_config.clone();
        self.fleet.drift_loading.insert(database.clone());
        self.fleet.drift_error = None;
        cx.notify();

        let task_database = database.clone();
        let handle = rt::tokio().spawn(async move {
            let runner = Runner::new(
                &repo,
                RunnerOptions {
                    server: config,
                    admin: None,
                    cluster: None,
                    no_cluster: true,
                    write: false,
                    dry_run: false,
                    overrides: BTreeMap::new(),
                },
            );
            let verifier = Verifier::new(&repo, &runner, binary);
            let drifts = verifier
                .verify(&Targets::Databases(vec![task_database.clone()]))
                .await
                .map_err(|error| error.to_string())?;
            Ok::<_, String>(
                drifts
                    .into_iter()
                    .next()
                    .map(|drift| drift.findings)
                    .unwrap_or_default(),
            )
        });

        cx.spawn(async move |this, cx| {
            let result = handle.await;
            this.update(cx, |this, cx| {
                this.fleet.drift_loading.remove(&database);
                match result {
                    Ok(Ok(findings)) => {
                        this.fleet.drift.insert(
                            database,
                            DriftInfo {
                                findings,
                                checked_at: Instant::now(),
                            },
                        );
                    }
                    Ok(Err(error)) => this.fleet.drift_error = Some(error),
                    Err(error) => this.fleet.drift_error = Some(error.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}
