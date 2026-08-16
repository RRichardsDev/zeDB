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
                native_port: Self::input("", "tcp auto", false, cx),
            }],
            user: Self::input("default", "default", false, cx),
            database: Self::input("", "optional", false, cx),
            password: Self::input("", "stored in macOS Keychain", true, cx),
            tier: EnvTier::Dev,
            read_only: true,
            driver_settings: Self::seeded_driver_settings(&[], cx),
            cloud: None,
            provision: ProvisionStage::Idle,
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
                    native_port: Self::input(
                        node.native_port
                            .map(|port| port.to_string())
                            .unwrap_or_default(),
                        "tcp auto",
                        false,
                        cx,
                    ),
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
            cloud: connection.cloud,
            provision: ProvisionStage::Idle,
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
            native_port: Self::input("", "tcp auto", false, cx),
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
        let mut bad_port = None;
        let nodes = form
            .nodes
            .iter()
            .map(|node| {
                let raw_port = value(&node.native_port);
                let native_port = if raw_port.is_empty() {
                    None
                } else {
                    match raw_port.parse::<u16>() {
                        Ok(port) if port != 0 => Some(port),
                        _ => {
                            bad_port = Some(raw_port);
                            None
                        }
                    }
                };
                ConnectionNode {
                    name: value(&node.name),
                    endpoint: value(&node.endpoint),
                    native_port,
                }
            })
            .collect::<Vec<_>>();
        if let Some(raw) = bad_port {
            return Err(format!("Native port {raw:?} is not a port number"));
        }

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
                cloud: form.cloud.clone(),
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
}
