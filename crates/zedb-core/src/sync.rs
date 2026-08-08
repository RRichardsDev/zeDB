//! Settings sync over a plain git repo (Phase 3.4 M1).
//!
//! The payload is one readable JSON file in a repo the user owns; git is
//! the transport and the history. Identity (GitHub sign-in) is not
//! involved here: any clonable URL works. Passwords never sync; the
//! connections file has no secret fields by construction and the
//! payload inherits that.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::connection::ConnectionConfig;
use crate::preferences::Preferences;

/// The payload file at the repo root.
pub const PAYLOAD_FILE: &str = "zedb-settings.json";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Payload {
    pub version: u32,
    /// Unix seconds at write time; last-writer-wins tiebreaker.
    pub updated_at: u64,
    /// Hostname of the machine that wrote it, for the git log's benefit.
    pub updated_by: String,
    pub preferences: Preferences,
    pub connections: Vec<ConnectionConfig>,
}

/// Preferences with machine-local fields removed: checkout paths and the
/// sync configuration itself never leave this machine.
pub fn sanitized_preferences(preferences: &Preferences) -> Preferences {
    let mut preferences = preferences.clone();
    preferences.fleet_repo = None;
    preferences.settings_sync_url = None;
    preferences.settings_sync_repo = None;
    preferences
}

pub fn build_payload(
    preferences: &Preferences,
    connections: &[ConnectionConfig],
    updated_at: u64,
    updated_by: String,
) -> Payload {
    Payload {
        version: 1,
        updated_at,
        updated_by,
        preferences: sanitized_preferences(preferences),
        connections: connections.to_vec(),
    }
}

/// A stable fingerprint of what the payload syncs (everything except the
/// write metadata), used to tell which side changed since the last sync.
pub fn content_hash(preferences: &Preferences, connections: &[ConnectionConfig]) -> String {
    let content = serde_json::json!({
        "preferences": sanitized_preferences(preferences),
        "connections": connections,
    });
    // FNV-1a over the canonical JSON: cheap, stable, no new dependency.
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in content.to_string().bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

pub fn payload_hash(payload: &Payload) -> String {
    content_hash(&payload.preferences, &payload.connections)
}

pub fn read_payload(root: &Path) -> Result<Option<Payload>, String> {
    let path = root.join(PAYLOAD_FILE);
    let data = match std::fs::read_to_string(&path) {
        Ok(data) => data,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("could not read {}: {error}", path.display())),
    };
    serde_json::from_str(&data)
        .map(Some)
        .map_err(|error| format!("invalid {}: {error}", path.display()))
}

pub fn write_payload(root: &Path, payload: &Payload) -> Result<(), String> {
    let path = root.join(PAYLOAD_FILE);
    let data = serde_json::to_string_pretty(payload).expect("serializable");
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, data)
        .and_then(|()| std::fs::rename(&temporary, &path))
        .map_err(|error| format!("could not write {}: {error}", path.display()))
}

/// Machine-local sync state: the content hash both sides agreed on at the
/// last successful sync. Lives next to preferences.json, never in the repo.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SyncState {
    pub last_synced_hash: Option<String>,
    pub last_synced_at: Option<u64>,
}

fn state_path() -> Option<PathBuf> {
    let dir = match std::env::var_os("ZEDB_CONFIG_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => dirs::config_dir()?.join("zedb"),
    };
    Some(dir.join("sync-state.json"))
}

pub fn load_state() -> SyncState {
    let Some(path) = state_path() else {
        return SyncState::default();
    };
    std::fs::read_to_string(path)
        .ok()
        .and_then(|data| serde_json::from_str(&data).ok())
        .unwrap_or_default()
}

pub fn save_state(state: &SyncState) {
    let Some(path) = state_path() else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, serde_json::to_string_pretty(state).expect("serializable"));
}

/// What a sync tick should do, decided from the three hashes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reconcile {
    /// Nothing changed anywhere.
    UpToDate,
    /// Only the repo moved: overwrite local settings with its payload.
    ApplyRepo,
    /// Only this machine moved (or the repo has no payload yet): push.
    PushLocal,
    /// Both moved since the last sync. Local wins and is pushed; the
    /// repo's version stays reachable in git history.
    PushLocalConflict,
}

pub fn reconcile(
    local_hash: &str,
    repo_hash: Option<&str>,
    last_synced: Option<&str>,
) -> Reconcile {
    let Some(repo_hash) = repo_hash else {
        // Empty repo: local is the only copy there is.
        return Reconcile::PushLocal;
    };
    if repo_hash == local_hash {
        return Reconcile::UpToDate;
    }
    let local_changed = last_synced != Some(local_hash);
    let repo_changed = last_synced != Some(repo_hash);
    match (local_changed, repo_changed) {
        (false, true) => Reconcile::ApplyRepo,
        (true, false) => Reconcile::PushLocal,
        (true, true) => Reconcile::PushLocalConflict,
        // Hashes differ yet neither side moved: stale stamp; trust the repo.
        (false, false) => Reconcile::ApplyRepo,
    }
}

/// Merge a pulled payload into local preferences, keeping machine-local
/// fields exactly as they are on this machine.
pub fn apply_preferences(local: &Preferences, pulled: &Preferences) -> Preferences {
    let mut merged = pulled.clone();
    merged.fleet_repo = local.fleet_repo.clone();
    merged.settings_sync_url = local.settings_sync_url.clone();
    merged.settings_sync_repo = local.settings_sync_repo.clone();
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::{ConnectionNode, EnvTier};

    fn sample_connections() -> Vec<ConnectionConfig> {
        vec![ConnectionConfig {
            name: "staging".into(),
            nodes: vec![ConnectionNode {
                name: "Node 1".into(),
                endpoint: "http://ch-1.example:8123".into(),
            }],
            user: "default".into(),
            database: None,
            tier: EnvTier::Staging,
            read_only: true,
        }]
    }

    #[test]
    fn payload_round_trip_and_redaction() {
        let directory = tempfile::tempdir().unwrap();
        let mut preferences = Preferences {
            vim_mode: true,
            fleet_repo: Some("/local/checkout".into()),
            ..Preferences::default()
        };
        preferences.settings_sync_url = Some("git@example.com:me/settings.git".into());
        preferences.settings_sync_repo = Some("/local/sync".into());

        let payload = build_payload(&preferences, &sample_connections(), 1_700_000_000, "mac".into());
        write_payload(directory.path(), &payload).unwrap();

        let raw = std::fs::read_to_string(directory.path().join(PAYLOAD_FILE)).unwrap();
        assert!(!raw.contains("password"), "payload must never carry secrets");
        assert!(!raw.contains("/local/checkout"), "machine-local paths stay home");
        assert!(!raw.contains("settings.git"), "sync config itself never syncs");

        let read = read_payload(directory.path()).unwrap().unwrap();
        assert_eq!(read, payload);
        assert_eq!(read_payload(&directory.path().join("missing")).unwrap(), None);
    }

    #[test]
    fn hash_ignores_machine_local_fields() {
        let base = Preferences::default();
        let mut local = base.clone();
        local.fleet_repo = Some("/somewhere".into());
        local.settings_sync_repo = Some("/sync".into());
        assert_eq!(
            content_hash(&base, &sample_connections()),
            content_hash(&local, &sample_connections())
        );
        let mut changed = base.clone();
        changed.vim_mode = !changed.vim_mode;
        assert_ne!(
            content_hash(&base, &sample_connections()),
            content_hash(&changed, &sample_connections())
        );
    }

    #[test]
    fn reconcile_decision_table() {
        use Reconcile::*;
        assert_eq!(reconcile("a", None, None), PushLocal);
        assert_eq!(reconcile("a", Some("a"), Some("a")), UpToDate);
        assert_eq!(reconcile("a", Some("a"), None), UpToDate);
        assert_eq!(reconcile("a", Some("b"), Some("a")), ApplyRepo);
        assert_eq!(reconcile("b", Some("a"), Some("a")), PushLocal);
        assert_eq!(reconcile("b", Some("c"), Some("a")), PushLocalConflict);
        assert_eq!(reconcile("b", Some("c"), None), PushLocalConflict);
    }

    #[test]
    fn apply_keeps_machine_local_fields() {
        let local = Preferences {
            fleet_repo: Some("/mine".into()),
            settings_sync_url: Some("url".into()),
            settings_sync_repo: Some("/sync".into()),
            ..Preferences::default()
        };
        let pulled = Preferences {
            vim_mode: true,
            fleet_repo: Some("/theirs".into()),
            ..Preferences::default()
        };
        let merged = apply_preferences(&local, &pulled);
        assert!(merged.vim_mode);
        assert_eq!(merged.fleet_repo.as_deref(), Some("/mine"));
        assert_eq!(merged.settings_sync_url.as_deref(), Some("url"));
        assert_eq!(merged.settings_sync_repo.as_deref(), Some("/sync"));
    }
}
