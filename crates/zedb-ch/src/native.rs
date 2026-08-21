//! Native (TCP) protocol transport.
//!
//! A persistent connection over ClickHouse's native protocol (9440 TLS,
//! with 9000 plaintext offered only for an explicitly plaintext `http://`
//! endpoint) via `klickhouse`, decoded into the same
//! driver-agnostic [`zedb_core::Value`] model the HTTP path produces. Reads
//! route here when a pooled connection exists ([`crate::ChClient::query`]);
//! anything mutating stays on HTTP, where a failure is unambiguous.
//!
//! Session posture mirrors HTTP: `readonly = 2` for read-only connections
//! and the cluster's driver settings are applied with `SET` right after the
//! handshake, so safety is still enforced server-side.

use std::sync::Arc;

use futures_util::StreamExt;
use tokio::io::{AsyncRead, AsyncWrite};
use zedb_core::{ColumnMeta, QueryResult, Value};

use crate::error::{ChError, Result};
use crate::ChConfig;

const NATIVE_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
const NATIVE_QUERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5 * 60);
const NATIVE_STREAM_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

mod codec;
mod pool;

pub use codec::host_of;
#[cfg(test)]
use codec::map_value;
use codec::{
    append_block_rows, block_columns, discovered_http_offset, discovered_native_ports, map_err,
    setting_literal, tls_connector,
};
pub use pool::{connect_pooled, evict, is_read_statement, pooled};

/// Which native endpoint a connection landed on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeEndpoint {
    /// Port 9440, native protocol over TLS (rustls).
    Tls,
    /// Port 9000, plaintext.
    Plain,
}

/// A live native-protocol connection. Cloning shares the underlying
/// socket (the inner client is a channel handle onto one connection task).
#[derive(Clone)]
pub struct NativeClient {
    inner: klickhouse::Client,
    transport: Arc<NativeTransport>,
    pub endpoint: NativeEndpoint,
}

/// Owns the real socket behind KlickHouse's client task. KlickHouse does not
/// expose its protocol-level Cancel packet, so stopping a long-lived query
/// closes this proxy and therefore the server connection immediately.
struct NativeTransport {
    abort: tokio::task::AbortHandle,
}

impl Drop for NativeTransport {
    fn drop(&mut self) {
        self.abort.abort();
    }
}

impl NativeClient {
    /// Connect to the native port of the HTTP config's host: the server's
    /// own `getServerPort('tcp_port_secure')` over TLS. Plain `tcp_port` is
    /// considered only when the HTTP endpoint is itself explicit plaintext
    /// `http://` (defaulting to 9440/9000 when discovery fails). Applies
    /// `readonly = 2` for read-only
    /// connections and the driver settings before returning, so a handed-out
    /// client is never missing the session posture.
    pub async fn connect(cfg: &ChConfig) -> Result<Self> {
        tokio::time::timeout(NATIVE_CONNECT_TIMEOUT, Self::connect_inner(cfg))
            .await
            .map_err(|_| ChError::NativeTransport("native connection deadline exceeded".into()))?
    }

    async fn connect_inner(cfg: &ChConfig) -> Result<Self> {
        let host = host_of(&cfg.url)
            .ok_or_else(|| ChError::NativeTransport(format!("no host in URL {:?}", cfg.url)))?;
        let options = klickhouse::ClientOptions {
            username: cfg.user.clone(),
            password: cfg.password.clone().unwrap_or_default(),
            default_database: cfg.database.clone().unwrap_or_default(),
            tcp_nodelay: true,
        };

        let http = crate::ChClient::new(cfg.clone());
        // The native port may be a guess, and another ClickHouse may be
        // listening there (port-forwards, side-by-side clusters, a stray
        // clickhouse-local). Every candidate below must prove it belongs
        // to the same server as the HTTP endpoint before being handed out.
        let http_uuid = http.server_uuid_http().await.map_err(|error| {
            ChError::NativeTransport(format!("could not verify server identity: {error}"))
        })?;
        let (secure_port, plain_port) = discovered_native_ports(&http).await;
        let secure_port = secure_port.unwrap_or(9440);
        let plain_port = plain_port.unwrap_or(9000);
        // The server advertises its own ports, but a remapped deployment
        // (docker publish, port-forward) reaches them elsewhere. The HTTP
        // remap offset is measurable, so shifted candidates go first; the
        // identity check decides what actually answered.
        let offset = discovered_http_offset(&http, &cfg.url).await;
        let candidates = native_candidates(cfg, secure_port, plain_port, offset);
        let mut failures = Vec::new();
        let mut chosen = None;
        for (endpoint, port) in candidates {
            let connected = match endpoint {
                NativeEndpoint::Tls => Self::connect_tls(&host, port, options.clone()).await,
                NativeEndpoint::Plain => Self::connect_plain(&host, port, options.clone()).await,
            };
            let (inner, transport) = match connected {
                Ok(connected) => connected,
                Err(error) => {
                    failures.push(format!("{port}: {error}"));
                    continue;
                }
            };
            let client = NativeClient {
                inner,
                transport,
                endpoint,
            };
            let native_uuid = match client.query("SELECT serverUUID()").await {
                Ok(result) => result
                    .rows
                    .first()
                    .and_then(|row| row.first())
                    .map(ToString::to_string),
                Err(error) => {
                    failures.push(format!("{port}: {error}"));
                    continue;
                }
            };
            if native_uuid.as_deref() != Some(http_uuid.as_str()) {
                failures.push(format!(
                    "{port}: answers as a different server than the HTTP endpoint"
                ));
                continue;
            }
            chosen = Some(client);
            break;
        }
        let Some(client) = chosen else {
            return Err(ChError::NativeTransport(format!(
                "no native port on {host}: {}",
                failures.join("; ")
            )));
        };
        client.apply_session_settings(cfg).await?;
        Ok(client)
    }

    async fn connect_tls(
        host: &str,
        port: u16,
        options: klickhouse::ClientOptions,
    ) -> std::result::Result<(klickhouse::Client, Arc<NativeTransport>), String> {
        let name = rustls_pki_types::ServerName::try_from(host.to_string())
            .map_err(|error| format!("bad TLS server name: {error}"))?;
        let connector = tls_connector().map_err(|error| error.to_string())?;
        let stream = tokio::net::TcpStream::connect((host, port))
            .await
            .map_err(|error| error.to_string())?;
        stream
            .set_nodelay(options.tcp_nodelay)
            .map_err(|error| error.to_string())?;
        let stream = connector
            .connect(name, stream)
            .await
            .map_err(|error| error.to_string())?;
        Self::connect_transport(stream, options).await
    }

    async fn connect_plain(
        host: &str,
        port: u16,
        options: klickhouse::ClientOptions,
    ) -> std::result::Result<(klickhouse::Client, Arc<NativeTransport>), String> {
        let stream = tokio::net::TcpStream::connect((host, port))
            .await
            .map_err(|error| error.to_string())?;
        stream
            .set_nodelay(options.tcp_nodelay)
            .map_err(|error| error.to_string())?;
        Self::connect_transport(stream, options).await
    }

    async fn connect_transport<S>(
        mut stream: S,
        options: klickhouse::ClientOptions,
    ) -> std::result::Result<(klickhouse::Client, Arc<NativeTransport>), String>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let (client_stream, mut proxy_stream) = tokio::io::duplex(64 * 1024);
        let proxy = tokio::spawn(async move {
            let _ = tokio::io::copy_bidirectional(&mut proxy_stream, &mut stream).await;
        });
        let transport = Arc::new(NativeTransport {
            abort: proxy.abort_handle(),
        });
        let (reader, writer) = tokio::io::split(client_stream);
        let inner = klickhouse::Client::connect_stream(reader, writer, options)
            .await
            .map_err(|error| error.to_string())?;
        Ok((inner, transport))
    }

    /// The session-level counterpart of the HTTP path's query-string
    /// params: read-only enforcement and driver settings, server-side.
    async fn apply_session_settings(&self, cfg: &ChConfig) -> Result<()> {
        for setting in &cfg.driver.settings {
            let name = setting.name.trim();
            let value = setting.value.trim();
            // connect_timeout shapes connection setup, not the session.
            if name.is_empty() || value.is_empty() || name == "connect_timeout" {
                continue;
            }
            if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                continue;
            }
            // Tolerate settings the server rejects (unknown name, wrong
            // scope); the HTTP path would fail per-query instead.
            let _ = self
                .inner
                .execute(format!("SET {name} = {}", setting_literal(value)))
                .await;
        }
        if cfg.read_only {
            // Must land: it is the safety posture. readonly=2 still allows
            // query-level settings but rejects writes and DDL.
            self.inner
                .execute("SET readonly = 2")
                .await
                .map_err(map_err)?;
        }
        Ok(())
    }

    /// Whether the underlying connection task has exited (socket gone).
    pub fn is_closed(&self) -> bool {
        self.transport.abort.is_finished() || self.inner.is_closed()
    }

    /// Close the native socket. Long-lived query users call this when their
    /// consumer stops so ClickHouse does not retain the server-side query.
    pub fn close(&self) {
        self.transport.abort.abort();
    }

    /// Run a query and materialize the full typed result, mirroring
    /// [`crate::ChClient::query`].
    pub async fn query(&self, sql: &str) -> Result<QueryResult> {
        match tokio::time::timeout(NATIVE_QUERY_TIMEOUT, async {
            let mut stream = self.inner.query_raw(sql).await.map_err(map_err)?;
            let mut columns: Vec<ColumnMeta> = Vec::new();
            let mut rows: Vec<Vec<Value>> = Vec::new();
            while let Some(block) = stream.next().await {
                let block = block.map_err(map_err)?;
                if columns.is_empty() && !block.column_types.is_empty() {
                    columns = block_columns(&block);
                }
                append_block_rows(block, &mut rows);
            }
            Ok(QueryResult { columns, rows })
        })
        .await
        {
            Ok(result) => result,
            Err(_) => {
                self.close();
                Err(ChError::NativeTransport(
                    "native query deadline exceeded; connection closed".into(),
                ))
            }
        }
    }

    /// Run a long-lived streaming query (e.g. `WATCH ... EVENTS`), invoking
    /// `on_block` per data block until the server ends the stream, the
    /// connection drops, or `on_block` returns `false`.
    pub async fn stream_blocks(
        &self,
        sql: &str,
        mut on_block: impl FnMut(Vec<ColumnMeta>, Vec<Vec<Value>>) -> bool,
    ) -> Result<()> {
        let mut stream = self.inner.query_raw(sql).await.map_err(map_err)?;
        loop {
            let block = tokio::time::timeout(NATIVE_STREAM_IDLE_TIMEOUT, stream.next())
                .await
                .map_err(|_| {
                    self.close();
                    ChError::NativeTransport("native stream idle deadline exceeded".into())
                })?;
            let Some(block) = block else {
                break;
            };
            let block = block.map_err(map_err)?;
            if block.rows == 0 {
                continue;
            }
            let columns = block_columns(&block);
            let mut rows = Vec::new();
            append_block_rows(block, &mut rows);
            if !on_block(columns, rows) {
                self.close();
                return Ok(());
            }
        }
        Ok(())
    }

    /// Run a statement, discarding output. Only used for session-scoped
    /// statements the tail needs (live view DDL); the app's general
    /// mutating path stays on HTTP.
    pub async fn execute(&self, sql: &str) -> Result<()> {
        match tokio::time::timeout(NATIVE_QUERY_TIMEOUT, self.inner.execute(sql)).await {
            Ok(result) => result.map_err(map_err),
            Err(_) => {
                self.close();
                Err(ChError::NativeTransport(
                    "native execute deadline exceeded; connection closed".into(),
                ))
            }
        }
    }
}

fn native_candidates(
    cfg: &ChConfig,
    secure_port: u16,
    plain_port: u16,
    offset: i32,
) -> Vec<(NativeEndpoint, u16)> {
    let allow_plain = crate::client::endpoint_allows_plaintext(&cfg.url);
    let shifted = |port: u16| {
        u16::try_from(i32::from(port) + offset)
            .ok()
            .filter(|shifted| *shifted != 0 && *shifted != port)
    };
    let mut candidates = Vec::new();
    if let Some(port) = cfg.native_port {
        // An explicitly configured port is configuration, not a guess. TLS
        // goes first, and plaintext exists only when the HTTP endpoint is
        // itself explicit plaintext.
        candidates.push((NativeEndpoint::Tls, port));
        if allow_plain {
            candidates.push((NativeEndpoint::Plain, port));
        }
    }
    for port in [shifted(secure_port), Some(secure_port)]
        .into_iter()
        .flatten()
    {
        candidates.push((NativeEndpoint::Tls, port));
    }
    if allow_plain {
        for port in [shifted(plain_port), Some(plain_port)]
            .into_iter()
            .flatten()
        {
            candidates.push((NativeEndpoint::Plain, port));
        }
    }
    candidates.dedup();
    candidates
}

#[cfg(test)]
mod security_tests {
    use super::*;

    fn config(url: &str) -> ChConfig {
        ChConfig {
            url: url.into(),
            user: "security-review".into(),
            password: Some("test-secret".into()),
            database: None,
            read_only: false,
            driver: Default::default(),
            native_port: Some(9441),
        }
    }

    #[test]
    fn plaintext_native_candidates_require_an_explicit_plaintext_endpoint() {
        for endpoint in [
            "https://db.example.com:8443",
            "http://user:secret@db.example.com:8123",
        ] {
            let candidates = native_candidates(&config(endpoint), 9440, 9000, 0);
            assert!(candidates
                .iter()
                .all(|(transport, _)| *transport == NativeEndpoint::Tls));
        }

        // An explicit http:// endpoint is the owner's plaintext choice;
        // the native side may then offer plaintext too, TLS still first.
        for endpoint in ["http://db.example.com:8123", "http://127.0.0.1:8123"] {
            let candidates = native_candidates(&config(endpoint), 9440, 9000, 0);
            assert_eq!(candidates.first(), Some(&(NativeEndpoint::Tls, 9441)));
            assert!(candidates
                .iter()
                .any(|(transport, _)| *transport == NativeEndpoint::Plain));
        }
    }
}
