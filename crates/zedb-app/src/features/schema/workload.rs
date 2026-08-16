//! The Workload tab: index and projection effectiveness measured from
//! the table's real traffic. One `system.query_log` aggregation finds
//! the heaviest query shapes; one EXPLAIN per shape yields per-index
//! pruning, weighted by how often each shape ran. Findings follow the
//! advisor voice: explainable, copyable DDL, never applied.

use gpui::prelude::*;
use zedb_ch::workload::{self, WorkloadReport};

use crate::*;

/// Traffic window and shape cap: enough to characterize a workload,
/// cheap enough to run on a click.
const WINDOW_DAYS: u32 = 7;
const SHAPE_LIMIT: usize = 12;

impl Workspace {
    pub(crate) fn load_workload(&mut self, cx: &mut Context<Self>) {
        let (connection_name, config, database_name, object_name) = {
            let Some(selected) = &self.schema.selected_object else {
                return;
            };
            if selected.workload.is_some() || selected.workload_loading {
                return;
            }
            let Some(connected) = &self.connection.connected else {
                return;
            };
            (
                connected.name.clone(),
                connected.client_config.clone(),
                selected.database.clone(),
                selected.object.name.clone(),
            )
        };

        if let Some(selected) = &mut self.schema.selected_object {
            selected.workload_loading = true;
            selected.workload_error = None;
        }
        cx.notify();

        // A real topology fans the log out over every replica; the
        // shape check keeps single-node installs on the bare table.
        let cluster = self.ops_cluster_options().first().cloned();
        let task = rt::tokio().spawn({
            let database = database_name.clone();
            let object = object_name.clone();
            async move { measure_workload(config, &database, &object, cluster).await }
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
                let Some(selected) = &mut this.schema.selected_object else {
                    return;
                };
                if selected.database != database_name || selected.object.name != object_name {
                    return;
                }
                selected.workload_loading = false;
                match result {
                    Ok(Ok(report)) => selected.workload = Some(report),
                    Ok(Err(error)) => selected.workload_error = Some(error),
                    Err(error) => selected.workload_error = Some(error.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn workload_panel(
        &self,
        selected: &SelectedSchemaObject,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        if selected.workload_loading {
            return div()
                .p_3()
                .text_sm()
                .text_color(theme::text_dim())
                .child("Measuring the workload from system.query_log\u{2026}");
        }
        if let Some(error) = &selected.workload_error {
            return div()
                .p_3()
                .text_sm()
                .text_color(theme::danger())
                .child(format!("Workload measurement failed: {error}"));
        }
        let Some(report) = &selected.workload else {
            return div()
                .p_3()
                .text_sm()
                .text_color(theme::text_dim())
                .child("No measurement yet");
        };
        if report.total_runs == 0 {
            return div()
                .p_3()
                .text_sm()
                .text_color(theme::text_dim())
                .child(format!(
                    "No SELECTs touched this table in the last {WINDOW_DAYS} days \
                     (or query_log is off on this node)"
                ));
        }

        // The scope is stated, never implied: a one-node log on a
        // multi-replica service is a fraction of the workload.
        let scope = match &report.scope {
            Some(cluster) => format!("all replicas of {cluster}"),
            None => "this node only".to_string(),
        };
        let header = format!(
            "Measured from system.query_log ({scope}) \u{b7} last {} days \u{b7} {} runs across {} shapes",
            report.window_days,
            Self::format_count(report.total_runs),
            report.shapes_analyzed,
        );
        let bar_width = 60.0_f32;
        let index_rows = report
            .indexes
            .iter()
            .map(|usage| {
                let fraction = usage.fraction();
                let bar_color = if fraction <= 0.2 {
                    theme::success()
                } else if fraction <= 0.7 {
                    theme::warning()
                } else {
                    theme::danger()
                };
                let share = (usage.runs_applied as f64 / report.total_runs as f64 * 100.0).round();
                div()
                    .px_3()
                    .py_0p5()
                    .flex()
                    .items_center()
                    .gap_2()
                    .font_family("Menlo")
                    .text_xs()
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
                    .child(div().flex_none().text_color(theme::text()).child(format!(
                        "{} \u{b7} lets {:.0}% of granules through \u{b7} in {share:.0}% of runs",
                        usage.label,
                        fraction * 100.0,
                    )))
            })
            .collect::<Vec<_>>();

        let finding_rows = report
            .findings
            .iter()
            .enumerate()
            .map(|(index, finding)| {
                let positive = finding.fix.is_none() && finding.title.contains("served");
                let rule_color = if positive {
                    theme::success()
                } else {
                    theme::warning()
                };
                div()
                    .mx_3()
                    .my_1()
                    .pl_2()
                    .py_1()
                    .border_l_2()
                    .border_color(rule_color)
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme::text())
                                    .child(finding.title.clone()),
                            )
                            .when_some(finding.fix.clone(), |row, fix| {
                                row.child(
                                    div()
                                        .id(("workload-copy", index))
                                        .px_1()
                                        .rounded(px(3.))
                                        .text_xs()
                                        .text_color(theme::text_dim())
                                        .child("copy DDL")
                                        .hover(|button| {
                                            button
                                                .bg(theme::hover())
                                                .text_color(theme::text())
                                                .cursor_pointer()
                                        })
                                        .on_click(cx.listener(move |_, _, _, cx| {
                                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                                fix.clone(),
                                            ));
                                        })),
                                )
                            }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::text_dim())
                            .child(finding.detail.clone()),
                    )
            })
            .collect::<Vec<_>>();

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_none()
                    .px_3()
                    .py_1()
                    .border_b_1()
                    .border_color(theme::border())
                    .text_xs()
                    .text_color(theme::text_dim())
                    .child(header),
            )
            .child(
                div()
                    .id("workload-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .py_1()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .px_3()
                            .py_1()
                            .text_xs()
                            .text_color(theme::text_dim())
                            .child("INDEXES \u{b7} run-weighted granule pruning"),
                    )
                    .children(index_rows)
                    .when(!report.findings.is_empty(), |column| {
                        column.child(
                            div()
                                .px_3()
                                .pt_2()
                                .pb_1()
                                .text_xs()
                                .text_color(theme::text_dim())
                                .child("FINDINGS"),
                        )
                    })
                    .children(finding_rows),
            )
    }
}

/// The whole measurement, off the UI thread: shapes, per-shape
/// EXPLAIN, defined skip indexes, projection hits, then the verdicts.
async fn measure_workload(
    config: ChConfig,
    database: &str,
    object: &str,
    cluster: Option<String>,
) -> Result<WorkloadReport, String> {
    let client = ChClient::new(config);
    let shapes_result = client
        .query(&workload::query_shapes_sql(
            database,
            object,
            WINDOW_DAYS,
            SHAPE_LIMIT,
            cluster.as_deref(),
        ))
        .await
        .map_err(|error| error.to_string())?;
    let shapes = workload::parse_query_shapes(&shapes_result);
    let total_runs = shapes.iter().map(|shape| shape.runs).sum();

    // One EXPLAIN per shape; shapes that no longer parse (dropped
    // columns, old syntax) are skipped rather than failing the run.
    let mut explained = Vec::new();
    for shape in &shapes {
        let statement = zedb_ch::explain::explain_statement(&shape.example);
        let Ok(result) = client.query(&statement).await else {
            continue;
        };
        let Some(raw) = result
            .rows
            .first()
            .and_then(|row| row.first())
            .map(|value| value.to_string())
        else {
            continue;
        };
        if let Ok(plan) = zedb_ch::explain::parse_explain_json(&raw) {
            explained.push((shape.runs, plan));
        }
    }
    let indexes = workload::aggregate_index_usage(&explained);

    let defined_skips = client
        .query(&workload::skip_indices_sql(database, object))
        .await
        .map(|result| workload::parse_skip_indices(&result))
        .unwrap_or_default();

    // Projection names from the schema, hits from the log; both
    // best-effort (system.projections needs a recent server).
    let projection_names: Vec<String> = client
        .table_projections(database, object)
        .await
        .map(|projections| {
            projections
                .into_iter()
                .map(|projection| projection.name)
                .collect()
        })
        .unwrap_or_default();
    let hits = client
        .query(&workload::projection_usage_sql(
            database,
            object,
            WINDOW_DAYS,
            cluster.as_deref(),
        ))
        .await
        .map(|result| workload::parse_projection_usage(&result))
        .unwrap_or_default();
    let projections: Vec<workload::ProjectionUsage> = projection_names
        .into_iter()
        .map(|name| {
            let count = hits
                .iter()
                .find(|(hit_name, _)| *hit_name == name)
                .map(|(_, count)| *count)
                .unwrap_or(0);
            workload::ProjectionUsage { name, hits: count }
        })
        .collect();

    let findings = workload::workload_findings(
        database,
        object,
        total_runs,
        &indexes,
        &defined_skips,
        &projections,
    );
    Ok(WorkloadReport {
        scope: cluster,
        window_days: WINDOW_DAYS,
        total_runs,
        shapes_analyzed: explained.len(),
        indexes,
        projections,
        findings,
    })
}
