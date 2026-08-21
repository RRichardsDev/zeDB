//! Persistence for connection configs: a JSON file in the platform config
//! directory. Secrets never touch this file; see [`crate::secrets`].

use std::path::{Path, PathBuf};

use crate::connection::ConnectionConfig;

/// Create `dir` (and parents) private to the owner. On unix the mode is
/// set at creation time so there is no world-readable window, and also
/// re-applied in case the directory already existed with looser bits.
pub(crate) fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        builder.mode(0o700);
    }
    match builder.create(dir) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    Ok(())
}

/// Write `data` to `path` atomically and private to the owner: the parent
/// directory is created 0700, the payload is written to a sibling temp file
/// created 0600 (mode applied at open time, no chmod race), then renamed
/// over `path`. Used for every local file that can hold SQL, endpoints, or
/// other material a co-resident user should not read.
pub(crate) fn write_private_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        create_private_dir(parent)?;
    }
    let temporary = path.with_extension("tmp");
    {
        use std::io::Write as _;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(data)?;
        file.sync_all()?;
    }
    std::fs::rename(&temporary, path)
}

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
    let data = serde_json::to_string_pretty(connections).expect("serializable");
    write_private_atomic(&path, data.as_bytes()).map_err(|source| StoreError::Io { path, source })
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
                    native_port: None,
                },
                ConnectionNode {
                    name: "Node 2".into(),
                    endpoint: "http://ch-2.example:8123".into(),
                    native_port: None,
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
