//! Minimal ClickHouse HTTP client.
//!
//! Schema queries can be materialized, while result queries can be decoded in
//! incremental batches as their HTTP body arrives.

use futures_util::StreamExt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use zedb_core::QueryResult;
use zedb_core::{ColumnMeta, DriverConfig, Value};

use crate::error::{ChError, Result};
use crate::rowbinary;

#[derive(Debug, Clone, Default)]
pub struct ChConfig {
    /// Base URL of the HTTP interface, e.g. `http://localhost:8123`.
    pub url: String,
    pub user: String,
    pub password: Option<String>,
    /// Default database for unqualified table names.
    pub database: Option<String>,
    /// When set, every request carries `readonly=2`: the server rejects
    /// writes and DDL while still allowing query-level settings. Safety is
    /// enforced server-side, not by client-side SQL inspection.
    pub read_only: bool,
    /// Per-cluster driver knobs (timeouts, extra ClickHouse settings).
    pub driver: DriverConfig,
}

pub struct ChClient {
    cfg: ChConfig,
    http: reqwest::Client,
}

#[derive(Debug)]
pub enum QueryStreamEvent {
    Columns(Vec<ColumnMeta>),
    Rows(Vec<Vec<Value>>),
    Progress(QueryProgress),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct QueryProgress {
    pub read_rows: Option<u64>,
    pub read_bytes: Option<u64>,
    pub total_rows: Option<u64>,
    pub received_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueryStreamSummary {
    pub rows: usize,
    pub capped: bool,
}

impl ChClient {
    pub fn new(cfg: ChConfig) -> Self {
        let connect_timeout = cfg
            .driver
            .settings
            .iter()
            .find(|setting| setting.name.trim() == "connect_timeout")
            .and_then(|setting| setting.value.trim().parse::<u64>().ok())
            .filter(|&secs| secs > 0)
            .unwrap_or(10);
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(connect_timeout))
            .build()
            .unwrap_or_default();
        Self { cfg, http }
    }

    /// Query-string pairs for this cluster's driver settings. The
    /// guarded (agent) path skips the execution cap: its own stricter
    /// cap must win.
    fn driver_params(&self, include_execution_cap: bool) -> Vec<(String, String)> {
        let mut params: Vec<(String, String)> = self
            .cfg
            .driver
            .settings
            .iter()
            .filter(|setting| {
                let name = setting.name.trim();
                // connect_timeout shapes the HTTP client, not the query.
                !name.is_empty() && !setting.value.trim().is_empty() && name != "connect_timeout"
            })
            .map(|setting| {
                (
                    setting.name.trim().to_string(),
                    setting.value.trim().to_string(),
                )
            })
            .collect();
        if !include_execution_cap {
            params.retain(|(name, _)| name != "max_execution_time");
        }
        params
    }

    /// Run a query with server-side guardrails on top of read-only:
    /// the agent-facing path (docs/PHASE-3.1.md M3). Time, row, and
    /// byte caps are enforced by ClickHouse, not by inspecting SQL.
    pub async fn query_guarded(
        &self,
        sql: &str,
        max_execution_time_secs: u32,
        max_result_rows: u64,
        max_bytes_to_read: u64,
    ) -> Result<QueryResult> {
        let time = max_execution_time_secs.to_string();
        let rows = max_result_rows.to_string();
        let bytes = max_bytes_to_read.to_string();
        let body = self
            .request(
                sql,
                &[
                    ("default_format", "RowBinaryWithNamesAndTypes"),
                    ("max_execution_time", &time),
                    ("max_result_rows", &rows),
                    ("result_overflow_mode", "break"),
                    ("max_bytes_to_read", &bytes),
                ],
            )
            .await?;
        rowbinary::decode(&body)
    }

    /// Run a query and materialize the full typed result.
    pub async fn query(&self, sql: &str) -> Result<QueryResult> {
        let body = self
            .request(sql, &[("default_format", "RowBinaryWithNamesAndTypes")])
            .await?;
        rowbinary::decode(&body)
    }

    /// Run a query and report decoded columns and rows as soon as complete
    /// values arrive from ClickHouse. Aborting the caller's task cancels the
    /// underlying HTTP request.
    pub async fn query_stream(
        &self,
        sql: &str,
        row_limit: usize,
        mut on_event: impl FnMut(QueryStreamEvent),
    ) -> Result<QueryStreamSummary> {
        let mut request = self
            .http
            .post(&self.cfg.url)
            .header("X-ClickHouse-User", &self.cfg.user)
            .body(sql.to_string());
        if let Some(password) = &self.cfg.password {
            request = request.header("X-ClickHouse-Key", password);
        }
        if let Some(database) = &self.cfg.database {
            request = request.query(&[("database", database.as_str())]);
        }
        if self.cfg.read_only {
            request = request.query(&[("readonly", "2")]);
        }
        for (name, value) in self.driver_params(true) {
            request = request.query(&[(name.as_str(), value.as_str())]);
        }
        let query_id = next_query_id();
        request = request.query(&[("query_id", query_id.as_str())]);
        request = request.query(&[("default_format", "RowBinaryWithNamesAndTypes")]);

        let mut progress_interval = tokio::time::interval(Duration::from_millis(100));
        progress_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut response_future = Box::pin(request.send());
        let response = loop {
            tokio::select! {
                response = &mut response_future => break response?,
                _ = progress_interval.tick() => {
                    if let Some(progress) = self.query_progress(&query_id).await {
                        on_event(QueryStreamEvent::Progress(progress));
                    }
                }
            }
        };
        let status = response.status();
        let code = response
            .headers()
            .get("X-ClickHouse-Exception-Code")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse().ok());
        if !status.is_success() {
            let bytes = response.bytes().await?;
            return Err(ChError::Server {
                code,
                message: String::from_utf8_lossy(&bytes).trim().to_string(),
            });
        }

        let mut decoder = rowbinary::StreamingDecoder::new();
        let mut sent_columns = false;
        let mut sent_rows = 0;
        let mut pending_rows = Vec::new();
        let mut received_bytes = 0;
        let mut body = response.bytes_stream();
        loop {
            tokio::select! {
                chunk = body.next() => {
                    let Some(chunk) = chunk else {
                        break;
                    };
                    let chunk = chunk?;
                    received_bytes += chunk.len() as u64;
                    let mut rows = decoder.push(&chunk)?;
                    if !sent_columns {
                        if let Some(columns) = decoder.columns() {
                            on_event(QueryStreamEvent::Columns(columns.to_vec()));
                            sent_columns = true;
                        }
                    }
                    if !rows.is_empty() {
                        let remaining = row_limit.saturating_sub(sent_rows + pending_rows.len());
                        let capped = rows.len() > remaining;
                        if capped {
                            rows.truncate(remaining);
                        }
                        pending_rows.extend(rows);
                        if pending_rows.len() >= 512 || capped {
                            sent_rows += pending_rows.len();
                            on_event(QueryStreamEvent::Rows(std::mem::take(&mut pending_rows)));
                        }
                        if capped {
                            on_event(QueryStreamEvent::Progress(QueryProgress {
                                received_bytes,
                                ..QueryProgress::default()
                            }));
                            return Ok(QueryStreamSummary {
                                rows: sent_rows,
                                capped: true,
                            });
                        }
                    }
                }
                _ = progress_interval.tick() => {
                    let mut progress = self.query_progress(&query_id).await.unwrap_or_default();
                    progress.received_bytes = received_bytes;
                    on_event(QueryStreamEvent::Progress(progress));
                }
            }
        }
        if !pending_rows.is_empty() {
            sent_rows += pending_rows.len();
            on_event(QueryStreamEvent::Rows(pending_rows));
        }
        on_event(QueryStreamEvent::Progress(QueryProgress {
            received_bytes,
            ..QueryProgress::default()
        }));
        decoder.finish()?;
        Ok(QueryStreamSummary {
            rows: sent_rows,
            capped: false,
        })
    }

    async fn query_progress(&self, query_id: &str) -> Option<QueryProgress> {
        let sql = format!(
            "SELECT read_rows, read_bytes, total_rows_approx FROM system.processes \
             WHERE query_id = '{query_id}' LIMIT 1"
        );
        let result = self.query(&sql).await.ok()?;
        let row = result.rows.first()?;
        Some(QueryProgress {
            read_rows: value_as_u64(row.first()?),
            read_bytes: value_as_u64(row.get(1)?),
            total_rows: value_as_u64(row.get(2)?).filter(|total| *total > 0),
            received_bytes: 0,
        })
    }

    /// Run a statement, discarding any output (DDL, INSERT, SET, ...).
    pub async fn execute(&self, sql: &str) -> Result<()> {
        self.request(sql, &[]).await.map(|_| ())
    }

    /// `GET /ping`: true when the server is up and answering.
    pub async fn ping(&self) -> bool {
        let url = format!("{}/ping", self.cfg.url.trim_end_matches('/'));
        let mut request = self
            .http
            .get(url)
            .header("X-ClickHouse-User", &self.cfg.user);
        if let Some(password) = &self.cfg.password {
            request = request.header("X-ClickHouse-Key", password);
        }
        match request.send().await {
            Ok(resp) => resp.status().is_success(),
            Err(_) => false,
        }
    }

    /// Run an authenticated, read-only query to validate the endpoint and
    /// credentials together. ClickHouse's `/ping` endpoint does not perform
    /// authentication, so it cannot be used as a connection test.
    pub async fn test_connection(&self) -> Result<()> {
        self.request("SELECT 1", &[]).await.map(|_| ())
    }

    async fn request(&self, sql: &str, params: &[(&str, &str)]) -> Result<Vec<u8>> {
        let mut req = self
            .http
            .post(&self.cfg.url)
            .header("X-ClickHouse-User", &self.cfg.user)
            .body(sql.to_string());
        if let Some(password) = &self.cfg.password {
            req = req.header("X-ClickHouse-Key", password);
        }
        if let Some(db) = &self.cfg.database {
            req = req.query(&[("database", db.as_str())]);
        }
        if self.cfg.read_only {
            req = req.query(&[("readonly", "2")]);
        }
        // Guarded callers pass their own caps in `params`; the driver's
        // execution cap stays out of their way.
        let guarded = params.iter().any(|(name, _)| *name == "max_execution_time");
        for (name, value) in self.driver_params(!guarded) {
            req = req.query(&[(name.as_str(), value.as_str())]);
        }
        req = req.query(params);

        let resp = req.send().await?;
        let status = resp.status();
        let code = resp
            .headers()
            .get("X-ClickHouse-Exception-Code")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse().ok());
        let bytes = resp.bytes().await?;
        if !status.is_success() {
            return Err(ChError::Server {
                code,
                message: String::from_utf8_lossy(&bytes).trim().to_string(),
            });
        }
        Ok(bytes.to_vec())
    }
}

fn value_as_u64(value: &Value) -> Option<u64> {
    match value {
        Value::UInt(value) => Some(*value),
        Value::Int(value) => u64::try_from(*value).ok(),
        _ => None,
    }
}

fn next_query_id() -> String {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!("zedb-{millis}-{}", NEXT_ID.fetch_add(1, Ordering::Relaxed))
}
