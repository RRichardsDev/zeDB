use crate::*;

use gpui::prelude::*;

impl Workspace {
    pub(crate) fn probe_connection(
        &mut self,
        connection: ConnectionConfig,
        password: Option<String>,
        draft: Option<ConnectionDraft>,
        cx: &mut Context<Self>,
    ) {
        let name = connection.name.clone();
        let nodes = connection.nodes.clone();
        let user = connection.user.clone();
        let database = connection.database.clone();
        let read_only = connection.read_only;
        let driver = connection.driver.clone();
        let connected_password = password.clone();
        self.connection.connecting = Some(name.clone());
        self.notice = Some(format!("Testing {} node(s) for {name}...", nodes.len()));
        cx.notify();

        let cache_name = name.clone();
        let task = rt::tokio().spawn(async move {
            let schema_cache = SchemaCache::for_connection(&cache_name);
            let mut health = Vec::with_capacity(nodes.len());
            for (node_index, node) in nodes.into_iter().enumerate() {
                let client = ChClient::new(ChConfig {
                    url: node.endpoint.clone(),
                    user: user.clone(),
                    password: password.clone(),
                    database: database.clone(),
                    read_only,
                    driver: driver.clone(),
                });
                let reachable = client.test_connection().await.is_ok();
                let memberships = if reachable {
                    client.cluster_memberships().await.unwrap_or_default()
                } else {
                    Vec::new()
                };
                health.push(EndpointHealth {
                    node_index,
                    name: node.name,
                    endpoint: node.endpoint,
                    reachable,
                    memberships,
                });
            }
            (health, schema_cache)
        });
        cx.spawn(async move |this, cx| {
            let Ok((health, schema_cache)) = task.await else {
                this.update(cx, |this, cx| {
                    this.connection.connecting = None;
                    this.flash_warning("Connection task stopped unexpectedly", cx);
                })
                .ok();
                return;
            };
            this.update(cx, |this, cx| {
                this.connection.connecting = None;
                let active_node = health.iter().find(|node| node.reachable).cloned();
                let reachable = health.iter().filter(|node| node.reachable).count();
                let total = health.len();

                let Some(active_node) = active_node else {
                    this.connection.endpoint_health.insert(name.clone(), health);
                    this.flash_warning(
                        format!("No node accepted the connection details for {name}"),
                        cx,
                    );
                    return;
                };

                if let Some(draft) = &draft {
                    let unlocked_previous_password =
                        draft.password.is_empty().then_some(&connected_password);
                    if let Err(error) = this.persist_draft(draft, unlocked_previous_password) {
                        this.notice = Some(error);
                        cx.notify();
                        return;
                    }
                    this.connection.form = None;
                }
                this.connection.endpoint_health.insert(name.clone(), health);
                this.fleet.write_unlocked = false;
                this.connection.connected = Some(ConnectedCluster {
                    name: name.clone(),
                    active_node: active_node.node_index,
                    active_endpoint: active_node.endpoint.clone(),
                    client_config: ChConfig {
                        url: active_node.endpoint.clone(),
                        user: connection.user.clone(),
                        password: connected_password.clone(),
                        database: connection.database.clone(),
                        read_only: connection.read_only,
                        driver: connection.driver.clone(),
                    },
                    apply_cluster: None,
                });
                this.connection
                    .password_cache
                    .insert(name.clone(), connected_password.clone());
                match schema_cache {
                    Ok(cache) => this.schema.cache = Some(cache),
                    Err(error) => {
                        this.schema.cache = None;
                        this.flash_warning(
                            format!("Connected, but the schema cache could not open: {error}"),
                            cx,
                        );
                    }
                }
                this.schema
                    .provider
                    .set_context(this.schema.cache.clone(), connection.database.clone());
                // Land in the query view; the connection screen's job
                // is done.
                this.show_fleet = false;
                this.show_query_editor = true;
                this.start_health_poll(cx);
                this.notice = Some(format!(
                    "Connected to {name} via {} ({reachable}/{total} nodes reachable)",
                    active_node.name
                ));
                this.load_schema_databases(cx);
                this.settings_sync_tick(cx);
                this.ops_reset(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Health poll: every five minutes run SELECT 1 through the active
    /// node; on failure flip to disconnected and mark the node unhealthy,
    /// so the next query attempt gets the usual connect-first warning.
    /// One quiet health probe plus update check, run on window refocus.
    pub(crate) fn focus_recheck(&mut self, cx: &mut Context<Self>) {
        self.theme_recheck(cx);
        self.settings_sync_tick(cx);
        // Update check: same quiet path as the periodic loop.
        let update_handle = rt::tokio().spawn(updates::check());
        cx.spawn(async move |this, cx| {
            let update = update_handle.await.ok().flatten();
            if let Some(update) = update {
                this.update(cx, |this, cx| {
                    let fresh = this
                        .update_available
                        .as_ref()
                        .map(|current| current.version != update.version)
                        .unwrap_or(true);
                    if fresh && this.update_phase == UpdatePhase::Available {
                        this.update_available = Some(update);
                        cx.notify();
                    }
                })
                .ok();
            }
        })
        .detach();

        // Health probe: one shot of the poll's body; a dead connection
        // disconnects exactly like the poll would.
        let Some(connected) = &self.connection.connected else {
            return;
        };
        let config = connected.client_config.clone();
        let name = connected.name.clone();
        let node_index = connected.active_node;
        let schema_cache = self.schema.cache.clone();
        let generation = self.health_poll_generation;
        cx.spawn(async move |this, cx| {
            let healthy = rt::tokio()
                .spawn(async move {
                    let client = ChClient::new(config);
                    if client.query("SELECT 1").await.is_err() {
                        return false;
                    }
                    if let Some(cache) = schema_cache {
                        let _ = cache.refresh_tables(&client).await;
                    }
                    true
                })
                .await
                .unwrap_or(false);
            if healthy {
                return;
            }
            this.update(cx, |this, cx| {
                if this.health_poll_generation != generation {
                    return;
                }
                let still_here = this
                    .connection
                    .connected
                    .as_ref()
                    .is_some_and(|connected| connected.name == name);
                if !still_here {
                    return;
                }
                this.connection.connected = None;
                this.schema.cache = None;
                this.schema.provider.set_context(None, None);
                this.fleet.write_unlocked = false;
                if let Some(health) = this.connection.endpoint_health.get_mut(&name) {
                    if let Some(node) = health.iter_mut().find(|node| node.node_index == node_index)
                    {
                        node.reachable = false;
                    }
                }
                this.flash_warning(
                    format!("Lost connection to {name}; the node stopped answering"),
                    cx,
                );
            })
            .ok();
        })
        .detach();
    }
}
