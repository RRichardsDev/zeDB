use crate::*;

use gpui::prelude::*;
use gpui_component::Disableable;

impl Workspace {
    pub(crate) fn refresh_schema_after_statements(&self, statements: &[String]) {
        let (Some(cache), Some(connected)) = (
            self.schema.cache.clone(),
            self.connection.connected.as_ref(),
        ) else {
            return;
        };
        let mut databases = statements
            .iter()
            .flat_map(|statement| {
                zedb_ch::schema_intelligence::touched_databases(
                    statement,
                    connected.client_config.database.as_deref(),
                )
            })
            .collect::<Vec<_>>();
        databases.sort();
        databases.dedup();
        if databases.is_empty() {
            return;
        }
        let config = connected.client_config.clone();
        rt::tokio().spawn(async move {
            for database in &databases {
                let _ = cache.invalidate_database(database);
            }
            let client = ChClient::new(config);
            if cache.refresh_tables(&client).await.is_ok() {
                for database in databases {
                    let _ = cache.refresh_columns(&client, &database).await;
                }
            }
        });
    }

    pub(crate) fn select_max_rows(&mut self, max_rows: MaxRows, cx: &mut Context<Self>) {
        if let Some(tab) = self.query.tabs.get_mut(self.query.active_tab) {
            tab.max_rows = max_rows;
        }
        cx.notify();
    }

    pub(crate) fn max_rows_selector(
        &self,
        running: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let active = &self.query.tabs[self.query.active_tab];
        let selected = active.max_rows;
        let action_context = active.editor.focus_handle(cx);
        Button::new("query-max-rows")
            .label(format!("Max rows: {}", selected.label()))
            .dropdown_caret(true)
            .compact()
            .outline()
            .disabled(running)
            .dropdown_menu(move |menu: PopupMenu, _, _| {
                menu.action_context(action_context.clone())
                    .min_w(px(164.))
                    .menu("1,000", Box::new(MaxRows1k))
                    .menu("10,000", Box::new(MaxRows10k))
                    .menu("50,000", Box::new(MaxRows50k))
                    .menu("100,000", Box::new(MaxRows100k))
                    .menu("1,000,000", Box::new(MaxRows1m))
                    .menu("Unlimited", Box::new(MaxRowsUnlimited))
            })
    }

    pub(crate) fn query_resize_handle(
        &self,
        id: &'static str,
        target: QueryResizeTarget,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        gpui::deferred(
            div()
                .id(id)
                .h(px(13.))
                .w_full()
                .mt(px(-6.))
                .mb(px(-6.))
                .flex_none()
                .relative()
                .cursor_row_resize()
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .top(px(6.))
                        .h(px(1.))
                        .bg(theme::border()),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                        this.query.resize = Some((target, f32::from(event.position.y)));
                        cx.notify();
                    }),
                ),
        )
    }

    /// Resume a run paused on a failed statement: skip it and continue, or
    /// cancel the remaining statements.
    pub(crate) fn resolve_statement_failure(&mut self, skip: bool, cx: &mut Context<Self>) {
        let Some(decision) = self.query.error_decision.take() else {
            return;
        };
        let _ = decision.send(skip);
        if skip {
            if let Some(tab) = self
                .query
                .tabs
                .iter_mut()
                .find(|tab| matches!(tab.outcome, QueryOutcome::StatementError { .. }))
            {
                tab.outcome = QueryOutcome::Running;
            }
        }
        cx.notify();
    }

    pub(crate) fn cancel_query(&mut self, cx: &mut Context<Self>) {
        let Some(abort) = self.query.abort.take() else {
            return;
        };
        abort.abort();
        self.query.error_decision = None;
        self.query.run_id += 1;
        if let Some(tab) = self.query.tabs.iter_mut().find(|tab| {
            matches!(
                tab.outcome,
                QueryOutcome::Running | QueryOutcome::StatementError { .. }
            )
        }) {
            tab.elapsed = tab.started_at.take().map(|started| started.elapsed());
            tab.outcome = QueryOutcome::Cancelled;
        }
        cx.notify();
    }
}
