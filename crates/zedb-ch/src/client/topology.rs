use super::*;

impl ChClient {
    /// Which cluster shards this node is a member of, from its own
    /// system.clusters (`is_local = 1`). Asking the node about itself
    /// works regardless of how its endpoint is reached (port-mapped
    /// docker, DNS aliases, load balancers).
    pub async fn cluster_memberships(&self) -> Result<Vec<ClusterMembership>> {
        let result = self
            .query(
                "SELECT cluster, shard_num, replica_num                  FROM system.clusters WHERE is_local = 1 ORDER BY cluster",
            )
            .await?;
        result
            .rows
            .into_iter()
            .map(|row| {
                let text = |value: &Value| match value {
                    Value::String(text) => Ok(text.clone()),
                    other => Err(ChError::Decode(format!(
                        "expected cluster name, got {other:?}"
                    ))),
                };
                let number = |value: &Value| match value {
                    Value::UInt(number) => Ok(*number),
                    Value::Int(number) => Ok(*number as u64),
                    other => Err(ChError::Decode(format!(
                        "expected shard number, got {other:?}"
                    ))),
                };
                Ok(ClusterMembership {
                    cluster: text(&row[0])?,
                    shard: number(&row[1])?,
                    replica: number(&row[2])?,
                })
            })
            .collect()
    }
}
