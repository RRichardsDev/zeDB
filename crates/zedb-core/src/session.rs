//! One-shot UI session handoff across an in-app update relaunch.
//!
//! The workspace saves its open query tabs just before restarting into a new
//! version; the next launch consumes the file exactly once. This is not
//! general session restore: a normal quit writes nothing.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::store::StoreError;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateSession {
    pub tabs: Vec<SavedQueryTab>,
    pub active_tab: usize,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedQueryTab {
    pub sql: String,
}

fn session_path() -> Result<PathBuf, StoreError> {
    let dir = match std::env::var_os("ZEDB_CONFIG_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => dirs::config_dir()
            .ok_or(StoreError::NoConfigDir)?
            .join("zedb"),
    };
    Ok(dir.join("update-session.json"))
}

pub fn save_update_session(session: &UpdateSession) -> Result<(), StoreError> {
    let path = session_path()?;
    save_at(session, path)
}

fn save_at(session: &UpdateSession, path: PathBuf) -> Result<(), StoreError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| StoreError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let data = serde_json::to_string(session).expect("serializable");
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, data).map_err(|source| StoreError::Io {
        path: temporary.clone(),
        source,
    })?;
    std::fs::rename(&temporary, &path).map_err(|source| StoreError::Io { path, source })
}

/// Read and delete the saved session. Returns `None` when there is nothing
/// to restore or the file is unreadable; either way it will not fire twice.
pub fn take_update_session() -> Option<UpdateSession> {
    take_at(session_path().ok()?)
}

fn take_at(path: PathBuf) -> Option<UpdateSession> {
    let data = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    serde_json::from_str(&data).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_round_trips_and_fires_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("update-session.json");
        let session = UpdateSession {
            tabs: vec![
                SavedQueryTab {
                    sql: "SELECT 1".into(),
                },
                SavedQueryTab {
                    sql: "SELECT 2".into(),
                },
            ],
            active_tab: 1,
        };
        save_at(&session, path.clone()).unwrap();

        let restored = take_at(path.clone()).expect("session expected");
        assert_eq!(restored.tabs.len(), 2);
        assert_eq!(restored.tabs[1].sql, "SELECT 2");
        assert_eq!(restored.active_tab, 1);

        assert!(take_at(path).is_none(), "second take must find nothing");
    }

    #[test]
    fn corrupt_session_is_consumed_silently() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("update-session.json");
        std::fs::write(&path, "not json").unwrap();
        assert!(take_at(path.clone()).is_none());
        assert!(!path.exists(), "corrupt file must still be deleted");
    }
}
