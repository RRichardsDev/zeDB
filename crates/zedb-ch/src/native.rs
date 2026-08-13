//! Native (TCP) protocol transport, Phase 10.
//!
//! A persistent connection over ClickHouse's native protocol (9440 TLS
//! preferred, then 9000 plaintext) via `klickhouse`, decoded into the same
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

mod codec;
mod pool;

pub use codec::host_of;
#[cfg(test)]
use codec::map_value;
use codec::{
    append_block_rows, block_columns, discovered_native_ports, map_err, setting_literal,
    tls_connector,
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
    /// own `getServerPort('tcp_port_secure')` over TLS first, then
    /// `tcp_port` plaintext (defaulting to 9440/9000 when discovery
    /// fails). Applies `readonly = 2` (for read-only connections) and the
    /// driver settings before returning, so a handed-out client is never
    /// missing the session posture.
    pub async fn connect(cfg: &ChConfig) -> Result<Self> {
        let host = host_of(&cfg.url)
            .ok_or_else(|| ChError::NativeTransport(format!("no host in URL {:?}", cfg.url)))?;
        let options = klickhouse::ClientOptions {
            username: cfg.user.clone(),
            password: cfg.password.clone().unwrap_or_default(),
            default_database: cfg.database.clone().unwrap_or_default(),
            tcp_nodelay: true,
        };

        let http = crate::ChClient::new(cfg.clone());
        let (secure_port, plain_port) = discovered_native_ports(&http).await;
        let secure_port = secure_port.unwrap_or(9440);
        let plain_port = plain_port.unwrap_or(9000);
        let client = match Self::connect_tls(&host, secure_port, options.clone()).await {
            Ok((inner, transport)) => NativeClient {
                inner,
                transport,
                endpoint: NativeEndpoint::Tls,
            },
            Err(tls_error) => match Self::connect_plain(&host, plain_port, options).await {
                Ok((inner, transport)) => NativeClient {
                    inner,
                    transport,
                    endpoint: NativeEndpoint::Plain,
                },
                Err(plain_error) => {
                    return Err(ChError::NativeTransport(format!(
                        "no native port on {host}: {secure_port}: {tls_error}; \
                             {plain_port}: {plain_error}"
                    )))
                }
            },
        };
        // The native port may be a guess, and another ClickHouse may be
        // listening there (port-forwards, side-by-side clusters, a stray
        // clickhouse-local). Prove the socket belongs to the same server
        // as the HTTP endpoint before handing it out.
        let http_uuid = http.server_uuid_http().await.map_err(|error| {
            ChError::NativeTransport(format!("could not verify server identity: {error}"))
        })?;
        let native_uuid = client
            .query("SELECT serverUUID()")
            .await?
            .rows
            .first()
            .and_then(|row| row.first())
            .map(ToString::to_string);
        if native_uuid.as_deref() != Some(http_uuid.as_str()) {
            return Err(ChError::NativeTransport(format!(
                "native port {} answers as a different server than the HTTP endpoint",
                match client.endpoint {
                    NativeEndpoint::Tls => secure_port,
                    NativeEndpoint::Plain => plain_port,
                }
            )));
        }
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
        while let Some(block) = stream.next().await {
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
        self.inner.execute(sql).await.map_err(map_err)
    }
}
