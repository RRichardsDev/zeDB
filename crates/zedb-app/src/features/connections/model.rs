use std::collections::HashMap;

use gpui::Entity;
use zedb_ch::{ChConfig, ClusterMembership};
use zedb_core::{ConnectionConfig, EnvTier};

use crate::components::text_input::TextInput;

pub(crate) struct ConnectionState {
    pub(crate) connections: Vec<ConnectionConfig>,
    pub(crate) selected: Option<usize>,
    pub(crate) connected: Option<ConnectedCluster>,
    pub(crate) connecting: Option<String>,
    pub(crate) endpoint_health: HashMap<String, Vec<EndpointHealth>>,
    pub(crate) password_cache: HashMap<String, Option<String>>,
    pub(crate) form: Option<ConnectionForm>,
    pub(crate) pending_delete: Option<String>,
    pub(crate) cloud: super::cloud::CloudLinkState,
}

impl ConnectionState {
    pub(crate) fn new(connections: Vec<ConnectionConfig>) -> Self {
        Self {
            selected: (!connections.is_empty()).then_some(0),
            connections,
            connected: None,
            connecting: None,
            endpoint_health: HashMap::new(),
            password_cache: HashMap::new(),
            form: None,
            pending_delete: None,
            cloud: super::cloud::CloudLinkState::new(),
        }
    }
}

pub(crate) struct ConnectionForm {
    pub(crate) editing: Option<usize>,
    pub(crate) original_name: Option<String>,
    pub(crate) name: Entity<TextInput>,
    pub(crate) nodes: Vec<NodeForm>,
    pub(crate) user: Entity<TextInput>,
    pub(crate) database: Entity<TextInput>,
    pub(crate) password: Entity<TextInput>,
    pub(crate) tier: EnvTier,
    pub(crate) read_only: bool,
    pub(crate) driver_settings: Vec<DriverSettingForm>,
    /// Carried through the form so saving keeps (or sets) the link to
    /// the ClickHouse Cloud service the connection came from.
    pub(crate) cloud: Option<zedb_core::CloudProvenance>,
}

pub(crate) struct DriverSettingForm {
    pub(crate) name: Entity<TextInput>,
    pub(crate) value: Entity<TextInput>,
}

pub(crate) struct NodeForm {
    pub(crate) name: Entity<TextInput>,
    pub(crate) endpoint: Entity<TextInput>,
}

#[derive(Clone)]
pub(crate) struct ConnectionDraft {
    pub(crate) config: ConnectionConfig,
    pub(crate) password: String,
    pub(crate) editing: Option<usize>,
    pub(crate) original_name: Option<String>,
}

pub(crate) struct ConnectedCluster {
    pub(crate) name: String,
    pub(crate) active_node: usize,
    pub(crate) active_endpoint: String,
    pub(crate) client_config: ChConfig,
    pub(crate) apply_cluster: Option<String>,
}

#[derive(Clone)]
pub(crate) struct EndpointHealth {
    pub(crate) node_index: usize,
    pub(crate) name: String,
    pub(crate) endpoint: String,
    pub(crate) reachable: bool,
    pub(crate) memberships: Vec<ClusterMembership>,
}

/// The first cluster in which two nodes sit on different shards.
pub(crate) fn differentiating_cluster(
    a: &[ClusterMembership],
    b: &[ClusterMembership],
) -> Option<String> {
    a.iter().find_map(|membership| {
        b.iter()
            .find(|other| other.cluster == membership.cluster)
            .filter(|other| other.shard != membership.shard)
            .map(|_| membership.cluster.clone())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn membership(cluster: &str, shard: u64, replica: u64) -> ClusterMembership {
        ClusterMembership {
            cluster: cluster.into(),
            shard,
            replica,
        }
    }

    #[test]
    fn differentiates_shards_but_not_replicas() {
        let node_a = vec![membership("default", 1, 1), membership("main", 1, 1)];
        let node_b = vec![membership("default", 1, 1), membership("main", 1, 2)];
        assert_eq!(differentiating_cluster(&node_a, &node_b), None);

        let node_a = vec![membership("main", 1, 1), membership("sharded", 1, 1)];
        let node_b = vec![membership("main", 1, 2), membership("sharded", 2, 1)];
        assert_eq!(
            differentiating_cluster(&node_a, &node_b),
            Some("sharded".into())
        );
        assert_eq!(differentiating_cluster(&[], &node_b), None);
    }
}
