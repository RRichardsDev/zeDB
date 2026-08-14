//! Saved connections as they persist to the settings store.
//!
//! A connection carries its environment tier so the UI can make production
//! unmistakable at a glance, and multiple nodes so a cluster is one entry
//! rather than one per replica. Passwords are deliberately absent: they live
//! in the Keychain (see [`crate::secrets`]) and never touch this file.

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

/// Per-cluster driver configuration: how zeDB's HTTP client talks to
/// this cluster. Everything optional; the defaults are what zeDB has
/// always done.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct DriverConfig {
    /// ClickHouse settings sent with every query on this cluster.
    /// Two names are special: `max_execution_time` is excluded from
    /// the agent-facing guarded path (its own stricter cap wins), and
    /// `connect_timeout` configures the HTTP client (seconds) instead
    /// of being sent to the server.
    pub settings: Vec<DriverSetting>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct DriverSetting {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionNode {
    pub name: String,
    pub endpoint: String,
}

fn deserialize_nodes<'de, D>(deserializer: D) -> Result<Vec<ConnectionNode>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum NodeOrEndpoint {
        Node(ConnectionNode),
        Endpoint(String),
    }

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(String),
        Many(Vec<NodeOrEndpoint>),
    }

    let values = match OneOrMany::deserialize(deserializer)? {
        OneOrMany::One(endpoint) => vec![NodeOrEndpoint::Endpoint(endpoint)],
        OneOrMany::Many(values) => values,
    };
    Ok(values
        .into_iter()
        .enumerate()
        .map(|(index, value)| match value {
            NodeOrEndpoint::Node(node) => node,
            NodeOrEndpoint::Endpoint(endpoint) => ConnectionNode {
                name: format!("Node {}", index + 1),
                endpoint,
            },
        })
        .collect())
}

/// A saved connection. Deliberately contains NO secret material; passwords
/// live in the OS keychain, keyed by the connection name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionConfig {
    /// Unique display name; also the keychain account key.
    pub name: String,
    /// HTTP endpoints for the nodes or load balancers that represent this
    /// logical cluster. Legacy configs with a singular `url` migrate on read.
    #[serde(
        default,
        alias = "endpoints",
        alias = "url",
        deserialize_with = "deserialize_nodes"
    )]
    pub nodes: Vec<ConnectionNode>,
    pub user: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database: Option<String>,
    /// Per-cluster driver knobs; defaults mean "as zeDB always did".
    #[serde(default)]
    pub driver: DriverConfig,
    #[serde(default)]
    pub tier: EnvTier,
    /// Read-only is the default posture; writing requires opting out.
    #[serde(default = "default_true")]
    pub read_only: bool,
    /// Set when the connection was created from a linked ClickHouse
    /// Cloud service, so the app can show service state and explain
    /// idle-service timeouts instead of a bare connection error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cloud: Option<CloudProvenance>,
}

/// Which ClickHouse Cloud service a connection came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudProvenance {
    pub org_id: String,
    pub service_id: String,
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
        assert_eq!(conn.nodes.len(), 1);
        assert_eq!(conn.nodes[0].name, "Node 1");
        assert_eq!(conn.nodes[0].endpoint, "http://h:8123");
    }

    #[test]
    fn cluster_endpoints_round_trip_as_a_list() {
        let json = r#"{
            "name": "local",
            "endpoints": ["http://localhost:8123", "http://localhost:8124"],
            "user": "zedb"
        }"#;
        let connection: ConnectionConfig = serde_json::from_str(json).unwrap();
        assert_eq!(connection.nodes.len(), 2);
        assert_eq!(connection.nodes[0].name, "Node 1");
        assert_eq!(connection.nodes[1].name, "Node 2");

        let serialized = serde_json::to_string(&connection).unwrap();
        assert!(serialized.contains("\"nodes\""));
        assert!(!serialized.contains("\"url\""));
    }

    #[test]
    fn named_nodes_round_trip() {
        let json = r#"{
            "name": "local",
            "nodes": [{"name": "Replica A", "endpoint": "http://localhost:8123"}],
            "user": "zedb"
        }"#;
        let connection: ConnectionConfig = serde_json::from_str(json).unwrap();
        assert_eq!(connection.nodes[0].name, "Replica A");

        let serialized = serde_json::to_string(&connection).unwrap();
        assert!(serialized.contains("Replica A"));
    }

    #[test]
    fn tier_serializes_snake_case() {
        let s = serde_json::to_string(&EnvTier::Production).unwrap();
        assert_eq!(s, "\"production\"");
    }
}
