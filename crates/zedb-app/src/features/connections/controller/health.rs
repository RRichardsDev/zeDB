use crate::*;

use gpui::prelude::*;

impl Workspace {
    pub(crate) fn start_health_poll(&mut self, cx: &mut Context<Self>) {
        self.health_poll_generation += 1;
        let generation = self.health_poll_generation;
        let Some(connected) = &self.connection.connected else {
            return;
        };
        let config = connected.client_config.clone();
        let name = connected.name.clone();
        let node_index = connected.active_node;
        let schema_cache = self.schema.cache.clone();
        cx.spawn(async move |this, cx| loop {
            Timer::after(Duration::from_secs(300)).await;
            let stale = this
                .update(cx, |this, _| this.health_poll_generation != generation)
                .unwrap_or(true);
            if stale {
                break;
            }
            let probe = config.clone();
            let cache = schema_cache.clone();
            let healthy = rt::tokio()
                .spawn(async move {
                    let client = ChClient::new(probe);
                    if client.query("SELECT 1").await.is_err() {
                        return false;
                    }
                    if let Some(cache) = cache {
                        let _ = cache.refresh_tables(&client).await;
                    }
                    true
                })
                .await
                .unwrap_or(false);
            if healthy {
                continue;
            }
            let stop = this
                .update(cx, |this, cx| {
                    if this.health_poll_generation != generation {
                        return true;
                    }
                    let still_here = this
                        .connection
                        .connected
                        .as_ref()
                        .is_some_and(|connected| connected.name == name);
                    if !still_here {
                        return true;
                    }
                    this.connection.connected = None;
                    this.schema.cache = None;
                    this.schema.provider.set_context(None, None);
                    this.fleet.write_unlocked = false;
                    if let Some(health) = this.connection.endpoint_health.get_mut(&name) {
                        if let Some(node) =
                            health.iter_mut().find(|node| node.node_index == node_index)
                        {
                            node.reachable = false;
                        }
                    }
                    this.flash_warning(
                        format!("Lost connection to {name}: health check failed"),
                        cx,
                    );
                    true
                })
                .unwrap_or(true);
            if stop {
                break;
            }
        })
        .detach();
    }

    pub(crate) fn disconnect(&mut self, cx: &mut Context<Self>) {
        self.health_poll_generation += 1;
        if let Some(connected) = self.connection.connected.take() {
            self.notice = Some(format!("Disconnected from {}", connected.name));
        }
        self.schema.cache = None;
        self.schema.provider.set_context(None, None);
        self.clear_schema();
        self.ops_reset(cx);
        cx.notify();
    }

    /// Set (or clear) the cluster the schema-apply actions target with
    /// `ON CLUSTER`. Chosen from the node selector.
    pub(crate) fn set_apply_cluster(&mut self, cluster: Option<String>, cx: &mut Context<Self>) {
        if let Some(connected) = self.connection.connected.as_mut() {
            connected.apply_cluster = cluster;
            cx.notify();
        }
    }

    pub(crate) fn select_node(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(connected_name) = self
            .connection
            .connected
            .as_ref()
            .map(|connected| connected.name.clone())
        else {
            return;
        };
        let Some(node) = self
            .connection
            .endpoint_health
            .get(&connected_name)
            .and_then(|health| health.iter().find(|node| node.node_index == index))
            .filter(|node| node.reachable)
            .cloned()
        else {
            return;
        };
        let previous_memberships = self
            .connection
            .connected
            .as_ref()
            .map(|connected| connected.active_node)
            .and_then(|active| {
                self.connection
                    .endpoint_health
                    .get(&connected_name)
                    .and_then(|health| health.iter().find(|node| node.node_index == active))
            })
            .map(|node| node.memberships.clone())
            .unwrap_or_default();
        let previous_config = self
            .connection
            .connected
            .as_ref()
            .map(|connected| connected.client_config.clone());
        let Some(connected) = self.connection.connected.as_mut() else {
            return;
        };
        if connected.active_node == node.node_index {
            return;
        }

        connected.active_node = node.node_index;
        connected.active_endpoint = node.endpoint.clone();
        connected.client_config.url = node.endpoint;
        connected.client_config.native_port = node.native_port;
        // Picking a specific node returns apply scope to that node.
        connected.apply_cluster = None;
        // Same shard (or unknown topology): switching is invisible for
        // data. A different shard is worth one honest sentence.
        self.notice = Some(
            match differentiating_cluster(&previous_memberships, &node.memberships) {
                Some(cluster) => format!(
                    "Using {} for {connected_name}: a different shard of {cluster}, \
                     so local tables show that shard's slice (Distributed tables \
                     are unaffected)",
                    node.name
                ),
                None => format!("Using {} for {connected_name}", node.name),
            },
        );
        self.load_schema_databases(cx);
        self.ops_reset(cx);
        self.restart_visible_tails(previous_config, cx);
        cx.notify();
    }
}
