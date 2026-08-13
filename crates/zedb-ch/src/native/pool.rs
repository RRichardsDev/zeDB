use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::*;

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
