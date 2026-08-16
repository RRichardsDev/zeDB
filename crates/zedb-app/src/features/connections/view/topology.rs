use crate::*;

use gpui::prelude::*;

impl Workspace {
    pub(crate) fn cluster_overview(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self
            .connection
            .selected
            .and_then(|index| self.connection.connections.get(index));
        let nodes = selected
            .map(|connection| {
                connection
                    .nodes
                    .iter()
                    .enumerate()
                    .map(|(index, configured_node)| {
                        let reachable = self
                            .connection
                            .endpoint_health
                            .get(&connection.name)
                            .and_then(|health| {
                                health
                                    .iter()
                                    .find(|node| node.node_index == index)
                                    .map(|node| node.reachable)
                            });
                        let (label, color) = match reachable {
                            Some(true) => ("reachable", theme::success()),
                            Some(false) => ("failed", theme::danger()),
                            None => ("not tested", theme::text_dim()),
                        };
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().size(px(7.)).rounded_full().bg(color))
                            .child(configured_node.name.clone())
                            .child(
                                div()
                                    .text_color(theme::text_dim())
                                    .child(configured_node.endpoint.clone()),
                            )
                            .child(div().text_xs().text_color(theme::text_dim()).child(label))
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        // Centering lives on a non-scroll wrapper: a flex scroll
        // container stretches its child to the viewport height and
        // clips the overflow before scrolling ever sees it.
        div()
            .id("cluster-overview-scroll")
            .size_full()
            .overflow_y_scroll()
            .p_6()
            .child(
                div().flex().justify_center().w_full().child(
                    div()
                        .w(px(680.))
                        .max_w_full()
                        .flex()
                        .flex_col()
                        .gap_4()
                        .child(
                            div()
                                .text_lg()
                                .text_color(theme::text())
                                .child("Cluster connection"),
                        )
                        .when_some(selected, |panel, connection| {
                            panel
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .child(connection.name.clone())
                                        .child(Self::tier_badge(connection.tier)),
                                )
                                .child(div().flex().flex_col().gap_2().children(nodes))
                                .children(self.topology_section(connection))
                                .children(self.cloud_usage_section(connection, cx))
                        })
                        .when(selected.is_none(), |panel| {
                            panel.child("Add or select a cluster connection to begin.")
                        }),
                ),
            )
    }

    /// A read-only shards-and-replicas view, built entirely from the
    /// memberships each node reported about itself at connect time.
    /// Nothing here is configurable; zeDB displays what the servers
    /// said. Absent topology (never connected, LBs, Cloud) renders
    /// nothing.
    pub(crate) fn topology_section(
        &self,
        connection: &ConnectionConfig,
    ) -> Option<impl IntoElement> {
        let health = self.connection.endpoint_health.get(&connection.name)?;
        // cluster -> shard -> node display names, insertion-ordered.
        type Shards = Vec<(u64, Vec<String>)>;
        let mut clusters: Vec<(String, Shards)> = Vec::new();
        for node in health {
            for membership in &node.memberships {
                // Each node's implicit "default" cluster contains only
                // itself; merging them across nodes would invent a
                // cluster that does not exist.
                if membership.cluster == "default" {
                    continue;
                }
                let cluster = match clusters
                    .iter_mut()
                    .find(|(name, _)| *name == membership.cluster)
                {
                    Some((_, shards)) => shards,
                    None => {
                        clusters.push((membership.cluster.clone(), Vec::new()));
                        &mut clusters.last_mut().expect("just pushed").1
                    }
                };
                match cluster
                    .iter_mut()
                    .find(|(shard, _)| *shard == membership.shard)
                {
                    Some((_, members)) => members.push(node.name.clone()),
                    None => cluster.push((membership.shard, vec![node.name.clone()])),
                }
            }
        }
        if clusters.is_empty() {
            return None;
        }
        for (_, shards) in &mut clusters {
            shards.sort_by_key(|(shard, _)| *shard);
        }

        Some(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(div().text_color(theme::text_dim()).child("Topology"))
                .children(clusters.into_iter().map(|(cluster, shards)| {
                    let shard_count = shards.len();
                    let replicas_per_shard = shards
                        .first()
                        .map(|(_, members)| members.len())
                        .unwrap_or(0);
                    let uniform = shards
                        .iter()
                        .all(|(_, members)| members.len() == replicas_per_shard);
                    let replica_summary = if uniform {
                        format!("{shard_count} shard(s) \u{d7} {replicas_per_shard} replica(s)")
                    } else {
                        format!("{shard_count} shards")
                    };
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .p_3()
                        .rounded(px(4.))
                        .border_1()
                        .border_color(theme::border())
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(div().text_color(theme::text()).child(cluster))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme::text_dim())
                                        .child(replica_summary),
                                ),
                        )
                        .children(shards.into_iter().map(|(shard, members)| {
                            div()
                                .flex()
                                .gap_2()
                                .text_sm()
                                .child(
                                    div()
                                        .w(px(80.))
                                        .flex_none()
                                        .text_color(theme::text_dim())
                                        .child(format!("shard {shard}")),
                                )
                                .child(div().text_color(theme::text()).child(members.join(", ")))
                        }))
                })),
        )
    }
}
