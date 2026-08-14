use crate::*;

use gpui::prelude::*;

impl Workspace {
    pub(crate) fn clear_schema(&mut self) {
        self.schema.connection = None;
        self.schema.loading = false;
        self.schema.databases.clear();
        self.schema.error = None;
        self.schema.selected_object = None;
    }

    pub(crate) fn load_schema_databases(&mut self, cx: &mut Context<Self>) {
        let Some(connected) = &self.connection.connected else {
            self.clear_schema();
            return;
        };
        let connection_name = connected.name.clone();
        let config = connected.client_config.clone();
        self.schema.connection = Some(connection_name.clone());
        self.schema.loading = true;
        self.schema.databases.clear();
        self.schema.error = None;
        self.schema.selected_object = None;
        if let Some(cache) = &self.schema.cache {
            self.schema.databases = database_nodes_from_cache(cache);
        }
        cx.notify();

        let cache = self.schema.cache.clone();
        let task = rt::tokio().spawn(async move {
            let client = ChClient::new(config);
            if let Some(cache) = cache {
                cache
                    .refresh_tables(&client)
                    .await
                    .map_err(|error| error.to_string())?;
                Ok(database_nodes_from_cache(&cache))
            } else {
                client
                    .list_databases()
                    .await
                    .map(|databases| {
                        databases
                            .into_iter()
                            .map(|meta| DatabaseNode {
                                meta,
                                expanded: false,
                                filter_collapsed: false,
                                loading: false,
                                objects: None,
                                error: None,
                            })
                            .collect()
                    })
                    .map_err(|error| error.to_string())
            }
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if this
                    .connection
                    .connected
                    .as_ref()
                    .map(|cluster| cluster.name.as_str())
                    != Some(connection_name.as_str())
                {
                    return;
                }
                this.schema.loading = false;
                match result {
                    Ok(Ok(databases)) => this.schema.databases = databases,
                    Ok(Err(error)) => this.schema.error = Some(error),
                    Err(error) => this.schema.error = Some(error.to_string()),
                }
                // The schema context just changed; refresh open editors'
                // diagnostics so a now-known database drops its stale
                // "unknown" squiggly without waiting for the next edit.
                this.refresh_schema_diagnostics(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Fetch a database's column metadata in the background if the cache
    /// is missing it; on success, re-run analysis so open editors update.
    pub(crate) fn warm_schema_columns(
        &mut self,
        database: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(cache), Some(connected)) = (
            self.schema.cache.clone(),
            self.connection.connected.as_ref(),
        ) else {
            return;
        };
        if !cache.needs_columns(&database) || !self.schema.warming.insert(database.clone()) {
            return;
        }
        let config = connected.client_config.clone();
        let task = rt::tokio().spawn({
            let database = database.clone();
            async move {
                let client = ChClient::new(config);
                cache.refresh_columns(&client, &database).await.is_ok()
            }
        });
        cx.spawn_in(window, async move |this, cx| {
            let warmed = task.await.unwrap_or(false);
            this.update_in(cx, |this, window, cx| {
                this.schema.warming.remove(&database);
                if warmed {
                    let editors: Vec<(usize, Entity<InputState>)> = this
                        .query
                        .tabs
                        .iter()
                        .map(|tab| (tab.id, tab.editor.clone()))
                        .collect();
                    for (id, editor) in editors {
                        this.schedule_schema_analysis(id, editor, window, cx);
                    }
                    // If the user already typed the trigger (say `e.`)
                    // while columns were cold, reopen the popup now.
                    if let Some(tab) = this.query.tabs.get(this.query.active_tab) {
                        let editor = tab.editor.clone();
                        if editor.read(cx).focus_handle(cx).is_focused(window) {
                            editor.update(cx, |editor, cx| {
                                editor.retrigger_completion(window, cx);
                            });
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Entry point for the Analyse button. On a writable connection the
    /// measurement step writes temporary tables, so ask for confirmation
    /// first; on a read-only connection nothing is written, so run the
    /// (read-only) scan straight away.
    pub(crate) fn request_analyze(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let writable = self
            .connection
            .connected
            .as_ref()
            .map(|cluster| cluster.name.clone())
            .map(|name| self.connection_is_writable(&name))
            .unwrap_or(false);
        if writable {
            if let Some(selected) = &mut self.schema.selected_object {
                selected.cardinality_confirming = true;
            }
            cx.notify();
        } else {
            self.analyze_cardinality(window, cx);
        }
    }

    /// The user confirmed the write; clear the prompt and run.
    pub(crate) fn confirm_analyze(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(selected) = &mut self.schema.selected_object {
            selected.cardinality_confirming = false;
        }
        self.analyze_cardinality(window, cx);
    }

    pub(crate) fn cancel_analyze(&mut self, cx: &mut Context<Self>) {
        if let Some(selected) = &mut self.schema.selected_object {
            selected.cardinality_confirming = false;
        }
        cx.notify();
    }

    /// Opt-in cardinality probe: scan the selected table once for
    /// each column's approximate distinct count, off the
    /// main thread, and store it on the selection. Feeds the codec
    /// advisor. Guarded so a stale result (selection changed while the
    /// scan ran) is dropped.
    pub(crate) fn analyze_cardinality(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let (connection_name, config, database_name, object_name, column_names) = {
            let Some(selected) = &self.schema.selected_object else {
                return;
            };
            if selected.cardinality_loading || selected.columns.is_empty() {
                return;
            }
            let Some(connected) = &self.connection.connected else {
                return;
            };
            (
                connected.name.clone(),
                connected.client_config.clone(),
                selected.database.clone(),
                selected.object.name.clone(),
                selected
                    .columns
                    .iter()
                    .map(|column| column.name.clone())
                    .collect::<Vec<_>>(),
            )
        };

        if let Some(selected) = &mut self.schema.selected_object {
            selected.cardinality_loading = true;
            selected.cardinality_error = None;
        }
        cx.notify();

        let task = rt::tokio().spawn({
            let database_name = database_name.clone();
            let object_name = object_name.clone();
            async move {
                ChClient::new(config)
                    .column_cardinalities(&database_name, &object_name, &column_names)
                    .await
            }
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if this
                    .connection
                    .connected
                    .as_ref()
                    .map(|cluster| cluster.name.as_str())
                    != Some(connection_name.as_str())
                {
                    return;
                }
                let Some(selected) = &mut this.schema.selected_object else {
                    return;
                };
                if selected.database != database_name || selected.object.name != object_name {
                    return;
                }
                selected.cardinality_loading = false;
                let to_cache = match result {
                    Ok(Ok(cardinalities)) => {
                        selected.cardinalities = Some(cardinalities.clone());
                        Some(cardinalities)
                    }
                    Ok(Err(error)) => {
                        selected.cardinality_error = Some(error.to_string());
                        None
                    }
                    Err(error) => {
                        selected.cardinality_error = Some(error.to_string());
                        None
                    }
                };
                // The `selected` borrow ends here; keep the result for the
                // session so reopening this table auto-loads it.
                if let Some(cardinalities) = to_cache {
                    this.schema
                        .cardinality_cache
                        .insert((connection_name, database_name, object_name), cardinalities);
                    // Cardinality is known; measure the actual savings of
                    // the actionable suggestions, writable connections
                    // only.
                    this.measure_suggestions(cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Load active parts grouped by partition for the selected object,
    /// off-thread. No-op if already loaded or loading. Guards against a
    /// stale connection/object.
    pub(crate) fn load_partitions(&mut self, cx: &mut Context<Self>) {
        let (connection_name, config, database_name, object_name) = {
            let Some(selected) = &self.schema.selected_object else {
                return;
            };
            if selected.partitions.is_some() || selected.partitions_loading {
                return;
            }
            let Some(connected) = &self.connection.connected else {
                return;
            };
            (
                connected.name.clone(),
                connected.client_config.clone(),
                selected.database.clone(),
                selected.object.name.clone(),
            )
        };

        if let Some(selected) = &mut self.schema.selected_object {
            selected.partitions_loading = true;
            selected.partitions_error = None;
        }
        cx.notify();

        let task = rt::tokio().spawn({
            let database_name = database_name.clone();
            let object_name = object_name.clone();
            async move {
                ChClient::new(config)
                    .table_partitions(&database_name, &object_name)
                    .await
            }
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if this
                    .connection
                    .connected
                    .as_ref()
                    .map(|cluster| cluster.name.as_str())
                    != Some(connection_name.as_str())
                {
                    return;
                }
                let Some(selected) = &mut this.schema.selected_object else {
                    return;
                };
                if selected.database != database_name || selected.object.name != object_name {
                    return;
                }
                selected.partitions_loading = false;
                match result {
                    Ok(Ok(partitions)) => selected.partitions = Some(partitions),
                    Ok(Err(error)) => selected.partitions_error = Some(error.to_string()),
                    Err(error) => selected.partitions_error = Some(error.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Poll `system.merges` for the selected object while the Parts tab is
    /// open, so in-progress merges and their progress update live. A
    /// generation guard stops the loop when the object or tab changes.
    pub(crate) fn start_merges_poll(&mut self, cx: &mut Context<Self>) {
        self.merges_poll_generation += 1;
        let generation = self.merges_poll_generation;
        let Some(selected) = &self.schema.selected_object else {
            return;
        };
        let Some(connected) = &self.connection.connected else {
            return;
        };
        let connection_name = connected.name.clone();
        let database = selected.database.clone();
        let object = selected.object.name.clone();
        self.merges_fetch(generation, cx);
        cx.spawn(async move |this, cx| loop {
            Timer::after(Duration::from_secs(2)).await;
            let live = this
                .update(cx, |this, cx| {
                    let live = this.merges_poll_generation == generation
                        && this
                            .connection
                            .connected
                            .as_ref()
                            .map(|cluster| cluster.name.as_str())
                            == Some(connection_name.as_str())
                        && this
                            .schema
                            .selected_object
                            .as_ref()
                            .is_some_and(|selected| {
                                selected.tab == ObjectInspectorTab::Parts
                                    && selected.database == database
                                    && selected.object.name == object
                            });
                    if live {
                        this.merges_fetch(generation, cx);
                    }
                    live
                })
                .unwrap_or(false);
            if !live {
                break;
            }
        })
        .detach();
    }

    /// One off-thread read of the selected object's in-progress merges.
    pub(crate) fn merges_fetch(&mut self, generation: u64, cx: &mut Context<Self>) {
        let (connection_name, config, database, object) = {
            let Some(selected) = &self.schema.selected_object else {
                return;
            };
            let Some(connected) = &self.connection.connected else {
                return;
            };
            (
                connected.name.clone(),
                connected.client_config.clone(),
                selected.database.clone(),
                selected.object.name.clone(),
            )
        };
        let guard_database = database.clone();
        let guard_object = object.clone();
        let task = rt::tokio().spawn(async move {
            ChClient::new(config)
                .active_merges(&database, &object)
                .await
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if this.merges_poll_generation != generation {
                    return;
                }
                if this
                    .connection
                    .connected
                    .as_ref()
                    .map(|cluster| cluster.name.as_str())
                    != Some(connection_name.as_str())
                {
                    return;
                }
                let Some(selected) = &mut this.schema.selected_object else {
                    return;
                };
                if selected.database != guard_database || selected.object.name != guard_object {
                    return;
                }
                // Keep the last snapshot on a transient error; a live poll
                // shouldn't blank out or spam.
                if let Ok(Ok(merges)) = result {
                    selected.merges = merges;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }
}
