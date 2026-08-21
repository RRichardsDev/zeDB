//! Persistence for connection configs: a JSON file in the platform config
//! directory. Secrets never touch this file; see [`crate::secrets`].

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::connection::ConnectionConfig;

/// Local JSON state may contain large SQL strings, but it must still have a
/// firm upper bound before allocation and deserialization.
pub(crate) const MAX_LOCAL_STATE_BYTES: u64 = 64 * 1024 * 1024;

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Create `dir` (and parents) private to the owner. On unix the mode is
/// set at creation time so there is no world-readable window, then
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
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Read one regular file without following its final symlink and without
/// allocating beyond `limit`. The same open file is checked and read, which
/// avoids a metadata-then-reopen race.
pub(crate) fn read_bounded(path: &Path, limit: u64) -> std::io::Result<Vec<u8>> {
    use std::io::Read as _;

    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    #[cfg(not(unix))]
    {
        if std::fs::symlink_metadata(path)?.file_type().is_symlink() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "refusing to follow a symlink",
            ));
        }
    }

    let mut file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "expected a regular file",
        ));
    }
    if metadata.len() > limit {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "file is {} bytes, over the {limit} byte limit",
                metadata.len()
            ),
        ));
    }

    let mut data = Vec::with_capacity(metadata.len().min(limit) as usize);
    file.by_ref().take(limit + 1).read_to_end(&mut data)?;
    if data.len() as u64 > limit {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("file grew beyond the {limit} byte limit while being read"),
        ));
    }
    Ok(data)
}

pub(crate) fn read_bounded_string(path: &Path, limit: u64) -> std::io::Result<String> {
    String::from_utf8(read_bounded(path, limit)?).map_err(|error| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("file is not valid UTF-8: {error}"),
        )
    })
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
    write_atomic_file(path, data)
}

/// Atomic 0600 write without changing the containing directory's mode. Sync
/// payloads live inside user-owned Git checkouts, whose root permissions zeDB
/// must not rewrite.
pub(crate) fn write_atomic_file(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("zedb-state");
    let mut last_collision = None;
    for _ in 0..32 {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temporary = parent.join(format!(
            ".{file_name}.tmp-{}-{sequence}",
            std::process::id()
        ));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        }
        let mut file = match options.open(&temporary) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                last_collision = Some(error);
                continue;
            }
            Err(error) => return Err(error),
        };
        let result = (|| {
            file.write_all(data)?;
            file.sync_all()?;
            drop(file);
            std::fs::rename(&temporary, path)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
            }
            if let Ok(directory) = std::fs::File::open(parent) {
                let _ = directory.sync_all();
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        return result;
    }
    Err(last_collision.unwrap_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "could not allocate a private temporary file",
        )
    }))
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
    let data = match read_bounded_string(&path, MAX_LOCAL_STATE_BYTES) {
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

    #[cfg(unix)]
    #[test]
    fn private_writer_repairs_existing_modes_and_avoids_fixed_temp_names() {
        use std::os::unix::fs::PermissionsExt as _;

        let directory = tempfile::tempdir().unwrap();
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = directory.path().join("state.json");
        std::fs::write(&path, b"old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        std::fs::write(directory.path().join("state.tmp"), b"do not follow").unwrap();

        write_private_atomic(&path, b"new").unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        assert_eq!(
            std::fs::metadata(directory.path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::read(directory.path().join("state.tmp")).unwrap(),
            b"do not follow"
        );
    }

    #[cfg(unix)]
    #[test]
    fn bounded_reader_rejects_symlinks_and_special_files() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("target");
        let link = directory.path().join("link");
        std::fs::write(&target, b"secret").unwrap();
        symlink(&target, &link).unwrap();

        assert!(read_bounded(&link, 64).is_err());
        assert!(read_bounded(Path::new("/dev/null"), 64).is_err());
    }
}
