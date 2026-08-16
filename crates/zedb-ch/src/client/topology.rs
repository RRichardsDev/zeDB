use super::*;

impl ChClient {
    /// Which cluster shards this node is a member of, from its own
    /// system.clusters (`is_local = 1`). Asking the node about itself
    /// works regardless of how its endpoint is reached (port-mapped
    /// docker, DNS aliases, load balancers).
    pub async fn cluster_memberships(&self) -> Result<Vec<ClusterMembership>> {
        // hosts counts the whole cluster (not just local rows) so
        // callers can tell a real topology from the degenerate
        // single-node cluster by shape instead of by name.
        let result = self
            .query(
                "SELECT local.cluster, local.shard_num, local.replica_num, sizes.hosts \
                 FROM system.clusters AS local \
                 INNER JOIN ( \
                     SELECT cluster, count() AS hosts \
                     FROM system.clusters GROUP BY cluster \
                 ) AS sizes ON sizes.cluster = local.cluster \
                 WHERE local.is_local = 1 ORDER BY local.cluster",
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
                    hosts: number(&row[3])?,
                })
            })
            .collect()
    }
}
