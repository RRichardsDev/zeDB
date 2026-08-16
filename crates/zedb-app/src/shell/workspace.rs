use crate::*;

use gpui::prelude::*;

impl Workspace {
    pub(crate) fn query_editor_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let tab_rows = self
            .query
            .tabs
            .iter()
            .enumerate()
            .map(|(index, tab)| {
                let tab_id = tab.id;
                let multiple = self.query.tabs.len() > 1;
                let has_right = index + 1 < self.query.tabs.len();
                let active = index == self.query.active_tab;
                // Tail tabs are labelled "Tail N" and wear a steel-blue,
                // top-rounded border so they read as a distinct live view.
                let tail_number = tab.tail.as_ref().map(|state| state.number);
                let is_tail = tail_number.is_some();
                let label = tab_display_name(tab);
                div()
                    .id(("query-tab", tab_id))
                    .flex_none()
                    .h_full()
                    .px_3()
                    .flex()
                    .items_center()
                    .whitespace_nowrap()
                    .when(!is_tail, |tab| {
                        tab.border_b_2()
                            .when(active, |tab| {
                                tab.border_color(theme::accent()).text_color(theme::text())
                            })
                            .when(!active, |tab| {
                                tab.border_color(theme::bg_sidebar())
                                    .text_color(theme::text_dim())
                                    .hover(|tab| tab.text_color(theme::text()).cursor_pointer())
                            })
                    })
                    .when(is_tail, |tab| {
                        tab.border_1()
                            .border_color(rgb(0x4682b4))
                            .rounded_t(px(5.))
                            .when(active, |tab| tab.text_color(theme::text()))
                            .when(!active, |tab| {
                                tab.text_color(theme::text_dim())
                                    .hover(|tab| tab.text_color(theme::text()).cursor_pointer())
                            })
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.query.active_tab = index;
                        cx.notify();
                    }))
                    // Drag to reorder: a ghost of the label follows the
                    // cursor, the drop target shows an accent left edge.
                    .on_drag(
                        DragTab {
                            index,
                            label: label.clone().into(),
                        },
                        |drag, _, _, cx| {
                            cx.stop_propagation();
                            cx.new(|_| drag.clone())
                        },
                    )
                    .drag_over::<DragTab>(|style, _, _, _| {
                        style.border_l_2().border_color(theme::accent())
                    })
                    .on_drop(cx.listener(move |this, drag: &DragTab, _, cx| {
                        this.reorder_query_tab(drag.index, index, cx);
                    }))
                    .context_menu(move |menu, _, _| {
                        menu.menu_with_enable(
                            "Close tab",
                            Box::new(CloseQueryTab { tab_id }),
                            multiple,
                        )
                        .menu_with_enable(
                            "Close others",
                            Box::new(CloseOtherQueryTabs { tab_id }),
                            multiple,
                        )
                        .menu_with_enable(
                            "Close to the right",
                            Box::new(CloseQueryTabsToRight { tab_id }),
                            has_right,
                        )
                    })
                    .gap_2()
                    .child(label)
                    .when(self.query.tabs.len() > 1, |tab_row| {
                        tab_row.child(
                            div()
                                .id(("close-query-tab", tab_id))
                                .size(px(18.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(3.))
                                .text_color(theme::text_dim())
                                .child("×")
                                .when(
                                    !matches!(
                                        tab.outcome,
                                        QueryOutcome::Running | QueryOutcome::StatementError { .. }
                                    ),
                                    |close| {
                                        close
                                            .hover(|close| {
                                                close
                                                    .bg(theme::hover())
                                                    .text_color(theme::text())
                                                    .cursor_pointer()
                                            })
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                cx.stop_propagation();
                                                this.close_query_tab(tab_id, cx);
                                            }))
                                    },
                                ),
                        )
                    })
            })
            .collect::<Vec<_>>();
        let active = self
            .query
            .tabs
            .get(self.query.active_tab)
            .expect("query editor requires an active tab");
        let running = matches!(
            active.outcome,
            QueryOutcome::Running | QueryOutcome::StatementError { .. }
        );
        let statement_failed = matches!(active.outcome, QueryOutcome::StatementError { .. });
        let error_text = match &active.outcome {
            QueryOutcome::Error(error) => Some(error.clone()),
            _ => None,
        };
        // Owned snapshot of the active tab's tail, so the strip renders
        // without re-borrowing self.
        let tail_info = active.tail.as_ref().map(|state| {
            // Dirty when the editor no longer matches the adopted query, so
            // the "update tail" button can appear.
            let editor_text = active.editor.read(cx).value().to_string();
            TailStripInfo {
                tab_id: active.id,
                key: state.query.key.clone(),
                paused: state.paused,
                error: state.error.clone(),
                rows: active.result_rows,
                native_available: state.native_available == Some(true),
                push: state.push,
                experimental_streaming_enabled: self.preferences.experimental_streaming_queries,
                dirty: editor_text.trim() != state.baseline.trim(),
            }
        });
        // Owned snapshot of the pre-flight estimate for the same reason.
        let estimate_info = active
            .estimate
            .as_ref()
            .map(|estimate| (active.id, estimate.clone()));
        // Ask needs a remembered agent that discovery has not ruled out.
        let ask_agent = self.preferences.last_agent.clone().filter(|name| {
            self.agent.agents.is_empty()
                || self.agent.agents.iter().any(|agent| agent.name == *name)
        });
        let ask_agent_icon = ask_agent.as_ref().map(|name| {
            self.agent
                .agents
                .iter()
                .find(|agent| agent.name == *name)
                .map(|agent| agent_pane::icon_for(&agent.id))
                .unwrap_or(match name.as_str() {
                    // Discovery may not have run yet; the built-ins
                    // are known by name.
                    "Claude Code" => "icons/agent-claude.svg",
                    "Codex" => "icons/agent-codex.svg",
                    _ => "icons/sparkle.svg",
                })
        });
        let has_result = active.has_result || active.explain.is_some();
        let result_capped = active.result_capped;
        let editor_height = active.editor_height;
        let status_height = active.status_height;
        let result_grid = active.result_grid.clone();
        let mut status = match &active.outcome {
            QueryOutcome::Idle => "Ready".to_string(),
            QueryOutcome::Running => format!("Running: {} row(s)", active.result_rows),
            QueryOutcome::Complete {
                columns,
                rows,
                skipped,
            } => {
                let mut text = if *columns == 0 {
                    // DDL and other resultless statements: an empty body
                    // with HTTP 200 is ClickHouse's whole success signal.
                    "OK: statement executed (no result set)".to_string()
                } else if result_capped {
                    format!("Showing first {rows} row(s), {columns} column(s)")
                } else {
                    format!("Complete: {rows} row(s), {columns} column(s)")
                };
                if *skipped > 0 {
                    text.push_str(&format!("  {skipped} statement(s) skipped"));
                }
                text
            }
            QueryOutcome::Error(error) => error.clone(),
            QueryOutcome::StatementError {
                index,
                total,
                message,
            } => {
                format!("Statement {} of {total} failed: {message}", index + 1)
            }
            QueryOutcome::Cancelled => "Query cancelled".to_string(),
        };
        if let Some(read_rows) = active.read_rows {
            if let Some(total_rows) = active.total_rows {
                status.push_str(&format!(
                    "  Read {} of {} rows",
                    Self::format_count(read_rows),
                    Self::format_count(total_rows)
                ));
            } else {
                status.push_str(&format!("  Read {} rows", Self::format_count(read_rows)));
            }
        }
        if let Some(read_bytes) = active.read_bytes {
            status.push_str(&format!("  {} read", Self::format_bytes(read_bytes)));
        } else if active.received_bytes > 0 {
            status.push_str(&format!(
                "  {} received",
                Self::format_bytes(active.received_bytes)
            ));
        }
        let elapsed = active
            .elapsed
            .or_else(|| active.started_at.map(|started| started.elapsed()))
            .map(format_query_duration);

        let editor_column = div()
            .h_full()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(36.))
                    .flex_none()
                    .flex()
                    .items_end()
                    .justify_between()
                    .bg(theme::bg_sidebar())
                    .border_b_1()
                    .border_color(theme::border())
                    .child(
                        // Tabs scroll (incl. shift-wheel) so they never push
                        // the toolbar off-screen; the toolbar is flex_none
                        // and always wins the space.
                        div()
                            .id("query-tabs-scroll")
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .flex()
                            .items_end()
                            .overflow_x_scroll()
                            .children(tab_rows)
                            .child(
                                div()
                                    .id("add-query-tab")
                                    .flex_none()
                                    .h_full()
                                    .px_3()
                                    .flex()
                                    .items_center()
                                    .text_color(theme::text_dim())
                                    .child("+")
                                    .hover(|button| {
                                        button.text_color(theme::text()).cursor_pointer()
                                    })
                                    // Dropping a dragged tab here sends it to
                                    // the very end (the one spot no tab's own
                                    // drop zone covers).
                                    .drag_over::<DragTab>(|style, _, _, _| {
                                        style.border_l_2().border_color(theme::accent())
                                    })
                                    .on_drop(cx.listener(|this, drag: &DragTab, _, cx| {
                                        let last = this.query.tabs.len().saturating_sub(1);
                                        this.reorder_query_tab(drag.index, last, cx);
                                    }))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.add_query_tab(window, cx)
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .flex_none()
                            .h_full()
                            .pr_2()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(self.max_rows_selector(running, cx))
                            .child(
                                div()
                                    .id("run-selection")
                                    .px_3()
                                    .py_1()
                                    .rounded(px(3.))
                                    .border_1()
                                    .border_color(theme::border())
                                    .text_color(theme::text_dim())
                                    .flex()
                                    .items_center()
                                    .gap_1p5()
                                    .child(
                                        svg()
                                            .path("icons/execute.svg")
                                            .size(px(13.))
                                            .text_color(theme::text_dim()),
                                    )
                                    .child("Execute")
                                    .tooltip(|window, cx| {
                                        gpui_component::tooltip::Tooltip::new(
                                            "Execute the selection, or every statement \u{b7} \u{2303}X",
                                        )
                                        .build(window, cx)
                                    })
                                    .when(!running, |button| {
                                        button
                                            .text_color(theme::text())
                                            .hover(|button| {
                                                button.bg(theme::hover()).cursor_pointer()
                                            })
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.run_selection(window, cx)
                                            }))
                                    }),
                            )
                            .child(
                                div()
                                    .id("run-query")
                                    .group("run-button")
                                    .px_3()
                                    .py_1()
                                    .rounded(px(3.))
                                    .map(|button| {
                                        if running {
                                            // Running at rest; Cancel on hover
                                            // (stacked labels: hover cannot
                                            // change text).
                                            button
                                                .relative()
                                                .bg(theme::hover())
                                                .text_color(theme::text_dim())
                                                .hover(|button| {
                                                    button
                                                        .bg(theme::danger_hover())
                                                        .text_color(theme::danger())
                                                        .cursor_pointer()
                                                })
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.cancel_query(cx)
                                                }))
                                                .child(
                                                    div()
                                                        .group_hover("run-button", |label| {
                                                            label.invisible()
                                                        })
                                                        .child("Running\u{2026}"),
                                                )
                                                .child(
                                                    div()
                                                        .absolute()
                                                        .inset_0()
                                                        .flex()
                                                        .items_center()
                                                        .justify_center()
                                                        .invisible()
                                                        .group_hover("run-button", |label| {
                                                            label.visible()
                                                        })
                                                        .child("Cancel"),
                                                )
                                        } else {
                                            button
                                                .bg(theme::primary())
                                                .text_color(theme::primary_foreground())
                                                .child("Run")
                                                .tooltip(|window, cx| {
                                                    gpui_component::tooltip::Tooltip::new(
                                                        "Run the statement at the cursor \u{b7} \u{2318}\u{21a9}",
                                                    )
                                                    .build(window, cx)
                                                })
                                                .hover(|button| {
                                                    button
                                                        .bg(theme::primary_hover())
                                                        .cursor_pointer()
                                                })
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.run_query(window, cx)
                                                }))
                                        }
                                    }),
                            )
                            .child(
                                div()
                                    .id("toggle-history")
                                    .size(px(24.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(3.))
                                    .when(self.history.open, |button| button.bg(theme::hover()))
                                    .child(
                                        svg().path("icons/history.svg").size(px(14.)).text_color(
                                            if self.history.open {
                                                theme::text()
                                            } else {
                                                theme::text_dim()
                                            },
                                        ),
                                    )
                                    .hover(|button| button.bg(theme::hover()).cursor_pointer())
                                    .tooltip(|window, cx| {
                                        gpui_component::tooltip::Tooltip::new(
                                            "Query history and saved queries",
                                        )
                                        .build(window, cx)
                                    })
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.history_toggle(cx)),
                                    ),
                            ),
                    ),
            )
            .child(
                div()
                    .when(!has_result, |editor| editor.flex_1())
                    .when(has_result, |editor| editor.h(px(editor_height)).flex_none())
                    .min_h_0()
                    .relative()
                    .bg(theme::bg())
                    .child(
                        Input::new(&active.editor)
                            .appearance(false)
                            .bordered(false)
                            .focus_bordered(false)
                            .pl(px(4.))
                            .h_full(),
                    )
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .top_0()
                            .bottom_0()
                            .w(px(50.))
                            .bg(rgba(0x15181c48))
                            .border_r_1()
                            .border_color(rgb(0x2b3037)),
                    ),
            )
            .when(has_result, |panel| {
                panel.child(self.query_resize_handle(
                    "query-editor-resize-handle",
                    QueryResizeTarget::Editor,
                    cx,
                ))
            })
            // The tail strip sits directly above the results, where the eye
            // is already resting on the newest rows.
            .when_some(tail_info, |panel, info| {
                panel.child(self.tail_strip(info, cx))
            })
            // The pre-flight estimate reads in the same place: above the
            // results the query would produce.
            .when_some(estimate_info, |panel, (tab_id, estimate)| {
                panel.child(self.estimate_strip(tab_id, estimate, cx))
            })
            .when(has_result, |panel| {
                panel.child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .border_t_1()
                        .border_color(theme::border())
                        .map(|pane| match active.explain.as_ref() {
                            Some(plan) => pane.child(self.explain_panel(plan, cx)),
                            None => pane.child(result_grid),
                        }),
                )
            })
            .when(active.advisor.is_some() && active.explain.is_none(), |panel| {
                panel.child(self.query_advisor_panel(active, cx))
            })
            .child(self.query_resize_handle(
                "query-status-resize-handle",
                QueryResizeTarget::Status,
                cx,
            ))
            .child(
                div()
                    .h(px(status_height))
                    .flex_none()
                    .px_3()
                    .py_2()
                    .overflow_y_scrollbar()
                    .border_t_1()
                    .border_color(theme::border())
                    .when(
                        matches!(
                            active.outcome,
                            QueryOutcome::Error(_) | QueryOutcome::StatementError { .. }
                        ),
                        |row| row.bg(rgb(0x2b2227)).text_color(theme::danger()),
                    )
                    .when(
                        !matches!(
                            active.outcome,
                            QueryOutcome::Error(_) | QueryOutcome::StatementError { .. }
                        ),
                        |row| row.text_color(theme::text_dim()),
                    )
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .items_start()
                            .justify_between()
                            .gap_4()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .items_center()
                                    .gap_3()
                                    .child(div().flex_1().min_w_0().child(status)),
                            )
                            .when_some(error_text, |row, error| {
                                let copy_error = error.clone();
                                row.child(
                                    div()
                                        .flex_none()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .child(
                                            div()
                                                .id("copy-error")
                                                .size(px(22.))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .rounded(px(3.))
                                                .hover(|button| {
                                                    button.bg(theme::hover()).cursor_pointer()
                                                })
                                                .tooltip(|window, cx| {
                                                    gpui_component::tooltip::Tooltip::new(
                                                        "Copy error",
                                                    )
                                                    .build(window, cx)
                                                })
                                                .on_click(cx.listener(move |_, _, _, cx| {
                                                    cx.write_to_clipboard(
                                                        ClipboardItem::new_string(
                                                            copy_error.clone(),
                                                        ),
                                                    );
                                                }))
                                                .child(
                                                    svg()
                                                        .path("icons/copy.svg")
                                                        .size(px(13.))
                                                        .text_color(theme::text_dim()),
                                                ),
                                        )
                                        .when_some(ask_agent.clone(), |actions, agent_name| {
                                            // Visible message: the error itself.
                                            // Hidden context: where it came from.
                                            let visible = format!(
                                                "This query failed, help me diagnose and fix it:\n{error}"
                                            );
                                            let mut hidden = format!(
                                                "Context (not shown to the user): the error came from zeDB query tab \"Query {}\"",
                                                active.id
                                            );
                                            match &active.failed_sql {
                                                Some(sql) => hidden.push_str(&format!(
                                                    ", which executed:\n```sql\n{sql}\n```\nIf you propose a corrected query with the propose_query tool, zeDB will replace the failed statement in that tab in place."
                                                )),
                                                None => hidden.push('.'),
                                            }
                                            let fix_target = active
                                                .failed_sql
                                                .clone()
                                                .map(|sql| (active.id, sql));
                                            actions.child(
                                                // Just the remembered agent's
                                                // logo; the tooltip names it.
                                                div()
                                                    .id("ask-agent-error")
                                                    .size(px(22.))
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .rounded(px(3.))
                                                    .hover(|button| {
                                                        button
                                                            .bg(theme::hover())
                                                            .cursor_pointer()
                                                    })
                                                    .on_click(cx.listener(
                                                        move |this, _, window, cx| {
                                                            this.agent_fix_target =
                                                                fix_target.clone();
                                                            this.agent_ask_about(
                                                                visible.clone(),
                                                                hidden.clone(),
                                                                window,
                                                                cx,
                                                            );
                                                        },
                                                    ))
                                                    .child(
                                                        svg()
                                                            .path(
                                                                ask_agent_icon
                                                                    .unwrap_or("icons/sparkle.svg"),
                                                            )
                                                            .size(px(14.))
                                                            .text_color(theme::text()),
                                                    )
                                                    .tooltip(move |window, cx| {
                                                        gpui_component::tooltip::Tooltip::new(
                                                            format!("Ask {agent_name}"),
                                                        )
                                                        .build(window, cx)
                                                    }),
                                            )
                                        }),
                                )
                            })
                            .when(statement_failed, |row| {
                                row.child(
                                    div()
                                        .flex_none()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .child(
                                            div()
                                                .id("skip-failed-statement")
                                                .px_2()
                                                .rounded(px(3.))
                                                .border_1()
                                                .border_color(theme::border())
                                                .text_color(theme::text())
                                                .hover(|button| {
                                                    button
                                                        .bg(theme::danger_hover())
                                                        .cursor_pointer()
                                                })
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.resolve_statement_failure(true, cx)
                                                }))
                                                .child("Skip"),
                                        )
                                        .child(
                                            div()
                                                .id("cancel-remaining-statements")
                                                .px_2()
                                                .rounded(px(3.))
                                                .border_1()
                                                .border_color(theme::border())
                                                .text_color(theme::text())
                                                .hover(|button| {
                                                    button
                                                        .bg(theme::danger_hover())
                                                        .cursor_pointer()
                                                })
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.resolve_statement_failure(false, cx)
                                                }))
                                                .child("Cancel rest"),
                                        ),
                                )
                            })
                            .when_some(elapsed, |row, elapsed| {
                                row.child(
                                    div()
                                        .flex_none()
                                        .text_color(theme::text_dim())
                                        .child(elapsed),
                                )
                            }),
                    ),
            );

        div()
            .size_full()
            .flex()
            .child(editor_column)
            .when(self.history.open, |root| {
                root.child(self.history_resize_handle(cx))
                    .child(self.history_drawer(cx))
            })
    }

    pub(crate) fn status_bar(&self) -> impl IntoElement {
        let status = self
            .notice
            .clone()
            .unwrap_or_else(|| match &self.connection.connected {
                Some(connected) => format!(
                    "Connected to {} via {}",
                    connected.name, connected.active_endpoint
                ),
                None => "Not connected".to_string(),
            });
        div()
            .h(px(28.))
            .flex_none()
            .w_full()
            .bg(theme::bg_status())
            .border_t_1()
            .border_color(theme::border())
            .px_3()
            .flex()
            .items_center()
            .justify_between()
            .text_xs()
            .text_color(if self.notice_warning {
                theme::danger()
            } else {
                theme::text_dim()
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .min_w_0()
                    .child(div().overflow_hidden().whitespace_nowrap().child(status))
                    .when_some(self.footer_vim_state(), |row, state| {
                        let (normal, label, command_line, recording) = state;
                        row.child(div().flex_none().child("|"))
                            .child(
                                div()
                                    .flex_none()
                                    .text_color(if normal {
                                        theme::toggle_knob_on()
                                    } else {
                                        theme::text_dim()
                                    })
                                    .child(format!("-- {label} --")),
                            )
                            .when_some(command_line, |row, command_line| {
                                let mut text = command_line.text;
                                let cursor = command_line.cursor.min(text.chars().count());
                                let byte = text
                                    .char_indices()
                                    .nth(cursor)
                                    .map(|(index, _)| index)
                                    .unwrap_or(text.len());
                                text.insert(byte, '\u{258c}');
                                row.child(
                                    div()
                                        .flex_none()
                                        .text_color(theme::text())
                                        .child(format!("{}{text}", command_line.prompt)),
                                )
                            })
                            .when_some(recording, |row, register| {
                                row.child(
                                    div()
                                        .flex_none()
                                        .text_color(theme::warning())
                                        .child(format!("recording @{register}")),
                                )
                            })
                    }),
            )
            .child(concat!("zedb ", env!("CARGO_PKG_VERSION")))
    }

    /// Vim state for the bottom bar: mode, command line, and recording
    /// register of the active query tab, when vim mode is on and the
    /// query editor is the active view.
    // The tuple is read once, immediately destructured by the only
    // caller (the status bar). A named struct for four borrowed
    // fields would cost more than it explains.
    #[allow(clippy::type_complexity)]
    pub(crate) fn footer_vim_state(
        &self,
    ) -> Option<(
        bool,
        &'static str,
        Option<CommandLineSnapshot>,
        Option<char>,
    )> {
        if !self.preferences.vim_mode || self.show_fleet || self.connection.connected.is_none() {
            return None;
        }
        let tab = self.query.tabs.get(self.query.active_tab)?;
        Some((
            tab.vim.mode() == modalkit::env::vim::VimMode::Normal,
            tab.vim.mode_label(),
            tab.vim_command_line.clone(),
            tab.vim_recording,
        ))
    }
}
