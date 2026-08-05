use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

use super::RepoError;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Exclusions {
    #[serde(default)]
    pub groups: BTreeMap<String, ExclusionGroup>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExclusionGroup {
    pub reason: String,
    pub databases: Vec<String>,
}

impl Exclusions {
    /// Load `exclusions.toml`; a missing file means no exclusions.
    pub fn load(path: &Path) -> Result<Self, RepoError> {
        if !path.is_file() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(path)?;
        toml::from_str(&text).map_err(|error| RepoError::Config {
            path: path.to_path_buf(),
            message: error.to_string(),
        })
    }

    /// Every excluded database with the group that excludes it.
    pub fn excluded(&self) -> impl Iterator<Item = (&str, &str)> {
        self.groups.iter().flat_map(|(name, group)| {
            group
                .databases
                .iter()
                .map(move |db| (db.as_str(), name.as_str()))
        })
    }

    pub fn is_excluded(&self, database: &str) -> bool {
        self.excluded().any(|(db, _)| db == database)
    }
}
