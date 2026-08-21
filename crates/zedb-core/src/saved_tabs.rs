//! Machine-local saved query tabs.
//!
//! Unlike saved queries, these tabs do not join settings sync. They can
//! contain unfinished or sensitive working SQL, so they stay on this machine
//! beside query history and the launch session.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use serde::{Deserialize, Serialize};

use crate::store::StoreError;

pub const SAVED_TAB_CAP: usize = 200;
static LOCAL_ID_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedTab {
    pub id: String,
    pub name: String,
    pub sql: String,
    /// `None` is Unlimited; every other value is the selected result cap.
    pub row_limit: Option<usize>,
    /// Unix seconds.
    #[serde(default)]
    pub saved_at: i64,
}

#[derive(Deserialize)]
struct LegacySavedTabSet {
    tabs: Vec<LegacySavedTab>,
    #[serde(default)]
    saved_at: i64,
}

#[derive(Deserialize)]
struct LegacySavedTab {
    id: String,
    name: String,
    sql: String,
    row_limit: Option<usize>,
}

/// A machine-local opaque identity. Labels are deliberately not identities:
/// users may rename tabs and may use the same label more than once.
pub fn new_local_id(prefix: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    let sequence = LOCAL_ID_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{nanos:x}-{sequence:x}")
}

fn saved_tabs_path() -> Result<PathBuf, StoreError> {
    let dir = match std::env::var_os("ZEDB_CONFIG_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => dirs::config_dir()
            .ok_or(StoreError::NoConfigDir)?
            .join("zedb"),
    };
    Ok(dir.join("saved-tabs.json"))
}

pub fn load_saved_tabs() -> Vec<SavedTab> {
    let Ok(path) = saved_tabs_path() else {
        return Vec::new();
    };
    load_at(&path)
}

fn load_at(path: &Path) -> Vec<SavedTab> {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    if let Ok(mut tabs) = serde_json::from_str::<Vec<SavedTab>>(&raw) {
        tabs.truncate(SAVED_TAB_CAP);
        return tabs;
    }

    let Ok(sets) = serde_json::from_str::<Vec<LegacySavedTabSet>>(&raw) else {
        return Vec::new();
    };
    let mut ids = HashSet::new();
    sets.into_iter()
        .flat_map(|set| {
            set.tabs.into_iter().map(move |tab| SavedTab {
                id: tab.id,
                name: tab.name,
                sql: tab.sql,
                row_limit: tab.row_limit,
                saved_at: set.saved_at,
            })
        })
        .map(|mut tab| {
            if tab.id.is_empty() || !ids.insert(tab.id.clone()) {
                tab.id = new_local_id("saved-tab");
                ids.insert(tab.id.clone());
            }
            tab
        })
        .take(SAVED_TAB_CAP)
        .collect()
}

pub fn save_saved_tabs(tabs: &[SavedTab]) -> Result<(), StoreError> {
    save_at(tabs, saved_tabs_path()?)
}

fn save_at(tabs: &[SavedTab], path: PathBuf) -> Result<(), StoreError> {
    // Saved tabs hold working SQL that can carry secrets; private mode.
    let json = serde_json::to_string_pretty(tabs).expect("saved tabs serialize");
    crate::store::write_private_atomic(&path, json.as_bytes())
        .map_err(|source| StoreError::Io { path, source })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_tabs_round_trip_with_duplicate_names_and_distinct_ids() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("saved-tabs.json");
        let tabs = vec![
            SavedTab {
                id: "tab-a".into(),
                name: "Overview".into(),
                sql: "SELECT 1".into(),
                row_limit: Some(1_000),
                saved_at: 41,
            },
            SavedTab {
                id: "tab-b".into(),
                name: "Overview".into(),
                sql: "SELECT 2".into(),
                row_limit: None,
                saved_at: 42,
            },
        ];

        save_at(&tabs, path.clone()).unwrap();
        let restored = load_at(&path);
        assert_eq!(restored, tabs);
        assert_eq!(restored[0].name, restored[1].name);
        assert_ne!(restored[0].id, restored[1].id);
    }

    #[test]
    fn legacy_tab_sets_migrate_to_individual_tabs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("saved-tabs.json");
        std::fs::write(
            &path,
            r#"[{"id":"set-a","name":"Work","tabs":[{"id":"tab-a","name":"One","sql":"SELECT 1","row_limit":1000},{"id":"tab-b","name":"Two","sql":"SELECT 2","row_limit":null}],"active_tab_id":"tab-b","saved_at":42}]"#,
        )
        .unwrap();

        let restored = load_at(&path);
        assert_eq!(restored.len(), 2);
        assert_eq!(restored[0].name, "One");
        assert_eq!(restored[1].sql, "SELECT 2");
        assert_eq!(restored[0].saved_at, 42);
    }
}
