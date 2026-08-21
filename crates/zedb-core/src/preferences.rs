//! User preferences, saved queries, and custom agents, persisted as one
//! settings file.
//!
//! Every field defaults, and the file is read with `serde(default)`, so a
//! settings file written by an older build still loads after an upgrade adds
//! a preference.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::store::StoreError;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub vim_mode: bool,
    /// Opt in to ClickHouse 26.6's experimental `STREAM CURSOR` tail
    /// transport. Disabled by default; instant tails otherwise keep using
    /// Live View `WATCH` where supported, then native fast polling.
    pub experimental_streaming_queries: bool,
    /// Last-opened migration repo, restored on launch (BYO git: this is
    /// just a local checkout path; git stays the user's workflow).
    pub fleet_repo: Option<String>,
    /// Migration repo per connection name: the repo follows the
    /// connection, so one cluster's chain never silently attaches to
    /// another cluster.
    pub fleet_repos: std::collections::BTreeMap<String, String>,
    /// Value rendered into ${cluster} for fleet operations.
    pub fleet_cluster: Option<String>,
    /// User-added ACP agents for the agent pane, beyond the built-ins.
    pub custom_agents: Vec<CustomAgent>,
    /// Legacy title-based grants retained only for settings compatibility.
    /// They are ignored by the app and excluded from settings sync because ACP
    /// display titles are not stable authority identities.
    pub agent_always_allow: Vec<String>,
    /// Agent pane width, remembered across launches.
    pub agent_pane_width: Option<f32>,
    /// The most recently started agent (by display name); cmd-n in the
    /// agent pane reuses it.
    pub last_agent: Option<String>,
    /// "dark" (default), "light", or "system".
    pub theme: Option<String>,
    /// Settings-sync remote URL. Machine-local: it is stripped from the
    /// sync payload itself.
    pub settings_sync_url: Option<String>,
    /// Local checkout path of the settings-sync repo. Machine-local.
    pub settings_sync_repo: Option<String>,
    /// Named query snippets; these sync (query HISTORY stays local).
    pub saved_queries: Vec<SavedQuery>,
    /// ClickHouse Cloud organizations linked for quick setup. Only the
    /// id and display name persist (and sync); the API key stays in
    /// each machine's Keychain.
    pub cloud_orgs: Vec<CloudOrgRef>,
}

/// A linked ClickHouse Cloud organization: no secret material.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudOrgRef {
    pub id: String,
    pub name: String,
}

/// A named query snippet in the history drawer's Saved tab.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedQuery {
    pub name: String,
    pub sql: String,
    /// Favorites sort to the top of the Saved tab.
    #[serde(default)]
    pub favorite: bool,
    /// When the query was saved (Unix seconds), for a "saved N ago" label.
    /// Zero on queries saved before this field existed (shown without a
    /// time).
    #[serde(default)]
    pub saved_at: i64,
}

/// A user-configured ACP-speaking agent: a name and a command line.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomAgent {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
}

fn preferences_path() -> Result<PathBuf, StoreError> {
    let dir = match std::env::var_os("ZEDB_CONFIG_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => dirs::config_dir()
            .ok_or(StoreError::NoConfigDir)?
            .join("zedb"),
    };
    let path = dir.join("settings.json");
    // The file was preferences.json before v0.1.6; carry it over once.
    if !path.exists() {
        let legacy = dir.join("preferences.json");
        if legacy.exists() {
            let _ = std::fs::rename(&legacy, &path);
        }
    }
    Ok(path)
}

/// Where the user-editable settings file lives (for "open settings.json").
pub fn settings_file_path() -> Result<PathBuf, StoreError> {
    preferences_path()
}

pub fn load_preferences() -> Result<Preferences, StoreError> {
    let path = preferences_path()?;
    let data = match crate::store::read_bounded_string(&path, crate::store::MAX_LOCAL_STATE_BYTES) {
        Ok(data) => data,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Preferences::default());
        }
        Err(source) => return Err(StoreError::Io { path, source }),
    };
    serde_json::from_str(&data).map_err(|source| StoreError::Parse { path, source })
}

pub fn save_preferences(preferences: &Preferences) -> Result<(), StoreError> {
    let path = preferences_path()?;
    // settings.json holds saved SQL, the sync repo URL, and agent command
    // lines; write it private to the owner.
    let data = serde_json::to_string_pretty(preferences).expect("serializable");
    crate::store::write_private_atomic(&path, data.as_bytes())
        .map_err(|source| StoreError::Io { path, source })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferences_default_and_round_trip() {
        let _environment = crate::store::CONFIG_ENV_LOCK.lock().unwrap();
        let directory = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("ZEDB_CONFIG_DIR", directory.path()) };

        assert_eq!(load_preferences().unwrap(), Preferences::default());

        let preferences = Preferences {
            vim_mode: true,
            experimental_streaming_queries: true,
            fleet_repo: Some("/tmp/repo".into()),
            fleet_repos: std::collections::BTreeMap::from([(
                "staging".to_string(),
                "/tmp/repo".to_string(),
            )]),
            fleet_cluster: None,
            custom_agents: vec![CustomAgent {
                name: "My Agent".into(),
                command: "my-agent".into(),
                args: vec!["acp".into()],
            }],
            agent_always_allow: vec!["Claude Code|mcp__zedb__drift".into()],
            agent_pane_width: Some(480.0),
            last_agent: Some("Claude Code".into()),
            theme: Some("dark".into()),
            settings_sync_url: Some("git@example.com:me/settings.git".into()),
            settings_sync_repo: Some("/tmp/sync".into()),
            saved_queries: vec![SavedQuery {
                name: "top tables".into(),
                sql: "SELECT 1".into(),
                favorite: true,
                saved_at: 1_700_000_000,
            }],
            cloud_orgs: vec![CloudOrgRef {
                id: "org-1".into(),
                name: "Acme".into(),
            }],
        };
        save_preferences(&preferences).unwrap();
        assert_eq!(load_preferences().unwrap(), preferences);

        unsafe { std::env::remove_var("ZEDB_CONFIG_DIR") };
    }

    #[test]
    fn legacy_preferences_json_migrates_to_settings_json() {
        let _environment = crate::store::CONFIG_ENV_LOCK.lock().unwrap();
        let directory = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("ZEDB_CONFIG_DIR", directory.path()) };

        let legacy = directory.path().join("preferences.json");
        std::fs::write(&legacy, r#"{"vim_mode": true}"#).unwrap();

        let loaded = load_preferences().unwrap();
        assert!(loaded.vim_mode);
        assert!(directory.path().join("settings.json").exists());
        assert!(!legacy.exists(), "legacy file is renamed, not copied");

        unsafe { std::env::remove_var("ZEDB_CONFIG_DIR") };
    }
}
