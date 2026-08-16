use crate::*;

use gpui::prelude::*;

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Agent-requested effects need a Window; apply them here.
        self.agent_drain_effects(window, cx);
        // Tabs are connection-scoped; keep the active tab visible (and
        // one tab existing) whatever changed since the last frame.
        self.ensure_visible_tab(window, cx);
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme::bg())
            .text_color(theme::text())
            .font_family("Menlo")
            .text_sm()
            .on_action(cx.listener(Self::run_query_action))
            .on_action(cx.listener(Self::run_selection_action))
            .on_action(cx.listener(|this, _: &SaveQueryTab, _, cx| this.save_active_query_tab(cx)))
            .on_action(cx.listener(|this, action: &CloseQueryTab, _, cx| {
                this.close_query_tab(action.tab_id, cx);
            }))
            .on_action(cx.listener(|this, action: &CloseOtherQueryTabs, _, cx| {
                this.close_other_query_tabs(action.tab_id, cx);
            }))
            .on_action(cx.listener(|this, action: &CloseQueryTabsToRight, _, cx| {
                this.close_query_tabs_to_right(action.tab_id, cx);
            }))
            .on_action(cx.listener(|this, action: &TailTable, window, cx| {
                this.start_tail(
                    action.database.clone(),
                    action.object.clone(),
                    action.cap,
                    window,
                    cx,
                );
            }))
            .on_action(
                cx.listener(|this, _: &MaxRows1k, _, cx| this.select_max_rows(MaxRows::Rows1k, cx)),
            )
            .on_action(
                cx.listener(|this, _: &MaxRows10k, _, cx| {
                    this.select_max_rows(MaxRows::Rows10k, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &MaxRows50k, _, cx| {
                    this.select_max_rows(MaxRows::Rows50k, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &MaxRows100k, _, cx| {
                this.select_max_rows(MaxRows::Rows100k, cx)
            }))
            .on_action(
                cx.listener(|this, _: &MaxRows1m, _, cx| this.select_max_rows(MaxRows::Rows1m, cx)),
            )
            .on_action(cx.listener(|this, _: &MaxRowsUnlimited, _, cx| {
                this.select_max_rows(MaxRows::Unlimited, cx)
            }))
            .on_action(
                cx.listener(|this, action: &SelectNode, _, cx| this.select_node(action.index, cx)),
            )
            .on_action(cx.listener(|this, action: &SetApplyCluster, _, cx| {
                this.set_apply_cluster(action.cluster.clone(), cx)
            }))
            .on_action(cx.listener(|this, _: &AddLocalConnection, _, cx| this.start_add(cx)))
            .on_action(cx.listener(|this, _: &AddCloudConnection, _, cx| this.cloud_open(cx)))
            .on_action(cx.listener(|this, action: &DuplicateConnection, _, cx| {
                this.duplicate_connection(action.index, cx)
            }))
            .on_action(cx.listener(|this, action: &EditConnection, _, cx| {
                this.connection.selected = Some(action.index);
                this.start_edit(cx)
            }))
            .on_action(cx.listener(|this, action: &DeleteConnection, _, cx| {
                this.connection.selected = Some(action.index);
                this.request_delete(cx)
            }))
            .on_action(cx.listener(|this, action: &grid_spike::HeaderSort, _, cx| {
                if let Some(tab) = this.query.tabs.get(this.query.active_tab) {
                    let grid = tab.result_grid.clone();
                    grid.update(cx, |grid, cx| grid.header_sort_action(action, cx));
                }
            }))
            // The grid's right-click Copy / Copy as CSV menu dispatches
            // to the window root, so handle it here and delegate to the
            // active tab's grid (cmd-C is handled on the grid itself).
            .on_action(cx.listener(|this, _: &grid_spike::Copy, _, cx| {
                if let Some(tab) = this.query.tabs.get(this.query.active_tab) {
                    let grid = tab.result_grid.clone();
                    grid.update(cx, |grid, cx| grid.copy_selected(cx));
                }
            }))
            .on_action(cx.listener(|this, _: &grid_spike::CopyAsCsv, _, cx| {
                if let Some(tab) = this.query.tabs.get(this.query.active_tab) {
                    let grid = tab.result_grid.clone();
                    grid.update(cx, |grid, cx| grid.copy_selected_csv(cx));
                }
            }))
            .on_action(
                cx.listener(|this, action: &grid_spike::HeaderFilter, window, cx| {
                    this.open_column_filter(action.column.clone(), window, cx)
                }),
            )
            .on_action(cx.listener(|this, action: &ops::SetOpsTopLimit, _, cx| {
                this.ops_set_top_limit(action.limit, cx)
            }))
            .on_action(cx.listener(|this, action: &ops::SetOpsScope, _, cx| {
                this.ops_set_scope(action.cluster.clone(), cx)
            }))
            .on_action(cx.listener(|this, action: &ViewObjectDdl, window, cx| {
                let object = this
                    .schema
                    .databases
                    .iter()
                    .find_map(|database| {
                        if database.meta.name != action.database {
                            return None;
                        }
                        database.objects.as_ref()?.iter().find_map(|object| {
                            (object.name == action.object).then(|| object.clone())
                        })
                    })
                    .or_else(|| {
                        // The sidebar may not have loaded that database's
                        // objects yet; the schema cache has them.
                        this.schema.cache.as_ref().and_then(|cache| {
                            cache
                                .snapshot()
                                .object(&action.database, &action.object)
                                .map(schema_object_from_cache)
                        })
                    });
                if let Some(object) = object {
                    this.select_schema_object(
                        action.database.clone(),
                        object,
                        ObjectInspectorTab::Ddl,
                        window,
                        cx,
                    )
                }
            }))
            .on_action(
                cx.listener(|this, action: &agent_pane::StartAgentThread, window, cx| {
                    if action.index != usize::MAX {
                        this.agent_start_thread(action.index, window, cx);
                    }
                }),
            )
            .on_action(
                cx.listener(|this, _: &agent_pane::OpenAddAgent, window, cx| {
                    this.agent_open_add_form(window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &ToggleAgentPane, window, cx| this.agent_toggle(window, cx)),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, _, cx| {
                    // Any click that reaches the root dismisses an open
                    // filter popover; clicks inside it stop propagation.
                    if let Some(tab) = this.query.tabs.get(this.query.active_tab) {
                        let grid = tab.result_grid.clone();
                        grid.update(cx, |grid, cx| {
                            grid.close_filter_panel(cx);
                        });
                    }
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                if this.resizing_sidebar {
                    this.sidebar_width = f32::from(event.position.x).clamp(180.0, 480.0);
                    cx.notify();
                }
                if this.resizing_sidebar_sections {
                    let viewport_height = f32::from(window.viewport_size().height);
                    let maximum = (viewport_height - 220.0).max(140.0);
                    this.connections_pane_height =
                        (f32::from(event.position.y) - 36.0).clamp(140.0, maximum);
                    cx.notify();
                }
                if this.agent.resizing {
                    let viewport_width = f32::from(window.viewport_size().width);
                    this.agent.width = (viewport_width - f32::from(event.position.x))
                        .clamp(300.0, (viewport_width - 500.0).max(300.0));
                    cx.notify();
                }
                if this.fleet.resizing_detail {
                    let viewport_width = f32::from(window.viewport_size().width);
                    this.fleet.detail_width = (viewport_width - f32::from(event.position.x))
                        .clamp(280.0, (viewport_width - 400.0).max(280.0));
                    cx.notify();
                }
                if let Some((start_width, start_x)) = this.history.resizing {
                    let width = start_width + (start_x - f32::from(event.position.x));
                    this.history.width = width.clamp(240.0, 640.0);
                    cx.notify();
                }
                if let Some((target, last_y)) = this.query.resize {
                    let current_y = f32::from(event.position.y);
                    let delta = current_y - last_y;
                    if let Some(tab) = this.query.tabs.get_mut(this.query.active_tab) {
                        match target {
                            QueryResizeTarget::Editor => {
                                tab.editor_height = (tab.editor_height + delta).clamp(80.0, 720.0);
                            }
                            QueryResizeTarget::Status => {
                                tab.status_height = (tab.status_height - delta).clamp(34.0, 240.0);
                            }
                        }
                    }
                    this.query.resize = Some((target, current_y));
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, _| {
                    this.resizing_sidebar = false;
                    this.resizing_sidebar_sections = false;
                    this.fleet.resizing_detail = false;
                    if this.agent.resizing {
                        this.agent.resizing = false;
                        this.preferences.agent_pane_width = Some(this.agent.width);
                        let _ = save_preferences(&this.preferences);
                    }
                    this.query.resize = None;
                    this.history.resizing = None;
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, _| {
                    this.resizing_sidebar = false;
                    this.resizing_sidebar_sections = false;
                    this.fleet.resizing_detail = false;
                    this.agent.resizing = false;
                    this.query.resize = None;
                    this.history.resizing = None;
                }),
            )
            .when(self.export.is_some(), |root| {
                root.child(self.export_overlay(cx))
            })
            .when(self.schema.pending_apply.is_some(), |root| {
                root.child(self.apply_confirm_overlay(cx))
            })
            .when(self.palette.open, |root| {
                root.child(self.command_palette_overlay(cx))
            })
            .child(self.title_bar(cx))
            .child(
                div()
                    .flex_1()
                    .w_full()
                    .min_h_0()
                    .flex()
                    .child(self.sidebar(cx))
                    .child(self.sidebar_resize_handle(cx))
                    .child(
                        div()
                            .flex_1()
                            .h_full()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            // ClickHouse yellow marks a live Cloud
                            // service: same value as the mark asset.
                            .when(self.active_connection_is_cloud(), |main| {
                                main.border_1().border_color(gpui::rgb(0xFFCC01))
                            })
                            .when(self.show_preferences, |main| {
                                main.child(self.preferences_panel(cx))
                            })
                            // The Cloud panel wins over an open form:
                            // a form's Add node leads there and comes
                            // back with the form intact.
                            .when(
                                !self.show_preferences && self.connection.cloud.open,
                                |main| main.child(self.cloud_panel(cx)),
                            )
                            .when(
                                !self.show_preferences
                                    && !self.connection.cloud.open
                                    && self.connection.form.is_some(),
                                |main| main.child(self.form_panel(cx)),
                            )
                            .when(
                                !self.show_preferences
                                    && self.connection.form.is_none()
                                    && !self.connection.cloud.open,
                                |main| {
                                    main.child(self.connection_toolbar(cx)).child(
                                        div()
                                            .flex_1()
                                            .min_h_0()
                                            .when(self.show_ops, |content| {
                                                content.child(self.ops_panel(cx))
                                            })
                                            .when(
                                                !self.show_ops && self.show_query_editor,
                                                |content| {
                                                    content.child(self.query_editor_panel(cx))
                                                },
                                            )
                                            .when(
                                                !self.show_ops
                                                    && !self.show_query_editor
                                                    && self.show_fleet,
                                                |content| content.child(self.fleet_panel(cx)),
                                            )
                                            .when(
                                                !self.show_ops
                                                    && !self.show_query_editor
                                                    && !self.show_fleet,
                                                |content| {
                                                    content
                                                        .when(
                                                            self.schema.selected_object.is_some(),
                                                            |content| {
                                                                content.child(
                                                                    self.schema_object_panel(
                                                                        window, cx,
                                                                    ),
                                                                )
                                                            },
                                                        )
                                                        .when(
                                                            self.schema.selected_object.is_none(),
                                                            |content| {
                                                                content
                                                                    .child(self.cluster_overview())
                                                            },
                                                        )
                                                },
                                            ),
                                    )
                                },
                            ),
                    )
                    .when(self.agent.open, |row| {
                        row.child(self.agent_panel(window, cx))
                    }),
            )
            .child(self.status_bar())
            .when(self.show_about, |root| root.child(self.about_panel(cx)))
    }
}
