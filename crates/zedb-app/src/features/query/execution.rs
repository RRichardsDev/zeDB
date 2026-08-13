use crate::*;

use gpui::prelude::*;
use gpui_component::Disableable;

impl Workspace {
    /// Run the selection as a single query, or the statement under the cursor
    /// when nothing is selected.
    pub(crate) fn run_query(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Run means run: a press during an in-flight query cancels it
        // and starts this one, instead of being silently swallowed.
        if self.query.abort.is_some() {
            self.cancel_query(cx);
        }
        let raw_sql = self.run_target_sql(window, cx);
        let full_text = self
            .query
            .tabs
            .get(self.query.active_tab)
            .map(|tab| tab.editor.read(cx).value().to_string())
            .unwrap_or_default();
        let sql = match resolve_query_variables(&raw_sql, &full_text) {
            Ok(sql) => sql,
            Err(error) => {
                self.flash_warning(error, cx);
                return;
            }
        };
        let offset = if sql.trim() == raw_sql.trim() {
            self.query.tabs.get(self.query.active_tab).and_then(|tab| {
                let editor = tab.editor.read(cx);
                nearest_occurrence(editor.value().as_ref(), raw_sql.trim(), editor.cursor())
            })
        } else {
            None
        };
        self.start_statements(vec![(sql.trim().to_string(), offset)], cx);
    }

    /// Run every statement in the selection (or the whole buffer when nothing
    /// is selected) one after another.
    pub(crate) fn run_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.query.abort.is_some() {
            self.cancel_query(cx);
        }
        let selection = self.selected_text(window, cx);
        let (full_text, cursor) = self
            .query
            .tabs
            .get(self.query.active_tab)
            .map(|tab| {
                let editor = tab.editor.read(cx);
                (editor.value().to_string(), editor.cursor())
            })
            .unwrap_or_default();
        // Offsets are absolute editor positions; a selection anchors its
        // relative offsets at the occurrence nearest the cursor.
        let (raw_text, base) = match selection {
            Some(selection) => {
                let base = nearest_occurrence(&full_text, &selection, cursor);
                (selection, base)
            }
            None => (full_text.clone(), Some(0)),
        };
        let text = match resolve_query_variables(&raw_text, &full_text) {
            Ok(text) => text,
            Err(error) => {
                self.flash_warning(error, cx);
                return;
            }
        };
        let transformed = text != raw_text;
        let statements = split_statements(&text)
            .into_iter()
            .filter_map(|(start, end)| {
                let raw = &text[start..end.min(text.len())];
                let statement = raw.trim();
                if statement.is_empty() {
                    return None;
                }
                let leading = raw.len() - raw.trim_start().len();
                let offset = if transformed {
                    None
                } else {
                    base.map(|base| base + start + leading)
                };
                Some((statement.to_string(), offset))
            })
            .collect();
        self.start_statements(statements, cx);
    }

    /// A grid header was clicked: rewrite the displayed statement's
    /// top-level ORDER BY, mirror it into the editor, and re-run it.
    pub(crate) fn grid_sort_requested(
        &mut self,
        tab_id: usize,
        sort: Vec<(String, bool)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.query.tabs.iter_mut().find(|tab| tab.id == tab_id) else {
            return;
        };
        let Some(statement) = tab.displayed_statement.clone() else {
            return;
        };
        let rewritten = zedb_ch::schema_intelligence::set_order_by(&statement, &sort);
        self.apply_rewritten_statement(statement, rewritten, window, cx);
    }

    /// Open the filter popover for a column, probing the server for its
    /// distinct values (capped, short-circuiting past ten) so even
    /// non-dictionary columns get checkboxes when they are small.
    pub(crate) fn open_column_filter(
        &mut self,
        column: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.query.tabs.get(self.query.active_tab) else {
            return;
        };
        let statement = tab.displayed_statement.clone();
        let prefill = statement
            .as_deref()
            .and_then(|statement| zedb_ch::schema_intelligence::column_filter(statement, &column));
        let grid = tab.result_grid.clone();
        let needs_probe = grid.update(cx, |grid, cx| {
            grid.begin_filter_panel(column.clone(), prefill, cx)
        });
        if !needs_probe {
            return;
        }
        let (Some(statement), Some(connected)) = (statement, self.connection.connected.as_ref())
        else {
            grid.update(cx, |grid, cx| {
                grid.finish_filter_panel(&column, None, window, cx)
            });
            return;
        };
        // Distinct within the query's other filters, unbounded by its
        // LIMIT, ignoring this column's own filter and the sort.
        let base = zedb_ch::schema_intelligence::set_column_filter(&statement, &column, None);
        let base = zedb_ch::schema_intelligence::set_order_by(&base, &[]);
        let base = zedb_ch::schema_intelligence::strip_top_level_limit(&base);
        let base = base.trim_end().trim_end_matches(';').to_string();
        let probe = format!(
            "SELECT DISTINCT `{}` AS value FROM ({base}) LIMIT 11",
            column.replace('`', "")
        );
        let config = connected.client_config.clone();
        let task = rt::tokio().spawn(async move {
            zedb_ch::ChClient::new(config)
                .query_guarded(&probe, 5, 32, 10 * 1024 * 1024 * 1024)
                .await
        });
        cx.spawn_in(window, async move |this, cx| {
            let values = match task.await {
                Ok(Ok(result)) => {
                    let has_null = result
                        .rows
                        .iter()
                        .any(|row| matches!(row.first(), Some(zedb_core::Value::Null)));
                    Some((
                        result
                            .rows
                            .into_iter()
                            .filter_map(|row| {
                                row.first().and_then(|value| match value {
                                    zedb_core::Value::Null => None,
                                    other => Some(other.to_string()),
                                })
                            })
                            .collect::<Vec<_>>(),
                        has_null,
                    ))
                }
                _ => None,
            };
            this.update_in(cx, |_, window, cx| {
                grid.update(cx, |grid, cx| {
                    grid.finish_filter_panel(&column, values, window, cx)
                });
            })
            .ok();
        })
        .detach();
    }

    /// A grid header asked for a filter change on the displayed statement.
    pub(crate) fn grid_filter_requested(
        &mut self,
        tab_id: usize,
        column: String,
        predicate: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.query.tabs.iter_mut().find(|tab| tab.id == tab_id) else {
            return;
        };
        let Some(statement) = tab.displayed_statement.clone() else {
            return;
        };
        let rewritten = zedb_ch::schema_intelligence::set_column_filter(
            &statement,
            &column,
            predicate.as_deref(),
        );
        self.apply_rewritten_statement(statement, rewritten, window, cx);
    }

    /// Mirror a rewritten statement into the editor and re-run it.
    pub(crate) fn apply_rewritten_statement(
        &mut self,
        statement: String,
        rewritten: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if rewritten == statement {
            return;
        }
        let Some(tab) = self.query.tabs.get_mut(self.query.active_tab) else {
            return;
        };
        // Later header interactions compose on this rewrite even before
        // it has run.
        tab.displayed_statement = Some(rewritten.clone());
        let offset = tab.displayed_statement_offset;
        let editor = tab.editor.clone();
        let value = editor.read(cx).value().to_string();
        // Position first: identical statements elsewhere in the buffer
        // must not swallow the rewrite. Text match is the fallback for
        // a buffer edited since the run.
        let position_match = offset
            .filter(|&offset| value.get(offset..offset + statement.len()) == Some(&statement[..]));
        // Fallback resolves by the occurrence nearest the last known
        // position (never blindly the first), so a drifted offset still
        // lands on the right twin.
        let splice_at =
            position_match.or_else(|| nearest_occurrence(&value, &statement, offset.unwrap_or(0)));
        if let Some(splice_at) = splice_at {
            if let Some(tab) = self.query.tabs.get_mut(self.query.active_tab) {
                tab.displayed_statement_offset = Some(splice_at);
            }
            let updated = format!(
                "{}{}{}",
                &value[..splice_at],
                rewritten,
                &value[splice_at + statement.len()..]
            );
            editor.update(cx, |editor, cx| editor.set_value(updated, window, cx));
        } else {
            if let Some(tab) = self.query.tabs.get_mut(self.query.active_tab) {
                tab.displayed_statement_offset = None;
            }
            self.flash_warning(
                "Query changed since it ran; rewriting the last executed statement",
                cx,
            );
        }
        // Coalesce rapid interactions into one run: debounce a beat and
        // cancel-and-restart anything in flight.
        self.query.rerun_generation += 1;
        let generation = self.query.rerun_generation;
        self.query.rerun_pending = Some(rewritten);
        cx.spawn(async move |this, cx| {
            Timer::after(Duration::from_millis(150)).await;
            this.update(cx, |this, cx| {
                if this.query.rerun_generation != generation {
                    return;
                }
                let Some(statement) = this.query.rerun_pending.take() else {
                    return;
                };
                if this.query.abort.is_some() {
                    this.cancel_query(cx);
                }
                let offset = this
                    .query
                    .tabs
                    .get(this.query.active_tab)
                    .and_then(|tab| tab.displayed_statement_offset);
                this.start_statements(vec![(statement, offset)], cx);
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn start_statements(
        &mut self,
        mut statements: Vec<(String, Option<usize>)>,
        cx: &mut Context<Self>,
    ) {
        if self.query.abort.is_some() {
            return;
        }
        let Some(connected) = &self.connection.connected else {
            self.flash_warning("Connect to a cluster before running a query", cx);
            return;
        };
        statements.retain(|(statement, _)| !statement.trim().is_empty());
        let Some(tab) = self.query.tabs.get_mut(self.query.active_tab) else {
            return;
        };
        if statements.is_empty() {
            tab.outcome = QueryOutcome::Error("Query is empty".into());
            cx.notify();
            return;
        }

        let tab_id = tab.id;
        tab.outcome = QueryOutcome::Running;
        tab.explain = None;
        tab.advisor = None;
        tab.advise_pending = false;
        tab.advisor_generation += 1;
        tab.failed_sql = None;
        tab.result_columns = 0;
        tab.result_rows = 0;
        // has_result stays as it was: an already-displayed result keeps
        // its pane (and its rows, via the grid's deferred swap) until
        // the replacement streams in.
        tab.result_capped = false;
        tab.read_rows = None;
        tab.read_bytes = None;
        tab.total_rows = None;
        tab.received_bytes = 0;
        tab.started_at = Some(Instant::now());
        tab.elapsed = None;
        let config = connected.client_config.clone();
        let row_limit = tab.max_rows.limit();
        // For the history record on completion; `statements` moves
        // into the runner task.
        let run_sqls: Vec<String> = statements.iter().map(|(sql, _)| sql.clone()).collect();
        self.query.run_id += 1;
        let run_id = self.query.run_id;
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let task = rt::tokio().spawn(async move {
            let client = ChClient::new(config);
            let total = statements.len();
            let mut summary: Option<QueryStreamSummary> = None;
            let mut skipped = 0usize;
            let mut succeeded = Vec::new();
            for (index, (sql, offset)) in statements.iter().enumerate() {
                let outcome = client
                    .query_stream(sql, row_limit.unwrap_or(usize::MAX), |event| {
                        let _ = sender.send(RunEvent::Stream(event));
                    })
                    .await;
                match outcome {
                    Ok(current) => {
                        summary = Some(current);
                        succeeded.push((sql.clone(), *offset));
                    }
                    Err(error) => {
                        let message = if total > 1 {
                            format!("Statement {} of {total} failed: {error}", index + 1)
                        } else {
                            error.to_string()
                        };
                        if index + 1 == total {
                            return Err(message);
                        }
                        let (decision, wait) = tokio::sync::oneshot::channel();
                        let _ = sender.send(RunEvent::StatementFailed {
                            index,
                            total,
                            message: error.to_string(),
                            decision,
                        });
                        // Pause until the user skips this statement or cancels
                        // the rest of the run. A dropped sender cancels.
                        if wait.await.unwrap_or(false) {
                            skipped += 1;
                        } else {
                            return Err(message);
                        }
                    }
                }
            }
            Ok((summary, skipped, succeeded))
        });
        self.query.abort = Some(task.abort_handle());
        cx.notify();

        cx.spawn(async move |this, cx| {
            while let Some(event) = receiver.recv().await {
                let keep_receiving = this
                    .update(cx, |this, cx| {
                        if this.query.run_id != run_id {
                            return false;
                        }
                        let Some(tab) = this.query.tabs.iter_mut().find(|tab| tab.id == tab_id)
                        else {
                            return false;
                        };
                        match event {
                            RunEvent::StatementFailed {
                                index,
                                total,
                                message,
                                decision,
                            } => {
                                this.query.error_decision = Some(decision);
                                tab.outcome = QueryOutcome::StatementError {
                                    index,
                                    total,
                                    message,
                                };
                            }
                            RunEvent::Stream(QueryStreamEvent::Started { query_id }) => {
                                tab.running_query_id = Some(query_id);
                            }
                            RunEvent::Stream(QueryStreamEvent::Columns(columns)) => {
                                tab.result_columns = columns.len();
                                tab.result_rows = 0;
                                tab.has_result = true;
                                // Each statement reports its own progress;
                                // never let one statement's totals stand
                                // for the next.
                                tab.read_rows = None;
                                tab.read_bytes = None;
                                tab.total_rows = None;
                                tab.received_bytes = 0;
                                tab.result_grid.update(cx, |grid, cx| {
                                    grid.begin_result(columns, row_limit, cx)
                                });
                            }
                            RunEvent::Stream(QueryStreamEvent::Rows(rows)) => {
                                tab.result_rows += rows.len();
                                tab.result_grid
                                    .update(cx, |grid, cx| grid.append_rows(rows, cx));
                            }
                            RunEvent::Stream(QueryStreamEvent::Progress(progress)) => {
                                if progress.read_rows.is_some() {
                                    tab.read_rows = progress.read_rows;
                                }
                                if progress.read_bytes.is_some() {
                                    tab.read_bytes = progress.read_bytes;
                                }
                                if progress.total_rows.is_some() {
                                    tab.total_rows = progress.total_rows;
                                }
                                tab.received_bytes = progress.received_bytes;
                            }
                        }
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !keep_receiving {
                    break;
                }
            }
            let result = task.await;
            this.update(cx, |this, cx| {
                if this.query.run_id != run_id {
                    return;
                }
                this.query.abort = None;
                this.query.error_decision = None;
                let Some(tab) = this.query.tabs.iter_mut().find(|tab| tab.id == tab_id) else {
                    return;
                };
                let advise_pending = std::mem::take(&mut tab.advise_pending);
                tab.elapsed = tab.started_at.take().map(|started| started.elapsed());
                let mut successful_statements = Vec::new();
                tab.outcome = match result {
                    Ok(Ok((summary, skipped, succeeded))) => {
                        let capped = summary.map(|summary| summary.capped).unwrap_or(false);
                        tab.result_capped = capped;
                        tab.result_grid
                            .update(cx, |grid, cx| grid.finish_result(capped, cx));
                        let outcome = QueryOutcome::Complete {
                            columns: tab.result_columns,
                            rows: tab.result_rows,
                            skipped,
                        };
                        successful_statements = succeeded;
                        outcome
                    }
                    Ok(Err(error)) => {
                        // A kill from the ops view tears the stream; the
                        // transport error would otherwise be misleading.
                        let killed = tab
                            .running_query_id
                            .as_ref()
                            .map(|id| this.ops_killed.contains(id))
                            .unwrap_or(false);
                        if killed {
                            QueryOutcome::Error("Query killed from the ops view".into())
                        } else if error.contains("Query was cancelled")
                            || error.contains("(394)")
                            || error.contains("code 394")
                        {
                            QueryOutcome::Error(
                                "Query was cancelled (KILL QUERY on the server)".into(),
                            )
                        } else {
                            QueryOutcome::Error(error)
                        }
                    }
                    Err(error) => QueryOutcome::Error(error.to_string()),
                };
                // Re-sync the sort indicator with reality: the executed
                // SQL on success, or the still-displayed old result's SQL
                // when the run failed after an optimistic indicator.
                if let Some((statement, offset)) = successful_statements.last() {
                    tab.displayed_statement = Some(statement.clone());
                    tab.displayed_statement_offset = *offset;
                }
                if let Some(statement) = tab.displayed_statement.clone() {
                    let sort = zedb_ch::schema_intelligence::top_level_order_by(&statement);
                    let filters = zedb_ch::schema_intelligence::column_filters(&statement);
                    tab.result_grid.update(cx, |grid, cx| {
                        grid.set_sort(sort, cx);
                        grid.set_filters(filters, cx);
                    });
                }
                let duration_ms = tab.elapsed.map(|elapsed| elapsed.as_millis() as u64);
                let result_rows = tab.result_rows as u64;
                let run_error = match &tab.outcome {
                    QueryOutcome::Error(error) => Some(error.clone()),
                    _ => None,
                };
                tab.failed_sql = run_error.is_some().then(|| run_sqls.join(";\n\n"));
                let successful_sql: Vec<String> = successful_statements
                    .iter()
                    .map(|(sql, _)| sql.clone())
                    .collect();
                if run_error.is_none() {
                    if !successful_sql.is_empty() {
                        this.history_record(&successful_sql, duration_ms, Some(result_rows), None);
                    }
                    if advise_pending {
                        this.run_query_advisor(tab_id, cx);
                    }
                } else if run_sqls.len() == 1 {
                    // Failed single-statement runs are history too: the
                    // statement you need to fix is the one you want back.
                    this.history_record(&run_sqls, duration_ms, None, run_error.as_deref());
                }
                this.refresh_schema_after_statements(&successful_sql);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

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
                            .child("No advice — the primary key is serving this query's filter."),
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
                                            "This query isn't using the primary key — help me make it faster."
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
