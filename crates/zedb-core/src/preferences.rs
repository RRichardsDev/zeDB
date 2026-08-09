use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::store::StoreError;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub vim_mode: bool,
    /// Last-opened migration repo, restored on launch (BYO git: this is
    /// just a local checkout path; git stays the user's workflow).
    pub fleet_repo: Option<String>,
    /// Value rendered into ${cluster} for fleet operations.
    pub fleet_cluster: Option<String>,
    /// User-added ACP agents for the agent pane, beyond the built-ins.
    pub custom_agents: Vec<CustomAgent>,
    /// Tools the user chose Always Allow for, as "agent|tool" keys;
    /// matching permission requests auto-approve across sessions.
    pub agent_always_allow: Vec<String>,
    /// Agent pane width, remembered across launches.
    pub agent_pane_width: Option<f32>,
    /// The most recently started agent (by display name); cmd-n in the
    /// agent pane reuses it.
    pub last_agent: Option<String>,
    /// "dark" (default), "light", or "system".
    pub theme: Option<String>,
    /// Settings-sync remote URL (Phase 3.4 M1). Machine-local: it is
    /// stripped from the sync payload itself.
    pub settings_sync_url: Option<String>,
    /// Local checkout path of the settings-sync repo. Machine-local.
    pub settings_sync_repo: Option<String>,
    /// Named query snippets; these sync (query HISTORY stays local).
    pub saved_queries: Vec<SavedQuery>,
}

/// A named query snippet in the history drawer's Saved tab.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedQuery {
    pub name: String,
    pub sql: String,
    /// Favorites sort to the top of the Saved tab.
    #[serde(default)]
    pub favorite: bool,
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
    let data = match std::fs::read_to_string(&path) {
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
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| StoreError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let data = serde_json::to_string_pretty(preferences).expect("serializable");
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, data).map_err(|source| StoreError::Io {
        path: temporary.clone(),
        source,
    })?;
    std::fs::rename(&temporary, &path).map_err(|source| StoreError::Io { path, source })
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
            fleet_repo: Some("/tmp/repo".into()),
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
