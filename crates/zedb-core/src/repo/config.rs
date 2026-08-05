use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

use super::RepoError;

pub const FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepoConfig {
    pub format: u32,
    pub engine: EngineConfig,
    #[serde(default)]
    pub tracking: TrackingConfig,
    #[serde(default)]
    pub fleet: FleetConfig,
    #[serde(default)]
    pub scopes: BTreeMap<String, ScopeConfig>,
    #[serde(default)]
    pub params: BTreeMap<String, ParamConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EngineConfig {
    pub kind: String,
    pub version: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrackingConfig {
    #[serde(default = "default_tracking_database")]
    pub database: String,
    #[serde(default)]
    pub cluster_param: Option<String>,
}

impl Default for TrackingConfig {
    fn default() -> Self {
        Self {
            database: default_tracking_database(),
            cluster_param: None,
        }
    }
}

fn default_tracking_database() -> String {
    "default".into()
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FleetConfig {
    #[serde(default)]
    pub registry_query: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScopeConfig {
    #[serde(default)]
    pub param: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParamConfig {
    #[serde(default)]
    pub default: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

impl RepoConfig {
    pub fn load(path: &Path) -> Result<Self, RepoError> {
        let text = std::fs::read_to_string(path)?;
        let config: RepoConfig = toml::from_str(&text).map_err(|error| RepoError::Config {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
        if config.format != FORMAT_VERSION {
            return Err(RepoError::Config {
                path: path.to_path_buf(),
                message: format!(
                    "format {} is not supported; this zedb understands format {FORMAT_VERSION}",
                    config.format
                ),
            });
        }
        if config.engine.kind != "clickhouse" {
            return Err(RepoError::Config {
                path: path.to_path_buf(),
                message: format!(
                    "engine kind {:?} is not supported; only \"clickhouse\" repos can migrate",
                    config.engine.kind
                ),
            });
        }
        Ok(config)
    }

    /// Parameter names usable in `${...}` placeholders: built-ins plus
    /// declared params plus scope params.
    pub fn declared_params(&self) -> impl Iterator<Item = &str> {
        super::BUILTIN_PARAMS
            .iter()
            .copied()
            .chain(self.params.keys().map(String::as_str))
            .chain(self.scopes.values().filter_map(|s| s.param.as_deref()))
    }
}
