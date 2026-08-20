use crate::*;

use gpui::prelude::*;

impl Workspace {
    pub(crate) fn node_selector(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let connected = self.connection.connected.as_ref()?;
        let connection = self
            .connection
            .connections
            .iter()
            .find(|connection| connection.name == connected.name)?;
        let health = self.connection.endpoint_health.get(&connected.name);
        // A cluster that puts any two of these nodes on different shards
        // makes the picker label every node's shard in that cluster.
        let shard_cluster = health.and_then(|health| {
            health.iter().find_map(|node| {
                health.iter().find_map(|other| {
                    differentiating_cluster(&node.memberships, &other.memberships)
                })
            })
        });
        let nodes = connection
            .nodes
            .iter()
            .enumerate()
            .map(|(index, node)| {
                let entry =
                    health.and_then(|health| health.iter().find(|item| item.node_index == index));
                let reachable = entry.map(|item| item.reachable).unwrap_or(false);
                let label = match (&shard_cluster, entry) {
                    (Some(cluster), Some(item)) => item
                        .memberships
                        .iter()
                        .find(|membership| &membership.cluster == cluster)
                        .map(|membership| format!("{}  ·  shard {}", node.name, membership.shard))
                        .unwrap_or_else(|| node.name.clone()),
                    _ => node.name.clone(),
                };
                (index, label, reachable)
            })
            .collect::<Vec<_>>();
        let active_name = connection
            .nodes
            .get(connected.active_node)
            .map(|node| node.name.clone())
            .unwrap_or_else(|| "Select node".into());
        // Clusters the connected node belongs to. Picking one runs
        // schema-apply actions ON CLUSTER instead of just this node.
        // Never offered on Cloud: a warehouse shares one catalog, so
        // DDL runs once and ON CLUSTER has nothing to add.
        let clusters = if connection.cloud.is_some() {
            Vec::new()
        } else {
            self.ops_cluster_options()
        };
        let apply_cluster = connected.apply_cluster.clone();
        // In cluster scope the label reads the cluster, not the node.
        let label = match &apply_cluster {
            Some(name) => format!("Cluster: {name}"),
            None => active_name,
        };
        // The toolbar renders on every view, including with no query tab
        // open at all (closing the last one returns to the overview), so
        // the editor's focus handle is optional here.
        let action_context = self
            .query
            .tabs
            .get(self.query.active_tab)
            .map(|tab| tab.editor.focus_handle(cx));

        Some(
            Button::new("active-node-selector")
                .label(label)
                .dropdown_caret(true)
                .compact()
                .outline()
                .dropdown_menu(move |menu: PopupMenu, _, _| {
                    let base = match action_context.clone() {
                        Some(handle) => menu.action_context(handle),
                        None => menu,
                    };
                    let mut menu = nodes.iter().cloned().fold(
                        base.min_w(px(180.)),
                        |menu, (index, name, reachable)| {
                            menu.menu_with_enable(name, Box::new(SelectNode { index }), reachable)
                        },
                    );
                    if !clusters.is_empty() {
                        menu = menu.separator();
                        for cluster in &clusters {
                            menu = menu.menu(
                                format!("Cluster: {cluster}"),
                                Box::new(SetApplyCluster {
                                    cluster: Some(cluster.clone()),
                                }),
                            );
                        }
                    }
                    menu
                }),
        )
    }
}
