//! Workload-measured index and projection effectiveness: aggregate a
//! table's real traffic from `system.query_log`, EXPLAIN the heaviest
//! query shapes, and judge each index and projection by what it
//! actually pruned or served, not by what one query might do.
//!
//! The measurement is per query *shape* (`normalized_query_hash`):
//! one EXPLAIN per shape, weighted by how often the shape ran. Advice
//! stays explainable and is never applied automatically.

use crate::explain::ExplainNode;
use crate::schema::escape_string;
use zedb_core::{QueryResult, Value};

/// One normalized query shape from the log: a representative example
/// plus how much it ran and read.
#[derive(Clone, Debug, PartialEq)]
pub struct QueryShape {
    pub example: String,
    pub runs: u64,
    pub read_rows: u64,
}

/// The heaviest SELECT shapes touching the table inside the window,
/// by rows read. `is_initial_query` keeps distributed fan-out legs
/// from double-counting.
/// The query_log source: fanned out over every replica when a real
/// cluster exists (each initial query logs on exactly one replica, so
/// the union is the whole workload), the bare table otherwise.
fn log_source(cluster: Option<&str>) -> String {
    match cluster {
        Some(cluster) => format!(
            "clusterAllReplicas('{}', system.query_log)",
            escape_string(cluster)
        ),
        None => "system.query_log".to_string(),
    }
}

pub fn query_shapes_sql(
    database: &str,
    table: &str,
    days: u32,
    limit: usize,
    cluster: Option<&str>,
) -> String {
    let target = escape_string(&format!("{database}.{table}"));
    format!(
        "SELECT any(query) AS example, count() AS runs, sum(read_rows) AS read_rows \
         FROM {log} \
         WHERE type = 'QueryFinish' AND query_kind = 'Select' AND is_initial_query \
           AND event_date >= today() - {days} \
           AND event_time > now() - INTERVAL {days} DAY \
           AND has(tables, '{target}') \
         GROUP BY normalized_query_hash \
         ORDER BY read_rows DESC \
         LIMIT {limit}",
        log = log_source(cluster)
    )
}

pub fn parse_query_shapes(result: &QueryResult) -> Vec<QueryShape> {
    rows_by_name(result, &["example", "runs", "read_rows"])
        .into_iter()
        .map(|row| QueryShape {
            example: row[0].clone(),
            runs: row[1].parse().unwrap_or(0),
            read_rows: row[2].parse().unwrap_or(0),
        })
        .collect()
}

/// A skip index as defined on the table.
#[derive(Clone, Debug, PartialEq)]
pub struct SkipIndexInfo {
    pub name: String,
    pub kind: String,
    pub expr: String,
}

pub fn skip_indices_sql(database: &str, table: &str) -> String {
    let db = escape_string(database);
    let object = escape_string(table);
    format!(
        "SELECT name, type_full, expr FROM system.data_skipping_indices \
         WHERE database = '{db}' AND table = '{object}' ORDER BY name"
    )
}

pub fn parse_skip_indices(result: &QueryResult) -> Vec<SkipIndexInfo> {
    rows_by_name(result, &["name", "type_full", "expr"])
        .into_iter()
        .map(|row| SkipIndexInfo {
            name: row[0].clone(),
            kind: row[1].clone(),
            expr: row[2].clone(),
        })
        .collect()
}

/// Projection hits inside the window. `query_log.projections` records
/// full names (`db.table.projection`); the tail segment is the name.
pub fn projection_usage_sql(
    database: &str,
    table: &str,
    days: u32,
    cluster: Option<&str>,
) -> String {
    let prefix = escape_string(&format!("{database}.{table}."));
    format!(
        "SELECT arrayJoin(projections) AS projection, count() AS hits \
         FROM {log} \
         WHERE type = 'QueryFinish' AND is_initial_query \
           AND event_date >= today() - {days} \
           AND event_time > now() - INTERVAL {days} DAY \
           AND startsWith(projection, '{prefix}') \
         GROUP BY projection",
        log = log_source(cluster)
    )
}

pub fn parse_projection_usage(result: &QueryResult) -> Vec<(String, u64)> {
    rows_by_name(result, &["projection", "hits"])
        .into_iter()
        .map(|row| {
            let name = row[0]
                .rsplit_once('.')
                .map(|(_, name)| name.to_string())
                .unwrap_or_else(|| row[0].clone());
            (name, row[1].parse().unwrap_or(0))
        })
        .collect()
}

/// One index's measured behavior across the analyzed shapes, weighted
/// by how often each shape ran.
#[derive(Clone, Debug, PartialEq)]
pub struct IndexUsage {
    /// "PRIMARY KEY" or the skip index's name.
    pub label: String,
    pub is_primary: bool,
    /// Runs of shapes whose plan consulted this index.
    pub runs_applied: u64,
    /// Run-weighted granules surviving this index.
    pub selected_weighted: u128,
    /// Run-weighted granules it was offered.
    pub initial_weighted: u128,
}

impl IndexUsage {
    /// Weighted fraction of granules the index let through (1.0 = it
    /// pruned nothing).
    pub fn fraction(&self) -> f64 {
        if self.initial_weighted == 0 {
            1.0
        } else {
            self.selected_weighted as f64 / self.initial_weighted as f64
        }
    }
}

/// Fold per-shape plans into per-index usage. Partition/MinMax plan
/// entries are ignored: the actionable levers are the primary key and
/// named skip indexes.
pub fn aggregate_index_usage(shapes: &[(u64, ExplainNode)]) -> Vec<IndexUsage> {
    let mut usages: Vec<IndexUsage> = Vec::new();
    fn walk(node: &ExplainNode, runs: u64, usages: &mut Vec<IndexUsage>) {
        for index in &node.indexes {
            let (label, is_primary) = match index.index_type.as_str() {
                "PrimaryKey" => ("PRIMARY KEY".to_string(), true),
                "Skip" => match &index.name {
                    Some(name) => (name.clone(), false),
                    None => continue,
                },
                _ => continue,
            };
            if index.initial_granules == 0 {
                continue;
            }
            let entry = match usages.iter_mut().find(|usage| usage.label == label) {
                Some(entry) => entry,
                None => {
                    usages.push(IndexUsage {
                        label,
                        is_primary,
                        runs_applied: 0,
                        selected_weighted: 0,
                        initial_weighted: 0,
                    });
                    usages.last_mut().expect("just pushed")
                }
            };
            entry.runs_applied += runs;
            entry.selected_weighted += index.selected_granules as u128 * runs as u128;
            entry.initial_weighted += index.initial_granules as u128 * runs as u128;
        }
        for child in &node.children {
            walk(child, runs, usages);
        }
    }
    for (runs, plan) in shapes {
        walk(plan, *runs, &mut usages);
    }
    usages
}

/// A projection's measured use inside the window.
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectionUsage {
    pub name: String,
    pub hits: u64,
}

/// One plain-language finding with optional copyable DDL. Never
/// applied by the tool.
#[derive(Clone, Debug, PartialEq)]
pub struct WorkloadFinding {
    pub title: String,
    pub detail: String,
    pub fix: Option<String>,
}

/// The whole measured picture for one table.
#[derive(Clone, Debug, PartialEq)]
pub struct WorkloadReport {
    /// The cluster the log was fanned out over; None means one
    /// node's log only, and the tab says so.
    pub scope: Option<String>,
    pub window_days: u32,
    pub total_runs: u64,
    pub shapes_analyzed: usize,
    pub indexes: Vec<IndexUsage>,
    pub projections: Vec<ProjectionUsage>,
    pub findings: Vec<WorkloadFinding>,
}

/// Judge the measurements. Rules are deliberately few and explainable;
/// every finding names its numbers.
pub fn workload_findings(
    database: &str,
    table: &str,
    total_runs: u64,
    indexes: &[IndexUsage],
    defined_skips: &[SkipIndexInfo],
    projections: &[ProjectionUsage],
) -> Vec<WorkloadFinding> {
    let mut findings = Vec::new();
    if total_runs == 0 {
        return findings;
    }
    let percent = |part: u64| (part as f64 / total_runs as f64 * 100.0).round() as u64;

    for usage in indexes {
        let share = percent(usage.runs_applied);
        let fraction = usage.fraction();
        if usage.is_primary {
            if fraction > 0.7 && share >= 30 {
                findings.push(WorkloadFinding {
                    title: "The primary key barely prunes this workload".into(),
                    detail: format!(
                        "Across {share}% of measured runs the primary key let \
                         {:.0}% of granules through. The common WHEREs are not \
                         covered by the ORDER BY; consider a skip index on the \
                         most-filtered column, or revisit the ORDER BY for new \
                         data.",
                        fraction * 100.0
                    ),
                    fix: None,
                });
            }
            continue;
        }
        if fraction > 0.9 {
            findings.push(WorkloadFinding {
                title: format!("Skip index {} never prunes", usage.label),
                detail: format!(
                    "It was consulted in {share}% of measured runs and let \
                     {:.0}% of granules through. It costs merge and insert \
                     work while saving nothing; dropping it is safe for reads.",
                    fraction * 100.0
                ),
                fix: Some(format!(
                    "ALTER TABLE {database}.{table} DROP INDEX {};",
                    usage.label
                )),
            });
        }
    }

    // Defined but absent from every measured plan: dead weight for
    // this workload (with the window named, since traffic shifts).
    for skip in defined_skips {
        if indexes.iter().any(|usage| usage.label == skip.name) {
            continue;
        }
        findings.push(WorkloadFinding {
            title: format!("Skip index {} went unused", skip.name),
            detail: format!(
                "No measured query shape consulted it ({} on {}). If the \
                 filter it serves has left the workload, dropping it saves \
                 insert and merge work.",
                skip.kind, skip.expr
            ),
            fix: Some(format!(
                "ALTER TABLE {database}.{table} DROP INDEX {};",
                skip.name
            )),
        });
    }

    for projection in projections {
        if projection.hits == 0 {
            findings.push(WorkloadFinding {
                title: format!("Projection {} served no queries", projection.name),
                detail: "It stores and merges a full extra copy of its columns \
                         while nothing in the window read from it."
                    .into(),
                fix: Some(format!(
                    "ALTER TABLE {database}.{table} DROP PROJECTION {};",
                    projection.name
                )),
            });
        } else {
            findings.push(WorkloadFinding {
                title: format!(
                    "Projection {} served {}% of measured runs",
                    projection.name,
                    percent(projection.hits)
                ),
                detail: "Earning its keep; keep it.".into(),
                fix: None,
            });
        }
    }

    findings
}

/// Match result columns by name and stringify each requested cell, so
/// column-order changes across server versions do not bite.
fn rows_by_name(result: &QueryResult, names: &[&str]) -> Vec<Vec<String>> {
    let positions: Option<Vec<usize>> = names
        .iter()
        .map(|name| {
            result
                .columns
                .iter()
                .position(|column| column.name == *name)
        })
        .collect();
    let Some(positions) = positions else {
        return Vec::new();
    };
    result
        .rows
        .iter()
        .map(|row| {
            positions
                .iter()
                .map(|&index| row.get(index).map(Value::to_string).unwrap_or_default())
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explain::{ExplainIndex, ExplainNode};

    fn read_node(indexes: Vec<ExplainIndex>) -> ExplainNode {
        ExplainNode {
            node_type: "ReadFromMergeTree".into(),
            description: None,
            indexes,
            children: Vec::new(),
        }
    }

    fn index(index_type: &str, name: Option<&str>, initial: u64, selected: u64) -> ExplainIndex {
        ExplainIndex {
            index_type: index_type.into(),
            name: name.map(str::to_string),
            keys: Vec::new(),
            condition: None,
            initial_parts: 1,
            selected_parts: 1,
            initial_granules: initial,
            selected_granules: selected,
        }
    }

    #[test]
    fn aggregates_weighted_by_runs() {
        let shapes = vec![
            // 90 runs pruning hard, 10 runs pruning nothing.
            (90, read_node(vec![index("PrimaryKey", None, 100, 5)])),
            (10, read_node(vec![index("PrimaryKey", None, 100, 100)])),
            (10, read_node(vec![index("Skip", Some("idx_a"), 50, 50)])),
        ];
        let usages = aggregate_index_usage(&shapes);
        let pk = usages.iter().find(|usage| usage.is_primary).unwrap();
        assert_eq!(pk.runs_applied, 100);
        // (90*5 + 10*100) / (100*100) = 1450/10000
        assert!((pk.fraction() - 0.145).abs() < 1e-9);
        let skip = usages.iter().find(|usage| usage.label == "idx_a").unwrap();
        assert_eq!(skip.runs_applied, 10);
        assert!((skip.fraction() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn findings_flag_dead_and_unused_indexes_and_projections() {
        let indexes = vec![
            IndexUsage {
                label: "PRIMARY KEY".into(),
                is_primary: true,
                runs_applied: 100,
                selected_weighted: 90,
                initial_weighted: 100,
            },
            IndexUsage {
                label: "idx_dead".into(),
                is_primary: false,
                runs_applied: 60,
                selected_weighted: 99,
                initial_weighted: 100,
            },
        ];
        let defined = vec![
            SkipIndexInfo {
                name: "idx_dead".into(),
                kind: "bloom_filter".into(),
                expr: "user_id".into(),
            },
            SkipIndexInfo {
                name: "idx_unused".into(),
                kind: "minmax".into(),
                expr: "ts".into(),
            },
        ];
        let projections = vec![
            ProjectionUsage {
                name: "by_user".into(),
                hits: 0,
            },
            ProjectionUsage {
                name: "daily".into(),
                hits: 40,
            },
        ];
        let findings = workload_findings("sat", "events", 100, &indexes, &defined, &projections);
        let titles: Vec<&str> = findings.iter().map(|f| f.title.as_str()).collect();
        assert!(titles.iter().any(|t| t.contains("primary key barely")));
        assert!(titles.iter().any(|t| t.contains("idx_dead never prunes")));
        assert!(titles.iter().any(|t| t.contains("idx_unused went unused")));
        assert!(titles.iter().any(|t| t.contains("by_user served no")));
        assert!(titles.iter().any(|t| t.contains("daily served 40%")));
        let drop_dead = findings
            .iter()
            .find(|f| f.title.contains("idx_dead"))
            .unwrap();
        assert_eq!(
            drop_dead.fix.as_deref(),
            Some("ALTER TABLE sat.events DROP INDEX idx_dead;")
        );
    }

    #[test]
    fn shape_sql_names_the_table_and_window() {
        let sql = query_shapes_sql("sat", "events", 7, 12, None);
        assert!(sql.contains("FROM system.query_log"));
        assert!(sql.contains("has(tables, 'sat.events')"));
        assert!(sql.contains("INTERVAL 7 DAY"));
        assert!(sql.contains("LIMIT 12"));
    }

    #[test]
    fn shape_sql_fans_out_over_the_cluster() {
        let sql = query_shapes_sql("sat", "events", 7, 12, Some("default"));
        assert!(sql.contains("clusterAllReplicas('default', system.query_log)"));
        let projections = projection_usage_sql("sat", "events", 7, Some("default"));
        assert!(projections.contains("clusterAllReplicas('default', system.query_log)"));
    }

    #[test]
    fn projection_usage_strips_the_table_prefix() {
        use zedb_core::ColumnMeta;
        let result = QueryResult {
            columns: vec![
                ColumnMeta {
                    name: "projection".into(),
                    type_name: "String".into(),
                },
                ColumnMeta {
                    name: "hits".into(),
                    type_name: "UInt64".into(),
                },
            ],
            rows: vec![vec![
                Value::String("sat.events.by_user".into()),
                Value::UInt(9),
            ]],
        };
        assert_eq!(
            parse_projection_usage(&result),
            vec![("by_user".to_string(), 9)]
        );
    }
}
