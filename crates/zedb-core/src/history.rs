//! Local query history: every statement zeDB runs, recorded per
//! connection. Machine-local by design (history is private; it never
//! joins the settings-sync payload), capped, and deduped on
//! consecutive repeats.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::store::StoreError;

/// Newest-first entries kept on disk.
pub const HISTORY_CAP: usize = 1000;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub sql: String,
    /// Connection name the statement ran against.
    pub connection: String,
    /// Unix seconds.
    pub at: i64,
    #[serde(default)]
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub rows: Option<u64>,
    /// First line of the failure, when the run failed.
    #[serde(default)]
    pub error: Option<String>,
}

fn history_path() -> Result<PathBuf, StoreError> {
    let dir = match std::env::var_os("ZEDB_CONFIG_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => dirs::config_dir()
            .ok_or(StoreError::NoConfigDir)?
            .join("zedb"),
    };
    Ok(dir.join("history.json"))
}

/// Missing or unreadable history is an empty history, never an error:
/// nothing here is worth blocking launch over. The cap is re-applied on
/// load so a hand-edited or oversized file cannot outgrow it in memory.
pub fn load_history() -> Vec<HistoryEntry> {
    let Ok(path) = history_path() else {
        return Vec::new();
    };
    let mut entries: Vec<HistoryEntry> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default();
    entries.truncate(HISTORY_CAP);
    entries
}

pub fn save_history(entries: &[HistoryEntry]) -> Result<(), StoreError> {
    let path = history_path()?;
    // History records every statement run, including DDL that embeds
    // credentials; keep it private to the owner and write atomically so a
    // torn write cannot silently erase the file.
    let json = serde_json::to_string_pretty(entries).expect("history serializes");
    crate::store::write_private_atomic(&path, json.as_bytes())
        .map_err(|source| StoreError::Io { path, source })
}

/// Prepend newest-first; re-running the identical statement on the
/// same connection replaces the head instead of stacking duplicates.
pub fn push_entry(entries: &mut Vec<HistoryEntry>, entry: HistoryEntry) {
    if let Some(head) = entries.first_mut() {
        if head.sql == entry.sql && head.connection == entry.connection {
            *head = entry;
            return;
        }
    }
    entries.insert(0, entry);
    entries.truncate(HISTORY_CAP);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(sql: &str, at: i64) -> HistoryEntry {
        HistoryEntry {
            sql: sql.into(),
            connection: "local".into(),
            at,
            duration_ms: None,
            rows: None,
            error: None,
        }
    }

    #[test]
    fn consecutive_repeats_replace_the_head() {
        let mut entries = Vec::new();
        push_entry(&mut entries, entry("SELECT 1", 10));
        push_entry(&mut entries, entry("SELECT 1", 20));
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].at, 20);
        push_entry(&mut entries, entry("SELECT 2", 30));
        push_entry(&mut entries, entry("SELECT 1", 40));
        // Non-consecutive repeats keep their history.
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].sql, "SELECT 1");
    }

    #[test]
    fn cap_holds() {
        let mut entries = Vec::new();
        for i in 0..(HISTORY_CAP + 50) {
            push_entry(&mut entries, entry(&format!("SELECT {i}"), i as i64));
        }
        assert_eq!(entries.len(), HISTORY_CAP);
        assert_eq!(entries[0].sql, format!("SELECT {}", HISTORY_CAP + 49));
    }

    #[test]
    fn round_trip_via_config_dir() {
        let _guard = crate::store::CONFIG_ENV_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("zedb-history-test-{}", std::process::id()));
        unsafe { std::env::set_var("ZEDB_CONFIG_DIR", &dir) };
        let entries = vec![entry("SELECT 42", 99)];
        save_history(&entries).unwrap();
        assert_eq!(load_history(), entries);
        unsafe { std::env::remove_var("ZEDB_CONFIG_DIR") };
        let _ = std::fs::remove_dir_all(dir);
    }
}
