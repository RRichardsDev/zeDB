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

/// A rustls connector over the platform's root store, pinned to the ring
/// provider: the workspace compiles both ring (via reqwest) and aws-lc
/// (via klickhouse's tokio-rustls default), and an unpinned builder panics
/// when two providers are linked.
fn tls_connector() -> Result<tokio_rustls::TlsConnector> {
    let mut roots = rustls::RootCertStore::empty();
    for cert in rustls_native_certs::load_native_certs().certs {
        let _ = roots.add(cert);
    }
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|error| ChError::NativeTransport(format!("TLS setup failed: {error}")))?
    .with_root_certificates(roots)
    .with_no_client_auth();
    Ok(tokio_rustls::TlsConnector::from(Arc::new(config)))
}

/// Ask the server over HTTP which native ports it actually listens on:
/// `(tcp_port_secure, tcp_port)`. Either is `None` when unset or when the
/// server predates `getServerPort`; callers fall back to 9440/9000.
async fn discovered_native_ports(http: &crate::ChClient) -> (Option<u16>, Option<u16>) {
    let port = |result: Result<QueryResult>| {
        result
            .ok()
            .and_then(|result| result.rows.into_iter().next())
            .and_then(|row| row.into_iter().next())
            .and_then(|value| match value {
                Value::UInt(port) => u16::try_from(port).ok(),
                Value::Int(port) => u16::try_from(port).ok(),
                _ => None,
            })
    };
    // Two statements: getServerPort throws for a port that isn't
    // configured, and a secure-only or plain-only server is normal.
    let secure = port(
        http.query_http("SELECT getServerPort('tcp_port_secure')")
            .await,
    );
    let plain = port(http.query_http("SELECT getServerPort('tcp_port')").await);
    (secure, plain)
}

/// The host of a ClickHouse HTTP URL (`http(s)://host:port/...` -> `host`).
pub fn host_of(url: &str) -> Option<String> {
    let after_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let authority = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(after_scheme);
    let host_port = authority
        .rsplit_once('@')
        .map(|(_, host)| host)
        .unwrap_or(authority);
    let host = host_port
        .rsplit_once(':')
        .map(|(host, _)| host)
        .unwrap_or(host_port);
    (!host.is_empty()).then(|| host.to_string())
}

/// A `SET` right-hand side: numeric values go bare, anything else quoted.
fn setting_literal(value: &str) -> String {
    if value.parse::<f64>().is_ok() {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
    }
}

fn map_err(error: klickhouse::KlickhouseError) -> ChError {
    match error {
        klickhouse::KlickhouseError::ServerException { code, message, .. } => ChError::Server {
            code: Some(code),
            message,
        },
        other => ChError::NativeTransport(other.to_string()),
    }
}

fn block_columns(block: &klickhouse::block::Block) -> Vec<ColumnMeta> {
    block
        .column_types
        .iter()
        .map(|(name, type_)| ColumnMeta {
            name: name.clone(),
            type_name: type_.to_string(),
        })
        .collect()
}

/// Column-major block data to row-major [`Value`] rows, appended.
fn append_block_rows(block: klickhouse::block::Block, rows: &mut Vec<Vec<Value>>) {
    let row_count = block.rows as usize;
    if row_count == 0 {
        return;
    }
    let types: Vec<klickhouse::Type> = block.column_types.values().cloned().collect();
    let mut columns: Vec<std::vec::IntoIter<klickhouse::Value>> = block
        .column_data
        .into_iter()
        .map(|(_, data)| data.into_iter())
        .collect();
    for _ in 0..row_count {
        let mut row = Vec::with_capacity(columns.len());
        for (index, column) in columns.iter_mut().enumerate() {
            let value = column.next().unwrap_or(klickhouse::Value::Null);
            row.push(map_value(types.get(index), value));
        }
        rows.push(row);
    }
}

/// Peel Nullable/LowCardinality wrappers to the value-shaped inner type.
fn strip_wrappers(type_: &klickhouse::Type) -> &klickhouse::Type {
    match type_ {
        klickhouse::Type::Nullable(inner) | klickhouse::Type::LowCardinality(inner) => {
            strip_wrappers(inner)
        }
        other => other,
    }
}

/// One klickhouse value into the driver-agnostic model, mirroring the
/// RowBinary decoder's normalizations (ints widen to Int/UInt, enums
/// resolve to their names, non-UTF-8 strings become bytes).
fn map_value(type_: Option<&klickhouse::Type>, value: klickhouse::Value) -> Value {
    use klickhouse::Value as KV;
    let type_ = type_.map(strip_wrappers);
    match value {
        KV::Null => Value::Null,
        KV::Int8(v) => Value::Int(v.into()),
        KV::Int16(v) => Value::Int(v.into()),
        KV::Int32(v) => Value::Int(v.into()),
        KV::Int64(v) => Value::Int(v),
        KV::Int128(v) => Value::Int128(v),
        KV::Int256(v) => Value::String(v.to_string()),
        KV::UInt8(v) => Value::UInt(v.into()),
        KV::UInt16(v) => Value::UInt(v.into()),
        KV::UInt32(v) => Value::UInt(v.into()),
        KV::UInt64(v) => Value::UInt(v),
        KV::UInt128(v) => Value::UInt128(v),
        KV::UInt256(v) => Value::String(v.to_string()),
        KV::Float32(v) => Value::Float(v.into()),
        KV::Float64(v) => Value::Float(v),
        KV::BFloat16(v) => Value::Float(v.to_f32().into()),
        KV::Decimal32(scale, v) => Value::Decimal {
            value: v.into(),
            scale: scale as u8,
        },
        KV::Decimal64(scale, v) => Value::Decimal {
            value: v.into(),
            scale: scale as u8,
        },
        KV::Decimal128(scale, v) => Value::Decimal {
            value: v,
            scale: scale as u8,
        },
        KV::Decimal256(_, v) => Value::String(v.to_string()),
        KV::String(bytes) => match String::from_utf8(bytes) {
            Ok(text) => Value::String(text),
            Err(error) => Value::Bytes(error.into_bytes()),
        },
        KV::Uuid(uuid) => Value::Uuid(*uuid.as_bytes()),
        KV::Date(date) => Value::Date(date.into()),
        KV::DateTime(dt) => match chrono::DateTime::<chrono::Utc>::try_from(dt) {
            Ok(utc) => Value::DateTime(utc),
            Err(_) => Value::Null,
        },
        KV::DateTime64(dt) => match chrono::DateTime::<chrono::Utc>::try_from(dt) {
            Ok(utc) => Value::DateTime(utc),
            Err(_) => Value::Null,
        },
        KV::Enum8(v) => enum_name(type_, v.into()).unwrap_or(Value::Int(v.into())),
        KV::Enum16(v) => enum_name(type_, v).unwrap_or(Value::Int(v.into())),
        KV::Array(items) => {
            let inner = type_.and_then(|t| t.unarray());
            Value::Array(
                items
                    .into_iter()
                    .map(|item| map_value(inner, item))
                    .collect(),
            )
        }
        KV::Tuple(items) => {
            let inners: Option<&Vec<klickhouse::Type>> = match type_ {
                Some(klickhouse::Type::Tuple(inners)) => Some(inners),
                _ => None,
            };
            Value::Tuple(
                items
                    .into_iter()
                    .enumerate()
                    .map(|(index, item)| {
                        map_value(inners.and_then(|inners| inners.get(index)), item)
                    })
                    .collect(),
            )
        }
        KV::Map(keys, values) => {
            let kv = type_.and_then(|t| t.unmap());
            Value::Map(
                keys.into_iter()
                    .zip(values)
                    .map(|(key, value)| {
                        (
                            map_value(kv.map(|(k, _)| k), key),
                            map_value(kv.map(|(_, v)| v), value),
                        )
                    })
                    .collect(),
            )
        }
        KV::Ipv4(ip) => Value::Ipv4(ip.into()),
        KV::Ipv6(ip) => Value::Ipv6(ip.into()),
        geo @ (KV::Point(_) | KV::Ring(_) | KV::Polygon(_) | KV::MultiPolygon(_)) => {
            Value::String(format!("{geo:?}"))
        }
    }
}

/// Resolve an enum discriminant to its symbolic name via the column type.
fn enum_name(type_: Option<&klickhouse::Type>, v: i16) -> Option<Value> {
    match type_ {
        Some(klickhouse::Type::Enum8(entries)) => entries
            .iter()
            .find(|(_, ev)| i16::from(*ev) == v)
            .map(|(name, _)| Value::Enum(name.clone())),
        Some(klickhouse::Type::Enum16(entries)) => entries
            .iter()
            .find(|(_, ev)| *ev == v)
            .map(|(name, _)| Value::Enum(name.clone())),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Connection pool
//
// `ChClient` is constructed fresh per operation all over the app, so the
// long-lived native connection lives in a process-wide pool keyed by the
// connection's identity. `pooled` never blocks a query on a TCP+TLS
// handshake: the first call kicks off a background connect and the caller
// rides HTTP until the connection is ready.
// ---------------------------------------------------------------------------

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

enum PoolEntry {
    /// A background connect is in flight; callers keep using HTTP.
    Connecting,
    Ready(NativeClient),
    /// Last connect failed; retried after [`RETRY_AFTER`].
    Failed(Instant),
}

/// How long a failed native connect suppresses reconnect attempts, so a
/// host with no native port doesn't pay a probe per query.
const RETRY_AFTER: Duration = Duration::from_secs(300);

static POOL: OnceLock<Mutex<HashMap<String, PoolEntry>>> = OnceLock::new();

fn pool() -> &'static Mutex<HashMap<String, PoolEntry>> {
    POOL.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Kill switch: `ZEDB_NATIVE=0` keeps everything on HTTP.
fn disabled() -> bool {
    std::env::var("ZEDB_NATIVE").is_ok_and(|value| value.trim() == "0")
}

/// The connection identity a pooled socket serves. Everything that shapes
/// the session is part of the key, so a settings or credentials change
/// gets its own connection.
fn pool_key(cfg: &ChConfig) -> Option<String> {
    let host = host_of(&cfg.url)?;
    let settings: Vec<String> = cfg
        .driver
        .settings
        .iter()
        .map(|setting| format!("{}={}", setting.name.trim(), setting.value.trim()))
        .collect();
    Some(format!(
        "{host}|{}|{}|{}|{}|{}",
        cfg.user,
        cfg.password.as_deref().unwrap_or(""),
        cfg.database.as_deref().unwrap_or(""),
        cfg.read_only,
        settings.join(",")
    ))
}

/// A ready pooled connection for this config, if one exists. The first
/// call (and the first after a closed socket or an expired failure) starts
/// a background connect and returns `None`; requires a tokio runtime
/// context to do so.
pub fn pooled(cfg: &ChConfig) -> Option<NativeClient> {
    if disabled() {
        return None;
    }
    let key = pool_key(cfg)?;
    let mut entries = pool().lock().ok()?;
    match entries.get(&key) {
        Some(PoolEntry::Ready(client)) if !client.is_closed() => return Some(client.clone()),
        Some(PoolEntry::Connecting) => return None,
        Some(PoolEntry::Failed(at)) if at.elapsed() < RETRY_AFTER => return None,
        _ => {}
    }
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return None;
    };
    entries.insert(key.clone(), PoolEntry::Connecting);
    drop(entries);
    let cfg = cfg.clone();
    handle.spawn(async move {
        let entry = match NativeClient::connect(&cfg).await {
            Ok(client) => PoolEntry::Ready(client),
            Err(_) => PoolEntry::Failed(Instant::now()),
        };
        if let Ok(mut entries) = pool().lock() {
            entries.insert(key, entry);
        }
    });
    None
}

/// Drop this config's pooled connection (after a transport error), so the
/// next query starts a fresh background connect instead of reusing a
/// broken socket.
pub fn evict(cfg: &ChConfig) {
    if let Some(key) = pool_key(cfg) {
        if let Ok(mut entries) = pool().lock() {
            entries.remove(&key);
        }
    }
}

/// The pooled connection, established in the foreground if need be: the
/// tail's instant-updates path wants a definite answer, not best-effort.
pub async fn connect_pooled(cfg: &ChConfig) -> Result<NativeClient> {
    if disabled() {
        return Err(ChError::NativeTransport(
            "native transport disabled (ZEDB_NATIVE=0)".into(),
        ));
    }
    let key = pool_key(cfg)
        .ok_or_else(|| ChError::NativeTransport(format!("no host in URL {:?}", cfg.url)))?;
    if let Ok(entries) = pool().lock() {
        if let Some(PoolEntry::Ready(client)) = entries.get(&key) {
            if !client.is_closed() {
                return Ok(client.clone());
            }
        }
    }
    match NativeClient::connect(cfg).await {
        Ok(client) => {
            if let Ok(mut entries) = pool().lock() {
                entries.insert(key, PoolEntry::Ready(client.clone()));
            }
            Ok(client)
        }
        Err(error) => {
            if let Ok(mut entries) = pool().lock() {
                entries.insert(key, PoolEntry::Failed(Instant::now()));
            }
            Err(error)
        }
    }
}

/// Whether a statement is safe to route over the native read path: reads
/// re-run harmlessly on HTTP fallback, anything else must not run twice.
pub fn is_read_statement(sql: &str) -> bool {
    let first = sql
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_uppercase();
    matches!(
        first.as_str(),
        "SELECT" | "WITH" | "SHOW" | "DESCRIBE" | "DESC" | "EXPLAIN" | "EXISTS"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_statements_are_recognized() {
        assert!(is_read_statement("  select 1"));
        assert!(is_read_statement("WITH x AS (SELECT 1) SELECT * FROM x"));
        assert!(is_read_statement("SHOW TABLES"));
        assert!(is_read_statement("EXPLAIN SELECT 1"));
        assert!(!is_read_statement("INSERT INTO t VALUES (1)"));
        assert!(!is_read_statement("ALTER TABLE t DROP COLUMN c"));
        assert!(!is_read_statement("SET readonly = 0"));
        assert!(!is_read_statement(""));
    }

    #[test]
    fn host_of_strips_scheme_port_and_credentials() {
        assert_eq!(host_of("http://localhost:8123").unwrap(), "localhost");
        assert_eq!(
            host_of("https://user:pw@ch.example.com:8443/path?x=1").unwrap(),
            "ch.example.com"
        );
        assert!(host_of("http://").is_none());
    }

    #[test]
    fn setting_literals_quote_non_numeric_values() {
        assert_eq!(setting_literal("300"), "300");
        assert_eq!(setting_literal("0.5"), "0.5");
        assert_eq!(setting_literal("break"), "'break'");
        assert_eq!(setting_literal("a'b"), "'a\\'b'");
    }

    #[test]
    fn values_map_to_the_driver_model() {
        use klickhouse::Value as KV;
        assert_eq!(map_value(None, KV::Int32(-5)), Value::Int(-5));
        assert_eq!(map_value(None, KV::UInt8(7)), Value::UInt(7));
        assert_eq!(
            map_value(None, KV::String(b"hi".to_vec())),
            Value::String("hi".into())
        );
        assert_eq!(
            map_value(None, KV::String(vec![0xff, 0xfe])),
            Value::Bytes(vec![0xff, 0xfe])
        );
        assert_eq!(
            map_value(None, KV::Decimal64(4, 123456)),
            Value::Decimal {
                value: 123456,
                scale: 4
            }
        );
        // Nullable/LowCardinality wrappers peel off for enum resolution.
        let enum_type = klickhouse::Type::Nullable(Box::new(klickhouse::Type::Enum8(vec![(
            "ok".to_string(),
            1i8,
        )])));
        assert_eq!(
            map_value(Some(&enum_type), KV::Enum8(1)),
            Value::Enum("ok".into())
        );
        assert_eq!(map_value(Some(&enum_type), KV::Enum8(9)), Value::Int(9));
    }

    #[test]
    fn arrays_and_maps_map_recursively() {
        use klickhouse::Value as KV;
        let array_type = klickhouse::Type::Array(Box::new(klickhouse::Type::UInt8));
        assert_eq!(
            map_value(
                Some(&array_type),
                KV::Array(vec![KV::UInt8(1), KV::UInt8(2)])
            ),
            Value::Array(vec![Value::UInt(1), Value::UInt(2)])
        );
        let map_type = klickhouse::Type::Map(
            Box::new(klickhouse::Type::String),
            Box::new(klickhouse::Type::UInt8),
        );
        assert_eq!(
            map_value(
                Some(&map_type),
                KV::Map(vec![KV::String(b"a".to_vec())], vec![KV::UInt8(1)])
            ),
            Value::Map(vec![(Value::String("a".into()), Value::UInt(1))])
        );
    }
}
