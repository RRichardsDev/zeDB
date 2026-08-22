use gpui::{Context, Window};
use zedb_ch::analytics::AnalyticsWindow;

use super::model::*;
use crate::{rt, Workspace};

impl Workspace {
    pub(crate) fn analytics_toggle(&mut self, cx: &mut Context<Self>) {
        if self.connection.connected.is_none() {
            self.flash_warning("Connect to a cluster to see query analytics", cx);
            return;
        }
        self.show_analytics = !self.show_analytics;
        if self.show_analytics {
            self.show_query_editor = false;
            self.show_fleet = false;
            self.show_ops = false;
            self.analytics_fetch(cx);
        }
        cx.notify();
    }

    /// The connection target changed: drop everything shown, and if
    /// the view is open against a live connection, fetch the new
    /// target immediately. The new target may not know the old
    /// scope's cluster, so scope resets to the node.
    pub(crate) fn analytics_reset(&mut self, cx: &mut Context<Self>) {
        self.analytics.generation += 1;
        self.analytics.scope = AnalyticsScope::Node;
        self.analytics.rows_meta.clear();
        self.analytics.sort.clear();
        self.analytics.filters.clear();
        self.analytics.loading = false;
        self.analytics.error = None;
        self.analytics.fetched_at = None;
        self.analytics_clear_drill_in();
        self.analytics.grid.update(cx, |grid, cx| {
            grid.release_rows();
            grid.set_filters(Vec::new(), cx);
            grid.set_sort(Vec::new(), cx);
        });
        if self.connection.connected.is_none() {
            self.show_analytics = false;
        } else if self.show_analytics {
            self.analytics_fetch(cx);
        }
        cx.notify();
    }

    pub(crate) fn analytics_fetch(&mut self, cx: &mut Context<Self>) {
        let Some(connected) = &self.connection.connected else {
            return;
        };
        // Reading the log must not write the log: same quiet setting
        // as the ops polls.
        let config = Self::ops_poll_config(&connected.client_config);
        self.analytics.loading = true;
        self.analytics.error = None;
        self.analytics.generation += 1;
        let generation = self.analytics.generation;
        let window = self.analytics.window;
        let cluster = self.analytics.scope.cluster().map(str::to_string);
        cx.notify();

        let sort = self.analytics.sort.clone();
        let having: Vec<String> = self
            .analytics
            .filters
            .iter()
            .map(|(_, predicate)| predicate.clone())
            .collect();
        let handle = rt::tokio().spawn(async move {
            let client = zedb_ch::ChClient::new(config);
            let sql = zedb_ch::analytics::fingerprint_grid_sql(
                window,
                cluster.as_deref(),
                &sort,
                &having,
            );
            client.query(&sql).await
        });
        cx.spawn(async move |this, cx| {
            let result = handle.await;
            this.update(cx, |this, cx| {
                if this.analytics.generation != generation {
                    return;
                }
                this.analytics.loading = false;
                this.analytics.fetched_at = Some(chrono::Local::now());
                match result {
                    Ok(Ok(mut result)) => {
                        // The trailing hash column is drill-in metadata,
                        // not display.
                        result.columns.pop();
                        this.analytics.rows_meta = result
                            .rows
                            .iter_mut()
                            .map(|row| {
                                let hash = match row.pop() {
                                    Some(zedb_core::Value::String(hash)) => hash,
                                    _ => String::new(),
                                };
                                let shape = match row.first() {
                                    Some(zedb_core::Value::String(shape)) => shape.clone(),
                                    _ => String::new(),
                                };
                                (hash, shape)
                            })
                            .collect();
                        let columns = result.columns.clone();
                        let sort = this.analytics.sort.clone();
                        let filters = this.analytics.filters.clone();
                        this.analytics.grid.update(cx, |grid, cx| {
                            grid.begin_result(columns, None, cx);
                            grid.append_rows(result.rows, cx);
                            grid.finish_result(false, cx);
                            grid.set_sort(sort, cx);
                            grid.set_filters(filters, cx);
                        });
                    }
                    Ok(Err(error)) => this.analytics.error = Some(error.to_string()),
                    Err(error) => this.analytics.error = Some(error.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Grid events from the fingerprint grid: sort and filter re-run
    /// the aggregation; a double-clicked row drills in.
    pub(crate) fn analytics_grid_event(
        &mut self,
        event: &crate::grid_spike::GridEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            crate::grid_spike::GridEvent::SortRequested { sort } => {
                self.analytics.sort = sort.clone();
                self.analytics_fetch(cx);
            }
            crate::grid_spike::GridEvent::FilterRequested { column, predicate } => {
                self.analytics.filters.retain(|(name, _)| name != column);
                if let Some(predicate) = predicate {
                    self.analytics
                        .filters
                        .push((column.clone(), predicate.clone()));
                }
                self.analytics_fetch(cx);
            }
            crate::grid_spike::GridEvent::RowActivated { row } => {
                if let Some((hash, shape)) = self.analytics.rows_meta.get(*row).cloned() {
                    if !hash.is_empty() {
                        self.analytics.selected_shape = shape;
                        self.analytics_select(hash, cx);
                    }
                }
            }
        }
    }

    pub(crate) fn analytics_set_window(&mut self, hours: u32, cx: &mut Context<Self>) {
        let window = match hours {
            1 => AnalyticsWindow::LastHour,
            24 => AnalyticsWindow::LastDay,
            _ => AnalyticsWindow::LastWeek,
        };
        if self.analytics.window == window {
            return;
        }
        self.analytics.window = window;
        self.analytics_clear_drill_in();
        self.analytics_fetch(cx);
    }

    pub(crate) fn analytics_set_scope(&mut self, cluster: Option<String>, cx: &mut Context<Self>) {
        let scope = match cluster {
            Some(name) => AnalyticsScope::Cluster(name),
            None => AnalyticsScope::Node,
        };
        if self.analytics.scope == scope {
            return;
        }
        self.analytics.scope = scope;
        self.analytics_clear_drill_in();
        self.analytics_fetch(cx);
    }

    fn analytics_clear_drill_in(&mut self) {
        self.analytics.selected = None;
        self.analytics.runs.clear();
        self.analytics.runs_loading = false;
        self.analytics.testimony = None;
        self.analytics.testimony_loading = false;
    }

    /// Drill into a fingerprint: select it and fetch its recent runs.
    pub(crate) fn analytics_select(&mut self, hash: String, cx: &mut Context<Self>) {
        if self.analytics.selected.as_deref() == Some(hash.as_str()) {
            return;
        }
        let Some(connected) = &self.connection.connected else {
            return;
        };
        let config = Self::ops_poll_config(&connected.client_config);
        self.analytics.selected = Some(hash.clone());
        self.analytics.runs.clear();
        self.analytics.runs_loading = true;
        self.analytics.testimony = None;
        self.analytics.testimony_loading = false;
        let generation = self.analytics.generation;
        let window = self.analytics.window;
        let cluster = self.analytics.scope.cluster().map(str::to_string);
        cx.notify();

        let request = hash.clone();
        let handle = rt::tokio().spawn(async move {
            let client = zedb_ch::ChClient::new(config);
            client
                .fingerprint_runs(&request, window, cluster.as_deref())
                .await
        });
        cx.spawn(async move |this, cx| {
            let result = handle.await;
            this.update(cx, |this, cx| {
                // Only the still-current drill-in may land.
                if this.analytics.generation != generation
                    || this.analytics.selected.as_deref() != Some(hash.as_str())
                {
                    return;
                }
                this.analytics.runs_loading = false;
                match result {
                    Ok(Ok(runs)) => this.analytics.runs = runs,
                    Ok(Err(error)) => this.analytics.error = Some(error.to_string()),
                    Err(error) => this.analytics.error = Some(error.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Fetch the testimony (ProfileEvents story) of one run.
    pub(crate) fn analytics_select_run(&mut self, query_id: String, cx: &mut Context<Self>) {
        if self
            .analytics
            .testimony
            .as_ref()
            .is_some_and(|(current, _)| *current == query_id)
        {
            return;
        }
        let Some(connected) = &self.connection.connected else {
            return;
        };
        let config = Self::ops_poll_config(&connected.client_config);
        self.analytics.testimony = None;
        self.analytics.testimony_loading = true;
        let generation = self.analytics.generation;
        let cluster = self.analytics.scope.cluster().map(str::to_string);
        cx.notify();

        let request = query_id.clone();
        let handle = rt::tokio().spawn(async move {
            let client = zedb_ch::ChClient::new(config);
            client.query_testimony(&request, cluster.as_deref()).await
        });
        cx.spawn(async move |this, cx| {
            let result = handle.await;
            this.update(cx, |this, cx| {
                if this.analytics.generation != generation || !this.analytics.testimony_loading {
                    return;
                }
                this.analytics.testimony_loading = false;
                match result {
                    Ok(Ok(Some(testimony))) => {
                        this.analytics.testimony = Some((query_id, testimony));
                    }
                    Ok(Ok(None)) => {
                        this.analytics.error = Some("query_log no longer has this run".to_string());
                    }
                    Ok(Err(error)) => this.analytics.error = Some(error.to_string()),
                    Err(error) => this.analytics.error = Some(error.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The fingerprint's normalized sample as a fresh editor tab, for
    /// hands-on iteration on the shape.
    pub(crate) fn analytics_open_sample(
        &mut self,
        sql: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_query_tab_with(&sql, window, cx);
    }
}
