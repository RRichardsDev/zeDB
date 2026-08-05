mod components;
mod grid_spike;
mod rt;
mod theme;

use std::collections::HashMap;

use gpui::{
    div, point, prelude::*, px, rgb, size, App, Application, Bounds, Context, Entity, IntoElement,
    TitlebarOptions, Window, WindowBounds, WindowOptions,
};
use zedb_ch::{ChClient, ChConfig};
use zedb_core::{load_connections, save_connections, ConnectionConfig, EnvTier};

use components::text_input::{self, TextInput};
use grid_spike::GridSpike;
use theme::{BG, BG_SIDEBAR, BG_STATUS, BORDER, TEXT, TEXT_DIM};

struct ConnectionForm {
    editing: Option<usize>,
    original_name: Option<String>,
    name: Entity<TextInput>,
    endpoints: Vec<Entity<TextInput>>,
    user: Entity<TextInput>,
    database: Entity<TextInput>,
    password: Entity<TextInput>,
    tier: EnvTier,
    read_only: bool,
}

#[derive(Clone)]
struct ConnectionDraft {
    config: ConnectionConfig,
    password: String,
    editing: Option<usize>,
    original_name: Option<String>,
}

struct ConnectedCluster {
    name: String,
    active_endpoint: String,
}

#[derive(Clone)]
struct EndpointHealth {
    endpoint: String,
    reachable: bool,
}

struct Workspace {
    grid: Entity<GridSpike>,
    connections: Vec<ConnectionConfig>,
    selected: Option<usize>,
    connected: Option<ConnectedCluster>,
    connecting: Option<String>,
    endpoint_health: HashMap<String, Vec<EndpointHealth>>,
    form: Option<ConnectionForm>,
    pending_delete: Option<String>,
    notice: Option<String>,
    show_grid_spike: bool,
}

impl Workspace {
    fn new(grid: Entity<GridSpike>) -> Self {
        match load_connections() {
            Ok(connections) => Self {
                selected: (!connections.is_empty()).then_some(0),
                connections,
                grid,
                connected: None,
                connecting: None,
                endpoint_health: HashMap::new(),
                form: None,
                pending_delete: None,
                notice: None,
                show_grid_spike: false,
            },
            Err(error) => Self {
                grid,
                connections: Vec::new(),
                selected: None,
                connected: None,
                connecting: None,
                endpoint_health: HashMap::new(),
                form: None,
                pending_delete: None,
                notice: Some(format!("Could not load connections: {error}")),
                show_grid_spike: false,
            },
        }
    }

    fn title_bar(&self) -> impl IntoElement {
        div()
            .h(px(36.))
            .flex_none()
            .w_full()
            .bg(rgb(BG_SIDEBAR))
            .border_b_1()
            .border_color(rgb(BORDER))
            .flex()
            .items_center()
            .pl(px(90.))
            .pr_3()
            .text_sm()
            .text_color(rgb(TEXT))
            .child("zeDB")
    }

    fn tier_color(tier: EnvTier) -> u32 {
        match tier {
            EnvTier::Dev => 0x3fb950,
            EnvTier::Staging => 0xd29922,
            EnvTier::Production => 0xf85149,
        }
    }

    fn tier_badge(tier: EnvTier) -> impl IntoElement {
        div()
            .px_2()
            .py(px(2.))
            .rounded(px(3.))
            .bg(rgb(Self::tier_color(tier)))
            .text_color(rgb(0x101215))
            .text_xs()
            .child(tier.label().to_uppercase())
    }

    fn sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self
            .connections
            .iter()
            .enumerate()
            .map(|(index, connection)| {
                let selected = self.selected == Some(index);
                let connected = self
                    .connected
                    .as_ref()
                    .map(|connected| connected.name.as_str())
                    == Some(connection.name.as_str());
                div()
                    .id(("connection", index))
                    .w_full()
                    .px_2()
                    .py_2()
                    .rounded(px(3.))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .when(selected, |row| row.bg(rgb(0x303640)))
                    .hover(|row| row.bg(rgb(0x2a2f37)).cursor_pointer())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected = Some(index);
                        this.pending_delete = None;
                        this.notice = None;
                        cx.notify();
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .text_color(rgb(TEXT))
                            .child(connection.name.clone())
                            .when(connected, |row| {
                                row.child(div().size(px(7.)).rounded_full().bg(rgb(0x3fb950)))
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(Self::tier_badge(connection.tier))
                            .child(format!("{} node(s)", connection.endpoints.len())),
                    )
            })
            .collect::<Vec<_>>();

        div()
            .w(px(240.))
            .flex_none()
            .h_full()
            .bg(rgb(BG_SIDEBAR))
            .border_r_1()
            .border_color(rgb(BORDER))
            .p_3()
            .flex()
            .flex_col()
            .gap_2()
            .text_sm()
            .text_color(rgb(TEXT_DIM))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child("CONNECTIONS")
                    .child(
                        div()
                            .id("add-connection")
                            .px_2()
                            .py_1()
                            .rounded(px(3.))
                            .text_color(rgb(TEXT))
                            .child("+")
                            .hover(|button| button.bg(rgb(0x303640)).cursor_pointer())
                            .on_click(cx.listener(|this, _, _, cx| this.start_add(cx))),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .when(rows.is_empty(), |list| {
                        list.child(
                            div()
                                .pt_3()
                                .text_color(rgb(TEXT_DIM))
                                .child("No saved connections"),
                        )
                    })
                    .children(rows),
            )
            .when(self.selected.is_some(), |sidebar| {
                sidebar.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .when_some(self.pending_delete.as_ref(), |panel, name| {
                            panel
                                .child(div().text_xs().text_color(rgb(0xf85149)).child(format!(
                                    "Delete {name}? This also removes its saved password."
                                )))
                                .child(
                                    div()
                                        .flex()
                                        .gap_2()
                                        .child(
                                            div()
                                                .id("cancel-delete-connection")
                                                .flex_1()
                                                .py_2()
                                                .flex()
                                                .justify_center()
                                                .rounded(px(3.))
                                                .border_1()
                                                .border_color(rgb(BORDER))
                                                .child("Cancel")
                                                .hover(|button| {
                                                    button.bg(rgb(0x303640)).cursor_pointer()
                                                })
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.cancel_delete(cx)
                                                })),
                                        )
                                        .child(
                                            div()
                                                .id("confirm-delete-connection")
                                                .flex_1()
                                                .py_2()
                                                .flex()
                                                .justify_center()
                                                .rounded(px(3.))
                                                .bg(rgb(0x8b2d2d))
                                                .text_color(rgb(0xffffff))
                                                .child("Delete")
                                                .hover(|button| {
                                                    button.bg(rgb(0xa43a3a)).cursor_pointer()
                                                })
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.confirm_delete(cx)
                                                })),
                                        ),
                                )
                        })
                        .when(self.pending_delete.is_none(), |panel| {
                            panel.child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .child(
                                        div()
                                            .id("edit-connection")
                                            .flex_1()
                                            .py_2()
                                            .flex()
                                            .justify_center()
                                            .rounded(px(3.))
                                            .border_1()
                                            .border_color(rgb(BORDER))
                                            .text_color(rgb(TEXT))
                                            .child("Edit")
                                            .hover(|button| {
                                                button.bg(rgb(0x303640)).cursor_pointer()
                                            })
                                            .on_click(
                                                cx.listener(|this, _, _, cx| this.start_edit(cx)),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .id("delete-connection")
                                            .flex_1()
                                            .py_2()
                                            .flex()
                                            .justify_center()
                                            .rounded(px(3.))
                                            .border_1()
                                            .border_color(rgb(0x8b2d2d))
                                            .text_color(rgb(0xf85149))
                                            .child("Delete")
                                            .when(self.connecting.is_none(), |button| {
                                                button
                                                    .hover(|button| {
                                                        button.bg(rgb(0x3d2528)).cursor_pointer()
                                                    })
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.request_delete(cx)
                                                    }))
                                            }),
                                    ),
                            )
                        }),
                )
            })
    }

    fn input(
        content: impl Into<String>,
        placeholder: &'static str,
        secret: bool,
        cx: &mut Context<Self>,
    ) -> Entity<TextInput> {
        let content = content.into();
        cx.new(|cx| TextInput::new(content, placeholder, secret, cx))
    }

    fn start_add(&mut self, cx: &mut Context<Self>) {
        self.pending_delete = None;
        self.form = Some(ConnectionForm {
            editing: None,
            original_name: None,
            name: Self::input("", "staging", false, cx),
            endpoints: vec![Self::input(
                "http://localhost:8123",
                "http://host:8123",
                false,
                cx,
            )],
            user: Self::input("default", "default", false, cx),
            database: Self::input("", "optional", false, cx),
            password: Self::input("", "stored in macOS Keychain", true, cx),
            tier: EnvTier::Dev,
            read_only: true,
        });
        self.notice = None;
        cx.notify();
    }

    fn start_edit(&mut self, cx: &mut Context<Self>) {
        let Some(index) = self.selected else {
            return;
        };
        let connection = self.connections[index].clone();
        self.pending_delete = None;
        self.form = Some(ConnectionForm {
            editing: Some(index),
            original_name: Some(connection.name.clone()),
            name: Self::input(connection.name, "staging", false, cx),
            endpoints: connection
                .endpoints
                .into_iter()
                .map(|endpoint| Self::input(endpoint, "http://host:8123", false, cx))
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
        });
        self.notice = None;
        cx.notify();
    }

    fn cancel_form(&mut self, cx: &mut Context<Self>) {
        self.form = None;
        self.notice = None;
        cx.notify();
    }

    fn request_delete(&mut self, cx: &mut Context<Self>) {
        let Some(connection) = self.selected.and_then(|index| self.connections.get(index)) else {
            return;
        };
        self.pending_delete = Some(connection.name.clone());
        self.notice = None;
        cx.notify();
    }

    fn cancel_delete(&mut self, cx: &mut Context<Self>) {
        self.pending_delete = None;
        cx.notify();
    }

    fn confirm_delete(&mut self, cx: &mut Context<Self>) {
        if self.connecting.is_some() {
            self.notice = Some("Wait for the connection test to finish before deleting".into());
            cx.notify();
            return;
        }
        let Some(index) = self.selected else {
            return;
        };
        let Some(connection) = self.connections.get(index).cloned() else {
            return;
        };
        if self.pending_delete.as_deref() != Some(connection.name.as_str()) {
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

        let mut updated = self.connections.clone();
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

        self.connections = updated;
        self.endpoint_health.remove(&connection.name);
        if self
            .connected
            .as_ref()
            .map(|connected| connected.name.as_str())
            == Some(connection.name.as_str())
        {
            self.connected = None;
        }
        self.selected = if self.connections.is_empty() {
            None
        } else {
            Some(index.min(self.connections.len() - 1))
        };
        self.pending_delete = None;
        self.form = None;
        self.notice = Some(format!("Deleted {}", connection.name));
        cx.notify();
    }

    fn cycle_tier(&mut self, cx: &mut Context<Self>) {
        if let Some(form) = &mut self.form {
            form.tier = form.tier.next();
            cx.notify();
        }
    }

    fn toggle_read_only(&mut self, cx: &mut Context<Self>) {
        if let Some(form) = &mut self.form {
            form.read_only = !form.read_only;
            cx.notify();
        }
    }

    fn add_endpoint(&mut self, cx: &mut Context<Self>) {
        let endpoint = Self::input("", "http://host:8123", false, cx);
        if let Some(form) = &mut self.form {
            form.endpoints.push(endpoint);
            cx.notify();
        }
    }

    fn remove_endpoint(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(form) = &mut self.form {
            if form.endpoints.len() > 1 && index < form.endpoints.len() {
                form.endpoints.remove(index);
                cx.notify();
            }
        }
    }

    fn draft_from_form(&self, cx: &Context<Self>) -> Result<ConnectionDraft, String> {
        let form = self.form.as_ref().ok_or("Connection form is not open")?;
        let value = |input: &Entity<TextInput>| input.read(cx).text().trim().to_string();
        let name = value(&form.name);
        let user = value(&form.user);
        let database = value(&form.database);
        let endpoints = form
            .endpoints
            .iter()
            .map(value)
            .filter(|endpoint| !endpoint.is_empty())
            .collect::<Vec<_>>();

        if name.is_empty() || user.is_empty() || endpoints.is_empty() {
            return Err("Name, user, and at least one endpoint are required".into());
        }
        if self
            .connections
            .iter()
            .enumerate()
            .any(|(index, connection)| Some(index) != form.editing && connection.name == name)
        {
            return Err(format!("A connection named {name:?} already exists"));
        }

        Ok(ConnectionDraft {
            config: ConnectionConfig {
                name,
                endpoints,
                user,
                database: (!database.is_empty()).then_some(database),
                tier: form.tier,
                read_only: form.read_only,
            },
            password: form.password.read(cx).text(),
            editing: form.editing,
            original_name: form.original_name.clone(),
        })
    }

    fn password_for_draft(&self, draft: &ConnectionDraft) -> Result<Option<String>, String> {
        if !draft.password.is_empty() {
            return Ok(Some(draft.password.clone()));
        }
        let key_name = draft.original_name.as_deref().unwrap_or(&draft.config.name);
        zedb_core::secrets::get_password(key_name)
            .map_err(|error| format!("Could not read macOS Keychain: {error}"))
    }

    fn persist_draft(&mut self, draft: &ConnectionDraft) -> Result<usize, String> {
        let name = &draft.config.name;
        let previous_connections = self.connections.clone();
        let previous_password = draft
            .original_name
            .as_deref()
            .map(zedb_core::secrets::get_password)
            .transpose()
            .map_err(|error| format!("Could not read macOS Keychain: {error}"))?
            .flatten();
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
            self.endpoint_health.remove(old_name);
            if self
                .connected
                .as_ref()
                .map(|connected| connected.name.as_str())
                == Some(old_name)
            {
                self.connected = None;
            }
        }
        self.connections = updated;
        self.selected = Some(index);
        Ok(index)
    }

    fn save_form(&mut self, cx: &mut Context<Self>) {
        let result = self
            .draft_from_form(cx)
            .and_then(|draft| self.persist_draft(&draft).map(|_| draft.config.name));
        match result {
            Ok(name) => {
                self.form = None;
                self.notice = Some(format!("Saved {name} without testing"));
            }
            Err(error) => self.notice = Some(error),
        }
        cx.notify();
    }

    fn save_and_connect(&mut self, cx: &mut Context<Self>) {
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

    fn connect_selected(&mut self, cx: &mut Context<Self>) {
        let Some(index) = self.selected else {
            return;
        };
        let connection = self.connections[index].clone();
        let password = match zedb_core::secrets::get_password(&connection.name) {
            Ok(password) => password,
            Err(error) => {
                self.notice = Some(format!("Could not read macOS Keychain: {error}"));
                cx.notify();
                return;
            }
        };
        self.probe_connection(connection, password, None, cx);
    }

    fn probe_connection(
        &mut self,
        connection: ConnectionConfig,
        password: Option<String>,
        draft: Option<ConnectionDraft>,
        cx: &mut Context<Self>,
    ) {
        let name = connection.name.clone();
        let endpoints = connection.endpoints.clone();
        let user = connection.user.clone();
        let database = connection.database.clone();
        let read_only = connection.read_only;
        self.connecting = Some(name.clone());
        self.notice = Some(format!("Testing {} node(s) for {name}...", endpoints.len()));
        cx.notify();

        let task = rt::tokio().spawn(async move {
            let mut health = Vec::with_capacity(endpoints.len());
            for endpoint in endpoints {
                let client = ChClient::new(ChConfig {
                    url: endpoint.clone(),
                    user: user.clone(),
                    password: password.clone(),
                    database: database.clone(),
                    read_only,
                });
                health.push(EndpointHealth {
                    endpoint,
                    reachable: client.test_connection().await.is_ok(),
                });
            }
            health
        });
        cx.spawn(async move |this, cx| {
            let health = task.await.unwrap_or_default();
            this.update(cx, |this, cx| {
                this.connecting = None;
                let active_endpoint = health
                    .iter()
                    .find(|node| node.reachable)
                    .map(|node| node.endpoint.clone());
                let reachable = health.iter().filter(|node| node.reachable).count();
                let total = health.len();

                let Some(active_endpoint) = active_endpoint else {
                    this.endpoint_health.insert(name.clone(), health);
                    this.notice = Some(format!(
                        "No node accepted the connection details for {name}"
                    ));
                    cx.notify();
                    return;
                };

                if let Some(draft) = &draft {
                    if let Err(error) = this.persist_draft(draft) {
                        this.notice = Some(error);
                        cx.notify();
                        return;
                    }
                    this.form = None;
                }
                this.endpoint_health.insert(name.clone(), health);
                this.connected = Some(ConnectedCluster {
                    name: name.clone(),
                    active_endpoint: active_endpoint.clone(),
                });
                this.notice = Some(format!(
                    "Connected to {name} via {active_endpoint} ({reachable}/{total} nodes reachable)"
                ));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn disconnect(&mut self, cx: &mut Context<Self>) {
        if let Some(connected) = self.connected.take() {
            self.notice = Some(format!("Disconnected from {}", connected.name));
        }
        cx.notify();
    }

    fn field(label: &'static str, input: Entity<TextInput>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(div().text_xs().text_color(rgb(TEXT_DIM)).child(label))
            .child(input)
    }

    fn form_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let form = self.form.as_ref().expect("form panel requires a form");
        let endpoint_count = form.endpoints.len();
        let endpoint_rows = form
            .endpoints
            .iter()
            .cloned()
            .enumerate()
            .map(|(index, endpoint)| {
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().flex_1().child(endpoint))
                    .when(endpoint_count > 1, |row| {
                        row.child(
                            div()
                                .id(("remove-endpoint", index))
                                .w(px(30.))
                                .h(px(30.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(3.))
                                .border_1()
                                .border_color(rgb(BORDER))
                                .child("-")
                                .hover(|button| button.bg(rgb(BG_SIDEBAR)).cursor_pointer())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.remove_endpoint(index, cx)
                                })),
                        )
                    })
            })
            .collect::<Vec<_>>();
        let heading = if form.editing.is_some() {
            "Edit cluster connection"
        } else {
            "Add cluster connection"
        };
        div()
            .id("connection-form-scroll")
            .size_full()
            .overflow_y_scroll()
            .bg(rgb(BG))
            .p_6()
            .flex()
            .justify_center()
            .child(
                div()
                    .w(px(520.))
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(div().text_lg().text_color(rgb(TEXT)).child(heading))
                    .child(Self::field("NAME", form.name.clone()))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(rgb(TEXT_DIM))
                                            .child("CLUSTER NODES"),
                                    )
                                    .child(
                                        div()
                                            .id("add-endpoint")
                                            .px_2()
                                            .py_1()
                                            .rounded(px(3.))
                                            .border_1()
                                            .border_color(rgb(BORDER))
                                            .child("+ Add node")
                                            .hover(|button| {
                                                button.bg(rgb(BG_SIDEBAR)).cursor_pointer()
                                            })
                                            .on_click(
                                                cx.listener(|this, _, _, cx| this.add_endpoint(cx)),
                                            ),
                                    ),
                            )
                            .children(endpoint_rows),
                    )
                    .child(Self::field("USER", form.user.clone()))
                    .child(Self::field("DATABASE", form.database.clone()))
                    .child(Self::field("PASSWORD", form.password.clone()))
                    .child(
                        div()
                            .flex()
                            .gap_3()
                            .child(
                                div()
                                    .id("cycle-tier")
                                    .flex_1()
                                    .h(px(34.))
                                    .px_3()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .rounded(px(3.))
                                    .border_1()
                                    .border_color(rgb(BORDER))
                                    .child("Environment")
                                    .child(Self::tier_badge(form.tier))
                                    .hover(|button| button.bg(rgb(BG_SIDEBAR)).cursor_pointer())
                                    .on_click(cx.listener(|this, _, _, cx| this.cycle_tier(cx))),
                            )
                            .child(
                                div()
                                    .id("toggle-read-only")
                                    .flex_1()
                                    .h(px(34.))
                                    .px_3()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .rounded(px(3.))
                                    .border_1()
                                    .border_color(rgb(BORDER))
                                    .child("Read only")
                                    .child(if form.read_only { "ON" } else { "OFF" })
                                    .when(form.read_only, |button| button.text_color(rgb(0x3fb950)))
                                    .hover(|button| button.bg(rgb(BG_SIDEBAR)).cursor_pointer())
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.toggle_read_only(cx)),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                div()
                                    .id("cancel-connection")
                                    .px_4()
                                    .py_2()
                                    .rounded(px(3.))
                                    .border_1()
                                    .border_color(rgb(BORDER))
                                    .child("Cancel")
                                    .when(self.connecting.is_none(), |button| {
                                        button
                                            .hover(|button| {
                                                button.bg(rgb(BG_SIDEBAR)).cursor_pointer()
                                            })
                                            .on_click(
                                                cx.listener(|this, _, _, cx| this.cancel_form(cx)),
                                            )
                                    }),
                            )
                            .child(
                                div()
                                    .id("save-offline")
                                    .px_4()
                                    .py_2()
                                    .rounded(px(3.))
                                    .border_1()
                                    .border_color(rgb(BORDER))
                                    .child("Save without testing")
                                    .when(self.connecting.is_none(), |button| {
                                        button
                                            .hover(|button| {
                                                button.bg(rgb(BG_SIDEBAR)).cursor_pointer()
                                            })
                                            .on_click(
                                                cx.listener(|this, _, _, cx| this.save_form(cx)),
                                            )
                                    }),
                            )
                            .child(
                                div()
                                    .id("save-and-connect")
                                    .px_4()
                                    .py_2()
                                    .rounded(px(3.))
                                    .bg(rgb(0x2f6f9f))
                                    .text_color(rgb(0xffffff))
                                    .child(if self.connecting.is_some() {
                                        "Testing nodes..."
                                    } else {
                                        "Save & Connect"
                                    })
                                    .when(self.connecting.is_none(), |button| {
                                        button
                                            .hover(|button| {
                                                button.bg(rgb(0x3884bd)).cursor_pointer()
                                            })
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.save_and_connect(cx)
                                            }))
                                    }),
                            ),
                    ),
            )
    }

    fn connection_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.selected.and_then(|index| self.connections.get(index));
        let selected_connected = selected
            .map(|connection| {
                self.connected
                    .as_ref()
                    .map(|connected| connected.name.as_str())
                    == Some(connection.name.as_str())
            })
            .unwrap_or(false);
        div()
            .h(px(38.))
            .flex_none()
            .w_full()
            .px_3()
            .flex()
            .items_center()
            .justify_between()
            .bg(rgb(BG_SIDEBAR))
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .when_some(selected, |row, connection| {
                        row.child(connection.name.clone())
                            .child(Self::tier_badge(connection.tier))
                            .child(format!("{} node(s)", connection.endpoints.len()))
                            .when_some(self.endpoint_health.get(&connection.name), |row, health| {
                                row.child(format!(
                                    "{} reachable",
                                    health.iter().filter(|node| node.reachable).count()
                                ))
                            })
                    })
                    .when(selected.is_none(), |row| row.child("Select a connection")),
            )
            .child(
                div()
                    .id("connect-toggle")
                    .px_3()
                    .py_1()
                    .rounded(px(3.))
                    .border_1()
                    .border_color(rgb(BORDER))
                    .text_color(rgb(TEXT))
                    .child(if self.connecting.is_some() {
                        "Connecting..."
                    } else if selected_connected {
                        "Disconnect"
                    } else {
                        "Connect"
                    })
                    .when(self.connecting.is_none() && selected.is_some(), |button| {
                        button
                            .hover(|button| button.bg(rgb(0x303640)).cursor_pointer())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if selected_connected {
                                    this.disconnect(cx);
                                } else {
                                    this.connect_selected(cx);
                                }
                            }))
                    }),
            )
    }

    fn toggle_grid_spike(&mut self, cx: &mut Context<Self>) {
        self.show_grid_spike = !self.show_grid_spike;
        cx.notify();
    }

    fn cluster_overview(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.selected.and_then(|index| self.connections.get(index));
        let nodes = selected
            .map(|connection| {
                connection
                    .endpoints
                    .iter()
                    .map(|endpoint| {
                        let reachable =
                            self.endpoint_health
                                .get(&connection.name)
                                .and_then(|health| {
                                    health
                                        .iter()
                                        .find(|node| node.endpoint == *endpoint)
                                        .map(|node| node.reachable)
                                });
                        let (label, color) = match reachable {
                            Some(true) => ("reachable", 0x3fb950),
                            Some(false) => ("failed", 0xf85149),
                            None => ("not tested", TEXT_DIM),
                        };
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().size(px(7.)).rounded_full().bg(rgb(color)))
                            .child(endpoint.clone())
                            .child(div().text_xs().text_color(rgb(TEXT_DIM)).child(label))
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        div().size_full().p_6().flex().justify_center().child(
            div()
                .w(px(560.))
                .flex()
                .flex_col()
                .gap_4()
                .child(
                    div()
                        .text_lg()
                        .text_color(rgb(TEXT))
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
                })
                .when(selected.is_none(), |panel| {
                    panel.child("Add or select a cluster connection to begin.")
                })
                .child(
                    div().pt_2().text_color(rgb(TEXT_DIM)).child(
                        "The live schema tree arrives in M4. Real query results arrive in M7.",
                    ),
                )
                .child(
                    div()
                        .id("open-grid-spike")
                        .w(px(250.))
                        .px_3()
                        .py_2()
                        .rounded(px(3.))
                        .border_1()
                        .border_color(rgb(BORDER))
                        .child("Open M2 synthetic grid spike")
                        .hover(|button| button.bg(rgb(BG_SIDEBAR)).cursor_pointer())
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_grid_spike(cx))),
                ),
        )
    }

    fn grid_spike_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(34.))
                    .flex_none()
                    .px_3()
                    .flex()
                    .items_center()
                    .justify_between()
                    .bg(rgb(0x4d3b16))
                    .text_color(rgb(0xf0c36a))
                    .child("M2 synthetic grid spike: this is not ClickHouse data")
                    .child(
                        div()
                            .id("close-grid-spike")
                            .px_2()
                            .py_1()
                            .rounded(px(3.))
                            .child("Close")
                            .hover(|button| button.bg(rgb(0x665020)).cursor_pointer())
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_grid_spike(cx))),
                    ),
            )
            .child(div().flex_1().min_h_0().child(self.grid.clone()))
    }

    fn status_bar(&self) -> impl IntoElement {
        let status = self
            .notice
            .clone()
            .unwrap_or_else(|| match &self.connected {
                Some(connected) => format!(
                    "Connected to {} via {}",
                    connected.name, connected.active_endpoint
                ),
                None => "Not connected".to_string(),
            });
        div()
            .h(px(28.))
            .flex_none()
            .w_full()
            .bg(rgb(BG_STATUS))
            .border_t_1()
            .border_color(rgb(BORDER))
            .px_3()
            .flex()
            .items_center()
            .justify_between()
            .text_xs()
            .text_color(rgb(TEXT_DIM))
            .child(status)
            .child(concat!("zedb ", env!("CARGO_PKG_VERSION"), " | M3"))
    }
}

impl Render for Workspace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(BG))
            .text_color(rgb(TEXT))
            .font_family("Menlo")
            .text_sm()
            .child(self.title_bar())
            .child(
                div()
                    .flex_1()
                    .w_full()
                    .min_h_0()
                    .flex()
                    .child(self.sidebar(cx))
                    .child(
                        div()
                            .flex_1()
                            .h_full()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .when(self.form.is_some(), |main| main.child(self.form_panel(cx)))
                            .when(self.form.is_none(), |main| {
                                main.child(self.connection_toolbar(cx)).child(
                                    div()
                                        .flex_1()
                                        .min_h_0()
                                        .when(self.show_grid_spike, |content| {
                                            content.child(self.grid_spike_panel(cx))
                                        })
                                        .when(!self.show_grid_spike, |content| {
                                            content.child(self.cluster_overview(cx))
                                        }),
                                )
                            }),
                    ),
            )
            .child(self.status_bar())
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        text_input::init(cx);
        let bounds = Bounds::centered(None, size(px(1200.), px(800.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("zeDB".into()),
                    appears_transparent: true,
                    traffic_light_position: Some(point(px(12.), px(12.))),
                }),
                ..Default::default()
            },
            |_, cx| {
                let grid = cx.new(GridSpike::new);
                cx.new(|_| Workspace::new(grid))
            },
        )
        .expect("failed to open window");
        cx.activate(true);
    });
}
