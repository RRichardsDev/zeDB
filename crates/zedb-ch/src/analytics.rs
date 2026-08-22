//! Query-log analytics: what happened over time, and why was it slow.
//!
//! Three reads over `system.query_log`, nothing else: normalized
//! fingerprints aggregated across a time window, the recent runs of
//! one fingerprint, and the ProfileEvents testimony of a single run
//! (the measured counterpart to EXPLAIN's prediction). Cluster scope
//! fans out via clusterAllReplicas exactly like the ops view; the
//! window keeps result sizes bounded, and every aggregate is cast to
//! a stable scalar type so decoding stays boring.

use zedb_core::{QueryResult, Value};

use crate::{ChClient, ChError};

type Result<T> = std::result::Result<T, ChError>;

/// How far back a fingerprint aggregation looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalyticsWindow {
    LastHour,
    LastDay,
    LastWeek,
}

impl AnalyticsWindow {
    pub fn hours(self) -> u32 {
        match self {
            Self::LastHour => 1,
            Self::LastDay => 24,
            Self::LastWeek => 24 * 7,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::LastHour => "1h",
            Self::LastDay => "24h",
            Self::LastWeek => "7d",
        }
    }
}

/// One normalized query shape, aggregated over the window.
#[derive(Debug, Clone)]
pub struct QueryFingerprint {
    /// `normalized_query_hash` as a string, the drill-in key.
    pub hash: String,
    /// A normalized example of the shape (literals replaced by `?`).
    pub sample: String,
    pub runs: u64,
    pub errors: u64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub total_ms: u64,
    pub max_memory: u64,
    pub read_rows: u64,
    pub read_bytes: u64,
    pub users: u64,
    pub last_seen: String,
}

/// One concrete run of a fingerprint.
#[derive(Debug, Clone)]
pub struct FingerprintRun {
    pub query_id: String,
    pub at: String,
    pub duration_ms: u64,
    pub memory: u64,
    pub read_rows: u64,
    /// Empty when the run finished cleanly.
    pub exception: String,
    /// Which node, in cluster scope; empty in node scope.
    pub host: String,
}

/// The measured story of a single run: the headline numbers plus every
/// non-zero ProfileEvent, largest first.
#[derive(Debug, Clone)]
pub struct QueryTestimony {
    pub query: String,
    pub duration_ms: u64,
    pub memory: u64,
    pub read_rows: u64,
    pub read_bytes: u64,
    pub result_rows: u64,
    pub exception: String,
    pub events: Vec<(String, u64)>,
}

/// query_log (or its cluster fan-out) with the shared noise filter:
/// only initial queries, never the log reading itself.
fn log_source(cluster: Option<&str>) -> String {
    match cluster {
        Some(name) => format!(
            "clusterAllReplicas({}, system.query_log)",
            crate::schema::escape_identifier(name)
        ),
        None => "system.query_log".into(),
    }
}

fn fingerprints_sql(window: AnalyticsWindow, cluster: Option<&str>) -> String {
    format!(
        "SELECT toString(normalized_query_hash) AS hash, \
            any(normalizeQuery(query)) AS sample, \
            toUInt64(countIf(type = 'QueryFinish')) AS runs, \
            toUInt64(countIf(type IN ('ExceptionBeforeStart', 'ExceptionWhileProcessing'))) AS errors, \
            toFloat64(quantileIf(0.5)(query_duration_ms, type = 'QueryFinish')) AS p50, \
            toFloat64(quantileIf(0.95)(query_duration_ms, type = 'QueryFinish')) AS p95, \
            toFloat64(quantileIf(0.99)(query_duration_ms, type = 'QueryFinish')) AS p99, \
            toUInt64(sum(query_duration_ms)) AS total_ms, \
            toUInt64(max(memory_usage)) AS max_memory, \
            toUInt64(sum(read_rows)) AS read_rows, \
            toUInt64(sum(read_bytes)) AS read_bytes, \
            toUInt64(uniqExact(user)) AS users, \
            toString(max(event_time)) AS last_seen \
         FROM {} \
         WHERE event_time > now() - INTERVAL {} HOUR \
           AND type != 'QueryStart' \
           AND is_initial_query \
           AND query NOT ILIKE '%system.query_log%' \
         GROUP BY normalized_query_hash \
         ORDER BY total_ms DESC \
         LIMIT 100",
        log_source(cluster),
        window.hours()
    )
}

/// Display columns of the grid-shaped fingerprint query, in order,
/// with the ORDER BY expression each maps to. The alias is what the
/// grid shows and what filter conjuncts reference (they run in
/// HAVING, where aliases of aggregates are legal); the expression is
/// what sorting really uses, so formatted columns still sort by their
/// raw number.
const GRID_COLUMNS: [(&str, &str); 11] = [
    ("shape", "shape"),
    ("runs", "runs"),
    ("err", "err"),
    ("p50_ms", "p50_ms"),
    ("p95_ms", "p95_ms"),
    ("p99_ms", "p99_ms"),
    ("total_ms", "total_ms"),
    ("peak_mem", "max(memory_usage)"),
    ("read", "sum(read_bytes)"),
    ("users", "users"),
    ("last_seen", "last_seen"),
];

/// The grid-shaped fingerprint aggregation: display-formatted columns
/// plus the drill-in hash last. `order` names display columns (unknown
/// names are ignored); `having` holds the grid's managed filter
/// conjuncts verbatim.
pub fn fingerprint_grid_sql(
    window: AnalyticsWindow,
    cluster: Option<&str>,
    order: &[(String, bool)],
    having: &[String],
) -> String {
    let having_clause = if having.is_empty() {
        String::new()
    } else {
        format!("HAVING ({}) ", having.join(") AND ("))
    };
    let mut order_terms: Vec<String> = order
        .iter()
        .filter_map(|(column, ascending)| {
            GRID_COLUMNS
                .iter()
                .find(|(alias, _)| alias == column)
                .map(|(_, expression)| {
                    format!("{expression} {}", if *ascending { "ASC" } else { "DESC" })
                })
        })
        .collect();
    if order_terms.is_empty() {
        order_terms.push("total_ms DESC".into());
    }
    format!(
        "SELECT any(normalizeQuery(query)) AS shape, \
            toUInt64(countIf(type = 'QueryFinish')) AS runs, \
            toUInt64(countIf(type IN ('ExceptionBeforeStart', 'ExceptionWhileProcessing'))) AS err, \
            toFloat64(round(quantileIf(0.5)(query_duration_ms, type = 'QueryFinish'), 1)) AS p50_ms, \
            toFloat64(round(quantileIf(0.95)(query_duration_ms, type = 'QueryFinish'), 1)) AS p95_ms, \
            toFloat64(round(quantileIf(0.99)(query_duration_ms, type = 'QueryFinish'), 1)) AS p99_ms, \
            toUInt64(sum(query_duration_ms)) AS total_ms, \
            formatReadableSize(max(memory_usage)) AS peak_mem, \
            formatReadableSize(sum(read_bytes)) AS read, \
            toUInt64(uniqExact(user)) AS users, \
            toString(max(event_time)) AS last_seen, \
            toString(normalized_query_hash) AS hash \
         FROM {} \
         WHERE event_time > now() - INTERVAL {} HOUR \
           AND type != 'QueryStart' \
           AND is_initial_query \
           AND query NOT ILIKE '%system.query_log%' \
         GROUP BY normalized_query_hash \
         {having_clause}ORDER BY {} \
         LIMIT 100",
        log_source(cluster),
        window.hours(),
        order_terms.join(", ")
    )
}

fn runs_sql(hash: &str, window: AnalyticsWindow, cluster: Option<&str>) -> String {
    let host = match cluster {
        Some(_) => "hostName()",
        None => "''",
    };
    format!(
        "SELECT query_id, toString(event_time) AS at, \
            toUInt64(query_duration_ms) AS duration_ms, \
            toUInt64(memory_usage) AS memory, \
            toUInt64(read_rows) AS read_rows, \
            exception, {host} \
         FROM {} \
         WHERE normalized_query_hash = {hash} \
           AND event_time > now() - INTERVAL {} HOUR \
           AND type != 'QueryStart' \
           AND is_initial_query \
         ORDER BY event_time DESC \
         LIMIT 30",
        log_source(cluster),
        window.hours()
    )
}

fn testimony_sql(query_id: &str, cluster: Option<&str>) -> String {
    format!(
        "SELECT query, \
            toUInt64(query_duration_ms) AS duration_ms, \
            toUInt64(memory_usage) AS memory, \
            toUInt64(read_rows) AS read_rows, \
            toUInt64(read_bytes) AS read_bytes, \
            toUInt64(result_rows) AS result_rows, \
            exception, \
            arrayStringConcat(arrayMap(pair -> concat(pair.1, '=', toString(pair.2)), \
                arraySort(pair -> -toInt64(pair.2), \
                    arrayFilter(pair -> pair.2 != 0, \
                        CAST(ProfileEvents, 'Array(Tuple(String, UInt64))')))), '\\n') AS events \
         FROM {} \
         WHERE query_id = '{}' AND type != 'QueryStart' \
         ORDER BY event_time DESC \
         LIMIT 1",
        log_source(cluster),
        crate::schema::escape_string(query_id)
    )
}

/// The drill-in key must be the decimal hash the aggregation returned;
/// anything else does not reach the SQL.
fn valid_hash(hash: &str) -> bool {
    !hash.is_empty() && hash.len() <= 20 && hash.bytes().all(|byte| byte.is_ascii_digit())
}

fn u64_at(row: &[Value], index: usize, label: &str) -> Result<u64> {
    match row.get(index) {
        Some(Value::UInt(value)) => Ok(*value),
        value => Err(ChError::Decode(format!(
            "expected UInt64 for {label}, got {value:?}"
        ))),
    }
}

fn f64_at(row: &[Value], index: usize, label: &str) -> Result<f64> {
    match row.get(index) {
        Some(Value::Float(value)) => Ok(*value),
        // quantileIf over zero matching rows yields NULL.
        Some(Value::Null) => Ok(0.0),
        value => Err(ChError::Decode(format!(
            "expected Float64 for {label}, got {value:?}"
        ))),
    }
}

fn string_at(row: &[Value], index: usize, label: &str) -> Result<String> {
    match row.get(index) {
        Some(Value::String(value)) => Ok(value.clone()),
        value => Err(ChError::Decode(format!(
            "expected String for {label}, got {value:?}"
        ))),
    }
}

fn parse_fingerprints(result: QueryResult) -> Result<Vec<QueryFingerprint>> {
    result
        .rows
        .into_iter()
        .map(|row| {
            Ok(QueryFingerprint {
                hash: string_at(&row, 0, "fingerprint hash")?,
                sample: string_at(&row, 1, "fingerprint sample")?,
                runs: u64_at(&row, 2, "fingerprint runs")?,
                errors: u64_at(&row, 3, "fingerprint errors")?,
                p50_ms: f64_at(&row, 4, "fingerprint p50")?,
                p95_ms: f64_at(&row, 5, "fingerprint p95")?,
                p99_ms: f64_at(&row, 6, "fingerprint p99")?,
                total_ms: u64_at(&row, 7, "fingerprint total")?,
                max_memory: u64_at(&row, 8, "fingerprint memory")?,
                read_rows: u64_at(&row, 9, "fingerprint read rows")?,
                read_bytes: u64_at(&row, 10, "fingerprint read bytes")?,
                users: u64_at(&row, 11, "fingerprint users")?,
                last_seen: string_at(&row, 12, "fingerprint last seen")?,
            })
        })
        .collect()
}

fn parse_runs(result: QueryResult) -> Result<Vec<FingerprintRun>> {
    result
        .rows
        .into_iter()
        .map(|row| {
            Ok(FingerprintRun {
                query_id: string_at(&row, 0, "run query_id")?,
                at: string_at(&row, 1, "run time")?,
                duration_ms: u64_at(&row, 2, "run duration")?,
                memory: u64_at(&row, 3, "run memory")?,
                read_rows: u64_at(&row, 4, "run read rows")?,
                exception: string_at(&row, 5, "run exception")?,
                host: string_at(&row, 6, "run host")?,
            })
        })
        .collect()
}

fn parse_testimony(result: QueryResult) -> Result<Option<QueryTestimony>> {
    let Some(row) = result.rows.into_iter().next() else {
        return Ok(None);
    };
    let events = string_at(&row, 7, "testimony events")?
        .lines()
        .filter_map(|line| {
            let (name, value) = line.split_once('=')?;
            Some((name.to_string(), value.parse().ok()?))
        })
        .collect();
    Ok(Some(QueryTestimony {
        query: string_at(&row, 0, "testimony query")?,
        duration_ms: u64_at(&row, 1, "testimony duration")?,
        memory: u64_at(&row, 2, "testimony memory")?,
        read_rows: u64_at(&row, 3, "testimony read rows")?,
        read_bytes: u64_at(&row, 4, "testimony read bytes")?,
        result_rows: u64_at(&row, 5, "testimony result rows")?,
        exception: string_at(&row, 6, "testimony exception")?,
        events,
    }))
}

impl ChClient {
    /// Normalized query shapes over the window, heaviest total time
    /// first, capped at 100.
    pub async fn query_fingerprints(
        &self,
        window: AnalyticsWindow,
        cluster: Option<&str>,
    ) -> Result<Vec<QueryFingerprint>> {
        parse_fingerprints(self.query(&fingerprints_sql(window, cluster)).await?)
    }

    /// The most recent runs of one fingerprint inside the window.
    pub async fn fingerprint_runs(
        &self,
        hash: &str,
        window: AnalyticsWindow,
        cluster: Option<&str>,
    ) -> Result<Vec<FingerprintRun>> {
        if !valid_hash(hash) {
            return Err(ChError::Decode(format!("not a fingerprint hash: {hash:?}")));
        }
        parse_runs(self.query(&runs_sql(hash, window, cluster)).await?)
    }

    /// The measured story of one run; None when query_log has no row
    /// for the id (rotated out, or never logged).
    pub async fn query_testimony(
        &self,
        query_id: &str,
        cluster: Option<&str>,
    ) -> Result<Option<QueryTestimony>> {
        parse_testimony(self.query(&testimony_sql(query_id, cluster)).await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_scopes_and_noise_filter_are_in_the_sql() {
        let sql = fingerprints_sql(AnalyticsWindow::LastDay, None);
        assert!(sql.contains("INTERVAL 24 HOUR"));
        assert!(sql.contains("FROM system.query_log"));
        assert!(sql.contains("is_initial_query"));
        assert!(sql.contains("NOT ILIKE '%system.query_log%'"));

        let sql = fingerprints_sql(AnalyticsWindow::LastHour, Some("zedb_cluster"));
        assert!(sql.contains("clusterAllReplicas(`zedb_cluster`, system.query_log)"));
        assert!(sql.contains("INTERVAL 1 HOUR"));
    }

    #[test]
    fn grid_sql_maps_sort_and_injects_filters_safely() {
        let sql = fingerprint_grid_sql(AnalyticsWindow::LastDay, None, &[], &[]);
        assert!(sql.contains("ORDER BY total_ms DESC"), "default sort");
        assert!(sql.ends_with("LIMIT 100"));

        let sql = fingerprint_grid_sql(
            AnalyticsWindow::LastDay,
            None,
            &[
                ("peak_mem".into(), true),
                ("nonsense".into(), true),
                ("runs".into(), false),
            ],
            &["shape ILIKE '%tenant%'".into(), "err > 0".into()],
        );
        assert!(
            sql.contains("ORDER BY max(memory_usage) ASC, runs DESC"),
            "formatted columns sort by their raw expression; unknown columns are dropped: {sql}"
        );
        assert!(sql.contains("HAVING (shape ILIKE '%tenant%') AND (err > 0)"));
    }

    #[test]
    fn drill_in_keys_are_validated_before_reaching_sql() {
        assert!(valid_hash("12345678901234567890"));
        for bad in ["", "abc", "123'; DROP TABLE x --", "123456789012345678901"] {
            assert!(!valid_hash(bad), "accepted {bad:?}");
        }
    }

    #[test]
    fn testimony_events_parse_and_order_survives() {
        let events: Vec<(String, u64)> = "SelectedRows=100\nRealTimeMicroseconds=42"
            .lines()
            .filter_map(|line| {
                let (name, value) = line.split_once('=')?;
                Some((name.to_string(), value.parse().ok()?))
            })
            .collect();
        assert_eq!(events[0], ("SelectedRows".into(), 100));
        assert_eq!(events[1], ("RealTimeMicroseconds".into(), 42));
    }
}
