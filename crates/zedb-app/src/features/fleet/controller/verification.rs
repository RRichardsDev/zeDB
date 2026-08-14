use std::collections::BTreeMap;
use std::time::Instant;

use gpui::Context;
use zedb_ch::runner::{Runner, RunnerOptions, Targets};
use zedb_ch::verify::Verifier;

use super::view::DriftInfo;
use crate::{rt, Workspace};

impl Workspace {
    /// Verify the whole fleet in one background pass; results land in
    /// the same per-database drift map the matrix badges read.
    pub(crate) fn fleet_verify_all(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.fleet.repo.clone() else {
            return;
        };
        let Some(connected) = &self.connection.connected else {
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
            // Resolve (and download, with the closest-release fallback)
            // in the task, like Check does; nothing to pre-cache.
            let binary = match zedb_ch::ensure_binary(&repo.config.engine.version).await {
                Ok(binary) => binary,
                Err(error) => {
                    let message = error.to_string();
                    for database in task_databases {
                        if results_tx.send((database, Err(message.clone()))).is_err() {
                            break;
                        }
                    }
                    return;
                }
            };
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
        if self.fleet.drift_loading.contains(&database) {
            return;
        }
        let config = connected.client_config.clone();
        self.fleet.drift_loading.insert(database.clone());
        self.fleet.drift_error = None;
        cx.notify();

        let task_database = database.clone();
        let handle = rt::tokio().spawn(async move {
            let binary = zedb_ch::ensure_binary(&repo.config.engine.version)
                .await
                .map_err(|error| error.to_string())?;
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
