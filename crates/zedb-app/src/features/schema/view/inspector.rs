use crate::*;

use gpui::prelude::*;

impl Workspace {
    pub(crate) fn format_count(value: u64) -> String {
        let digits = value.to_string();
        let mut formatted = String::with_capacity(digits.len() + digits.len() / 3);
        for (index, character) in digits.chars().enumerate() {
            if index > 0 && (digits.len() - index).is_multiple_of(3) {
                formatted.push(',');
            }
            formatted.push(character);
        }
        formatted
    }

    pub(crate) fn format_bytes(bytes: u64) -> String {
        const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
        let mut value = bytes as f64;
        let mut unit = 0;
        while value >= 1000.0 && unit < UNITS.len() - 1 {
            value /= 1000.0;
            unit += 1;
        }
        if unit == 0 {
            format!("{bytes} {}", UNITS[unit])
        } else {
            format!("{value:.1} {}", UNITS[unit])
        }
    }

    pub(crate) fn schema_object_panel(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let selected = self
            .schema
            .selected_object
            .as_ref()
            .expect("schema object panel requires a selection");
        let cardinalities = selected.cardinalities.clone();
        let measured = selected.measured.clone();
        let applying = selected.applying;
        let applying_slow = selected.applying_slow;
        let advisor_db = selected.database.clone();
        let advisor_table = selected.object.name.clone();
        let advisor_rows = selected.object.total_rows.unwrap_or(0);
        // In cluster scope the generated statements run ON CLUSTER.
        let advisor_cluster = self
            .connection
            .connected
            .as_ref()
            .and_then(|connected| connected.apply_cluster.clone());
        let column_rows = selected
            .columns
            .iter()
            .enumerate()
            .map(|(index, column)| {
                // 0 compressed bytes means the object holds no data (a
                // view, or an empty table); show a dash rather than
                // "0 B / NaNx".
                let has_data = column.compressed_bytes > 0;
                let compressed = if has_data {
                    Self::format_bytes(column.compressed_bytes)
                } else {
                    "\u{2014}".to_string()
                };
                let uncompressed = if column.uncompressed_bytes > 0 {
                    Self::format_bytes(column.uncompressed_bytes)
                } else {
                    "\u{2014}".to_string()
                };
                let ratio = if has_data {
                    format!(
                        "{:.1}x",
                        column.uncompressed_bytes as f64 / column.compressed_bytes as f64
                    )
                } else {
                    "\u{2014}".to_string()
                };
                let codec = if column.codec.is_empty() {
                    "\u{2014}".to_string()
                } else {
                    column.codec.clone()
                };
                // Storage advice is computed once the cardinality probe
                // has run. None before then.
                let distinct_count = cardinalities
                    .as_ref()
                    .and_then(|values| values.get(index))
                    .copied();
                let advice = distinct_count.map(|distinct| {
                    storage_advisor::advise(
                        &storage_advisor::ColumnFacts {
                            name: &column.name,
                            type_name: &column.type_name,
                            codec: &column.codec,
                            distinct,
                            total_rows: advisor_rows,
                            compressed_bytes: column.compressed_bytes,
                            uncompressed_bytes: column.uncompressed_bytes,
                        },
                        &advisor_db,
                        &advisor_table,
                        advisor_cluster.as_deref(),
                    )
                });
                div()
                    .id(("schema-column", index))
                    .h(px(30.))
                    .flex_none()
                    .px_3()
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(theme::border())
                    .when(index % 2 == 1, |row| row.bg(rgb(0x1f2329)))
                    .child(
                        // Preferred width, but allowed to shrink (and
                        // truncate) on a narrow window so the storage
                        // columns and codec are not pushed off the edge.
                        div()
                            .w(px(220.))
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_color(theme::text())
                            .child(column.name.clone()),
                    )
                    .child(
                        div()
                            .w(px(300.))
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_color(theme::text_dim())
                            .child(type_highlight::styled(&column.type_name)),
                    )
                    .child(
                        div()
                            .w(px(100.))
                            .flex_none()
                            .pl_4()
                            .text_right()
                            .text_color(theme::text())
                            .child(compressed),
                    )
                    .child(
                        div()
                            .w(px(100.))
                            .flex_none()
                            .pl_4()
                            .text_right()
                            .text_color(theme::text_dim())
                            .child(uncompressed),
                    )
                    .child(
                        div()
                            .w(px(80.))
                            .flex_none()
                            .pl_4()
                            .text_right()
                            .text_color(theme::text_dim())
                            .child(ratio),
                    )
                    .child(
                        div()
                            .flex_1()
                            // A floor so CODEC keeps a usable width and it
                            // is COLUMN/TYPE that shrink on a narrow window,
                            // rather than the codec clipping off the edge.
                            .min_w(px(230.))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .pl_4()
                            .text_color(theme::text_dim())
                            // Codec expressions read like nested type
                            // calls (CODEC(Delta(8), ZSTD(1))), so the
                            // type colorer highlights them the same way;
                            // the plain "—" placeholder passes through dim.
                            .child(type_highlight::styled(&codec)),
                    )
                    // The advisor's verdict. Actionable ones are clickable
                    // and copy their ALTER; the distinct count and reason
                    // ride along in a tooltip.
                    .when_some(advice, |row, advice| {
                        // The analysis lane: a green tick (hover = why it is
                        // fine) when there is nothing to do, or an action
                        // icon that opens the query editor with the ALTER
                        // (hover = what it will do and why) when there is.
                        let base = || {
                            div()
                                .w(px(110.))
                                .flex_none()
                                .border_l_1()
                                .border_color(theme::border())
                                .flex()
                                .items_center()
                                .justify_center()
                        };
                        let evidence = distinct_count
                            .map(|distinct| {
                                format!("{} distinct values", Self::format_count(distinct))
                            })
                            .unwrap_or_default();
                        let cell = match advice {
                            storage_advisor::Advice::Good(reason)
                            | storage_advisor::Advice::Leave(reason) => base()
                                .id(("advice", index))
                                .child(
                                    svg()
                                        .path("icons/verify.svg")
                                        .size(px(14.))
                                        .text_color(theme::success()),
                                )
                                .tooltip(move |window, cx| {
                                    gpui_component::tooltip::Tooltip::new(reason.clone())
                                        .build(window, cx)
                                })
                                .into_any_element(),
                            storage_advisor::Advice::Suggest {
                                label,
                                reason,
                                apply,
                                editor_sql,
                                ..
                            } if applying == Some(index) && applying_slow => {
                                // A slow in-place apply is in progress.
                                let _ = (label, reason, apply, editor_sql);
                                base().child(Self::advice_spinner()).into_any_element()
                            }
                            storage_advisor::Advice::Suggest {
                                label,
                                reason,
                                apply,
                                editor_sql,
                                ..
                            } => {
                                // The measured savings from the trial run,
                                // once it lands (writable connections only).
                                let saving = measured.get(&index).copied();
                                let saving_label = saving
                                    .map(|ratio| format!("{ratio:.1}\u{00d7}"))
                                    .unwrap_or_default();
                                let measured_line = saving
                                    .map(|ratio| {
                                        format!("\nMeasured {ratio:.1}x smaller on a sample.")
                                    })
                                    .unwrap_or_default();
                                let tooltip = format!(
                                    "Suggest {label}: {reason} ({evidence}).{measured_line}\n\
                                     Left-click to apply \u{00b7} right-click to open in the \
                                     query editor:\n{editor_sql}"
                                );
                                let editor_left = editor_sql.clone();
                                base()
                                    .id(("advice", index))
                                    .gap_1()
                                    .cursor_pointer()
                                    .rounded(px(3.))
                                    .hover(|cell| cell.bg(theme::hover()))
                                    .child(
                                        svg()
                                            .path("icons/query-plus.svg")
                                            .size(px(14.))
                                            .text_color(theme::accent()),
                                    )
                                    .when(!saving_label.is_empty(), |cell| {
                                        cell.child(
                                            div()
                                                .text_xs()
                                                .text_color(theme::accent())
                                                .child(saving_label),
                                        )
                                    })
                                    // Left-click: apply in place (staging/dev),
                                    // or open the editor (prod / read-only), or
                                    // confirm first on a large table.
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.request_apply(
                                            index,
                                            apply.clone(),
                                            editor_left.clone(),
                                            window,
                                            cx,
                                        );
                                    }))
                                    // Right-click: open in the query editor
                                    // (nothing on production).
                                    .on_mouse_down(
                                        MouseButton::Right,
                                        cx.listener(move |this, _, window, cx| {
                                            this.open_suggestion_in_editor(
                                                editor_sql.clone(),
                                                window,
                                                cx,
                                            );
                                        }),
                                    )
                                    .tooltip(move |window, cx| {
                                        gpui_component::tooltip::Tooltip::new(tooltip.clone())
                                            .build(window, cx)
                                    })
                                    .into_any_element()
                            }
                            storage_advisor::Advice::Unknown => base().into_any_element(),
                        };
                        row.child(cell)
                    })
            })
            .collect::<Vec<_>>();

        let loading = selected.loading;
        let error = selected.error.clone();
        let details = selected.details.clone();
        let ddl_editor = selected.ddl_editor.clone();
        let engine_editor = selected.engine_editor.clone();
        if ddl_editor.focus_handle(cx).is_focused(window)
            || engine_editor.focus_handle(cx).is_focused(window)
        {
            window.blur();
        }
        let tab = selected.tab;
        let tab_bar = div()
            .h(px(34.))
            .flex_none()
            .px_3()
            .flex()
            .items_end()
            .gap_4()
            .border_b_1()
            .border_color(theme::border())
            .children(
                [
                    (ObjectInspectorTab::Overview, "Overview"),
                    (ObjectInspectorTab::Columns, "Columns"),
                    (ObjectInspectorTab::Parts, "Parts"),
                    (ObjectInspectorTab::Projections, "Projections"),
                    (ObjectInspectorTab::Workload, "Workload"),
                    (ObjectInspectorTab::Dependencies, "Dependencies"),
                    (ObjectInspectorTab::Ddl, "DDL"),
                ]
                .into_iter()
                .enumerate()
                .map(|(index, (button_tab, label))| {
                    div()
                        .id(("object-inspector-tab", index))
                        .h_full()
                        .px_1()
                        .flex()
                        .items_center()
                        .border_b_2()
                        .when(tab == button_tab, |button| {
                            button
                                .border_color(theme::accent())
                                .text_color(theme::text())
                        })
                        .when(tab != button_tab, |button| {
                            button
                                .border_color(theme::bg())
                                .text_color(theme::text_dim())
                                .hover(|button| button.text_color(theme::text()).cursor_pointer())
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(selected) = &mut this.schema.selected_object {
                                selected.tab = button_tab;
                                cx.notify();
                            }
                            if button_tab == ObjectInspectorTab::Parts {
                                this.load_partitions(cx);
                                this.start_merges_poll(cx);
                            }
                            if button_tab == ObjectInspectorTab::Dependencies {
                                this.load_dependencies(cx);
                            }
                            if button_tab == ObjectInspectorTab::Projections {
                                this.load_projections(cx);
                            }
                            if button_tab == ObjectInspectorTab::Workload {
                                this.load_workload(cx);
                            }
                        }))
                        .child(label)
                }),
            );

        let content = match tab {
            ObjectInspectorTab::Overview => {
                let has_engine_definition = details
                    .as_ref()
                    .map(|details| !details.engine_full.is_empty())
                    .unwrap_or(false);
                let metadata = details.as_ref().map(|details| {
                    let sharding_key = zedb_ch::distributed_sharding_key(&details.engine_full)
                        .map(|key| ("Sharding key", key));
                    [
                        ("Partition key", details.partition_key.clone()),
                        ("Order by", details.sorting_key.clone()),
                        ("Primary key", details.primary_key.clone()),
                    ]
                    .into_iter()
                    .chain(sharding_key)
                    .map(|(label, value)| {
                        div()
                            .py_3()
                            .border_b_1()
                            .border_color(theme::border())
                            .flex()
                            .gap_4()
                            .child(
                                div()
                                    .w(px(150.))
                                    .flex_none()
                                    .text_color(theme::text_dim())
                                    .child(label),
                            )
                            .child(div().flex_1().min_w_0().text_color(theme::text()).child(
                                if value.is_empty() {
                                    "None".to_string()
                                } else {
                                    value
                                },
                            ))
                    })
                    .collect::<Vec<_>>()
                });
                div().flex_1().min_h_0().child(
                    div()
                        .id("object-overview")
                        .size_full()
                        .overflow_y_scroll()
                        .px_4()
                        .py_2()
                        .when(loading, |panel| {
                            panel.child(
                                div()
                                    .py_3()
                                    .text_color(theme::text_dim())
                                    .child("Loading details..."),
                            )
                        })
                        .when_some(error.as_ref(), |panel, error| {
                            panel.child(
                                div()
                                    .py_3()
                                    .text_color(theme::danger())
                                    .child(error.clone()),
                            )
                        })
                        .when(has_engine_definition, |panel| {
                            panel.child(
                                div()
                                    .py_3()
                                    .border_b_1()
                                    .border_color(theme::border())
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_color(theme::text_dim())
                                            .child("Engine definition"),
                                    )
                                    .child(
                                        div()
                                            .id("engine-definition")
                                            .w_full()
                                            .h(px(132.))
                                            .rounded(px(3.))
                                            .border_1()
                                            .border_color(theme::border())
                                            .bg(theme::bg_sunken())
                                            .overflow_hidden()
                                            .child(
                                                Input::new(&engine_editor)
                                                    .appearance(false)
                                                    .bordered(false)
                                                    .focus_bordered(false)
                                                    .disabled(true)
                                                    .tab_index(-1)
                                                    .h_full(),
                                            ),
                                    ),
                            )
                        })
                        .when_some(metadata, |panel, rows| panel.children(rows)),
                )
            }
            ObjectInspectorTab::Columns => div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                // Opt-in cardinality prompt. Shows until the probe
                // has run; the result is cached for the session,
                // so a re-opened table skips the prompt and auto-loads.
                .when(
                    selected.cardinalities.is_none() && !selected.columns.is_empty(),
                    |body| {
                        let loading = selected.cardinality_loading;
                        let confirming = selected.cardinality_confirming;
                        // On a writable connection the measurement step
                        // writes a temporary table, so the prompt asks
                        // before running. The ghost green primary button:
                        // green outline that fills on hover. The label sits
                        // in a child driven by group-hover, so it flips to
                        // dark on the green fill (a plain hover text_color
                        // does not repaint the text child in this gpui).
                        let ghost = |id: &'static str, label: &'static str| {
                            div()
                                .id(id)
                                .group(id)
                                .px_3()
                                .py_1()
                                .rounded(px(4.))
                                .border_1()
                                .border_color(theme::success())
                                .text_xs()
                                .text_color(theme::success())
                                .cursor_pointer()
                                .hover(|button| {
                                    button.bg(theme::success()).border_color(theme::success())
                                })
                                .child(
                                    div()
                                        .group_hover(id, |label| label.text_color(rgb(0x14171c)))
                                        .child(label),
                                )
                        };
                        let message = if confirming {
                            "Measuring the suggestions creates a temporary table on the \
                             server (dropped afterwards). Continue?"
                        } else {
                            "Analyse per-column cardinality to suggest better codecs and \
                             types. Scans the table once."
                        };
                        body.child(
                            div()
                                .flex_none()
                                .px_3()
                                .py_2()
                                .flex()
                                .items_center()
                                .justify_between()
                                .border_b_1()
                                .border_color(theme::border())
                                .bg(theme::bg_sunken())
                                .child(div().text_xs().text_color(theme::text_dim()).child(message))
                                .child(if loading {
                                    div()
                                        .px_3()
                                        .py_1()
                                        .text_xs()
                                        .text_color(theme::text_dim())
                                        .child("Analysing\u{2026}")
                                        .into_any_element()
                                } else if confirming {
                                    div()
                                        .flex()
                                        .gap_2()
                                        .child(
                                            div()
                                                .id("cancel-analyze")
                                                .px_3()
                                                .py_1()
                                                .rounded(px(4.))
                                                .text_xs()
                                                .text_color(theme::text_dim())
                                                .cursor_pointer()
                                                .hover(|button| {
                                                    button
                                                        .bg(theme::hover())
                                                        .text_color(theme::text())
                                                })
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.cancel_analyze(cx)
                                                }))
                                                .child("Cancel"),
                                        )
                                        .child(ghost("confirm-analyze", "Continue").on_click(
                                            cx.listener(|this, _, window, cx| {
                                                this.confirm_analyze(window, cx)
                                            }),
                                        ))
                                        .into_any_element()
                                } else {
                                    ghost("analyze-cardinality", "Analyse")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.request_analyze(window, cx)
                                        }))
                                        .into_any_element()
                                }),
                        )
                    },
                )
                .child(
                    div()
                        .h(px(28.))
                        .flex_none()
                        .px_3()
                        .flex()
                        .items_center()
                        .bg(theme::bg_sidebar())
                        .border_b_1()
                        .border_color(theme::border())
                        .text_xs()
                        .text_color(theme::text_dim())
                        .child(
                            div()
                                .w(px(220.))
                                .min_w_0()
                                .overflow_hidden()
                                .child("COLUMN"),
                        )
                        .child(div().w(px(300.)).min_w_0().overflow_hidden().child("TYPE"))
                        .child(
                            div()
                                .w(px(100.))
                                .flex_none()
                                .pl_4()
                                .text_right()
                                .child("COMPRESSED"),
                        )
                        .child(
                            div()
                                .w(px(100.))
                                .flex_none()
                                .pl_4()
                                .text_right()
                                .child("UNCOMP."),
                        )
                        .child(
                            div()
                                .w(px(80.))
                                .flex_none()
                                .pl_4()
                                .text_right()
                                .child("RATIO"),
                        )
                        .child(div().flex_1().min_w(px(230.)).pl_4().child("CODEC"))
                        // ADVICE is the analysis lane, set off from the
                        // data columns by a divider and the accent color so
                        // "the data" and "the analysis" read as separate.
                        // Appears once the probe has run.
                        .when(selected.cardinalities.is_some(), |header| {
                            header.child(
                                div()
                                    .w(px(110.))
                                    .flex_none()
                                    .border_l_1()
                                    .border_color(theme::border())
                                    .flex()
                                    .justify_center()
                                    .text_color(theme::accent())
                                    .child("ADVICE"),
                            )
                        }),
                )
                // Per-column bytes only exist for Wide parts; when the
                // table is entirely Compact, explain the empty columns
                // rather than leaving a wall of dashes.
                .when_some(
                    selected
                        .storage
                        .as_ref()
                        .filter(|storage| storage.wide_parts == 0 && storage.compact_parts > 0),
                    |body, storage| {
                        body.child(
                            div()
                                .flex_none()
                                .px_3()
                                .py_1()
                                .bg(theme::bg_sidebar())
                                .border_b_1()
                                .border_color(theme::border())
                                .text_xs()
                                .text_color(theme::text_dim())
                                .child(format!(
                                    "Per-column sizes need Wide parts; all {} parts are Compact \
                                     (see the table ratio above).",
                                    storage.compact_parts
                                )),
                        )
                    },
                )
                .child(
                    div()
                        .id("schema-columns")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .when(loading, |columns| {
                            columns.child(
                                div()
                                    .p_3()
                                    .text_color(theme::text_dim())
                                    .child("Loading columns..."),
                            )
                        })
                        .when_some(error.as_ref(), |columns, error| {
                            columns
                                .child(div().p_3().text_color(theme::danger()).child(error.clone()))
                        })
                        .when(
                            !loading && error.is_none() && selected.columns.is_empty(),
                            |columns| {
                                columns.child(
                                    div()
                                        .p_3()
                                        .text_color(theme::text_dim())
                                        .child("No columns"),
                                )
                            },
                        )
                        .children(column_rows),
                ),
            ObjectInspectorTab::Parts => self.parts_panel(selected, cx),
            ObjectInspectorTab::Workload => self.workload_panel(selected, cx),
            ObjectInspectorTab::Dependencies => self.dependencies_panel(selected, cx),
            ObjectInspectorTab::Projections => self.projections_panel(selected),
            ObjectInspectorTab::Ddl => {
                let ddl = details
                    .as_ref()
                    .map(|details| details.create_table_query.clone())
                    .unwrap_or_default();
                let clipboard_ddl = ddl.clone();
                let has_ddl = !loading && error.is_none() && !ddl.is_empty();
                div().flex_1().min_h_0().flex().flex_col().child(
                    div()
                        .id("object-ddl")
                        .group("ddl-editor")
                        .relative()
                        .flex_1()
                        .min_h_0()
                        .m_3()
                        .overflow_hidden()
                        .rounded(px(3.))
                        .border_1()
                        .border_color(theme::border())
                        .bg(theme::bg_sunken())
                        .text_color(theme::text())
                        .when(loading, |panel| panel.child("Loading DDL..."))
                        .when_some(error.as_ref(), |panel, error| {
                            panel.child(div().text_color(theme::danger()).child(error.clone()))
                        })
                        .when(!loading && error.is_none() && ddl.is_empty(), |panel| {
                            panel
                                .p_3()
                                .child(div().text_color(theme::text_dim()).child("DDL unavailable"))
                        })
                        .when(has_ddl, |panel| {
                            panel.child(
                                Input::new(&ddl_editor)
                                    .appearance(false)
                                    .bordered(false)
                                    .focus_bordered(false)
                                    .disabled(true)
                                    .tab_index(-1)
                                    .h_full(),
                            )
                        })
                        // Copy sits over the top-right of the editor,
                        // revealed on hover, instead of a dedicated bar.
                        .when(has_ddl, |panel| {
                            panel.child(
                                div()
                                    .id("copy-object-ddl")
                                    .absolute()
                                    .top(px(6.))
                                    .right(px(6.))
                                    .size(px(22.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(3.))
                                    .bg(theme::bg_sidebar())
                                    .invisible()
                                    .group_hover("ddl-editor", |icon| icon.visible())
                                    .hover(|icon| icon.cursor_pointer())
                                    .tooltip(|window, cx| {
                                        gpui_component::tooltip::Tooltip::new("Copy DDL")
                                            .build(window, cx)
                                    })
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            clipboard_ddl.clone(),
                                        ));
                                    }))
                                    .child(
                                        svg()
                                            .path("icons/copy.svg")
                                            .size(px(13.))
                                            .text_color(theme::text_dim()),
                                    ),
                            )
                        }),
                )
            }
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_none()
                    .px_4()
                    .py_3()
                    .border_b_1()
                    .border_color(theme::border())
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div().text_lg().text_color(theme::text()).child(format!(
                                    "{}.{}",
                                    selected.database, selected.object.name
                                )),
                            )
                            .child(
                                div()
                                    .px_2()
                                    .py(px(2.))
                                    .rounded(px(3.))
                                    .bg(theme::hover())
                                    .text_xs()
                                    .text_color(theme::text_dim())
                                    .child(selected.object.kind.label()),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_4()
                            .text_xs()
                            .child(
                                div()
                                    .flex()
                                    .gap_1()
                                    .child(div().text_color(theme::text()).child("Engine:"))
                                    .child(
                                        div()
                                            .text_color(theme::text_dim())
                                            .child(selected.object.engine.clone()),
                                    ),
                            )
                            .when_some(selected.object.total_rows, |row, rows| {
                                row.child(
                                    div()
                                        .flex()
                                        .gap_1()
                                        .child(div().text_color(theme::text()).child("Rows:"))
                                        .child(
                                            div()
                                                .text_color(theme::text_dim())
                                                .child(Self::format_count(rows)),
                                        ),
                                )
                            })
                            .when_some(selected.object.total_bytes, |row, bytes| {
                                let distributed = selected.object.engine == "Distributed";
                                let text = if distributed {
                                    format!("({})", Self::format_bytes(bytes))
                                } else {
                                    Self::format_bytes(bytes)
                                };
                                let size = div()
                                    .flex()
                                    .gap_1()
                                    .child(div().text_color(theme::text()).child("Size:"))
                                    .child(div().text_color(theme::text_dim()).child(text));
                                row.child(if distributed {
                                    size.id("object-size-virtual")
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
                            .when_some(selected.storage.as_ref(), |row, storage| {
                                // Table-wide compression is always
                                // available. Per-column sizes need Wide
                                // parts; this ratio does not.
                                let ratio = storage.uncompressed_bytes as f64
                                    / storage.compressed_bytes.max(1) as f64;
                                let raw = Self::format_bytes(storage.uncompressed_bytes);
                                row.child(
                                    div()
                                        .id("object-compression")
                                        .flex()
                                        .gap_1()
                                        .child(div().text_color(theme::text()).child("Ratio:"))
                                        .child(
                                            div()
                                                .text_color(theme::text_dim())
                                                .child(format!("{ratio:.2}x")),
                                        )
                                        .tooltip(move |window, cx| {
                                            gpui_component::tooltip::Tooltip::new(format!(
                                                "{raw} uncompressed"
                                            ))
                                            .build(window, cx)
                                        }),
                                )
                            }),
                    ),
            )
            .child(tab_bar)
            .child(content)
    }
}
