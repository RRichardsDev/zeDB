use std::collections::HashMap;
use std::hash::{BuildHasher, Hasher};
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

/// `ChClient` is constructed fresh per operation all over the app, so the
/// long-lived native connection has to outlive any one client: it lives
/// here, process-wide, keyed by the connection's identity.
static POOL: OnceLock<Mutex<HashMap<String, PoolEntry>>> = OnceLock::new();
static POOL_KEY_SALT: OnceLock<[u8; 32]> = OnceLock::new();

fn pool() -> &'static Mutex<HashMap<String, PoolEntry>> {
    POOL.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Kill switch: `ZEDB_NATIVE=0` keeps everything on HTTP.
fn disabled() -> bool {
    std::env::var("ZEDB_NATIVE").is_ok_and(|value| value.trim() == "0")
}

fn pool_key_salt() -> &'static [u8; 32] {
    POOL_KEY_SALT.get_or_init(|| {
        let state = std::collections::hash_map::RandomState::new();
        let mut salt = [0u8; 32];
        for (index, chunk) in salt.chunks_exact_mut(8).enumerate() {
            let mut hasher = state.build_hasher();
            hasher.write_usize(index);
            chunk.copy_from_slice(&hasher.finish().to_le_bytes());
        }
        salt
    })
}

fn session_digest(cfg: &ChConfig) -> String {
    use sha2::{Digest as _, Sha256};

    let mut digest = Sha256::new();
    digest.update(pool_key_salt());
    digest.update(cfg.password.as_deref().unwrap_or("").as_bytes());
    for setting in &cfg.driver.settings {
        digest.update([0]);
        digest.update(setting.name.trim().as_bytes());
        digest.update([b'=']);
        digest.update(setting.value.trim().as_bytes());
    }
    format!("{:x}", digest.finalize())
}

/// The connection identity a pooled socket serves. Everything that shapes
/// the session is part of the key, so a settings or credentials change
/// gets its own connection. The key carries the full URL, not just the
/// host: two nodes of one cluster reached through different ports on the
/// same host must never share a socket, or one node's reads silently
/// execute on the other.
fn pool_key(cfg: &ChConfig) -> Option<String> {
    let endpoint = cfg.url.trim();
    if endpoint.is_empty() {
        return None;
    }
    Some(format!(
        "{endpoint}|{:?}|{}|{}|{}|{}",
        cfg.native_port,
        cfg.user,
        cfg.database.as_deref().unwrap_or(""),
        cfg.read_only,
        session_digest(cfg)
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
    fn pool_key_separates_nodes_on_the_same_host() {
        let node = |url: &str| crate::ChConfig {
            url: url.into(),
            user: "zedb".into(),
            password: None,
            database: None,
            read_only: false,
            driver: Default::default(),
            native_port: None,
        };
        let one = pool_key(&node("http://localhost:8123")).unwrap();
        let two = pool_key(&node("http://localhost:8124")).unwrap();
        assert_ne!(one, two);
        assert!(pool_key(&node("  ")).is_none());
    }

    #[test]
    fn pool_key_does_not_retain_passwords_or_setting_values() {
        let mut cfg = crate::ChConfig {
            url: "http://localhost:8123".into(),
            user: "zedb".into(),
            password: Some("super-secret-password".into()),
            database: None,
            read_only: false,
            driver: Default::default(),
            native_port: None,
        };
        cfg.driver.settings.push(zedb_core::DriverSetting {
            name: "api_token".into(),
            value: "super-secret-token".into(),
        });
        let key = pool_key(&cfg).unwrap();
        assert!(!key.contains("super-secret-password"));
        assert!(!key.contains("super-secret-token"));
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
