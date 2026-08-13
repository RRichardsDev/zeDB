use crate::*;

use gpui::prelude::*;

impl Workspace {
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
}
