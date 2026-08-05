use serde::{Deserialize, Serialize};

/// Environment tier of a connection. Drives the visual identity in the UI
/// (an unmistakable badge) and, later, the safety ladder for mutating
/// operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum EnvTier {
    #[default]
    Dev,
    Staging,
    Production,
}

impl EnvTier {
    pub fn label(&self) -> &'static str {
        match self {
            EnvTier::Dev => "dev",
            EnvTier::Staging => "staging",
            EnvTier::Production => "prod",
        }
    }

    pub fn next(&self) -> EnvTier {
        match self {
            EnvTier::Dev => EnvTier::Staging,
            EnvTier::Staging => EnvTier::Production,
            EnvTier::Production => EnvTier::Dev,
        }
    }
}

fn default_true() -> bool {
    true
}

fn deserialize_endpoints<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(String),
        Many(Vec<String>),
    }

    Ok(match OneOrMany::deserialize(deserializer)? {
        OneOrMany::One(endpoint) => vec![endpoint],
        OneOrMany::Many(endpoints) => endpoints,
    })
}

/// A saved connection. Deliberately contains NO secret material; passwords
/// live in the OS keychain, keyed by the connection name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionConfig {
    /// Unique display name; also the keychain account key.
    pub name: String,
    /// HTTP endpoints for the nodes or load balancers that represent this
    /// logical cluster. Legacy configs with a singular `url` migrate on read.
    #[serde(default, alias = "url", deserialize_with = "deserialize_endpoints")]
    pub endpoints: Vec<String>,
    pub user: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database: Option<String>,
    #[serde(default)]
    pub tier: EnvTier,
    /// Read-only is the default posture; writing requires opting out.
    #[serde(default = "default_true")]
    pub read_only: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_safe() {
        let json = r#"{"name": "x", "url": "http://h:8123", "user": "u"}"#;
        let conn: ConnectionConfig = serde_json::from_str(json).unwrap();
        assert!(conn.read_only, "read-only must default on");
        assert_eq!(conn.tier, EnvTier::Dev);
        assert_eq!(conn.endpoints, ["http://h:8123"]);
    }

    #[test]
    fn cluster_endpoints_round_trip_as_a_list() {
        let json = r#"{
            "name": "local",
            "endpoints": ["http://localhost:8123", "http://localhost:8124"],
            "user": "zedb"
        }"#;
        let connection: ConnectionConfig = serde_json::from_str(json).unwrap();
        assert_eq!(connection.endpoints.len(), 2);

        let serialized = serde_json::to_string(&connection).unwrap();
        assert!(serialized.contains("\"endpoints\""));
        assert!(!serialized.contains("\"url\""));
    }

    #[test]
    fn tier_serializes_snake_case() {
        let s = serde_json::to_string(&EnvTier::Production).unwrap();
        assert_eq!(s, "\"production\"");
    }
}
