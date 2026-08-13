use crate::*;

use gpui::prelude::*;
impl Workspace {
    pub(crate) fn open_query_editor(&mut self, cx: &mut Context<Self>) {
        self.show_query_editor = true;
        self.show_fleet = false;
        self.show_ops = false;
        cx.notify();
    }

    pub(crate) fn make_query_tab(
        id: usize,
        sql: &str,
        schema_provider: Rc<SchemaProvider>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> QueryTab {
        let default_value = sql.to_string();
        let editor = cx.new(|cx| {
            let mut editor = InputState::new(window, cx)
                .code_editor("sql")
                .default_value(default_value);
            editor.lsp.completion_provider = Some(schema_provider.clone());
            editor.lsp.hover_provider = Some(schema_provider.clone());
            // Right-clicking a recognized table adds "View DDL" to the
            // editor's context menu.
            editor.context_menu_extension = Some(Rc::new(move |text, offset, menu| {
                let Some((snapshot, default_database)) = schema_provider.snapshot() else {
                    return menu;
                };
                let sql = text.to_string();
                match zedb_ch::schema_intelligence::object_at(
                    &snapshot,
                    default_database.as_deref(),
                    &sql,
                    offset,
                ) {
                    Some((database, object)) => menu
                        .separator()
                        .menu("View DDL", Box::new(ViewObjectDdl { database, object })),
                    None => menu,
                }
            }));
            editor
        });
        cx.subscribe_in(
            &editor,
            window,
            move |this, state, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Change) {
                    // Text changed outside modalkit (completion accepted,
                    // programmatic insert): resync vim's shadow buffer or
                    // the next vim keystroke would revert the edit. Costs
                    // vim undo history for that buffer, which beats losing
                    // the text itself.
                    if this.preferences.vim_mode {
                        let value = state.read(cx).value().to_string();
                        let cursor = state.read(cx).cursor_position();
                        if let Some(tab) = this.query.tabs.iter_mut().find(|tab| tab.id == id) {
                            if tab.vim.text() != value {
                                tab.vim.reset(
                                    &value,
                                    cursor.line as usize,
                                    cursor.character as usize,
                                );
                            }
                        }
                    }
                    this.schedule_schema_analysis(id, state.clone(), window, cx);
                }
            },
        )
        .detach();
        let result_grid = cx.new(GridSpike::new);
        cx.subscribe_in(
            &result_grid,
            window,
            move |this, _, event: &grid_spike::GridEvent, window, cx| match event {
                grid_spike::GridEvent::SortRequested { sort } => {
                    this.grid_sort_requested(id, sort.clone(), window, cx);
                }
                grid_spike::GridEvent::FilterRequested { column, predicate } => {
                    this.grid_filter_requested(id, column.clone(), predicate.clone(), window, cx);
                }
            },
        )
        .detach();
        QueryTab {
            persistent_id: zedb_core::new_local_id("tab"),
            saved_tab_id: None,
            name: format!("Tab {id}"),
            id,
            editor,
            result_grid: result_grid.clone(),
            result_columns: 0,
            result_rows: 0,
            has_result: false,
            max_rows: MaxRows::Rows100k,
            result_capped: false,
            read_rows: None,
            read_bytes: None,
            total_rows: None,
            received_bytes: 0,
            editor_height: 220.0,
            status_height: 52.0,
            outcome: QueryOutcome::Idle,
            started_at: None,
            elapsed: None,
            vim: VimController::new(sql),
            vim_command_line: None,
            vim_recording: None,
            schema_analysis_generation: 0,
            explain: None,
            advisor: None,
            advise_pending: false,
            advisor_generation: 0,
            failed_sql: None,
            displayed_statement: None,
            displayed_statement_offset: None,
            running_query_id: None,
            tail: None,
        }
    }

    /// Agent-facing: show the query editor, creating a tab if none.
    pub(crate) fn open_query_editor_for_agent(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.query.tabs.is_empty() {
            self.add_query_tab(window, cx);
        } else {
            self.show_query_editor = true;
            self.show_fleet = false;
            cx.notify();
        }
    }

    /// Agent-facing: a new query tab pre-filled with SQL, focused.
    pub(crate) fn open_query_tab_with(
        &mut self,
        sql: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = self.query.next_tab_id;
        self.query.next_tab_id += 1;
        let tab = Self::make_query_tab(id, sql, self.schema.provider.clone(), window, cx);
        self.query.tabs.push(tab);
        self.query.active_tab = self.query.tabs.len() - 1;
        self.show_query_editor = true;
        self.show_fleet = false;
        cx.notify();
    }

    /// The environment tier of the active connection (prod/staging/dev).
    pub(crate) fn active_tier(&self) -> Option<EnvTier> {
        let name = self
            .connection
            .connected
            .as_ref()
            .map(|cluster| cluster.name.as_str())?;
        self.connection
            .connections
            .iter()
            .find(|connection| connection.name == name)
            .map(|connection| connection.tier)
    }

    /// Left-click on an advice icon. Applying rewrites data, so the policy
    /// is: never apply in place on **production** (open the editor to run
    /// deliberately); on a read-only connection there is nowhere to apply
    /// (open the editor); on writable staging/dev apply in place, but if
    /// the table is large first confirm, since it rewrites a lot of data.
    pub(crate) fn request_apply(
        &mut self,
        index: usize,
        apply: Vec<String>,
        editor_sql: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let is_prod = self.active_tier() == Some(EnvTier::Production);
        let writable = self
            .connection
            .connected
            .as_ref()
            .map(|cluster| cluster.name.clone())
            .map(|name| self.connection_is_writable(&name))
            .unwrap_or(false);
        if is_prod || !writable {
            self.open_query_tab_with(&editor_sql, window, cx);
            return;
        }
        const LARGE_TABLE_BYTES: u64 = 1_000_000_000; // ~1 GB
        let large = self
            .schema
            .selected_object
            .as_ref()
            .and_then(|selected| selected.object.total_bytes)
            .is_some_and(|bytes| bytes > LARGE_TABLE_BYTES);
        if large {
            self.schema.pending_apply = Some((index, apply));
            cx.notify();
        } else {
            self.apply_suggestion(index, apply, window, cx);
        }
    }

    pub(crate) fn confirm_apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((index, apply)) = self.schema.pending_apply.take() {
            self.apply_suggestion(index, apply, window, cx);
        }
    }

    pub(crate) fn cancel_apply(&mut self, cx: &mut Context<Self>) {
        self.schema.pending_apply = None;
        cx.notify();
    }

    /// Right-click on an advice icon: open the suggestion in the query
    /// editor. Does nothing on production (per the apply policy).
    pub(crate) fn open_suggestion_in_editor(
        &mut self,
        editor_sql: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.active_tier() == Some(EnvTier::Production) {
            return;
        }
        self.open_query_tab_with(&editor_sql, window, cx);
    }

    /// A small spinning indicator for a slow in-place apply, a rotating
    /// refresh icon (gpui-component's Spinner needs an asset the app does
    /// not serve, so this reuses the whitelisted icon).
    pub(crate) fn advice_spinner() -> impl IntoElement {
        use gpui::{percentage, Animation, AnimationExt as _, Transformation};
        use gpui_component::Sizable as _;
        gpui_component::Icon::empty()
            .path("icons/refresh.svg")
            .with_size(gpui_component::Size::Small)
            .text_color(theme::text_dim())
            .with_animation(
                "advice-spin",
                Animation::new(Duration::from_secs(1)).repeat(),
                |icon, delta| icon.transform(Transformation::rotate(percentage(delta))),
            )
    }

    /// The large-table apply confirmation (Phase 8, Tier 3). Deferred so
    /// it paints above everything, with an occluding backdrop that dims
    /// the window and dismisses on an outside click.
    pub(crate) fn apply_confirm_overlay(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let size = self
            .schema
            .selected_object
            .as_ref()
            .and_then(|selected| selected.object.total_bytes)
            .map(Self::format_bytes)
            .unwrap_or_default();
        gpui::deferred(
            div()
                .id("apply-confirm")
                .occlude()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::rgba(0x00000088))
                .on_click(cx.listener(|this, _, _, cx| this.cancel_apply(cx)))
                .child(
                    div()
                        .id("apply-dialog")
                        .occlude()
                        .w(px(440.))
                        .p_4()
                        .rounded(px(6.))
                        .bg(theme::bg())
                        .border_1()
                        .border_color(theme::border())
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(div().text_color(theme::text()).child("Apply this change?"))
                        .child(div().text_xs().text_color(theme::text_dim()).child(format!(
                            "This rewrites the whole table (about {size}). It can take a while \
                         and use significant resources. Continue?"
                        )))
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap_2()
                                .child(
                                    div()
                                        .id("apply-cancel")
                                        .px_3()
                                        .py_1()
                                        .rounded(px(4.))
                                        .text_xs()
                                        .text_color(theme::text_dim())
                                        .cursor_pointer()
                                        .hover(|button| {
                                            button.bg(theme::hover()).text_color(theme::text())
                                        })
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.cancel_apply(cx)),
                                        )
                                        .child("Cancel"),
                                )
                                .child(
                                    div()
                                        .id("apply-continue")
                                        .group("apply-continue")
                                        .px_3()
                                        .py_1()
                                        .rounded(px(4.))
                                        .border_1()
                                        .border_color(theme::warning())
                                        .text_xs()
                                        .text_color(theme::warning())
                                        .cursor_pointer()
                                        .hover(|button| {
                                            button
                                                .bg(theme::warning())
                                                .border_color(theme::warning())
                                        })
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.confirm_apply(window, cx)
                                        }))
                                        .child(
                                            div()
                                                .group_hover("apply-continue", |label| {
                                                    label.text_color(rgb(0x14171c))
                                                })
                                                .child("Continue"),
                                        ),
                                ),
                        ),
                ),
        )
    }

    /// Run a suggestion's statements in order on the current connection,
    /// off the main thread, then re-fetch just this table's columns and
    /// storage and update them in place. Updating in place (rather than
    /// re-selecting the object) keeps cardinality/measurement and avoids
    /// flashing the whole pane through a loading state: only the changed
    /// column's numbers and advice repaint.
    pub(crate) fn apply_suggestion(
        &mut self,
        index: usize,
        apply: Vec<String>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(connected) = &self.connection.connected else {
            return;
        };
        let connection_name = connected.name.clone();
        let config = connected.client_config.clone();
        let Some(selected) = &mut self.schema.selected_object else {
            return;
        };
        let database = selected.database.clone();
        let object_name = selected.object.name.clone();
        selected.applying = Some(index);
        selected.applying_slow = false;
        cx.notify();

        // Show a spinner only if the apply runs past this, so quick ones
        // do not flicker.
        cx.spawn(async move |this, cx| {
            Timer::after(Duration::from_secs(3)).await;
            this.update(cx, |this, cx| {
                if let Some(selected) = &mut this.schema.selected_object {
                    if selected.applying == Some(index) {
                        selected.applying_slow = true;
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();

        let task = rt::tokio().spawn({
            let database = database.clone();
            let object_name = object_name.clone();
            async move {
                let client = ChClient::new(config);
                for statement in &apply {
                    client.execute(statement).await?;
                }
                let columns = client.list_columns(&database, &object_name).await?;
                let storage = client
                    .table_storage(&database, &object_name)
                    .await
                    .ok()
                    .flatten();
                Ok::<_, zedb_ch::ChError>((columns, storage))
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
                if let Some(selected) = &mut this.schema.selected_object {
                    selected.applying = None;
                    selected.applying_slow = false;
                }
                match result {
                    Ok(Ok((columns, storage))) => {
                        if let Some(selected) = &mut this.schema.selected_object {
                            if selected.database == database && selected.object.name == object_name
                            {
                                selected.columns = columns;
                                selected.storage = storage;
                            }
                        }
                        this.flash_notice("Applied", cx);
                    }
                    _ => this.flash_warning("Could not apply the change", cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn schedule_schema_analysis(
        &mut self,
        tab_id: usize,
        editor: Entity<InputState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.query.tabs.iter_mut().find(|tab| tab.id == tab_id) else {
            return;
        };
        tab.schema_analysis_generation += 1;
        let generation = tab.schema_analysis_generation;
        let sql = editor.read(cx).value().to_string();
        let Some((snapshot, default_database)) = self.schema.provider.snapshot() else {
            editor.update(cx, |editor, cx| {
                if let Some(diagnostics) = editor.diagnostics_mut() {
                    diagnostics.clear();
                }
                cx.notify();
            });
            return;
        };
        let task = rt::tokio().spawn(async move {
            tokio::time::sleep(Duration::from_millis(180)).await;
            let issues = zedb_ch::schema_intelligence::analyze_sql(
                &snapshot,
                default_database.as_deref(),
                &sql,
            );
            let referenced = zedb_ch::schema_intelligence::referenced_databases(
                &snapshot,
                default_database.as_deref(),
                &sql,
            );
            (sql, issues, referenced)
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok((sql, issues, referenced)) = task.await else {
                return;
            };
            this.update_in(cx, |this, window, cx| {
                for database in referenced {
                    this.warm_schema_columns(database, window, cx);
                }
                let Some(tab) = this.query.tabs.iter().find(|tab| tab.id == tab_id) else {
                    return;
                };
                if tab.schema_analysis_generation != generation {
                    return;
                }
                editor.update(cx, |editor, cx| {
                    let Some(diagnostics) = editor.diagnostics_mut() else {
                        return;
                    };
                    diagnostics.clear();
                    diagnostics.extend(issues.into_iter().map(|issue| {
                        let range = byte_range_to_lsp(&sql, issue.range);
                        Diagnostic {
                            range: range.start..range.end,
                            severity: DiagnosticSeverity::Hint,
                            source: Some("zeDB schema".into()),
                            message: issue.message.into(),
                            ..Default::default()
                        }
                    }));
                    cx.notify();
                });
            })
            .ok();
        })
        .detach();
    }

    /// Re-run schema analysis on every open editor against the current
    /// snapshot, refreshing diagnostics. Called when the schema context
    /// changes (node / cluster switch, schema reload) so a stale "unknown
    /// database/table" squiggly clears once the object is known again.
    /// Unlike [`Self::schedule_schema_analysis`] it does not warm column
    /// metadata, so it needs no window; column-level hints refresh on the
    /// next edit.
    pub(crate) fn refresh_schema_diagnostics(&mut self, cx: &mut Context<Self>) {
        let jobs: Vec<(usize, u64, Entity<InputState>)> = self
            .query
            .tabs
            .iter_mut()
            .map(|tab| {
                tab.schema_analysis_generation += 1;
                (tab.id, tab.schema_analysis_generation, tab.editor.clone())
            })
            .collect();
        let snapshot = self.schema.provider.snapshot();
        for (tab_id, generation, editor) in jobs {
            let Some((snapshot, default_database)) = snapshot.clone() else {
                // No schema context: clear any stale diagnostics outright.
                editor.update(cx, |editor, cx| {
                    if let Some(diagnostics) = editor.diagnostics_mut() {
                        diagnostics.clear();
                    }
                    cx.notify();
                });
                continue;
            };
            let sql = editor.read(cx).value().to_string();
            let task = rt::tokio().spawn(async move {
                let issues = zedb_ch::schema_intelligence::analyze_sql(
                    &snapshot,
                    default_database.as_deref(),
                    &sql,
                );
                (sql, issues)
            });
            cx.spawn(async move |this, cx| {
                let Ok((sql, issues)) = task.await else {
                    return;
                };
                this.update(cx, |this, cx| {
                    let Some(tab) = this.query.tabs.iter().find(|tab| tab.id == tab_id) else {
                        return;
                    };
                    if tab.schema_analysis_generation != generation {
                        return;
                    }
                    editor.update(cx, |editor, cx| {
                        let Some(diagnostics) = editor.diagnostics_mut() else {
                            return;
                        };
                        diagnostics.clear();
                        diagnostics.extend(issues.into_iter().map(|issue| {
                            let range = byte_range_to_lsp(&sql, issue.range);
                            Diagnostic {
                                range: range.start..range.end,
                                severity: DiagnosticSeverity::Hint,
                                source: Some("zeDB schema".into()),
                                message: issue.message.into(),
                                ..Default::default()
                            }
                        }));
                        cx.notify();
                    });
                })
                .ok();
            })
            .detach();
        }
    }
}
