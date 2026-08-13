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

    /// Opt-in cardinality probe (Phase 8, Tier 2): scan the selected
    /// table once for each column's approximate distinct count, off the
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
                    // the actionable suggestions (Tier 3), writable
                    // connections only.
                    this.measure_suggestions(cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Load active parts grouped by partition for the selected object
    /// (Phase 9, Part B), off-thread. No-op if already loaded or loading;
    /// pass `force` to reload. Guards against a stale connection/object.
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

    /// The Parts tab: active parts grouped by partition, with a "too many
    /// parts" warning. Reads the connected node's `system.parts`.
    pub(crate) fn parts_panel(
        &self,
        selected: &SelectedSchemaObject,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        // A single partition with this many active parts is worth flagging
        // (ClickHouse delays inserts around 150, throws around 300).
        const TOO_MANY_PARTS: u64 = 100;

        let loading = selected.partitions_loading;
        let error = selected.partitions_error.clone();
        let partitions = selected.partitions.clone().unwrap_or_default();
        let total_parts: u64 = partitions.iter().map(|partition| partition.parts).sum();
        let total_rows: u64 = partitions.iter().map(|partition| partition.rows).sum();
        let total_compressed: u64 = partitions
            .iter()
            .map(|partition| partition.compressed_bytes)
            .sum();
        let busiest = partitions.iter().map(|p| p.parts).max().unwrap_or(0);

        let num_cell = |width: f32, text: String, dim: bool| {
            div()
                .w(px(width))
                .flex_none()
                .text_right()
                .when(dim, |cell| cell.text_color(theme::text_dim()))
                .child(text)
        };
        let header_cell = |width: f32, text: &'static str| {
            div()
                .w(px(width))
                .flex_none()
                .text_right()
                .text_color(theme::text_dim())
                .child(text)
        };

        let rows: Vec<_> = partitions
            .iter()
            .map(|partition| {
                let ratio = if partition.compressed_bytes > 0 {
                    format!(
                        "{:.1}x",
                        partition.uncompressed_bytes as f64 / partition.compressed_bytes as f64
                    )
                } else {
                    "-".to_string()
                };
                let label = if partition.partition == "tuple()" {
                    "(unpartitioned)".to_string()
                } else {
                    partition.partition.clone()
                };
                let hot = partition.parts >= TOO_MANY_PARTS;
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .py_1()
                    .border_b_1()
                    .border_color(theme::border())
                    .text_sm()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_color(theme::text())
                            .child(label),
                    )
                    .child(
                        num_cell(70.0, Self::format_count(partition.parts), false)
                            .when(hot, |cell| cell.text_color(theme::warning())),
                    )
                    .child(num_cell(110.0, Self::format_count(partition.rows), true))
                    .child(num_cell(
                        110.0,
                        Self::format_bytes(partition.compressed_bytes),
                        true,
                    ))
                    .child(num_cell(
                        110.0,
                        Self::format_bytes(partition.uncompressed_bytes),
                        true,
                    ))
                    .child(num_cell(60.0, ratio, true))
                    .child(num_cell(
                        60.0,
                        Self::format_count(partition.max_level),
                        true,
                    ))
            })
            .collect();

        // Compact count: 42_401_792 -> "42.4M", so a live merge line never
        // wraps.
        let compact = |n: u64| -> String {
            let (value, suffix) = if n >= 1_000_000_000 {
                (n as f64 / 1e9, "B")
            } else if n >= 1_000_000 {
                (n as f64 / 1e6, "M")
            } else if n >= 1_000 {
                (n as f64 / 1e3, "K")
            } else {
                return n.to_string();
            };
            let text = format!("{value:.1}");
            format!("{}{suffix}", text.strip_suffix(".0").unwrap_or(&text))
        };

        // Live merges (auto-refreshed by the poll): a thin progress bar and
        // a compact, dot-separated status line. Mutations are tagged.
        let merge_rows: Vec<_> = selected
            .merges
            .iter()
            .map(|merge| {
                let pct = merge.progress_pct.min(100);
                let partition = if merge.partition_id.is_empty() || merge.partition_id == "all" {
                    "(unpartitioned)".to_string()
                } else {
                    merge.partition_id.clone()
                };
                // The stats that ride the right edge; the progress (bar + %)
                // stays grouped with the label on the left.
                let stats = format!(
                    "{}\u{2192}1 \u{b7} {} rows \u{b7} {} \u{b7} {}s",
                    merge.num_parts,
                    compact(merge.rows_written),
                    Self::format_bytes(merge.memory_usage),
                    merge.elapsed_secs,
                );
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .py_1p5()
                    .border_b_1()
                    .border_color(theme::border())
                    .text_sm()
                    .whitespace_nowrap()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(div().text_color(theme::text()).child(partition))
                            .when(merge.is_mutation, |row| {
                                row.child(
                                    div()
                                        .text_xs()
                                        .text_color(theme::text_dim())
                                        .child("mutation"),
                                )
                            })
                            .child(
                                div()
                                    .w(px(120.))
                                    .h(px(6.))
                                    .rounded(px(3.))
                                    .bg(theme::border())
                                    .child(
                                        div()
                                            .h_full()
                                            .w(px(120. * pct as f32 / 100.))
                                            .rounded(px(3.))
                                            .bg(theme::accent()),
                                    ),
                            )
                            .child(
                                div()
                                    .w(px(40.))
                                    .text_color(theme::text_dim())
                                    .child(format!("{pct}%")),
                            ),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_xs()
                            .text_color(theme::text_dim())
                            .child(stats),
                    )
            })
            .collect();
        let has_merges = !merge_rows.is_empty();

        let body = div()
            .id("object-parts")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px_4()
            .py_2()
            .map(|panel| {
                if loading && partitions.is_empty() {
                    panel.child(
                        div()
                            .py_3()
                            .text_color(theme::text_dim())
                            .child("Loading parts\u{2026}"),
                    )
                } else if let Some(error) = error {
                    panel.child(div().py_3().text_color(theme::danger()).child(error))
                } else if partitions.is_empty() {
                    panel.child(div().py_3().text_color(theme::text_dim()).child(
                        "No active parts. This object stores nothing on disk (a view or \
                             dictionary), or the table is empty.",
                    ))
                } else {
                    panel
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_3()
                                .py_1()
                                .border_b_1()
                                .border_color(theme::border())
                                .text_xs()
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .text_color(theme::text_dim())
                                        .child("Partition"),
                                )
                                .child(header_cell(70.0, "Parts"))
                                .child(header_cell(110.0, "Rows"))
                                .child(header_cell(110.0, "Compressed"))
                                .child(header_cell(110.0, "Uncompressed"))
                                .child(header_cell(60.0, "Ratio"))
                                .child(header_cell(60.0, "Level")),
                        )
                        .children(rows)
                }
            });

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            // Summary + refresh.
            .child(
                div()
                    .flex_none()
                    .px_4()
                    .py_2()
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(theme::border())
                    .child(div().text_xs().text_color(theme::text_dim()).child(
                        if partitions.is_empty() {
                            String::new()
                        } else {
                            format!(
                                "{} partition(s) \u{b7} {} active parts \u{b7} {} rows \u{b7} {}",
                                Self::format_count(partitions.len() as u64),
                                Self::format_count(total_parts),
                                Self::format_count(total_rows),
                                Self::format_bytes(total_compressed),
                            )
                        },
                    ))
                    .child(
                        div()
                            .id("refresh-parts")
                            .size(px(22.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(3.))
                            .child(
                                svg()
                                    .path("icons/refresh.svg")
                                    .size(px(13.))
                                    .text_color(theme::text_dim()),
                            )
                            .hover(|button| button.bg(theme::hover()).cursor_pointer())
                            .tooltip(|window, cx| {
                                gpui_component::tooltip::Tooltip::new("Refresh").build(window, cx)
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(selected) = &mut this.schema.selected_object {
                                    selected.partitions = None;
                                }
                                this.load_partitions(cx);
                            })),
                    ),
            )
            // Live merges (shown only while something is merging).
            .when(has_merges, |panel| {
                panel.child(
                    div()
                        .flex_none()
                        .px_4()
                        .py_2()
                        .border_b_1()
                        .border_color(theme::border())
                        .bg(theme::bg_sunken())
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme::text_dim())
                                .pb_1()
                                .child("Merges in progress"),
                        )
                        .children(merge_rows),
                )
            })
            // A single partition with too many parts slows reads and inserts.
            .when(busiest >= TOO_MANY_PARTS, |panel| {
                panel.child(
                    div()
                        .flex_none()
                        .px_4()
                        .py_2()
                        .border_b_1()
                        .border_color(theme::border())
                        .bg(theme::bg_status())
                        .text_xs()
                        .text_color(theme::warning())
                        .child(format!(
                            "A partition has {} active parts. Many small parts slow reads \
                             and inserts; consider OPTIMIZE, fewer partitions, or larger \
                             inserts.",
                            Self::format_count(busiest)
                        )),
                )
            })
            .child(body)
    }

    /// Load the materialized-view lineage for the selected object
    /// (Phase 9, Part C), off-thread. No-op if already loaded or loading.
    pub(crate) fn load_dependencies(&mut self, cx: &mut Context<Self>) {
        let (connection_name, config, database_name, object_name) = {
            let Some(selected) = &self.schema.selected_object else {
                return;
            };
            if selected.dependencies.is_some() || selected.dependencies_loading {
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
            selected.dependencies_loading = true;
            selected.dependencies_error = None;
        }
        cx.notify();

        let task = rt::tokio().spawn({
            let database_name = database_name.clone();
            let object_name = object_name.clone();
            async move {
                ChClient::new(config)
                    .object_dependencies(&database_name, &object_name)
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
                selected.dependencies_loading = false;
                match result {
                    Ok(Ok(dependencies)) => selected.dependencies = Some(dependencies),
                    Ok(Err(error)) => selected.dependencies_error = Some(error.to_string()),
                    Err(error) => selected.dependencies_error = Some(error.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Load the projections attached to the selected object (Phase 9,
    /// Part C), off-thread. No-op if already loaded or loading.
    pub(crate) fn load_projections(&mut self, cx: &mut Context<Self>) {
        let (connection_name, config, database_name, object_name) = {
            let Some(selected) = &self.schema.selected_object else {
                return;
            };
            if selected.projections.is_some() || selected.projections_loading {
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
            selected.projections_loading = true;
            selected.projections_error = None;
        }
        cx.notify();

        let task = rt::tokio().spawn({
            let database_name = database_name.clone();
            let object_name = object_name.clone();
            async move {
                ChClient::new(config)
                    .table_projections(&database_name, &object_name)
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
                selected.projections_loading = false;
                match result {
                    Ok(Ok(projections)) => selected.projections = Some(projections),
                    Ok(Err(error)) => selected.projections_error = Some(error.to_string()),
                    Err(error) => selected.projections_error = Some(error.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Select the `db.table` a dependency node points at, landing on the
    /// Dependencies tab so the lineage can be walked node by node. Uses the
    /// loaded schema meta when available, else a minimal placeholder the
    /// detail load fills in.
    pub(crate) fn navigate_to_dependency(
        &mut self,
        full: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((database, name)) = full.split_once('.') else {
            return;
        };
        let meta = self
            .schema
            .databases
            .iter()
            .find(|node| node.meta.name == database)
            .and_then(|node| node.objects.as_ref())
            .and_then(|objects| objects.iter().find(|object| object.name == name).cloned())
            .unwrap_or_else(|| SchemaObjectMeta {
                name: name.to_string(),
                engine: String::new(),
                kind: SchemaObjectKind::Table,
                total_rows: None,
                total_bytes: None,
            });
        self.select_schema_object(
            database.to_string(),
            meta,
            ObjectInspectorTab::Dependencies,
            window,
            cx,
        );
    }

    /// The Dependencies tab: the object's materialized-view lineage as
    /// clickable source -> view -> target chains (walk the graph), plus its
    /// projections. This object is emphasized; missing tables are flagged.
    pub(crate) fn dependencies_panel(
        &self,
        selected: &SelectedSchemaObject,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let this = format!("{}.{}", selected.database, selected.object.name);
        let missing: Vec<String> = selected
            .dependencies
            .as_ref()
            .map(|deps| deps.missing_tables.clone())
            .unwrap_or_default();

        // Unique element-id base per chain so clickable nodes don't collide.
        let mut chain_base = 0usize;
        let section = |title: &'static str| {
            div()
                .text_xs()
                .text_color(theme::text_dim())
                .pt_3()
                .pb_1()
                .child(title)
        };

        let mut body = div()
            .id("object-graph")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px_4()
            .py_2();

        if selected.dependencies_loading && selected.dependencies.is_none() {
            return div().flex_1().min_h_0().child(
                body.child(
                    div()
                        .py_3()
                        .text_color(theme::text_dim())
                        .child("Loading lineage\u{2026}"),
                ),
            );
        }
        if let Some(error) = &selected.dependencies_error {
            return div().flex_1().min_h_0().child(
                body.child(
                    div()
                        .py_3()
                        .text_color(theme::danger())
                        .child(error.clone()),
                ),
            );
        }

        let deps = selected.dependencies.clone().unwrap_or_default();
        let has_lineage =
            deps.is_materialized_view || !deps.feeds.is_empty() || !deps.written_by.is_empty();

        if !has_lineage {
            body = body.child(
                div()
                    .py_3()
                    .text_color(theme::text_dim())
                    .child("No materialized views read from or write to this object."),
            );
        } else {
            if !deps.missing_tables.is_empty() {
                body = body.child(
                    div()
                        .mt_1()
                        .px_3()
                        .py_2()
                        .rounded(px(3.))
                        .border_1()
                        .border_color(theme::danger())
                        .text_xs()
                        .text_color(theme::danger())
                        .child(
                            "A referenced table no longer exists: this pipeline is broken and \
                             may be silently dropping inserts.",
                        ),
                );
            }
            if deps.is_materialized_view {
                body = body.child(section("This materialized view"));
                let mut nodes = Vec::new();
                if let Some(source) = deps.reads_from.clone() {
                    nodes.push((source, false));
                }
                nodes.push((this.clone(), true));
                if let Some(target) = deps.writes_to.clone() {
                    nodes.push((target, false));
                }
                body = body.child(self.dep_chain(nodes, &missing, chain_base, cx));
                chain_base += 10;
            }
            if !deps.written_by.is_empty() {
                body = body.child(section("Written by"));
                for mv in &deps.written_by {
                    let mut nodes = Vec::new();
                    if let Some(source) = mv.source.clone() {
                        nodes.push((source, false));
                    }
                    nodes.push((mv.view.clone(), false));
                    nodes.push((this.clone(), true));
                    body = body.child(self.dep_chain(nodes, &missing, chain_base, cx));
                    chain_base += 10;
                }
            }
            if !deps.feeds.is_empty() {
                body = body.child(section("Feeds"));
                for mv in &deps.feeds {
                    let mut nodes = vec![(this.clone(), true), (mv.view.clone(), false)];
                    if let Some(target) = mv.target.clone() {
                        nodes.push((target, false));
                    }
                    body = body.child(self.dep_chain(nodes, &missing, chain_base, cx));
                    chain_base += 10;
                }
            }
        }

        div().flex_1().min_h_0().child(body)
    }

    /// The Projections tab: the alternate sorted / pre-aggregated copies of
    /// this table's data that ClickHouse keeps in sync (Phase 9, Part C).
    pub(crate) fn projections_panel(&self, selected: &SelectedSchemaObject) -> gpui::Div {
        let mut body = div()
            .id("object-projections")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px_4()
            .py_2()
            // A one-line explanation of what a projection is.
            .child(
                div()
                    .pt_1()
                    .pb_2()
                    .text_xs()
                    .text_color(theme::text_dim())
                    .child(
                        "A projection is a hidden, always-in-sync copy of this table's data in a \
                         different sort order (or pre-aggregated). Queries that match it read far \
                         less, at the cost of extra storage and write work.",
                    ),
            );

        if selected.projections_loading && selected.projections.is_none() {
            return div().flex_1().min_h_0().child(
                body.child(
                    div()
                        .py_3()
                        .text_color(theme::text_dim())
                        .child("Loading projections\u{2026}"),
                ),
            );
        }
        if let Some(error) = &selected.projections_error {
            return div().flex_1().min_h_0().child(
                body.child(
                    div()
                        .py_3()
                        .text_color(theme::danger())
                        .child(error.clone()),
                ),
            );
        }

        let projections = selected.projections.clone().unwrap_or_default();
        if projections.is_empty() {
            body = body.child(
                div()
                    .py_3()
                    .text_color(theme::text_dim())
                    .child("This table has no projections."),
            );
        } else {
            for projection in &projections {
                body = body.child(
                    div()
                        .py_1p5()
                        .border_b_1()
                        .border_color(theme::border())
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap_3()
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .text_sm()
                                        .child(
                                            div()
                                                .text_color(theme::text())
                                                .child(projection.name.clone()),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(theme::text_dim())
                                                .child(projection.kind.clone()),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex_none()
                                        .text_xs()
                                        .text_color(theme::text_dim())
                                        .child(format!(
                                            "{} rows \u{b7} {} \u{b7} {} parts",
                                            Self::format_count(projection.rows),
                                            Self::format_bytes(projection.compressed_bytes),
                                            Self::format_count(projection.parts),
                                        )),
                                ),
                        )
                        .child(
                            div()
                                .text_xs()
                                .font_family("Menlo")
                                .text_color(theme::text_dim())
                                .overflow_hidden()
                                .whitespace_nowrap()
                                // An Aggregate projection's value is what it
                                // aggregates; a Normal one, its order.
                                .child(
                                    if projection.kind == "Aggregate"
                                        || projection.sorting_key.is_empty()
                                    {
                                        projection.query.clone()
                                    } else {
                                        format!("ORDER BY {}", projection.sorting_key)
                                    },
                                ),
                        ),
                );
            }
        }

        div().flex_1().min_h_0().child(body)
    }

    /// One arrow-joined lineage chain. `this` node is emphasized, missing
    /// tables are flagged in danger, and every other node is clickable to
    /// navigate to it. `base` seeds unique element ids.
    pub(crate) fn dep_chain(
        &self,
        nodes: Vec<(String, bool)>,
        missing: &[String],
        base: usize,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let group = SharedString::from(format!("dep-chain-{base}"));
        let mut row = div()
            .group(group.clone())
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .py_1p5()
            .text_sm();
        for (index, (label, is_self)) in nodes.into_iter().enumerate() {
            if index > 0 {
                row = row.child(
                    div()
                        .flex_none()
                        .text_color(theme::text_dim())
                        .child("\u{2192}"),
                );
            }
            let is_missing = missing.contains(&label);
            let mut pill = div()
                .id(("dep-node", base + index))
                .flex_none()
                .px_2()
                .py_0p5()
                .rounded(px(3.))
                .border_1()
                .when(is_missing, |pill| {
                    pill.border_color(theme::danger())
                        .text_color(theme::danger())
                })
                .when(is_self && !is_missing, |pill| {
                    // Accent border by default, but it yields to whichever
                    // node in the chain is under the cursor.
                    pill.border_color(theme::accent())
                        .bg(theme::selected())
                        .text_color(theme::text())
                        .group_hover(group.clone(), |pill| pill.border_color(theme::border()))
                })
                .when(!is_self && !is_missing, |pill| {
                    pill.border_color(theme::border())
                        .text_color(theme::text_dim())
                })
                .child(if is_missing {
                    format!("{label}  (missing)")
                } else {
                    label.clone()
                });
            if !is_self && !is_missing {
                let target = label.clone();
                pill = pill
                    .hover(|pill| {
                        pill.border_color(theme::accent())
                            .bg(theme::hover())
                            .cursor_pointer()
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.navigate_to_dependency(target.clone(), window, cx)
                    }));
            }
            row = row.child(pill);
        }
        row
    }

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
