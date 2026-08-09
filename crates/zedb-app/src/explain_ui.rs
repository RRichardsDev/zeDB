//! EXPLAIN visualized (docs/PHASE-7.1-IDEAS.md): the plan tree for
//! the statement under the cursor, drawn in the results pane, with
//! per-read-node index pruning (parts/granules selected vs initial)
//! as colored utilization bars. Works on every server version with
//! `EXPLAIN json = 1` (verified back to 25.8).

use gpui::{div, prelude::*, px, Context, Window};
use zedb_ch::explain::{ExplainIndex, ExplainNode};

use crate::{rt, theme, Workspace};

impl Workspace {
    /// Explain the statement Run would run: same targeting rules.
    pub(crate) fn explain_query(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.connected.is_none() {
            self.flash_warning("Connect to a cluster before explaining a query", cx);
            return;
        }
        let sql = self.run_target_sql(window, cx);
        if sql.trim().is_empty() {
            self.flash_warning("Nothing to explain", cx);
            return;
        }
        let Some(connected) = self.connected.as_ref() else {
            return;
        };
        let config = connected.client_config.clone();
        let Some(tab) = self.query_tabs.get(self.active_query_tab) else {
            return;
        };
        let tab_id = tab.id;
        let statement = zedb_ch::explain::explain_statement(&sql);
        let task = rt::tokio().spawn(async move {
            let client = zedb_ch::ChClient::new(config);
            let result = client.query(&statement).await?;
            let raw = result
                .rows
                .first()
                .and_then(|row| row.first())
                .map(|value| value.to_string())
                .unwrap_or_default();
            zedb_ch::explain::parse_explain_json(&raw)
        });
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            this.update(cx, |this, cx| {
                match outcome {
                    Ok(Ok(plan)) => {
                        if let Some(tab) = this.query_tabs.iter_mut().find(|tab| tab.id == tab_id) {
                            tab.explain = Some(plan);
                        }
                    }
                    Ok(Err(error)) => this.flash_warning(format!("EXPLAIN failed: {error}"), cx),
                    Err(_) => {}
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The SQL Run would target right now: the selection, or the
    /// statement at the cursor.
    pub(crate) fn run_target_sql(&mut self, window: &mut Window, cx: &mut Context<Self>) -> String {
        self.selected_text(window, cx).unwrap_or_else(|| {
            self.query_tabs
                .get(self.active_query_tab)
                .map(|tab| {
                    tab.editor.update(cx, |editor, _| {
                        let text = editor.value().to_string();
                        crate::statement_at_cursor(&text, editor.cursor())
                            .map(str::to_string)
                            .unwrap_or_default()
                    })
                })
                .unwrap_or_default()
        })
    }

    /// The plan tree pane, drawn in place of the results grid.
    pub(crate) fn explain_panel(
        &self,
        plan: &ExplainNode,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mut rows: Vec<gpui::AnyElement> = Vec::new();
        flatten(plan, "", true, true, &mut rows);

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme::bg())
            .child(
                div()
                    .flex_none()
                    .px_3()
                    .py_1()
                    .border_b_1()
                    .border_color(theme::border())
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::text_dim())
                            .child("QUERY PLAN \u{b7} reads show index pruning"),
                    )
                    .child(
                        div()
                            .id("explain-close")
                            .px_1()
                            .rounded(px(3.))
                            .text_color(theme::text_dim())
                            .child("\u{00d7}")
                            .hover(|close| {
                                close
                                    .bg(theme::hover())
                                    .text_color(theme::text())
                                    .cursor_pointer()
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(tab) = this.query_tabs.get_mut(this.active_query_tab) {
                                    tab.explain = None;
                                }
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .id("explain-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .py_1()
                    .children(rows),
            )
    }
}

/// Node categories drive the coloring: reads carry the payload tint,
/// everything else is structural.
fn node_color(node_type: &str) -> gpui::Hsla {
    if node_type.starts_with("Read") {
        theme::table_tint()
    } else if matches!(
        node_type,
        "Filter" | "Sorting" | "Limit" | "LimitBy" | "Distinct" | "Window"
    ) {
        theme::warning()
    } else if node_type.contains("Join") || node_type.contains("Union") {
        theme::filter_tint()
    } else if node_type.contains("Aggregating") {
        theme::success()
    } else {
        theme::accent()
    }
}

fn flatten(
    node: &ExplainNode,
    prefix: &str,
    is_last: bool,
    is_root: bool,
    rows: &mut Vec<gpui::AnyElement>,
) {
    let (branch, child_prefix) = if is_root {
        (String::new(), String::new())
    } else if is_last {
        (format!("{prefix}\u{2514} "), format!("{prefix}   "))
    } else {
        (format!("{prefix}\u{251c} "), format!("{prefix}\u{2502}  "))
    };

    let mut row = div()
        .px_3()
        .py_0p5()
        .flex()
        .items_center()
        .gap_2()
        .font_family("Menlo")
        .text_sm()
        .whitespace_nowrap()
        .overflow_hidden();
    if !branch.is_empty() {
        row = row.child(
            div()
                .flex_none()
                .text_color(theme::text_dim())
                .child(branch),
        );
    }
    row = row.child(
        div()
            .flex_none()
            .text_color(node_color(&node.node_type))
            .child(node.node_type.clone()),
    );
    if let Some(description) = &node.description {
        row = row.child(
            div()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_xs()
                .text_color(theme::text_dim())
                .child(description.clone()),
        );
    }
    rows.push(row.into_any_element());

    for index in &node.indexes {
        rows.push(index_row(index, &child_prefix));
    }

    let count = node.children.len();
    for (position, child) in node.children.iter().enumerate() {
        flatten(child, &child_prefix, position + 1 == count, false, rows);
    }
}

/// One index entry under a read node: keys, parts/granules pruning,
/// and a utilization bar (green pruned hard, red full scan).
fn index_row(index: &ExplainIndex, prefix: &str) -> gpui::AnyElement {
    let fraction = if index.initial_granules > 0 {
        index.selected_granules as f64 / index.initial_granules as f64
    } else {
        1.0
    };
    let bar_color = if fraction <= 0.2 {
        theme::success()
    } else if fraction <= 0.7 {
        theme::warning()
    } else {
        theme::danger()
    };
    let keys = if index.keys.is_empty() {
        String::new()
    } else {
        format!("({})", index.keys.join(", "))
    };
    let label = format!(
        "{}{} \u{b7} {}/{} parts \u{b7} {}/{} granules",
        index.index_type,
        keys,
        index.selected_parts,
        index.initial_parts,
        index.selected_granules,
        index.initial_granules,
    );
    let bar_width = 60.0_f32;
    div()
        .px_3()
        .py_0p5()
        .flex()
        .items_center()
        .gap_2()
        .font_family("Menlo")
        .text_xs()
        .whitespace_nowrap()
        .overflow_hidden()
        .child(
            div()
                .flex_none()
                .text_color(theme::text_dim())
                .child(format!("{prefix}   ")),
        )
        .child(
            div()
                .flex_none()
                .w(px(bar_width))
                .h(px(5.))
                .rounded(px(2.))
                .bg(theme::row_stripe())
                .child(
                    div()
                        .w(px(bar_width * (fraction as f32).clamp(0.02, 1.0)))
                        .h(px(5.))
                        .rounded(px(2.))
                        .bg(bar_color),
                ),
        )
        .child(div().flex_none().text_color(theme::text()).child(label))
        .when_some(index.condition.clone(), |row, condition| {
            row.child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_color(theme::text_dim())
                    .child(condition),
            )
        })
        .into_any_element()
}
