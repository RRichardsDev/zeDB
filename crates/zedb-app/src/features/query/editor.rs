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
            connection: None,
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
            estimate: None,
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
        let label = next_tab_label(self.query.tabs.iter().map(|tab| tab.name.as_str()));
        let mut tab = Self::make_query_tab(id, sql, self.schema.provider.clone(), window, cx);
        tab.name = label;
        tab.connection = self.active_connection_name();
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
        let Some(connected) = self.connection.connected.as_ref() else {
            return;
        };
        let connection = connected.name.clone();
        if !apply_in_place_allowed(connected.client_config.read_only, self.active_tier()) {
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
            let Some(selected) = self.schema.selected_object.as_ref() else {
                return;
            };
            self.schema.pending_apply = Some(PendingApply {
                index,
                apply,
                connection,
                database: selected.database.clone(),
                object: selected.object.name.clone(),
            });
            cx.notify();
        } else {
            self.apply_suggestion(index, apply, window, cx);
        }
    }

    pub(crate) fn confirm_apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pending) = self.schema.pending_apply.take() else {
            return;
        };
        let context_matches = pending.matches_context(
            self.connection
                .connected
                .as_ref()
                .map(|connected| (connected.name.as_str(), connected.client_config.read_only)),
            self.active_tier(),
            self.schema
                .selected_object
                .as_ref()
                .map(|selected| (selected.database.as_str(), selected.object.name.as_str())),
        );
        if !context_matches {
            self.flash_warning(
                "The connection or table changed; review the suggestion again",
                cx,
            );
            return;
        }
        self.apply_suggestion(pending.index, pending.apply, window, cx);
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
}
