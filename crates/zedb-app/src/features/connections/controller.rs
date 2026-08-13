use crate::*;

use gpui::prelude::*;

impl Workspace {
    pub(crate) fn input(
        content: impl Into<String>,
        placeholder: &'static str,
        secret: bool,
        cx: &mut Context<Self>,
    ) -> Entity<TextInput> {
        let content = content.into();
        cx.new(|cx| TextInput::new(content, placeholder, secret, cx))
    }

    pub(crate) fn start_add(&mut self, cx: &mut Context<Self>) {
        self.connection.pending_delete = None;
        self.connection.form = Some(ConnectionForm {
            editing: None,
            original_name: None,
            name: Self::input("", "staging", false, cx),
            nodes: vec![NodeForm {
                name: Self::input("Node 1", "Node 1", false, cx),
                endpoint: Self::input("http://localhost:8123", "http://host:8123", false, cx),
            }],
            user: Self::input("default", "default", false, cx),
            database: Self::input("", "optional", false, cx),
            password: Self::input("", "stored in macOS Keychain", true, cx),
            tier: EnvTier::Dev,
            read_only: true,
            driver_settings: Self::seeded_driver_settings(&[], cx),
        });
        self.notice = None;
        cx.notify();
    }

    pub(crate) fn start_edit(&mut self, cx: &mut Context<Self>) {
        let Some(index) = self.connection.selected else {
            return;
        };
        let connection = self.connection.connections[index].clone();
        self.connection.pending_delete = None;
        self.connection.form = Some(ConnectionForm {
            editing: Some(index),
            original_name: Some(connection.name.clone()),
            name: Self::input(connection.name, "staging", false, cx),
            nodes: connection
                .nodes
                .into_iter()
                .map(|node| NodeForm {
                    name: Self::input(node.name, "Node name", false, cx),
                    endpoint: Self::input(node.endpoint, "http://host:8123", false, cx),
                })
                .collect(),
            user: Self::input(connection.user, "default", false, cx),
            database: Self::input(
                connection.database.unwrap_or_default(),
                "optional",
                false,
                cx,
            ),
            password: Self::input("", "leave blank to keep existing", true, cx),
            tier: connection.tier,
            read_only: connection.read_only,
            driver_settings: Self::seeded_driver_settings(&connection.driver.settings, cx),
        });
        self.notice = None;
        cx.notify();
    }

    pub(crate) fn cancel_form(&mut self, cx: &mut Context<Self>) {
        self.connection.form = None;
        self.notice = None;
        cx.notify();
    }

    pub(crate) fn request_delete(&mut self, cx: &mut Context<Self>) {
        let Some(connection) = self
            .connection
            .selected
            .and_then(|index| self.connection.connections.get(index))
        else {
            return;
        };
        self.connection.pending_delete = Some(connection.name.clone());
        self.notice = None;
        cx.notify();
    }

    pub(crate) fn cancel_delete(&mut self, cx: &mut Context<Self>) {
        self.connection.pending_delete = None;
        cx.notify();
    }

    pub(crate) fn confirm_delete(&mut self, cx: &mut Context<Self>) {
        if self.connection.connecting.is_some() {
            self.notice = Some("Wait for the connection test to finish before deleting".into());
            cx.notify();
            return;
        }
        let Some(index) = self.connection.selected else {
            return;
        };
        let Some(connection) = self.connection.connections.get(index).cloned() else {
            return;
        };
        if self.connection.pending_delete.as_deref() != Some(connection.name.as_str()) {
            return;
        }

        let previous_password = match zedb_core::secrets::get_password(&connection.name) {
            Ok(password) => password,
            Err(error) => {
                self.notice = Some(format!("Could not read macOS Keychain: {error}"));
                cx.notify();
                return;
            }
        };
        if let Err(error) = zedb_core::secrets::delete_password(&connection.name) {
            self.notice = Some(format!(
                "Could not remove password from macOS Keychain: {error}"
            ));
            cx.notify();
            return;
        }

        let mut updated = self.connection.connections.clone();
        updated.remove(index);
        if let Err(error) = save_connections(&updated) {
            let restore_error = previous_password.as_deref().and_then(|password| {
                zedb_core::secrets::set_password(&connection.name, password).err()
            });
            self.notice = Some(match restore_error {
                Some(restore_error) => format!(
                    "Could not delete connection: {error}. Could not restore its Keychain password: {restore_error}"
                ),
                None => format!("Could not delete connection: {error}"),
            });
            cx.notify();
            return;
        }

        self.connection.connections = updated;
        self.connection.endpoint_health.remove(&connection.name);
        self.connection.password_cache.remove(&connection.name);
        if self
            .connection
            .connected
            .as_ref()
            .map(|connected| connected.name.as_str())
            == Some(connection.name.as_str())
        {
            self.connection.connected = None;
            self.fleet.write_unlocked = false;
            self.clear_schema();
        }
        self.connection.selected = if self.connection.connections.is_empty() {
            None
        } else {
            Some(index.min(self.connection.connections.len() - 1))
        };
        self.connection.pending_delete = None;
        self.connection.form = None;
        self.notice = Some(format!("Deleted {}", connection.name));
        self.settings_sync_tick(cx);
        cx.notify();
    }

    /// The connection form's inputs in visual order, for tab cycling.
    pub(crate) fn form_focus_order(&self) -> Vec<Entity<components::text_input::TextInput>> {
        let Some(form) = &self.connection.form else {
            return Vec::new();
        };
        let mut order = vec![form.name.clone()];
        for node in &form.nodes {
            order.push(node.name.clone());
            order.push(node.endpoint.clone());
        }
        order.push(form.user.clone());
        order.push(form.database.clone());
        order.push(form.password.clone());
        for setting in &form.driver_settings {
            order.push(setting.name.clone());
            order.push(setting.value.clone());
        }
        order
    }

    /// Tab / shift-tab moves focus between the form's fields.
    pub(crate) fn form_tab(
        &mut self,
        backwards: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let order = self.form_focus_order();
        if order.is_empty() {
            return;
        }
        let focused = order
            .iter()
            .position(|input| input.read(cx).focus_handle(cx).is_focused(window));
        let next = match focused {
            Some(index) if backwards => (index + order.len() - 1) % order.len(),
            Some(index) => (index + 1) % order.len(),
            None => 0,
        };
        window.focus(&order[next].read(cx).focus_handle(cx));
        cx.notify();
    }

    pub(crate) fn cycle_tier(&mut self, cx: &mut Context<Self>) {
        if let Some(form) = &mut self.connection.form {
            form.tier = form.tier.next();
            cx.notify();
        }
    }

    pub(crate) fn toggle_read_only(&mut self, cx: &mut Context<Self>) {
        if let Some(form) = &mut self.connection.form {
            form.read_only = !form.read_only;
            cx.notify();
        }
    }

    /// The saved driver settings as form rows, with the two well-known
    /// names seeded (empty, so they drop on save unless filled) when
    /// absent. They behave exactly like manually added rows.
    pub(crate) fn seeded_driver_settings(
        saved: &[zedb_core::DriverSetting],
        cx: &mut Context<Self>,
    ) -> Vec<DriverSettingForm> {
        let mut rows: Vec<DriverSettingForm> = Vec::new();
        for name in ["max_execution_time", "connect_timeout"] {
            if !saved.iter().any(|setting| setting.name == name) {
                rows.push(DriverSettingForm {
                    name: Self::input(name, "setting", false, cx),
                    value: Self::input(
                        "",
                        if name == "connect_timeout" {
                            "10"
                        } else {
                            "seconds"
                        },
                        false,
                        cx,
                    ),
                });
            }
        }
        rows.extend(saved.iter().map(|setting| DriverSettingForm {
            name: Self::input(setting.name.clone(), "setting", false, cx),
            value: Self::input(setting.value.clone(), "value", false, cx),
        }));
        rows
    }

    pub(crate) fn remove_driver_setting(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(form) = &mut self.connection.form {
            if index < form.driver_settings.len() {
                form.driver_settings.remove(index);
                cx.notify();
            }
        }
    }

    pub(crate) fn add_driver_setting(&mut self, cx: &mut Context<Self>) {
        let setting = DriverSettingForm {
            name: Self::input("", "setting", false, cx),
            value: Self::input("", "value", false, cx),
        };
        if let Some(form) = &mut self.connection.form {
            form.driver_settings.push(setting);
            cx.notify();
        }
    }

    pub(crate) fn add_endpoint(&mut self, cx: &mut Context<Self>) {
        let next_number = self
            .connection
            .form
            .as_ref()
            .map(|form| form.nodes.len() + 1)
            .unwrap_or(1);
        let node = NodeForm {
            name: Self::input(format!("Node {next_number}"), "Node name", false, cx),
            endpoint: Self::input("", "http://host:8123", false, cx),
        };
        if let Some(form) = &mut self.connection.form {
            form.nodes.push(node);
            cx.notify();
        }
    }

    pub(crate) fn remove_endpoint(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(form) = &mut self.connection.form {
            if form.nodes.len() > 1 && index < form.nodes.len() {
                form.nodes.remove(index);
                cx.notify();
            }
        }
    }

    pub(crate) fn draft_from_form(&self, cx: &Context<Self>) -> Result<ConnectionDraft, String> {
        let form = self
            .connection
            .form
            .as_ref()
            .ok_or("Connection form is not open")?;
        let value = |input: &Entity<TextInput>| input.read(cx).text().trim().to_string();
        let name = value(&form.name);
        let user = value(&form.user);
        let database = value(&form.database);
        let nodes = form
            .nodes
            .iter()
            .map(|node| ConnectionNode {
                name: value(&node.name),
                endpoint: value(&node.endpoint),
            })
            .collect::<Vec<_>>();

        if name.is_empty()
            || user.is_empty()
            || nodes.is_empty()
            || nodes
                .iter()
                .any(|node| node.name.is_empty() || node.endpoint.is_empty())
        {
            return Err("Name, user, and every node name and endpoint are required".into());
        }
        let mut node_names = std::collections::HashSet::new();
        if nodes.iter().any(|node| !node_names.insert(&node.name)) {
            return Err("Node names must be unique within a connection".into());
        }
        if self
            .connection
            .connections
            .iter()
            .enumerate()
            .any(|(index, connection)| Some(index) != form.editing && connection.name == name)
        {
            return Err(format!("A connection named {name:?} already exists"));
        }

        let driver = zedb_core::DriverConfig {
            settings: form
                .driver_settings
                .iter()
                .filter_map(|setting| {
                    let name = value(&setting.name);
                    let setting_value = value(&setting.value);
                    (!name.is_empty() && !setting_value.is_empty()).then_some(
                        zedb_core::DriverSetting {
                            name,
                            value: setting_value,
                        },
                    )
                })
                .collect(),
        };

        Ok(ConnectionDraft {
            config: ConnectionConfig {
                name,
                nodes,
                user,
                database: (!database.is_empty()).then_some(database),
                tier: form.tier,
                read_only: form.read_only,
                driver,
            },
            password: form.password.read(cx).text(),
            editing: form.editing,
            original_name: form.original_name.clone(),
        })
    }

    /// Duplicate a saved connection under a fresh name. Passwords live
    /// in the keychain keyed by connection name, so the copy has no
    /// credentials until its first connect asks for them.
    pub(crate) fn duplicate_connection(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(original) = self.connection.connections.get(index) else {
            return;
        };
        let mut copy = original.clone();
        let base = format!("{} copy", copy.name);
        let mut name = base.clone();
        let mut suffix = 2;
        while self.connection.connections.iter().any(|c| c.name == name) {
            name = format!("{base} {suffix}");
            suffix += 1;
        }
        copy.name = name.clone();
        self.connection.connections.push(copy);
        match save_connections(&self.connection.connections) {
            Ok(()) => {
                self.connection.selected = Some(self.connection.connections.len() - 1);
                self.notice = Some(format!(
                    "Duplicated as \"{name}\"; the password is not copied, connecting will ask for it"
                ));
                self.notice_warning = false;
                self.settings_sync_tick(cx);
            }
            Err(error) => {
                self.connection.connections.pop();
                self.notice = Some(format!("Could not save connections: {error}"));
                self.notice_warning = true;
                self.notice_flash_id += 1;
            }
        }
        cx.notify();
    }

    pub(crate) fn sidebar_resize_handle(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // Deferred paint keeps the whole band topmost, so the resize
        // cursor shows across it instead of only the exposed sliver.
        gpui::deferred(
            div()
                .id("sidebar-resize-handle")
                .w(px(13.))
                .h_full()
                .ml(px(-6.))
                .mr(px(-6.))
                .flex_none()
                .relative()
                .cursor_col_resize()
                .child(
                    div()
                        .absolute()
                        .left(px(6.))
                        .top_0()
                        .bottom_0()
                        .w(px(1.))
                        .bg(theme::border()),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, _, cx| {
                        this.resizing_sidebar = true;
                        cx.notify();
                    }),
                ),
        )
    }

    pub(crate) fn sidebar_section_resize_handle(&self, cx: &mut Context<Self>) -> impl IntoElement {
        gpui::deferred(
            div()
                .id("sidebar-section-resize-handle")
                .h(px(13.))
                .w_full()
                .mt(px(-6.))
                .mb(px(-6.))
                .flex_none()
                .relative()
                .cursor_row_resize()
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .top(px(6.))
                        .h(px(1.))
                        .bg(theme::border()),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, _, cx| {
                        this.resizing_sidebar_sections = true;
                        cx.notify();
                    }),
                ),
        )
    }

    pub(crate) fn password_for_draft(
        &self,
        draft: &ConnectionDraft,
    ) -> Result<Option<String>, String> {
        if !draft.password.is_empty() {
            return Ok(Some(draft.password.clone()));
        }
        let key_name = draft.original_name.as_deref().unwrap_or(&draft.config.name);
        zedb_core::secrets::get_password(key_name)
            .map_err(|error| format!("Could not read macOS Keychain: {error}"))
    }

    pub(crate) fn persist_draft(
        &mut self,
        draft: &ConnectionDraft,
        unlocked_previous_password: Option<&Option<String>>,
    ) -> Result<usize, String> {
        let name = &draft.config.name;
        let previous_connections = self.connection.connections.clone();
        let previous_password = match draft.original_name.as_deref() {
            None => None,
            Some(_) if unlocked_previous_password.is_some() => {
                unlocked_previous_password.cloned().flatten()
            }
            Some(old_name) => zedb_core::secrets::get_password(old_name)
                .map_err(|error| format!("Could not read macOS Keychain: {error}"))?,
        };
        let mut updated = previous_connections.clone();
        let index = match draft.editing {
            Some(index) => {
                updated[index] = draft.config.clone();
                index
            }
            None => {
                updated.push(draft.config.clone());
                updated.len() - 1
            }
        };
        save_connections(&updated)
            .map_err(|error| format!("Could not save connections: {error}"))?;

        let secret_result = if draft.password.is_empty() {
            match draft.original_name.as_deref() {
                Some(old) if old != name => zedb_core::secrets::rename(old, name),
                _ => Ok(()),
            }
        } else {
            zedb_core::secrets::set_password(name, &draft.password).and_then(|_| {
                if let Some(old) = draft.original_name.as_deref().filter(|old| *old != name) {
                    zedb_core::secrets::delete_password(old)?;
                }
                Ok(())
            })
        };
        if let Err(error) = secret_result {
            let rollback_error = save_connections(&previous_connections).err();
            if let Some(old_name) = draft.original_name.as_deref() {
                if let Some(password) = previous_password.as_deref() {
                    let _ = zedb_core::secrets::set_password(old_name, password);
                }
            }
            if draft.original_name.as_deref() != Some(name.as_str()) {
                let _ = zedb_core::secrets::delete_password(name);
            }
            return Err(match rollback_error {
                Some(rollback_error) => format!(
                    "Could not update macOS Keychain: {error}. Could not roll back connection config: {rollback_error}"
                ),
                None => format!("Could not update macOS Keychain: {error}"),
            });
        }

        if let Some(old_name) = draft.original_name.as_deref() {
            self.connection.endpoint_health.remove(old_name);
            if self
                .connection
                .connected
                .as_ref()
                .map(|connected| connected.name.as_str())
                == Some(old_name)
            {
                self.connection.connected = None;
                self.fleet.write_unlocked = false;
                self.fleet.write_unlocked = false;
                self.clear_schema();
            }
        }
        self.connection.connections = updated;
        self.connection.selected = Some(index);
        Ok(index)
    }

    pub(crate) fn save_form(&mut self, cx: &mut Context<Self>) {
        let result = self
            .draft_from_form(cx)
            .and_then(|draft| self.persist_draft(&draft, None).map(|_| draft.config.name));
        match result {
            Ok(name) => {
                self.connection.form = None;
                self.notice = Some(format!("Saved {name} without testing"));
                self.settings_sync_tick(cx);
            }
            Err(error) => self.notice = Some(error),
        }
        cx.notify();
    }

    pub(crate) fn save_and_connect(&mut self, cx: &mut Context<Self>) {
        let draft = match self.draft_from_form(cx) {
            Ok(draft) => draft,
            Err(error) => {
                self.notice = Some(error);
                cx.notify();
                return;
            }
        };
        let password = match self.password_for_draft(&draft) {
            Ok(password) => password,
            Err(error) => {
                self.notice = Some(error);
                cx.notify();
                return;
            }
        };
        self.probe_connection(draft.config.clone(), password, Some(draft), cx);
    }

    pub(crate) fn connect_selected(&mut self, cx: &mut Context<Self>) {
        let Some(index) = self.connection.selected else {
            return;
        };
        let connection = self.connection.connections[index].clone();
        let password = match self
            .connection
            .password_cache
            .get(&connection.name)
            .cloned()
        {
            Some(password) => password,
            None => match zedb_core::secrets::get_password(&connection.name) {
                Ok(password) => password,
                Err(error) => {
                    self.notice = Some(format!("Could not read macOS Keychain: {error}"));
                    cx.notify();
                    return;
                }
            },
        };
        self.probe_connection(connection, password, None, cx);
    }

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
        let Some(connected) = self.connection.connected.as_mut() else {
            return;
        };
        if connected.active_node == node.node_index {
            return;
        }

        connected.active_node = node.node_index;
        connected.active_endpoint = node.endpoint.clone();
        connected.client_config.url = node.endpoint;
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
        cx.notify();
    }
}
