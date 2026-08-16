use crate::*;

use gpui::prelude::*;

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
        // Running stamps the tab with the connection it ran on; from now
        // on the tab lives in that connection's scope.
        let connection_name = connected.name.clone();
        let Some(tab) = self.query.tabs.get_mut(self.query.active_tab) else {
            return;
        };
        tab.connection = Some(connection_name);
        if statements.is_empty() {
            tab.outcome = QueryOutcome::Error("Query is empty".into());
            cx.notify();
            return;
        }

        let tab_id = tab.id;
        tab.outcome = QueryOutcome::Running;
        tab.explain = None;
        tab.estimate = None;
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
}
