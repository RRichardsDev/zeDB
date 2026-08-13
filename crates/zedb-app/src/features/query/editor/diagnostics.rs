use crate::*;

use gpui::prelude::*;

impl Workspace {
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
