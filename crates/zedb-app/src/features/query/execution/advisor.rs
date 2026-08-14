use crate::*;

use gpui::prelude::*;

impl Workspace {
    /// Run the query behind a saved query and, on completion, advise on
    /// it. Opens the SQL in a fresh tab (so the results and advice are
    /// visible together), runs it, and flags the run for advising.
    pub(crate) fn advise_saved_query(
        &mut self,
        sql: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.connection.connected.is_none() {
            self.flash_warning("Connect to a cluster before advising a query", cx);
            return;
        }
        self.open_query_tab_with(&sql, window, cx);
        self.start_statements(vec![(sql, None)], cx);
        if let Some(tab) = self.query.tabs.get_mut(self.query.active_tab) {
            tab.advise_pending = true;
        }
    }

    /// Compute the advisor result for the displayed statement off-thread:
    /// EXPLAIN it (reusing the plan parser), turn the plan + run stats into
    /// facts, and store the ranked findings. Always stores `Some` when
    /// invoked, so an empty result shows a "looks fine" note rather than
    /// nothing. A generation + connection guard drops a stale result.
    pub(crate) fn run_query_advisor(&mut self, tab_id: usize, cx: &mut Context<Self>) {
        let Some(connected) = self.connection.connected.as_ref() else {
            return;
        };
        let connection_name = connected.name.clone();
        let config = connected.client_config.clone();
        let Some(tab) = self.query.tabs.iter().find(|tab| tab.id == tab_id) else {
            return;
        };
        let Some(sql) = tab.displayed_statement.clone() else {
            return;
        };
        let read_rows = tab.read_rows.unwrap_or(0);
        let result_rows = tab.result_rows as u64;
        let read_bytes = tab.read_bytes.unwrap_or(0);
        let capped = tab.result_capped;
        let generation = tab.advisor_generation;

        // Only a read can be EXPLAINed; anything else gets an empty (looks
        // fine) result so the invoked lane still gives feedback.
        if !query_advisor::is_advisable_select(&sql) {
            if let Some(tab) = self.query.tabs.iter_mut().find(|tab| tab.id == tab_id) {
                tab.advisor = Some(Vec::new());
                cx.notify();
            }
            return;
        }

        // The WHERE columns with whether each is a range filter, so the fix
        // DDL names the real column and picks the right index type; the
        // column's type is fetched below once we know the table.
        let filters: Vec<(String, bool)> = zedb_ch::schema_intelligence::column_filters(&sql)
            .into_iter()
            .map(|(name, conjunct)| (name, is_range_predicate(&conjunct)))
            .collect();
        // A top-level GROUP BY marks a rollup the advisor can suggest a
        // projection / materialized view for (vs a global aggregate); when
        // present we also rebuild the projection body from the SQL so the
        // fix is copyable DDL, not just prose.
        let has_group_by = zedb_ch::schema_intelligence::has_group_by(&sql);
        let aggregate_projection = zedb_ch::schema_intelligence::aggregate_projection(&sql);
        let explain_sql = zedb_ch::explain::explain_statement(&sql);
        // Compute the findings off-thread: EXPLAIN, then (once we know the
        // table) fetch its true sorting key and the filtered columns' types
        // from system.*. EXPLAIN's PrimaryKey "Keys" only lists the keys the
        // WHERE touched, not the table's full ORDER BY, so we can't tell the
        // leading key from it.
        let task = rt::tokio().spawn(async move {
            let client = ChClient::new(config);
            let raw = client
                .query(&explain_sql)
                .await
                .ok()?
                .rows
                .first()
                .and_then(|row| row.first())
                .map(|value| value.to_string())
                .unwrap_or_default();
            let plan = zedb_ch::explain::parse_explain_json(&raw).ok()?;
            let mut facts =
                query_advisor::facts_from_plan(&plan, read_rows, result_rows, read_bytes, capped);
            facts.has_group_by = has_group_by;
            facts.aggregate_projection = aggregate_projection;
            if let Some((order_by, partition_key)) =
                fetch_table_keys(&client, facts.table.as_deref()).await
            {
                if !order_by.is_empty() {
                    facts.order_by = order_by;
                }
                facts.partition_key = partition_key;
            }
            let types = fetch_column_types(&client, facts.table.as_deref()).await;
            let mut filter_columns: Vec<query_advisor::FilterColumn> = filters
                .into_iter()
                .map(|(name, is_range)| query_advisor::FilterColumn {
                    base_type: types.get(&name).cloned().unwrap_or_default(),
                    name,
                    is_range,
                    distinct: None,
                })
                .collect();
            // For equality filters whose cardinality we can't infer from the
            // type, probe uniqCombined so the index choice (set vs bloom,
            // and the bloom rate) fits the data. One batched query.
            if let Some((database, table)) = facts
                .table
                .as_deref()
                .and_then(|table| table.split_once('.'))
            {
                let probe: Vec<String> = filter_columns
                    .iter()
                    .filter(|column| query_advisor::needs_cardinality_probe(column))
                    .map(|column| column.name.clone())
                    .collect();
                if !probe.is_empty() {
                    if let Ok(distincts) =
                        client.column_cardinalities(database, table, &probe).await
                    {
                        for (name, distinct) in probe.iter().zip(distincts) {
                            if let Some(column) = filter_columns
                                .iter_mut()
                                .find(|column| &column.name == name)
                            {
                                column.distinct = Some(distinct);
                            }
                        }
                    }
                }
            }
            facts.filter_columns = filter_columns;
            Some(query_advisor::advise(&facts))
        });

        cx.spawn(async move |this, cx| {
            let findings = task.await.ok().flatten().unwrap_or_default();
            this.update(cx, |this, cx| {
                if this.connection.connected.as_ref().map(|c| c.name.as_str())
                    != Some(connection_name.as_str())
                {
                    return;
                }
                let Some(tab) = this.query.tabs.iter_mut().find(|tab| tab.id == tab_id) else {
                    return;
                };
                if tab.advisor_generation != generation {
                    return;
                }
                tab.advisor = Some(findings);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The query-advisor lane under the results: ranked findings, each a
    /// plain-language diagnosis plus a copyable fix. Kept visually quiet
    /// (an accent rule, not an alarm) and dismissible.
    pub(crate) fn query_advisor_panel(
        &self,
        tab: &QueryTab,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let tab_id = tab.id;
        let advised_sql = tab.displayed_statement.clone().unwrap_or_default();
        // The optional agent hand-off, exactly like the error bar: only
        // when a usable agent is remembered, and it rides silent context.
        let ask_agent_icon: Option<String> = self
            .preferences
            .last_agent
            .clone()
            .filter(|name| {
                self.agent.agents.is_empty()
                    || self.agent.agents.iter().any(|agent| agent.name == *name)
            })
            .map(|name| {
                self.agent
                    .agents
                    .iter()
                    .find(|agent| agent.name == name)
                    .map(|agent| agent_pane::icon_for(&agent.id))
                    .unwrap_or(match name.as_str() {
                        "Claude Code" => "icons/agent-claude.svg",
                        "Codex" => "icons/agent-codex.svg",
                        _ => "icons/sparkle.svg",
                    })
                    .to_string()
            });
        // A small square icon action for the fix line (copy / open / ask).
        let advisor_action = |id: (&'static str, usize), icon: &str| {
            div()
                .id(id)
                .size(px(18.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(3.))
                .hover(|button| button.bg(theme::hover()).cursor_pointer())
                .child(
                    svg()
                        .path(icon.to_string())
                        .size(px(12.))
                        .text_color(theme::text_dim()),
                )
        };
        let mut panel = div()
            .flex_none()
            .flex()
            .flex_col()
            .gap_2()
            .px_3()
            .py_2()
            .border_t_1()
            .border_color(theme::border())
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(theme::warning())
                            .child("QUERY ADVISOR"),
                    )
                    .child(
                        div()
                            .id("advisor-dismiss")
                            .px_1()
                            .rounded(px(3.))
                            .text_color(theme::text_dim())
                            .child("x")
                            .hover(|button| button.bg(theme::hover()).cursor_pointer())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(tab) =
                                    this.query.tabs.iter_mut().find(|tab| tab.id == tab_id)
                                {
                                    tab.advisor = None;
                                    cx.notify();
                                }
                            })),
                    ),
            );
        let findings = tab.advisor.as_deref().unwrap_or(&[]);
        if findings.is_empty() {
            panel = panel.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .border_l_2()
                    .border_color(theme::success())
                    .pl_2()
                    .child(
                        svg()
                            .path("icons/verify.svg")
                            .size(px(12.))
                            .text_color(theme::success()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::text_dim())
                            .child("No advice: the primary key is serving this query's filter."),
                    ),
            );
        }
        for (index, finding) in findings.iter().enumerate() {
            let editor_sql = finding.editor_sql.clone();
            // Findings without copyable DDL (e.g. the partition and
            // aggregate advice) still get a copy button: it copies the
            // suggestion prose, so every row has a copy action.
            let copy_fix_text = editor_sql.is_none().then(|| finding.fix.clone());
            panel = panel.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .border_l_2()
                    .border_color(theme::warning())
                    .pl_2()
                    .child(div().text_color(theme::text()).child(finding.title.clone()))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::text_dim())
                            .child(finding.detail.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(
                                // min-width: 0 lets the flex child shrink
                                // below its content width so the fix text
                                // wraps instead of overflowing (and being
                                // clipped) when the panel narrows.
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_xs()
                                    .text_color(theme::text_dim())
                                    .child(finding.fix.clone()),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .when_some(editor_sql, |actions, sql| {
                                        let copy_sql = sql.clone();
                                        actions
                                            .child(
                                                advisor_action(("advisor-copy", index), "icons/copy.svg")
                                                    .tooltip(|window, cx| {
                                                        gpui_component::tooltip::Tooltip::new(
                                                            "Copy fix",
                                                        )
                                                        .build(window, cx)
                                                    })
                                                    .on_click(cx.listener(move |_, _, _, cx| {
                                                        cx.write_to_clipboard(
                                                            gpui::ClipboardItem::new_string(
                                                                copy_sql.clone(),
                                                            ),
                                                        );
                                                    })),
                                            )
                                            .child(
                                                advisor_action(
                                                    ("advisor-open", index),
                                                    "icons/query-plus.svg",
                                                )
                                                .tooltip(|window, cx| {
                                                    gpui_component::tooltip::Tooltip::new(
                                                        "Open fix in editor",
                                                    )
                                                    .build(window, cx)
                                                })
                                                .on_click(cx.listener(move |this, _, window, cx| {
                                                    this.open_query_tab_with(&sql, window, cx);
                                                })),
                                            )
                                    })
                                    .when_some(copy_fix_text, |actions, fix| {
                                        actions.child(
                                            advisor_action(("advisor-copy", index), "icons/copy.svg")
                                                .tooltip(|window, cx| {
                                                    gpui_component::tooltip::Tooltip::new(
                                                        "Copy suggestion",
                                                    )
                                                    .build(window, cx)
                                                })
                                                .on_click(cx.listener(move |_, _, _, cx| {
                                                    cx.write_to_clipboard(
                                                        gpui::ClipboardItem::new_string(fix.clone()),
                                                    );
                                                })),
                                        )
                                    })
                                    .when_some(ask_agent_icon.clone(), |actions, icon| {
                                        // Silent hand-off, mirroring the error
                                        // bar: a plain ask, the finding + query
                                        // rides as hidden context.
                                        let visible =
                                            "This query isn't using the primary key; help me make it faster."
                                                .to_string();
                                        let mut hidden = format!(
                                            "Context (not shown to the user): from the zeDB query advisor. Finding: {}\nSuggested fix: {}",
                                            finding.detail, finding.fix,
                                        );
                                        if let Some(ddl) = &finding.editor_sql {
                                            hidden.push_str(&format!(
                                                "\nSuggested DDL:\n```sql\n{ddl}\n```"
                                            ));
                                        }
                                        if !advised_sql.is_empty() {
                                            hidden.push_str(&format!(
                                                "\nThe query was:\n```sql\n{advised_sql}\n```"
                                            ));
                                        }
                                        hidden.push_str(
                                            "\nDo not open a migration for this. Put the DDL in the query editor with propose_query so the user can review and run it, or explain the trade-offs.",
                                        );
                                        actions.child(
                                            advisor_action(("advisor-agent", index), &icon)
                                                .tooltip(|window, cx| {
                                                    gpui_component::tooltip::Tooltip::new(
                                                        "Ask your agent",
                                                    )
                                                    .build(window, cx)
                                                })
                                                .on_click(cx.listener(move |this, _, window, cx| {
                                                    this.agent_ask_about(
                                                        visible.clone(),
                                                        hidden.clone(),
                                                        window,
                                                        cx,
                                                    );
                                                })),
                                        )
                                    }),
                            ),
                    ),
            );
        }
        panel
    }
}
