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

const MAX_MATERIALIZED_RESPONSE_BYTES: u64 = 1024 * 1024 * 1024;
#[cfg(not(test))]
const HTTP_REQUEST_TIMEOUT: Duration = Duration::from_secs(5 * 60);
#[cfg(test)]
const HTTP_REQUEST_TIMEOUT: Duration = Duration::from_millis(250);
const MAX_ERROR_RESPONSE_BYTES: u64 = 1024 * 1024;

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
    /// Hosts in the whole cluster: 1 means the degenerate self-only
    /// cluster every node carries, which is not a topology.
    pub hosts: u64,
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
            .timeout(HTTP_REQUEST_TIMEOUT)
            // ClickHouse credentials use custom headers that Reqwest does not
            // classify as sensitive. Following redirects could forward them
            // to another authority, so database requests never redirect.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap_or_default();
        Self { cfg, http }
    }

    pub(super) fn ensure_acceptable_endpoint(&self) -> Result<()> {
        if endpoint_is_acceptable(&self.cfg.url) {
            return Ok(());
        }
        Err(ChError::InsecureTransport(
            "endpoint must be a plain http:// or https:// URL with credentials in the dedicated user and password fields, never in the URL"
                .into(),
        ))
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
        max_result_bytes: u64,
    ) -> Result<QueryResult> {
        let time = max_execution_time_secs.to_string();
        let rows = max_result_rows.to_string();
        let read_bytes = max_bytes_to_read.to_string();
        let result_bytes = max_result_bytes.to_string();
        let body = self
            .request_bounded(
                sql,
                &[
                    ("default_format", "RowBinaryWithNamesAndTypes"),
                    ("max_execution_time", &time),
                    ("max_result_rows", &rows),
                    ("max_result_bytes", &result_bytes),
                    ("result_overflow_mode", "throw"),
                    ("max_bytes_to_read", &read_bytes),
                ],
                max_result_bytes,
            )
            .await?;
        rowbinary::decode(&body)
    }

    /// Run a query and materialize the full typed result.
    ///
    /// Reads prefer the pooled native (TCP) connection when one is up;
    /// the first query kicks off a background connect and rides
    /// HTTP. A native transport or decode failure falls back to HTTP for
    /// this query: only reads route natively ([`crate::native::is_read_statement`]
    /// is a strict allowlist), so a replay is harmless. A server error does
    /// not fall back (the query really ran). Mutating statements never
    /// route natively and are never replayed.
    pub async fn query(&self, sql: &str) -> Result<QueryResult> {
        if crate::native::is_read_statement(sql) {
            if let Some(native) = crate::native::pooled(&self.cfg) {
                match native.query(sql).await {
                    Ok(result) => return Ok(result),
                    Err(error @ ChError::Server { .. }) => return Err(error),
                    // Transport or decode trouble: this read falls back to
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
        if self.ensure_acceptable_endpoint().is_err() {
            return false;
        }
        let url = format!("{}/ping", self.cfg.url.trim_end_matches('/'));
        // ClickHouse does not authenticate /ping. Sending credentials here
        // creates exposure without adding a useful connection check.
        match self.http.get(url).send().await {
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
        self.request_bounded(sql, params, MAX_MATERIALIZED_RESPONSE_BYTES)
            .await
    }

    async fn request_bounded(
        &self,
        sql: &str,
        params: &[(&str, &str)],
        max_response_bytes: u64,
    ) -> Result<Vec<u8>> {
        self.ensure_acceptable_endpoint()?;
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
        let limit = if status.is_success() {
            max_response_bytes
        } else {
            MAX_ERROR_RESPONSE_BYTES
        };
        let bytes = collect_response_bounded(resp, limit).await?;
        if !status.is_success() {
            return Err(ChError::Server {
                code,
                message: String::from_utf8_lossy(&bytes).trim().to_string(),
            });
        }
        Ok(bytes)
    }
}

async fn collect_response_bounded(response: reqwest::Response, limit: u64) -> Result<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|length| length > limit)
    {
        return Err(ChError::ResponseTooLarge { limit });
    }
    let capacity = response
        .content_length()
        .and_then(|length| usize::try_from(length).ok())
        .unwrap_or(0)
        .min(64 * 1024);
    let mut bytes = Vec::with_capacity(capacity);
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        let next = (bytes.len() as u64)
            .checked_add(chunk.len() as u64)
            .ok_or(ChError::ResponseTooLarge { limit })?;
        if next > limit {
            return Err(ChError::ResponseTooLarge { limit });
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

/// Endpoint policy: `https://` and `http://` are both accepted. Many
/// ClickHouse clusters expose no TLS at all, so an explicit `http://`
/// URL is the owner's deliberate plaintext choice (accepted risk on
/// ZCH-002), not a mistake to refuse. URLs that embed credentials are
/// always rejected: they leak into logs, history, and audit trails.
pub(crate) fn endpoint_is_acceptable(endpoint: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(endpoint) else {
        return false;
    };
    if !url.username().is_empty() || url.password().is_some() {
        return false;
    }
    matches!(url.scheme(), "http" | "https") && url.host_str().is_some()
}

/// Whether the native transport may offer plaintext candidates for this
/// configuration. TLS is still tried first; plaintext exists only when
/// the HTTP side is itself explicitly plaintext.
pub(crate) fn endpoint_allows_plaintext(endpoint: &str) -> bool {
    endpoint_is_acceptable(endpoint)
        && reqwest::Url::parse(endpoint).is_ok_and(|url| url.scheme() == "http")
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

#[cfg(test)]
mod security_tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn config(url: String) -> ChConfig {
        ChConfig {
            url,
            user: "security-review".into(),
            password: Some("test-secret".into()),
            database: None,
            read_only: false,
            driver: DriverConfig::default(),
            native_port: None,
        }
    }

    async fn read_request(stream: &mut tokio::net::TcpStream) -> String {
        let mut bytes = vec![0; 8192];
        let length = stream.read(&mut bytes).await.unwrap();
        String::from_utf8_lossy(&bytes[..length]).into_owned()
    }

    #[test]
    fn http_and_https_endpoints_without_url_credentials_are_accepted() {
        assert!(endpoint_is_acceptable("https://db.example.com:8443"));
        assert!(endpoint_is_acceptable("http://db.example.com:8123"));
        assert!(endpoint_is_acceptable("http://localhost:8123"));
        assert!(endpoint_is_acceptable("http://127.0.0.1:8123"));
        assert!(endpoint_is_acceptable("http://[::1]:8123"));
        assert!(!endpoint_is_acceptable("not a URL"));
        assert!(!endpoint_is_acceptable("ftp://db.example.com"));
        assert!(!endpoint_is_acceptable(
            "https://user:secret@db.example.com:8443"
        ));
        assert!(!endpoint_is_acceptable("http://user@db.example.com:8123"));

        assert!(endpoint_allows_plaintext("http://db.example.com:8123"));
        assert!(!endpoint_allows_plaintext("https://db.example.com:8443"));
        assert!(!endpoint_allows_plaintext(
            "http://user:secret@db.example.com:8123"
        ));
    }

    #[tokio::test]
    async fn url_embedded_credentials_are_refused_before_connecting() {
        let client = ChClient::new(config("http://user:secret@192.0.2.1:8123".into()));
        assert!(matches!(
            client.test_connection().await,
            Err(ChError::InsecureTransport(_))
        ));
    }

    #[tokio::test]
    async fn ping_sends_no_clickhouse_credentials() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_request(&mut stream).await;
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOk")
                .await
                .unwrap();
            request
        });

        let client = ChClient::new(config(format!("http://{address}")));
        assert!(client.ping().await);
        let request = server.await.unwrap().to_ascii_lowercase();
        assert!(!request.contains("x-clickhouse-user"));
        assert!(!request.contains("x-clickhouse-key"));
        assert!(!request.contains("test-secret"));
    }

    #[tokio::test]
    async fn database_requests_do_not_follow_redirects() {
        let destination = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let destination_address = destination.local_addr().unwrap();
        let redirector = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let redirector_address = redirector.local_addr().unwrap();

        let redirect = tokio::spawn(async move {
            let (mut stream, _) = redirector.accept().await.unwrap();
            let request = read_request(&mut stream).await;
            let response = format!(
                "HTTP/1.1 302 Found\r\nLocation: http://{destination_address}/capture\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            request
        });

        let client = ChClient::new(config(format!("http://{redirector_address}")));
        assert!(matches!(
            client.test_connection().await,
            Err(ChError::Server { .. })
        ));
        let first_request = redirect.await.unwrap();
        assert!(first_request.contains("test-secret"));
        assert!(
            tokio::time::timeout(Duration::from_millis(200), destination.accept())
                .await
                .is_err(),
            "redirect destination received a credential-bearing request"
        );
    }

    #[tokio::test]
    async fn bounded_requests_stop_before_materializing_large_responses() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let _ = read_request(&mut stream).await;
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 16\r\nConnection: close\r\n\r\n0123456789abcdef",
                )
                .await
                .unwrap();
        });

        let client = ChClient::new(config(format!("http://{address}")));
        let result = client.request_bounded("SELECT 1", &[], 8).await;
        assert!(matches!(
            result,
            Err(ChError::ResponseTooLarge { limit: 8 })
        ));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn stalled_http_peer_hits_the_whole_request_deadline() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let _ = read_request(&mut stream).await;
            tokio::time::sleep(Duration::from_secs(2)).await;
        });

        let client = ChClient::new(config(format!("http://{address}")));
        let started = std::time::Instant::now();
        assert!(!client.ping().await);
        assert!(started.elapsed() < Duration::from_secs(1));
        server.abort();
    }
}
