//! UI session persistence across launches.
//!
//! The workspace saves its open query tabs on every quit (including the
//! update restart) and the next launch consumes the file exactly once, so a
//! stale session can never keep reappearing after the user closes its tabs.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::store::StoreError;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedSession {
    pub tabs: Vec<SavedQueryTab>,
    pub active_tab: usize,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedQueryTab {
    pub id: String,
    pub saved_tab_id: Option<String>,
    pub name: String,
    pub sql: String,
    /// Owning connection; `None` shows on every connection.
    pub connection: Option<String>,
}

fn session_path() -> Result<PathBuf, StoreError> {
    let dir = match std::env::var_os("ZEDB_CONFIG_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => dirs::config_dir()
            .ok_or(StoreError::NoConfigDir)?
            .join("zedb"),
    };
    Ok(dir.join("session.json"))
}

pub fn save_session(session: &SavedSession) -> Result<(), StoreError> {
    let path = session_path()?;
    save_at(session, path)
}

fn save_at(session: &SavedSession, path: PathBuf) -> Result<(), StoreError> {
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
pub fn take_session() -> Option<SavedSession> {
    take_at(session_path().ok()?)
}

fn take_at(path: PathBuf) -> Option<SavedSession> {
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
        let session = SavedSession {
            tabs: vec![
                SavedQueryTab {
                    id: "tab-a".into(),
                    saved_tab_id: Some("saved-a".into()),
                    name: "Same name".into(),
                    sql: "SELECT 1".into(),
                },
                SavedQueryTab {
                    id: "tab-b".into(),
                    saved_tab_id: Some("saved-b".into()),
                    name: "Same name".into(),
                    sql: "SELECT 2".into(),
                },
            ],
            active_tab: 1,
        };
        save_at(&session, path.clone()).unwrap();

        let restored = take_at(path.clone()).expect("session expected");
        assert_eq!(restored.tabs.len(), 2);
        assert_eq!(restored.tabs[1].sql, "SELECT 2");
        assert_eq!(restored.tabs[0].name, restored.tabs[1].name);
        assert_ne!(restored.tabs[0].id, restored.tabs[1].id);
        assert_eq!(restored.tabs[1].saved_tab_id.as_deref(), Some("saved-b"));
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
