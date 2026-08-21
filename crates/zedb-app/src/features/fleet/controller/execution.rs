use std::collections::BTreeMap;
use std::time::Instant;

use gpui::{prelude::*, Context};
use zedb_ch::runner::{Runner, RunnerOptions, Targets};

use super::view::{
    fleet_confirmation_valid, fleet_context_matches, FleetAction, FleetConfirmation,
    PendingFleetAction,
};
use crate::components::text_input::TextInput;
use crate::{rt, Workspace};

impl Workspace {
    pub(crate) fn fleet_tier(&self) -> zedb_core::EnvTier {
        self.connection
            .connected
            .as_ref()
            .and_then(|connected| {
                self.connection
                    .connections
                    .iter()
                    .find(|connection| connection.name == connected.name)
            })
            .map(|connection| connection.tier)
            // If the connected identity is no longer present in saved
            // settings, fail closed into the strongest confirmation tier.
            .unwrap_or(zedb_core::EnvTier::Production)
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
        let Some(connected) = self.connection.connected.as_ref() else {
            self.notice = Some("Connect to a cluster before confirming a mutation".into());
            cx.notify();
            return;
        };
        let Some(repo) = self.fleet.repo.as_ref() else {
            self.notice = Some("Open a migration repository first".into());
            cx.notify();
            return;
        };
        self.fleet.pending_action = Some(PendingFleetAction {
            action,
            connection: connected.name.clone(),
            repo_root: repo.root.clone(),
        });
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
        let Some(pending) = self.fleet.pending_action.clone() else {
            return;
        };
        let Some(repo) = self.fleet.repo.clone() else {
            return;
        };
        let Some(connected) = &self.connection.connected else {
            return;
        };
        let class = self.action_rollback_class(&pending.action);
        let structural = matches!(
            class,
            Some(Some(zedb_core::repo::RollbackClass::Structural))
        );
        let irreversible = matches!(
            class,
            Some(None) | Some(Some(zedb_core::repo::RollbackClass::Irreversible))
        );
        let required_phrase = pending
            .action
            .required_phrase(self.fleet_tier(), irreversible);
        let typed_phrase = self.fleet.confirm_input.read(cx).text();
        let confirmation = FleetConfirmation {
            pending: &pending,
            current_connection: Some(connected.name.as_str()),
            current_repo: Some(repo.root.as_path()),
            write_unlocked: self.fleet.write_unlocked,
            running: self.fleet.action_running,
            completed: self.fleet.action_result.is_some(),
            structural,
            acknowledged: self.fleet.ack_structural,
            required_phrase: required_phrase.as_deref(),
            typed_phrase: &typed_phrase,
        };
        if !fleet_context_matches(&confirmation) {
            self.fleet.pending_action = None;
            self.notice = Some(
                "The connection or repository changed; review and confirm the action again".into(),
            );
            self.notice_warning = true;
            cx.notify();
            return;
        }
        if !fleet_confirmation_valid(&confirmation) {
            self.notice = Some("Complete the confirmation before running this action".into());
            self.notice_warning = true;
            cx.notify();
            return;
        }
        let action = pending.action;
        // The ${cluster} for fleet operations comes from the top
        // toolbar's node/cluster selector (apply_cluster), the single
        // cluster choice in the app. On Cloud it is forced off: the
        // warehouse shares one catalog and the runner's no-cluster
        // path strips every ON CLUSTER clause before it can reach
        // the server.
        let cluster = if self.active_connection_is_cloud() {
            None
        } else {
            connected.apply_cluster.clone()
        };
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
}
