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
        let label = active_name;
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
                    nodes.iter().cloned().fold(
                        base.min_w(px(180.)),
                        |menu, (index, name, reachable)| {
                            menu.menu_with_enable(name, Box::new(SelectNode { index }), reachable)
                        },
                    )
                }),
        )
    }

    /// The working-scope control beside the node picker: what ops and
    /// analytics show, and what schema and fleet mutations target
    /// (ON CLUSTER). Hidden when the topology offers no cluster.
    pub(crate) fn scope_selector(&self, _cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let connected = self.connection.connected.as_ref()?;
        let connection = self
            .connection
            .connections
            .iter()
            .find(|connection| connection.name == connected.name)?;
        if connection.cloud.is_some() {
            return None;
        }
        let clusters = self.ops_cluster_options();
        if clusters.is_empty() {
            return None;
        }
        let scope_label = match &connected.apply_cluster {
            Some(name) => format!("cluster {name}"),
            None => "this node".to_string(),
        };
        Some(
            Button::new("working-scope-selector")
                .label(scope_label)
                .dropdown_caret(true)
                .compact()
                .outline()
                .dropdown_menu(move |menu: PopupMenu, _, _| {
                    let menu = menu
                        .min_w(px(200.))
                        .menu("This node", Box::new(SetApplyCluster { cluster: None }));
                    clusters.iter().fold(menu, |menu, cluster| {
                        menu.menu(
                            format!("Cluster: {cluster}"),
                            Box::new(SetApplyCluster {
                                cluster: Some(cluster.clone()),
                            }),
                        )
                    })
                }),
        )
    }
}
