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
    // Tabs carry working SQL, which routinely embeds credentials; the file
    // is written private to the owner.
    let data = serde_json::to_string(session).expect("serializable");
    crate::store::write_private_atomic(&path, data.as_bytes())
        .map_err(|source| StoreError::Io { path, source })
}

/// Read and delete the saved session. Returns `None` when there is nothing
/// to restore or the file is unreadable; either way it will not fire twice.
pub fn take_session() -> Option<SavedSession> {
    take_at(session_path().ok()?)
}

fn take_at(path: PathBuf) -> Option<SavedSession> {
    // A crash between the temp write and the rename can leave a stale
    // *.tmp behind; clear it so it cannot outlive the session it held.
    let _ = std::fs::remove_file(path.with_extension("tmp"));
    let data =
        crate::store::read_bounded_string(&path, crate::store::MAX_LOCAL_STATE_BYTES).ok()?;
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
                    connection: None,
                },
                SavedQueryTab {
                    id: "tab-b".into(),
                    saved_tab_id: Some("saved-b".into()),
                    name: "Same name".into(),
                    sql: "SELECT 2".into(),
                    connection: None,
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
