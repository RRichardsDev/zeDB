use crate::*;

use gpui::prelude::*;

impl Workspace {
    pub(crate) fn sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self
            .connection
            .connections
            .iter()
            .enumerate()
            .map(|(index, connection)| {
                let selected = self.connection.selected == Some(index);
                let cloud_state = self.cloud_state_label(connection);
                let connected = self
                    .connection
                    .connected
                    .as_ref()
                    .map(|connected| connected.name.as_str())
                    == Some(connection.name.as_str());
                div()
                    .id(("connection", index))
                    .group("connection-row")
                    .w_full()
                    .px_2()
                    .py_2()
                    .rounded(px(3.))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .when(selected, |row| row.bg(theme::hover()))
                    .hover(|row| row.bg(theme::row_hover()).cursor_pointer())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        // A second click on the already-selected row
                        // brings up its Cluster connection screen (the
                        // only route back to it while connected).
                        if this.connection.selected == Some(index)
                            && (this.show_query_editor || this.show_ops)
                        {
                            this.show_query_editor = false;
                            this.show_fleet = false;
                            this.show_ops = false;
                        }
                        this.connection.selected = Some(index);
                        this.connection.pending_delete = None;
                        this.notice = None;
                        cx.notify();
                    }))
                    .context_menu(move |menu, _, _| {
                        menu.menu("Edit", Box::new(EditConnection { index }))
                            .menu("Duplicate", Box::new(DuplicateConnection { index }))
                            .menu("Delete", Box::new(DeleteConnection { index }))
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .text_color(theme::text())
                            .child(
                                // Name plus an inline muted node count "(N)"
                                // at rest; hovering hides it and reveals the
                                // full "N nodes" line below. The name gives
                                // way (truncates) before the badge column
                                // does: marks must survive any sidebar width.
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .items_center()
                                    .gap_1p5()
                                    .child(
                                        div()
                                            .min_w_0()
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .child(connection.name.clone()),
                                    )
                                    .child(
                                        div()
                                            .flex_none()
                                            .text_xs()
                                            .text_color(theme::text_dim())
                                            .group_hover("connection-row", |count| {
                                                count.invisible()
                                            })
                                            .child(format!("({})", connection.nodes.len())),
                                    ),
                            )
                            .child(
                                // At rest the row wears only two small
                                // marks: a triangle in the environment
                                // color and a square in the read/write
                                // color. Hovering the row swaps in the
                                // full pills. Only the small marks hold
                                // width in flow; the pills overlay on hover
                                // (with a masking background), so a row
                                // never truncates its name to reserve pill
                                // space it is not showing.
                                div()
                                    .flex_none()
                                    .relative()
                                    .flex()
                                    .items_center()
                                    .justify_end()
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(3.))
                                            .group_hover("connection-row", |marks| {
                                                marks.invisible()
                                            })
                                            .when(connected, |marks| {
                                                marks.child(
                                                    div()
                                                        .size(px(7.))
                                                        .rounded_full()
                                                        .bg(theme::success())
                                                        .mr_1(),
                                                )
                                            })
                                            .child(Self::write_glyph(connection.read_only))
                                            .child(Self::tier_glyph(connection.tier)),
                                    )
                                    .child(
                                        div()
                                            .absolute()
                                            .right_0()
                                            .flex()
                                            .items_center()
                                            .gap_1()
                                            .pl_1()
                                            .bg(theme::row_hover())
                                            .invisible()
                                            .group_hover("connection-row", |pills| pills.visible())
                                            .when(connected, |pills| {
                                                pills.child(
                                                    div()
                                                        .size(px(7.))
                                                        .rounded_full()
                                                        .bg(theme::success())
                                                        .mr_1(),
                                                )
                                            })
                                            .child(Self::write_badge_small(connection.read_only))
                                            .child(Self::tier_badge_small(connection.tier)),
                                    ),
                            ),
                    )
                    .when_some(cloud_state, |row, state| {
                        // The linked Cloud service is not running: its own
                        // line under the name, clear of the badge column.
                        row.child(div().text_xs().text_color(theme::text_dim()).child(state))
                    })
                    .child(
                        // The full "N nodes" line is collapsed at rest (the
                        // inline "(N)" stands in) and expands on hover.
                        div()
                            .max_h(px(0.))
                            .overflow_hidden()
                            .text_color(theme::text_dim())
                            .group_hover("connection-row", |line| line.max_h(px(20.)))
                            .child({
                                let count = connection.nodes.len();
                                if connection.cloud.is_some() && count > 1 {
                                    // A warehouse: compute pools over
                                    // one dataset, not cluster nodes.
                                    format!("{count} compute \u{b7} shared data")
                                } else {
                                    format!("{count} node{}", if count == 1 { "" } else { "s" })
                                }
                            }),
                    )
            })
            .collect::<Vec<_>>();

        div()
            .w(px(self.sidebar_width))
            .flex_none()
            .h_full()
            .bg(theme::bg_sidebar())
            .flex()
            .flex_col()
            .text_sm()
            .text_color(theme::text_dim())
            .child(
                div()
                    .h(px(self.connections_pane_height))
                    .min_h_0()
                    .flex_none()
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child("CONNECTIONS")
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .child(
                                        div()
                                            .id("link-cloud")
                                            .px_2()
                                            .py_1()
                                            .rounded(px(3.))
                                            .text_xs()
                                            .flex()
                                            .items_center()
                                            .gap_1()
                                            // The ClickHouse mark, in its own
                                            // brand yellow, names whose cloud
                                            // this is.
                                            .child(
                                                svg()
                                                    .path("icons/clickhouse.svg")
                                                    .size(px(11.))
                                                    .text_color(rgb(0xFFCC01)),
                                            )
                                            .child("Cloud")
                                            .when(self.connection.cloud.open, |button| {
                                                button.bg(theme::hover())
                                            })
                                            .hover(|button| {
                                                button
                                                    .bg(theme::hover())
                                                    .text_color(theme::text())
                                                    .cursor_pointer()
                                            })
                                            .tooltip(|window, cx| {
                                                gpui_component::tooltip::Tooltip::new(
                                                    "Link ClickHouse Cloud services",
                                                )
                                                .build(window, cx)
                                            })
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                if this.connection.cloud.open {
                                                    this.cloud_close(cx)
                                                } else {
                                                    this.cloud_open(cx)
                                                }
                                            })),
                                    )
                                    .child(
                                        div()
                                            .id("add-connection")
                                            .px_2()
                                            .py_1()
                                            .rounded(px(3.))
                                            .text_color(theme::text())
                                            .child("+")
                                            .hover(|button| {
                                                button.bg(theme::hover()).cursor_pointer()
                                            })
                                            .on_click(
                                                cx.listener(|this, _, _, cx| this.start_add(cx)),
                                            ),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .id("connection-list")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scrollbar()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .when(rows.is_empty(), |list| {
                                list.child(
                                    div()
                                        .pt_3()
                                        .text_color(theme::text_dim())
                                        .child("No saved connections"),
                                )
                                .child(
                                    div()
                                        .id("link-cloud-empty")
                                        .pt_1()
                                        .text_xs()
                                        .text_color(theme::accent())
                                        .child("Link ClickHouse Cloud")
                                        .hover(|button| button.cursor_pointer())
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.cloud_open(cx)),
                                        ),
                                )
                            })
                            .children(rows),
                    )
                    .when(self.connection.selected.is_some(), |sidebar| {
                        sidebar.child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .when_some(self.connection.pending_delete.as_ref(), |panel, name| {
                                    panel
                                        .child(div().text_xs().text_color(theme::danger()).child(
                                            format!(
                                                "Delete {name}? This also removes its saved password."
                                            ),
                                        ))
                                        .child(
                                            div()
                                                .flex()
                                                .justify_end()
                                                .gap_1()
                                                .child(
                                                    div()
                                                        .id("cancel-delete-connection")
                                                        .px_2()
                                                        .py_1()
                                                        .rounded(px(3.))
                                                        .text_xs()
                                                        .text_color(theme::text_dim())
                                                        .child("Cancel")
                                                        .hover(|button| {
                                                            button
                                                                .bg(theme::hover())
                                                                .text_color(theme::text())
                                                                .cursor_pointer()
                                                        })
                                                        .on_click(cx.listener(
                                                            |this, _, _, cx| {
                                                                this.cancel_delete(cx)
                                                            },
                                                        )),
                                                )
                                                .child(
                                                    div()
                                                        .id("confirm-delete-connection")
                                                        .px_2()
                                                        .py_1()
                                                        .rounded(px(3.))
                                                        .text_xs()
                                                        .bg(rgb(0x6f2929))
                                                        .text_color(rgb(0xffb4ad))
                                                        .child("Delete")
                                                        .hover(|button| {
                                                            button
                                                                .bg(rgb(0x8b3434))
                                                                .text_color(theme::text_bright())
                                                                .cursor_pointer()
                                                        })
                                                        .on_click(cx.listener(
                                                            |this, _, _, cx| {
                                                                this.confirm_delete(cx)
                                                            },
                                                        )),
                                                ),
                                        )
                                })
                                .when(self.connection.pending_delete.is_none(), |panel| {
                                    panel.child(
                                        div()
                                            .h(px(32.))
                                            .mx(px(-12.))
                                            .mb(px(-12.))
                                            .px_2()
                                            .flex()
                                            .items_center()
                                            .justify_end()
                                            .gap_1()
                                            .border_t_1()
                                            .border_color(theme::border())
                                            .child(
                                                div()
                                                    .id("duplicate-connection")
                                                    .size(px(24.))
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .rounded(px(3.))
                                                    .text_color(theme::text_dim())
                                                    .child(
                                                        svg()
                                                            .path("icons/copy.svg")
                                                            .size(px(14.))
                                                            .text_color(theme::text_dim()),
                                                    )
                                                    .hover(|button| {
                                                        button
                                                            .bg(theme::hover())
                                                            .text_color(theme::text())
                                                            .cursor_pointer()
                                                    })
                                                    .tooltip(|window, cx| {
                                                        gpui_component::tooltip::Tooltip::new(
                                                            "Duplicate connection",
                                                        )
                                                        .build(window, cx)
                                                    })
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        if let Some(index) = this.connection.selected {
                                                            this.duplicate_connection(index, cx)
                                                        }
                                                    })),
                                            )
                                            .child(
                                                div()
                                                    .id("edit-connection")
                                                    .size(px(24.))
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .rounded(px(3.))
                                                    .text_color(theme::text_dim())
                                                    .child(
                                                        svg()
                                                            .path("icons/edit.svg")
                                                            .size(px(14.))
                                                            .text_color(theme::text_dim()),
                                                    )
                                                    .hover(|button| {
                                                        button
                                                            .bg(theme::hover())
                                                            .text_color(theme::text())
                                                            .cursor_pointer()
                                                    })
                                                    .tooltip(|window, cx| {
                                                        gpui_component::tooltip::Tooltip::new(
                                                            "Edit connection",
                                                        )
                                                        .build(window, cx)
                                                    })
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.start_edit(cx)
                                                    })),
                                            )
                                            .child(
                                                div()
                                                    .id("delete-connection")
                                                    .size(px(24.))
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .rounded(px(3.))
                                                    .text_color(theme::text_dim())
                                                    .child(
                                                        svg()
                                                            .path("icons/trash.svg")
                                                            .size(px(14.))
                                                            .text_color(theme::text_dim()),
                                                    )
                                                    .when(self.connection.connecting.is_none(), |button| {
                                                        button
                                                            .hover(|button| {
                                                                button
                                                                    .bg(theme::danger_hover())
                                                                    .text_color(theme::danger())
                                                                    .cursor_pointer()
                                                            })
                                                            .tooltip(|window, cx| {
                                                                gpui_component::tooltip::Tooltip::new(
                                                                    "Delete connection",
                                                                )
                                                                .build(window, cx)
                                                            })
                                                            .on_click(cx.listener(
                                                                |this, _, _, cx| {
                                                                    this.request_delete(cx)
                                                                },
                                                            ))
                                                    }),
                                            ),
                                    )
                                }),
                        )
                    }),
            )
            .child(self.sidebar_section_resize_handle(cx))
            .child(self.schema_sidebar(cx))
    }

    pub(crate) fn schema_kind_label(kind: SchemaObjectKind, engine: &str) -> &'static str {
        match kind {
            // A Distributed table holds no data of its own; it scatters
            // over the cluster's shard-local tables.
            SchemaObjectKind::Table if engine == "Distributed" => "DT",
            SchemaObjectKind::Table => "T",
            SchemaObjectKind::View => "V",
            SchemaObjectKind::MaterializedView => "MV",
            SchemaObjectKind::Dictionary => "D",
        }
    }

    pub(crate) fn schema_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let filter = self.schema.filter.read(cx).text().to_lowercase();
        let cache_status = self.schema.cache.as_ref().map(|cache| {
            let snapshot = cache.snapshot();
            format!(
                "{} of {} databases ready",
                snapshot.warmed_databases(),
                snapshot.databases.len()
            )
        });
        let selected = self
            .schema
            .selected_object
            .as_ref()
            .map(|selected| (selected.database.as_str(), selected.object.name.as_str()));
        let database_rows = self
            .schema
            .databases
            .iter()
            .enumerate()
            .filter_map(|(database_index, database)| {
                let database_matches = database.meta.name.to_lowercase().contains(&filter);
                let matching_objects = database
                    .objects
                    .as_ref()
                    .map(|objects| {
                        objects
                            .iter()
                            .filter(|object| {
                                filter.is_empty()
                                    || database_matches
                                    || object.name.to_lowercase().contains(&filter)
                            })
                            .cloned()
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                if !filter.is_empty() && !database_matches && matching_objects.is_empty() {
                    return None;
                }

                let database_name = database.meta.name.clone();
                let show_objects = if filter.is_empty() {
                    database.expanded
                } else {
                    !database.filter_collapsed
                };
                let object_rows = matching_objects
                    .into_iter()
                    .enumerate()
                    .map(|(object_index, object)| {
                        let is_selected =
                            selected == Some((database_name.as_str(), object.name.as_str()));
                        let row_database = database_name.clone();
                        let row_object = object.clone();
                        let size_id = database_index.saturating_mul(100_000) + object_index;
                        div()
                            .id((
                                "schema-object",
                                database_index.saturating_mul(100_000) + object_index,
                            ))
                            .h(px(26.))
                            .pl_5()
                            .pr_2()
                            .flex()
                            .items_center()
                            .gap_2()
                            .rounded(px(3.))
                            .when(is_selected, |row| row.bg(theme::hover()))
                            .hover(|row| row.bg(theme::row_hover()).cursor_pointer())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                // Keep whatever inspector tab is open, so
                                // moving between tables stays in context.
                                let tab = this
                                    .schema
                                    .selected_object
                                    .as_ref()
                                    .map(|selected| selected.tab)
                                    .unwrap_or(ObjectInspectorTab::Overview);
                                this.select_schema_object(
                                    row_database.clone(),
                                    row_object.clone(),
                                    tab,
                                    window,
                                    cx,
                                )
                            }))
                            .context_menu({
                                let database = database_name.clone();
                                let engine = object.engine.clone();
                                let object = object.name.clone();
                                move |menu, window, cx| {
                                    let menu = menu.menu(
                                        "View DDL",
                                        Box::new(ViewObjectDdl {
                                            database: database.clone(),
                                            object: object.clone(),
                                        }),
                                    );
                                    // Tail is a MergeTree-family thing (a
                                    // monotonic key to advance on). The
                                    // submenu is the retained-row cap the
                                    // user opts into; the initial load is
                                    // always small either way.
                                    if engine.contains("MergeTree") {
                                        let database = database.clone();
                                        let object = object.clone();
                                        menu.submenu("Tail", window, cx, move |menu, _, _| {
                                            let caps: [(&str, Option<usize>); 6] = [
                                                ("20 rows", Some(20)),
                                                ("50 rows", Some(50)),
                                                ("100 rows", Some(100)),
                                                ("500 rows", Some(500)),
                                                ("1000 rows", Some(1000)),
                                                ("Unlimited", None),
                                            ];
                                            caps.into_iter().fold(menu, |menu, (label, cap)| {
                                                menu.menu(
                                                    label,
                                                    Box::new(TailTable {
                                                        database: database.clone(),
                                                        object: object.clone(),
                                                        cap,
                                                    }),
                                                )
                                            })
                                        })
                                    } else {
                                        menu
                                    }
                                }
                            })
                            .child(
                                div()
                                    .w(px(20.))
                                    .text_xs()
                                    .text_color(theme::text_dim())
                                    .child(Self::schema_kind_label(object.kind, &object.engine)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_color(theme::text())
                                    .child(object.name),
                            )
                            .when_some(object.total_bytes, |row, bytes| {
                                // Parentheses mark a derived number: a
                                // Distributed table's size is its local
                                // table summed across shards.
                                let distributed = object.engine == "Distributed";
                                let text = if distributed {
                                    format!("({})", Self::format_bytes(bytes))
                                } else {
                                    Self::format_bytes(bytes)
                                };
                                let size = div()
                                    .flex_none()
                                    .text_size(px(9.))
                                    .text_color(theme::text_dim())
                                    .child(text);
                                row.child(if distributed {
                                    size.id(("schema-object-size", size_id))
                                        .tooltip(|window, cx| {
                                            gpui_component::tooltip::Tooltip::new(
                                                "Virtual: the local table summed across shards",
                                            )
                                            .build(window, cx)
                                        })
                                        .into_any_element()
                                } else {
                                    size.into_any_element()
                                })
                            })
                    })
                    .collect::<Vec<_>>();

                Some(
                    div()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .id(("schema-database", database_index))
                                .h(px(26.))
                                .px_2()
                                .flex()
                                .items_center()
                                .gap_2()
                                .rounded(px(3.))
                                .hover(|row| row.bg(theme::row_hover()).cursor_pointer())
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.toggle_schema_database(database_index, window, cx)
                                }))
                                .child(if show_objects { "▾" } else { "▸" })
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_color(theme::text())
                                        .child(database.meta.name.clone()),
                                ),
                        )
                        .when(database.loading, |node| {
                            node.child(
                                div()
                                    .pl_5()
                                    .h(px(24.))
                                    .flex()
                                    .items_center()
                                    .text_xs()
                                    .child("Loading..."),
                            )
                        })
                        .when_some(database.error.as_ref(), |node, error| {
                            node.child(
                                div()
                                    .pl_5()
                                    .pr_2()
                                    .py_1()
                                    .text_xs()
                                    .text_color(theme::danger())
                                    .child(error.clone()),
                            )
                        })
                        .when(show_objects, |node| node.children(object_rows)),
                )
            })
            .collect::<Vec<_>>();

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(34.))
                    .px_3()
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_xs()
                    .child("SCHEMA")
                    .when(self.connection.connected.is_some(), |header| {
                        header.child(
                            div()
                                .id("refresh-schema")
                                .size(px(24.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(3.))
                                .text_color(theme::text_dim())
                                .child(
                                    svg()
                                        .path("icons/refresh.svg")
                                        .size(px(14.))
                                        .text_color(theme::text_dim()),
                                )
                                .hover(|button| {
                                    button
                                        .bg(theme::hover())
                                        .text_color(theme::text())
                                        .cursor_pointer()
                                })
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.load_schema_databases(cx)),
                                ),
                        )
                    }),
            )
            .when(self.connection.connected.is_some(), |panel| {
                panel.child(div().px_2().pb_2().child(self.schema.filter.clone()))
            })
            .when_some(cache_status, |panel, status| {
                panel.child(
                    div()
                        .px_3()
                        .pb_1()
                        .text_xs()
                        .text_color(theme::text_dim())
                        .child(status),
                )
            })
            .child(
                div()
                    .id("schema-tree")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_1()
                    .when(self.connection.connected.is_none(), |tree| {
                        tree.child(
                            div()
                                .px_2()
                                .py_2()
                                .text_xs()
                                .child("Connect to browse schema"),
                        )
                    })
                    .when(self.schema.loading, |tree| {
                        tree.child(div().px_2().py_2().text_xs().child("Loading databases..."))
                    })
                    .when_some(self.schema.error.as_ref(), |tree, error| {
                        tree.child(
                            div()
                                .px_2()
                                .py_2()
                                .text_xs()
                                .text_color(theme::danger())
                                .child(error.clone()),
                        )
                    })
                    .children(database_rows),
            )
    }
}
