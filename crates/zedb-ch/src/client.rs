//! Minimal ClickHouse HTTP client.
//!
//! Schema queries can be materialized, while result queries can be decoded in
//! incremental batches as their HTTP body arrives.

use futures_util::StreamExt;
use zedb_core::QueryResult;
use zedb_core::{ColumnMeta, Value};

use crate::error::{ChError, Result};
use crate::rowbinary;

#[derive(Debug, Clone)]
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
}

pub struct ChClient {
    cfg: ChConfig,
    http: reqwest::Client,
}

#[derive(Debug)]
pub enum QueryStreamEvent {
    Columns(Vec<ColumnMeta>),
    Rows(Vec<Vec<Value>>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueryStreamSummary {
    pub rows: usize,
    pub capped: bool,
}

impl ChClient {
    pub fn new(cfg: ChConfig) -> Self {
        Self {
            cfg,
            http: reqwest::Client::new(),
        }
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
        request = request.query(&[("default_format", "RowBinaryWithNamesAndTypes")]);

        let response = request.send().await?;
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
        let mut body = response.bytes_stream();
        while let Some(chunk) = body.next().await {
            let mut rows = decoder.push(&chunk?)?;
            if !sent_columns {
                if let Some(columns) = decoder.columns() {
                    on_event(QueryStreamEvent::Columns(columns.to_vec()));
                    sent_columns = true;
                }
            }
            if !rows.is_empty() {
                let remaining = row_limit.saturating_sub(sent_rows);
                if rows.len() > remaining {
                    rows.truncate(remaining);
                    if !rows.is_empty() {
                        sent_rows += rows.len();
                        on_event(QueryStreamEvent::Rows(rows));
                    }
                    return Ok(QueryStreamSummary {
                        rows: sent_rows,
                        capped: true,
                    });
                }
                sent_rows += rows.len();
                on_event(QueryStreamEvent::Rows(rows));
            }
        }
        decoder.finish()?;
        Ok(QueryStreamSummary {
            rows: sent_rows,
            capped: false,
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
