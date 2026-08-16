//! Persistence for connection configs: a JSON file in the platform config
//! directory. Secrets never touch this file; see [`crate::secrets`].

use std::path::PathBuf;

use crate::connection::ConnectionConfig;

#[cfg(test)]
pub(crate) static CONFIG_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("could not determine config directory")]
    NoConfigDir,
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid config file {path}: {source}")]
    Parse {
        path: PathBuf,
        source: serde_json::Error,
    },
}

/// Resolves the config file path. `ZEDB_CONFIG_DIR` overrides the platform
/// default (used by tests; also handy for portable setups).
fn config_path() -> Result<PathBuf, StoreError> {
    let dir = match std::env::var_os("ZEDB_CONFIG_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => dirs::config_dir()
            .ok_or(StoreError::NoConfigDir)?
            .join("zedb"),
    };
    Ok(dir.join("connections.json"))
}

pub fn load_connections() -> Result<Vec<ConnectionConfig>, StoreError> {
    let path = config_path()?;
    let data = match std::fs::read_to_string(&path) {
        Ok(data) => data,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => return Err(StoreError::Io { path, source }),
    };
    serde_json::from_str(&data).map_err(|source| StoreError::Parse { path, source })
}

pub fn save_connections(connections: &[ConnectionConfig]) -> Result<(), StoreError> {
    let path = config_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| StoreError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let data = serde_json::to_string_pretty(connections).expect("serializable");
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, data).map_err(|source| StoreError::Io {
        path: tmp.clone(),
        source,
    })?;
    std::fs::rename(&tmp, &path).map_err(|source| StoreError::Io { path, source })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::{ConnectionNode, EnvTier};

    #[test]
    fn round_trip_and_missing_file() {
        let _environment = CONFIG_ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        // Env vars are process-global; this test is the only one touching
        // this variable.
        unsafe { std::env::set_var("ZEDB_CONFIG_DIR", dir.path()) };

        assert_eq!(load_connections().unwrap(), Vec::new());

        let conns = vec![ConnectionConfig {
            name: "staging".into(),
            nodes: vec![
                ConnectionNode {
                    name: "Node 1".into(),
                    endpoint: "http://ch-1.example:8123".into(),
                },
                ConnectionNode {
                    name: "Node 2".into(),
                    endpoint: "http://ch-2.example:8123".into(),
                },
            ],
            user: "default".into(),
            database: None,
            tier: EnvTier::Staging,
            read_only: true,
            driver: Default::default(),
            cloud: None,
        }];
        save_connections(&conns).unwrap();
        assert_eq!(load_connections().unwrap(), conns);

        let raw = std::fs::read_to_string(dir.path().join("connections.json")).unwrap();
        assert!(!raw.contains("password"), "no secret-shaped keys on disk");

        unsafe { std::env::remove_var("ZEDB_CONFIG_DIR") };
    }
}
