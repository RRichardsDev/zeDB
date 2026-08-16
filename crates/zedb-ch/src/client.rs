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

mod export;
mod streaming;
mod topology;

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
    /// Explicit native (TCP) port for this endpoint, when the user
    /// configured one. Trusted first by the native connect; discovery
    /// heuristics remain the fallback. The identity check applies
    /// either way.
    pub native_port: Option<u16>,
}

pub struct ChClient {
    cfg: ChConfig,
    http: reqwest::Client,
}

#[derive(Debug)]
pub enum QueryStreamEvent {
    /// Sent first: the server-side query_id, for correlation (e.g.
    /// recognizing a KILL initiated from the ops view).
    Started {
        query_id: String,
    },
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

/// One row of `system.clusters WHERE is_local = 1`: this node's place
/// in one named cluster.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClusterMembership {
    pub cluster: String,
    pub shard: u64,
    pub replica: u64,
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
    pub(super) fn driver_params(&self, include_execution_cap: bool) -> Vec<(String, String)> {
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
        // Compressed transfer by default: reqwest negotiates zstd/gzip
        // via Accept-Encoding and decompresses the stream transparently;
        // ClickHouse only compresses when asked. A user-provided row
        // wins (e.g. "0" to turn it off).
        if !params
            .iter()
            .any(|(name, _)| name == "enable_http_compression")
        {
            params.push(("enable_http_compression".into(), "1".into()));
        }
        // JSON columns arrive as their string form; RowBinary's native
        // JSON serialization (dynamic paths) is not decoded.
        if !params
            .iter()
            .any(|(name, _)| name == "output_format_binary_write_json_as_string")
        {
            params.push((
                "output_format_binary_write_json_as_string".into(),
                "1".into(),
            ));
        }
        params
    }

    /// Run a query with server-side guardrails on top of read-only: the
    /// agent-facing path. Time, row, and byte caps are enforced by
    /// ClickHouse, not by inspecting SQL.
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
    ///
    /// Reads prefer the pooled native (TCP) connection when one is up;
    /// the first query kicks off a background connect and rides
    /// HTTP. A native transport failure falls back to HTTP for this
    /// query and evicts the broken connection; a server error does not
    /// (the query really ran). Mutating statements never route natively.
    pub async fn query(&self, sql: &str) -> Result<QueryResult> {
        if crate::native::is_read_statement(sql) {
            if let Some(native) = crate::native::pooled(&self.cfg) {
                match native.query(sql).await {
                    Ok(result) => return Ok(result),
                    Err(error @ ChError::Server { .. }) => return Err(error),
                    // Transport or decode trouble: this query falls back to
                    // HTTP. Only a dead socket evicts the connection; a
                    // per-query gap (e.g. a type the native decoder can't
                    // parse) keeps the healthy connection pooled.
                    Err(_) => {
                        if native.is_closed() {
                            crate::native::evict(&self.cfg);
                        }
                    }
                }
            }
        }
        let body = self
            .request(sql, &[("default_format", "RowBinaryWithNamesAndTypes")])
            .await?;
        rowbinary::decode(&body)
    }

    /// Run a query strictly over HTTP, never routed to the native
    /// transport: the native connect path uses this to interrogate the
    /// server it is about to pair with, and tests use it to compare the
    /// two transports' decoding.
    pub async fn query_http(&self, sql: &str) -> Result<QueryResult> {
        let body = self
            .request(sql, &[("default_format", "RowBinaryWithNamesAndTypes")])
            .await?;
        rowbinary::decode(&body)
    }

    /// This server's instance UUID over HTTP only (never routed natively):
    /// the native transport uses it to prove a guessed native port belongs
    /// to the same server as the HTTP endpoint.
    pub(crate) async fn server_uuid_http(&self) -> Result<String> {
        let result = self.query_http("SELECT serverUUID()").await?;
        match result.rows.first().and_then(|row| row.first()) {
            Some(value) => Ok(value.to_string()),
            None => Err(ChError::Decode("serverUUID() returned no rows".into())),
        }
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
