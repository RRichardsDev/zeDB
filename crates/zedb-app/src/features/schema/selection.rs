use crate::*;

use gpui::prelude::*;

impl Workspace {
    /// True when the named connection allows writes (needed for the
    /// Tier 3 measurement, which builds a throwaway table).
    pub(crate) fn connection_is_writable(&self, name: &str) -> bool {
        self.connection
            .connections
            .iter()
            .find(|connection| connection.name.as_str() == name)
            .map(|connection| !connection.read_only)
            .unwrap_or(false)
    }

    /// Measure the actual size savings of each actionable suggestion for
    /// the current selection (Phase 8, Tier 3). Runs one throwaway-table
    /// trial per suggested column, off the main thread, and stores the
    /// result. Only on writable connections; a no-op otherwise.
    pub(crate) fn measure_suggestions(&mut self, cx: &mut Context<Self>) {
        let (connection_name, config, database, table) = {
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
        if !self.connection_is_writable(&connection_name) {
            return;
        }

        // Collect the actionable columns and how to build their trials.
        let jobs: Vec<(usize, String, String, String)> = {
            let Some(selected) = &self.schema.selected_object else {
                return;
            };
            let Some(cardinalities) = &selected.cardinalities else {
                return;
            };
            let total_rows = selected.object.total_rows.unwrap_or(0);
            selected
                .columns
                .iter()
                .enumerate()
                .filter(|(index, _)| !selected.measured.contains_key(index))
                .filter_map(|(index, column)| {
                    let distinct = cardinalities.get(index).copied().unwrap_or(0);
                    let advice = storage_advisor::advise(
                        &storage_advisor::ColumnFacts {
                            name: &column.name,
                            type_name: &column.type_name,
                            codec: &column.codec,
                            distinct,
                            total_rows,
                            compressed_bytes: column.compressed_bytes,
                            uncompressed_bytes: column.uncompressed_bytes,
                        },
                        &database,
                        &table,
                        // Trial defs are cluster-independent.
                        None,
                    );
                    if let storage_advisor::Advice::Suggest {
                        base_def, cand_def, ..
                    } = advice
                    {
                        Some((index, column.name.clone(), base_def, cand_def))
                    } else {
                        None
                    }
                })
                .collect()
        };

        for (index, column_name, base_def, cand_def) in jobs {
            let config = config.clone();
            let database = database.clone();
            let table = table.clone();
            let connection_name = connection_name.clone();
            // Globally-unique trial-table name so concurrent trials (even
            // two tables in the same database) never drop each other's.
            static TRIAL_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let seq = TRIAL_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let trial_name = format!("_zedb_codec_trial_{seq}");
            let task_database = database.clone();
            let task_table = table.clone();
            let task = rt::tokio().spawn(async move {
                ChClient::new(config)
                    .measure_codec_savings(
                        &task_database,
                        &task_table,
                        &column_name,
                        &base_def,
                        &cand_def,
                        &trial_name,
                    )
                    .await
            });
            cx.spawn(async move |this, cx| {
                let result = task.await;
                this.update(cx, |this, cx| {
                    if let Ok(Ok(Some(ratio))) = result {
                        this.schema
                            .measured_cache
                            .entry((connection_name.clone(), database.clone(), table.clone()))
                            .or_default()
                            .insert(index, ratio);
                        if this
                            .connection
                            .connected
                            .as_ref()
                            .map(|cluster| cluster.name.as_str())
                            == Some(connection_name.as_str())
                        {
                            if let Some(selected) = &mut this.schema.selected_object {
                                if selected.database == database && selected.object.name == table {
                                    selected.measured.insert(index, ratio);
                                }
                            }
                        }
                        cx.notify();
                    }
                })
                .ok();
            })
            .detach();
        }
    }

    pub(crate) fn toggle_schema_database(
        &mut self,
        database_index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let filter_active = !self.schema.filter.read(cx).text().trim().is_empty();
        let Some(database) = self.schema.databases.get_mut(database_index) else {
            return;
        };
        let shown = if filter_active {
            database.filter_collapsed = !database.filter_collapsed;
            !database.filter_collapsed
        } else {
            database.expanded = !database.expanded;
            database.expanded
        };
        if shown {
            if let Some(cache) = &self.schema.cache {
                cache.touch_database(&database.meta.name);
                if let Some(cached) = cache.snapshot().database(&database.meta.name) {
                    database.objects = Some(
                        cached
                            .objects
                            .values()
                            .map(schema_object_from_cache)
                            .collect(),
                    );
                }
            }
        }
        let needs_object_load = shown && database.objects.is_none() && !database.loading;
        let database_name = database.meta.name.clone();
        if shown {
            self.warm_schema_columns(database_name.clone(), window, cx);
        }
        if !needs_object_load {
            cx.notify();
            return;
        }
        let Some(connected) = &self.connection.connected else {
            return;
        };
        let connection_name = connected.name.clone();
        let config = connected.client_config.clone();
        let Some(database) = self.schema.databases.get_mut(database_index) else {
            return;
        };
        database.loading = true;
        database.error = None;
        cx.notify();

        let task = rt::tokio().spawn({
            let database_name = database_name.clone();
            async move {
                ChClient::new(config)
                    .list_schema_objects(&database_name)
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
                let Some(database) = this
                    .schema
                    .databases
                    .iter_mut()
                    .find(|database| database.meta.name == database_name)
                else {
                    return;
                };
                database.loading = false;
                match result {
                    Ok(Ok(objects)) => database.objects = Some(objects),
                    Ok(Err(error)) => database.error = Some(error.to_string()),
                    Err(error) => database.error = Some(error.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn select_schema_object(
        &mut self,
        database_name: String,
        object: SchemaObjectMeta,
        tab: ObjectInspectorTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(connected) = &self.connection.connected else {
            return;
        };
        let connection_name = connected.name.clone();
        let config = connected.client_config.clone();
        let object_name = object.name.clone();
        // Auto-load cardinality if this table was analyzed earlier this
        // session, so it shows without re-prompting.
        let cache_key = (
            connection_name.clone(),
            database_name.clone(),
            object_name.clone(),
        );
        let cached_cardinalities = self.schema.cardinality_cache.get(&cache_key).cloned();
        let cached_measured = self
            .schema
            .measured_cache
            .get(&cache_key)
            .cloned()
            .unwrap_or_default();
        let window_handle = window.window_handle();
        let ddl_editor = cx.new(|cx| InputState::new(window, cx).code_editor("sql"));
        let engine_editor = cx.new(|cx| {
            InputState::new(window, cx)
                .code_editor("sql")
                .line_number(false)
        });
        self.schema.selected_object = Some(SelectedSchemaObject {
            database: database_name.clone(),
            object,
            loading: true,
            columns: Vec::new(),
            details: None,
            storage: None,
            cardinalities: cached_cardinalities,
            cardinality_loading: false,
            cardinality_error: None,
            cardinality_confirming: false,
            measured: cached_measured,
            applying: None,
            applying_slow: false,
            partitions: None,
            partitions_loading: false,
            partitions_error: None,
            merges: Vec::new(),
            dependencies: None,
            dependencies_loading: false,
            dependencies_error: None,
            projections: None,
            projections_loading: false,
            projections_error: None,
            ddl_editor: ddl_editor.clone(),
            engine_editor: engine_editor.clone(),
            tab,
            error: None,
        });
        self.show_query_editor = false;
        cx.notify();

        // A carried-over tab must load its own data (tab clicks normally do).
        match tab {
            ObjectInspectorTab::Parts => {
                self.load_partitions(cx);
                self.start_merges_poll(cx);
            }
            ObjectInspectorTab::Dependencies => self.load_dependencies(cx),
            ObjectInspectorTab::Projections => self.load_projections(cx),
            _ => {}
        }

        if let Some(cache) = &self.schema.cache {
            cache.touch_database(&database_name);
        }
        self.warm_schema_columns(database_name.clone(), window, cx);

        let task = rt::tokio().spawn({
            let database_name = database_name.clone();
            let object_name = object_name.clone();
            async move {
                let client = ChClient::new(config);
                tokio::join!(
                    client.list_columns(&database_name, &object_name),
                    client.object_details(&database_name, &object_name),
                    client.table_storage(&database_name, &object_name)
                )
            }
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            if let Ok((_, Ok(details), _)) = &result {
                let ddl = details.create_table_query.clone();
                let engine = format_engine_definition(&details.engine_full);
                cx.update_window(window_handle, |_, window, cx| {
                    ddl_editor.update(cx, |editor, cx| editor.set_value(ddl, window, cx));
                    engine_editor.update(cx, |editor, cx| editor.set_value(engine, window, cx));
                })
                .ok();
            }
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
                selected.loading = false;
                match result {
                    Ok((Ok(columns), Ok(details), storage)) => {
                        selected.columns = columns;
                        selected.details = Some(details);
                        // Storage is supplementary; a failure here (e.g.
                        // no access to system.parts) must not fail the
                        // whole load, so drop it to None rather than error.
                        selected.storage = storage.ok().flatten();
                    }
                    Ok((Err(error), _, _)) | Ok((_, Err(error), _)) => {
                        selected.error = Some(error.to_string());
                    }
                    Err(error) => selected.error = Some(error.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}
