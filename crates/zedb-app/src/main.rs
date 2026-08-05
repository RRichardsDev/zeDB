mod components;
mod grid_spike;
mod rt;
mod theme;

use std::{borrow::Cow, collections::HashMap};

use gpui::{
    div, point, prelude::*, px, rgb, size, svg, App, Application, AssetSource, Bounds, Context,
    Entity, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, SharedString,
    TitlebarOptions, Window, WindowBounds, WindowOptions,
};
use zedb_ch::{ChClient, ChConfig, ColumnInfo, DatabaseMeta, SchemaObjectKind, SchemaObjectMeta};
use zedb_core::{load_connections, save_connections, ConnectionConfig, EnvTier};

use components::text_input::{self, TextInput};
use grid_spike::GridSpike;
use theme::{BG, BG_SIDEBAR, BG_STATUS, BORDER, DANGER, SUCCESS, TEXT, TEXT_DIM};

struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        let bytes: Option<&'static [u8]> = match path {
            "icons/edit.svg" => Some(include_bytes!("../assets/icons/edit.svg")),
            "icons/refresh.svg" => Some(include_bytes!("../assets/icons/refresh.svg")),
            "icons/trash.svg" => Some(include_bytes!("../assets/icons/trash.svg")),
            _ => None,
        };
        Ok(bytes.map(Cow::Borrowed))
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        Ok(match path {
            "icons" => vec!["edit.svg".into(), "refresh.svg".into(), "trash.svg".into()],
            _ => Vec::new(),
        })
    }
}

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
    client_config: ChConfig,
}

#[derive(Clone)]
struct EndpointHealth {
    endpoint: String,
    reachable: bool,
}

struct DatabaseNode {
    meta: DatabaseMeta,
    expanded: bool,
    loading: bool,
    objects: Option<Vec<SchemaObjectMeta>>,
    error: Option<String>,
}

struct SelectedSchemaObject {
    database: String,
    object: SchemaObjectMeta,
    loading: bool,
    columns: Vec<ColumnInfo>,
    error: Option<String>,
}

struct Workspace {
    grid: Entity<GridSpike>,
    connections: Vec<ConnectionConfig>,
    selected: Option<usize>,
    connected: Option<ConnectedCluster>,
    connecting: Option<String>,
    endpoint_health: HashMap<String, Vec<EndpointHealth>>,
    password_cache: HashMap<String, Option<String>>,
    form: Option<ConnectionForm>,
    pending_delete: Option<String>,
    schema_filter: Entity<TextInput>,
    schema_connection: Option<String>,
    schema_loading: bool,
    schema_databases: Vec<DatabaseNode>,
    schema_error: Option<String>,
    selected_schema_object: Option<SelectedSchemaObject>,
    notice: Option<String>,
    show_grid_spike: bool,
    sidebar_width: f32,
    resizing_sidebar: bool,
}

impl Workspace {
    fn new(grid: Entity<GridSpike>, cx: &mut Context<Self>) -> Self {
        let schema_filter = Self::input("", "Filter schema", false, cx);
        cx.observe(&schema_filter, |_, _, cx| cx.notify()).detach();
        match load_connections() {
            Ok(connections) => Self {
                selected: (!connections.is_empty()).then_some(0),
                connections,
                grid,
                connected: None,
                connecting: None,
                endpoint_health: HashMap::new(),
                password_cache: HashMap::new(),
                form: None,
                pending_delete: None,
                schema_filter,
                schema_connection: None,
                schema_loading: false,
                schema_databases: Vec::new(),
                schema_error: None,
                selected_schema_object: None,
                notice: None,
                show_grid_spike: false,
                sidebar_width: 240.0,
                resizing_sidebar: false,
            },
            Err(error) => Self {
                grid,
                connections: Vec::new(),
                selected: None,
                connected: None,
                connecting: None,
                endpoint_health: HashMap::new(),
                password_cache: HashMap::new(),
                form: None,
                pending_delete: None,
                schema_filter,
                schema_connection: None,
                schema_loading: false,
                schema_databases: Vec::new(),
                schema_error: None,
                selected_schema_object: None,
                notice: Some(format!("Could not load connections: {error}")),
                show_grid_spike: false,
                sidebar_width: 240.0,
                resizing_sidebar: false,
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

    fn tier_colors(tier: EnvTier) -> (u32, u32) {
        match tier {
            EnvTier::Dev => (0x294132, 0x8abe94),
            EnvTier::Staging => (0x463b28, 0xc7a969),
            EnvTier::Production => (0x472d31, 0xd4868d),
        }
    }

    fn tier_badge(tier: EnvTier) -> impl IntoElement {
        let (background, foreground) = Self::tier_colors(tier);
        div()
            .px_2()
            .py(px(2.))
            .rounded(px(3.))
            .bg(rgb(background))
            .text_color(rgb(foreground))
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
                            .child(Self::tier_badge(connection.tier)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(format!("{} node(s)", connection.endpoints.len()))
                            .when(connected, |row| {
                                row.child(div().size(px(7.)).rounded_full().bg(rgb(SUCCESS)))
                            }),
                    )
            })
            .collect::<Vec<_>>();

        div()
            .w(px(self.sidebar_width))
            .flex_none()
            .h_full()
            .bg(rgb(BG_SIDEBAR))
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
                    .id("connection-list")
                    .max_h(px(220.))
                    .overflow_y_scroll()
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
                                .child(div().text_xs().text_color(rgb(DANGER)).child(format!(
                                    "Delete {name}? This also removes its saved password."
                                )))
                                .child(
                                    div()
                                        .flex()
                                        .justify_end()
                                        .gap_1()
                                        .child(
                                            div()
                                                .id("cancel-delete-connection")
                                                .px_2()
                                                .py_1()
                                                .rounded(px(3.))
                                                .text_xs()
                                                .text_color(rgb(TEXT_DIM))
                                                .child("Cancel")
                                                .hover(|button| {
                                                    button
                                                        .bg(rgb(0x303640))
                                                        .text_color(rgb(TEXT))
                                                        .cursor_pointer()
                                                })
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.cancel_delete(cx)
                                                })),
                                        )
                                        .child(
                                            div()
                                                .id("confirm-delete-connection")
                                                .px_2()
                                                .py_1()
                                                .rounded(px(3.))
                                                .text_xs()
                                                .bg(rgb(0x6f2929))
                                                .text_color(rgb(0xffb4ad))
                                                .child("Delete")
                                                .hover(|button| {
                                                    button
                                                        .bg(rgb(0x8b3434))
                                                        .text_color(rgb(0xffffff))
                                                        .cursor_pointer()
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
                                    .h(px(32.))
                                    .mx(px(-12.))
                                    .px_2()
                                    .flex()
                                    .items_center()
                                    .justify_end()
                                    .gap_1()
                                    .border_t_1()
                                    .border_color(rgb(BORDER))
                                    .child(
                                        div()
                                            .id("edit-connection")
                                            .size(px(24.))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .rounded(px(3.))
                                            .text_color(rgb(TEXT_DIM))
                                            .child(
                                                svg()
                                                    .path("icons/edit.svg")
                                                    .size(px(14.))
                                                    .text_color(rgb(TEXT_DIM)),
                                            )
                                            .hover(|button| {
                                                button
                                                    .bg(rgb(0x303640))
                                                    .text_color(rgb(TEXT))
                                                    .cursor_pointer()
                                            })
                                            .on_click(
                                                cx.listener(|this, _, _, cx| this.start_edit(cx)),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .id("delete-connection")
                                            .size(px(24.))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .rounded(px(3.))
                                            .text_color(rgb(TEXT_DIM))
                                            .child(
                                                svg()
                                                    .path("icons/trash.svg")
                                                    .size(px(14.))
                                                    .text_color(rgb(TEXT_DIM)),
                                            )
                                            .when(self.connecting.is_none(), |button| {
                                                button
                                                    .hover(|button| {
                                                        button
                                                            .bg(rgb(0x3d2528))
                                                            .text_color(rgb(DANGER))
                                                            .cursor_pointer()
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
            .child(self.schema_sidebar(cx))
    }

    fn schema_kind_label(kind: SchemaObjectKind) -> &'static str {
        match kind {
            SchemaObjectKind::Table => "T",
            SchemaObjectKind::View => "V",
            SchemaObjectKind::MaterializedView => "MV",
            SchemaObjectKind::Dictionary => "D",
        }
    }

    fn schema_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let filter = self.schema_filter.read(cx).text().to_lowercase();
        let selected = self
            .selected_schema_object
            .as_ref()
            .map(|selected| (selected.database.as_str(), selected.object.name.as_str()));
        let database_rows = self
            .schema_databases
            .iter()
            .enumerate()
            .filter_map(|(database_index, database)| {
                let database_matches = database.meta.name.to_lowercase().contains(&filter);
                let matching_objects = database
                    .objects
                    .as_ref()
                    .map(|objects| {
                        objects
                            .iter()
                            .filter(|object| {
                                filter.is_empty()
                                    || database_matches
                                    || object.name.to_lowercase().contains(&filter)
                            })
                            .cloned()
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                if !filter.is_empty() && !database_matches && matching_objects.is_empty() {
                    return None;
                }

                let database_name = database.meta.name.clone();
                let show_objects = database.expanded || !filter.is_empty();
                let object_rows = matching_objects
                    .into_iter()
                    .enumerate()
                    .map(|(object_index, object)| {
                        let is_selected =
                            selected == Some((database_name.as_str(), object.name.as_str()));
                        let row_database = database_name.clone();
                        let row_object = object.clone();
                        div()
                            .id((
                                "schema-object",
                                database_index.saturating_mul(100_000) + object_index,
                            ))
                            .h(px(26.))
                            .pl_5()
                            .pr_2()
                            .flex()
                            .items_center()
                            .gap_2()
                            .rounded(px(3.))
                            .when(is_selected, |row| row.bg(rgb(0x303640)))
                            .hover(|row| row.bg(rgb(0x2a2f37)).cursor_pointer())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.select_schema_object(
                                    row_database.clone(),
                                    row_object.clone(),
                                    cx,
                                )
                            }))
                            .child(
                                div()
                                    .w(px(20.))
                                    .text_xs()
                                    .text_color(rgb(TEXT_DIM))
                                    .child(Self::schema_kind_label(object.kind)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_color(rgb(TEXT))
                                    .child(object.name),
                            )
                    })
                    .collect::<Vec<_>>();

                Some(
                    div()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .id(("schema-database", database_index))
                                .h(px(26.))
                                .px_2()
                                .flex()
                                .items_center()
                                .gap_2()
                                .rounded(px(3.))
                                .hover(|row| row.bg(rgb(0x2a2f37)).cursor_pointer())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.toggle_schema_database(database_index, cx)
                                }))
                                .child(if database.expanded { "▾" } else { "▸" })
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_color(rgb(TEXT))
                                        .child(database.meta.name.clone()),
                                ),
                        )
                        .when(database.loading, |node| {
                            node.child(
                                div()
                                    .pl_5()
                                    .h(px(24.))
                                    .flex()
                                    .items_center()
                                    .text_xs()
                                    .child("Loading..."),
                            )
                        })
                        .when_some(database.error.as_ref(), |node, error| {
                            node.child(
                                div()
                                    .pl_5()
                                    .pr_2()
                                    .py_1()
                                    .text_xs()
                                    .text_color(rgb(DANGER))
                                    .child(error.clone()),
                            )
                        })
                        .when(show_objects, |node| node.children(object_rows)),
                )
            })
            .collect::<Vec<_>>();

        div()
            .mx(px(-12.))
            .mb(px(-12.))
            .flex_1()
            .min_h_0()
            .border_t_1()
            .border_color(rgb(BORDER))
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(34.))
                    .px_3()
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_xs()
                    .child("SCHEMA")
                    .when(self.connected.is_some(), |header| {
                        header.child(
                            div()
                                .id("refresh-schema")
                                .size(px(24.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(3.))
                                .text_color(rgb(TEXT_DIM))
                                .child(
                                    svg()
                                        .path("icons/refresh.svg")
                                        .size(px(14.))
                                        .text_color(rgb(TEXT_DIM)),
                                )
                                .hover(|button| {
                                    button
                                        .bg(rgb(0x303640))
                                        .text_color(rgb(TEXT))
                                        .cursor_pointer()
                                })
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.load_schema_databases(cx)),
                                ),
                        )
                    }),
            )
            .when(self.connected.is_some(), |panel| {
                panel.child(div().px_2().pb_2().child(self.schema_filter.clone()))
            })
            .child(
                div()
                    .id("schema-tree")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_1()
                    .when(self.connected.is_none(), |tree| {
                        tree.child(
                            div()
                                .px_2()
                                .py_2()
                                .text_xs()
                                .child("Connect to browse schema"),
                        )
                    })
                    .when(self.schema_loading, |tree| {
                        tree.child(div().px_2().py_2().text_xs().child("Loading databases..."))
                    })
                    .when_some(self.schema_error.as_ref(), |tree, error| {
                        tree.child(
                            div()
                                .px_2()
                                .py_2()
                                .text_xs()
                                .text_color(rgb(DANGER))
                                .child(error.clone()),
                        )
                    })
                    .children(database_rows),
            )
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
        self.password_cache.remove(&connection.name);
        if self
            .connected
            .as_ref()
            .map(|connected| connected.name.as_str())
            == Some(connection.name.as_str())
        {
            self.connected = None;
            self.clear_schema();
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

    fn sidebar_resize_handle(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("sidebar-resize-handle")
            .w(px(8.))
            .h_full()
            .ml(px(-4.))
            .mr(px(-4.))
            .flex_none()
            .relative()
            .cursor_col_resize()
            .child(
                div()
                    .absolute()
                    .left(px(3.))
                    .top_0()
                    .bottom_0()
                    .w(px(1.))
                    .bg(rgb(BORDER)),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, _, cx| {
                    this.resizing_sidebar = true;
                    cx.notify();
                }),
            )
    }

    fn password_for_draft(&self, draft: &ConnectionDraft) -> Result<Option<String>, String> {
        if !draft.password.is_empty() {
            return Ok(Some(draft.password.clone()));
        }
        let key_name = draft.original_name.as_deref().unwrap_or(&draft.config.name);
        zedb_core::secrets::get_password(key_name)
            .map_err(|error| format!("Could not read macOS Keychain: {error}"))
    }

    fn persist_draft(
        &mut self,
        draft: &ConnectionDraft,
        unlocked_previous_password: Option<&Option<String>>,
    ) -> Result<usize, String> {
        let name = &draft.config.name;
        let previous_connections = self.connections.clone();
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
            self.endpoint_health.remove(old_name);
            if self
                .connected
                .as_ref()
                .map(|connected| connected.name.as_str())
                == Some(old_name)
            {
                self.connected = None;
                self.clear_schema();
            }
        }
        self.connections = updated;
        self.selected = Some(index);
        Ok(index)
    }

    fn save_form(&mut self, cx: &mut Context<Self>) {
        let result = self
            .draft_from_form(cx)
            .and_then(|draft| self.persist_draft(&draft, None).map(|_| draft.config.name));
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
        let password = match self.password_cache.get(&connection.name).cloned() {
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
        let connected_password = password.clone();
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
                    let unlocked_previous_password =
                        draft.password.is_empty().then_some(&connected_password);
                    if let Err(error) =
                        this.persist_draft(draft, unlocked_previous_password)
                    {
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
                    client_config: ChConfig {
                        url: active_endpoint.clone(),
                        user: connection.user.clone(),
                        password: connected_password.clone(),
                        database: connection.database.clone(),
                        read_only: connection.read_only,
                    },
                });
                this.password_cache
                    .insert(name.clone(), connected_password.clone());
                this.notice = Some(format!(
                    "Connected to {name} via {active_endpoint} ({reachable}/{total} nodes reachable)"
                ));
                this.load_schema_databases(cx);
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
        self.clear_schema();
        cx.notify();
    }

    fn clear_schema(&mut self) {
        self.schema_connection = None;
        self.schema_loading = false;
        self.schema_databases.clear();
        self.schema_error = None;
        self.selected_schema_object = None;
    }

    fn load_schema_databases(&mut self, cx: &mut Context<Self>) {
        let Some(connected) = &self.connected else {
            self.clear_schema();
            return;
        };
        let connection_name = connected.name.clone();
        let config = connected.client_config.clone();
        self.schema_connection = Some(connection_name.clone());
        self.schema_loading = true;
        self.schema_databases.clear();
        self.schema_error = None;
        self.selected_schema_object = None;
        cx.notify();

        let task = rt::tokio().spawn(async move { ChClient::new(config).list_databases().await });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if this.connected.as_ref().map(|cluster| cluster.name.as_str())
                    != Some(connection_name.as_str())
                {
                    return;
                }
                this.schema_loading = false;
                match result {
                    Ok(Ok(databases)) => {
                        this.schema_databases = databases
                            .into_iter()
                            .map(|meta| DatabaseNode {
                                meta,
                                expanded: false,
                                loading: false,
                                objects: None,
                                error: None,
                            })
                            .collect();
                    }
                    Ok(Err(error)) => this.schema_error = Some(error.to_string()),
                    Err(error) => this.schema_error = Some(error.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn toggle_schema_database(&mut self, database_index: usize, cx: &mut Context<Self>) {
        let Some(database) = self.schema_databases.get_mut(database_index) else {
            return;
        };
        database.expanded = !database.expanded;
        if !database.expanded || database.objects.is_some() || database.loading {
            cx.notify();
            return;
        }
        let Some(connected) = &self.connected else {
            return;
        };
        let connection_name = connected.name.clone();
        let config = connected.client_config.clone();
        let database_name = database.meta.name.clone();
        database.loading = true;
        database.error = None;
        cx.notify();

        let task = rt::tokio().spawn({
            let database_name = database_name.clone();
            async move {
                ChClient::new(config)
                    .list_schema_objects(&database_name)
                    .await
            }
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if this.connected.as_ref().map(|cluster| cluster.name.as_str())
                    != Some(connection_name.as_str())
                {
                    return;
                }
                let Some(database) = this
                    .schema_databases
                    .iter_mut()
                    .find(|database| database.meta.name == database_name)
                else {
                    return;
                };
                database.loading = false;
                match result {
                    Ok(Ok(objects)) => database.objects = Some(objects),
                    Ok(Err(error)) => database.error = Some(error.to_string()),
                    Err(error) => database.error = Some(error.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn select_schema_object(
        &mut self,
        database_name: String,
        object: SchemaObjectMeta,
        cx: &mut Context<Self>,
    ) {
        let Some(connected) = &self.connected else {
            return;
        };
        let connection_name = connected.name.clone();
        let config = connected.client_config.clone();
        let object_name = object.name.clone();
        self.selected_schema_object = Some(SelectedSchemaObject {
            database: database_name.clone(),
            object,
            loading: true,
            columns: Vec::new(),
            error: None,
        });
        self.show_grid_spike = false;
        cx.notify();

        let task = rt::tokio().spawn({
            let database_name = database_name.clone();
            let object_name = object_name.clone();
            async move {
                ChClient::new(config)
                    .list_columns(&database_name, &object_name)
                    .await
            }
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if this.connected.as_ref().map(|cluster| cluster.name.as_str())
                    != Some(connection_name.as_str())
                {
                    return;
                }
                let Some(selected) = &mut this.selected_schema_object else {
                    return;
                };
                if selected.database != database_name || selected.object.name != object_name {
                    return;
                }
                selected.loading = false;
                match result {
                    Ok(Ok(columns)) => selected.columns = columns,
                    Ok(Err(error)) => selected.error = Some(error.to_string()),
                    Err(error) => selected.error = Some(error.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
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
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(div().text_xs().text_color(rgb(TEXT_DIM)).child("NAME"))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(div().flex_1().child(form.name.clone()))
                                    .child(
                                        div()
                                            .id("cycle-tier")
                                            .h(px(34.))
                                            .px_1()
                                            .flex()
                                            .items_center()
                                            .rounded(px(3.))
                                            .child(Self::tier_badge(form.tier))
                                            .hover(|button| {
                                                button.bg(rgb(BG_SIDEBAR)).cursor_pointer()
                                            })
                                            .on_click(
                                                cx.listener(|this, _, _, cx| this.cycle_tier(cx)),
                                            ),
                                    ),
                            ),
                    )
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
                        div().flex().justify_end().child(
                            div()
                                .id("toggle-read-only")
                                .w(px(250.))
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
                                .when(form.read_only, |button| button.text_color(rgb(SUCCESS)))
                                .hover(|button| button.bg(rgb(BG_SIDEBAR)).cursor_pointer())
                                .on_click(cx.listener(|this, _, _, cx| this.toggle_read_only(cx))),
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
                            Some(true) => ("reachable", SUCCESS),
                            Some(false) => ("failed", DANGER),
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

    fn format_count(value: u64) -> String {
        let digits = value.to_string();
        let mut formatted = String::with_capacity(digits.len() + digits.len() / 3);
        for (index, character) in digits.chars().enumerate() {
            if index > 0 && (digits.len() - index).is_multiple_of(3) {
                formatted.push(',');
            }
            formatted.push(character);
        }
        formatted
    }

    fn format_bytes(bytes: u64) -> String {
        const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
        let mut value = bytes as f64;
        let mut unit = 0;
        while value >= 1000.0 && unit < UNITS.len() - 1 {
            value /= 1000.0;
            unit += 1;
        }
        if unit == 0 {
            format!("{bytes} {}", UNITS[unit])
        } else {
            format!("{value:.1} {}", UNITS[unit])
        }
    }

    fn schema_object_panel(&self) -> impl IntoElement {
        let selected = self
            .selected_schema_object
            .as_ref()
            .expect("schema object panel requires a selection");
        let column_rows = selected
            .columns
            .iter()
            .enumerate()
            .map(|(index, column)| {
                div()
                    .id(("schema-column", index))
                    .h(px(30.))
                    .flex_none()
                    .px_3()
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .when(index % 2 == 1, |row| row.bg(rgb(0x1f2329)))
                    .child(
                        div()
                            .w_1_3()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_color(rgb(TEXT))
                            .child(column.name.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_color(rgb(TEXT_DIM))
                            .child(column.type_name.clone()),
                    )
            })
            .collect::<Vec<_>>();

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_none()
                    .px_4()
                    .py_3()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div().text_lg().text_color(rgb(TEXT)).child(format!(
                                    "{}.{}",
                                    selected.database, selected.object.name
                                )),
                            )
                            .child(
                                div()
                                    .px_2()
                                    .py(px(2.))
                                    .rounded(px(3.))
                                    .bg(rgb(0x303640))
                                    .text_xs()
                                    .text_color(rgb(TEXT_DIM))
                                    .child(selected.object.kind.label()),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_4()
                            .text_xs()
                            .text_color(rgb(TEXT_DIM))
                            .child(format!("Engine  {}", selected.object.engine))
                            .when_some(selected.object.total_rows, |row, rows| {
                                row.child(format!("Rows  {}", Self::format_count(rows)))
                            })
                            .when_some(selected.object.total_bytes, |row, bytes| {
                                row.child(format!("Size  {}", Self::format_bytes(bytes)))
                            }),
                    ),
            )
            .child(
                div()
                    .h(px(28.))
                    .flex_none()
                    .px_3()
                    .flex()
                    .items_center()
                    .bg(rgb(BG_SIDEBAR))
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .text_xs()
                    .text_color(rgb(TEXT_DIM))
                    .child(div().w_1_3().child("COLUMN"))
                    .child(div().flex_1().child("TYPE")),
            )
            .child(
                div()
                    .id("schema-columns")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .when(selected.loading, |columns| {
                        columns.child(
                            div()
                                .p_3()
                                .text_color(rgb(TEXT_DIM))
                                .child("Loading columns..."),
                        )
                    })
                    .when_some(selected.error.as_ref(), |columns, error| {
                        columns.child(div().p_3().text_color(rgb(DANGER)).child(error.clone()))
                    })
                    .when(
                        !selected.loading
                            && selected.error.is_none()
                            && selected.columns.is_empty(),
                        |columns| {
                            columns.child(div().p_3().text_color(rgb(TEXT_DIM)).child("No columns"))
                        },
                    )
                    .children(column_rows),
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
            .child(concat!("zedb ", env!("CARGO_PKG_VERSION"), " | M4"))
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
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                if this.resizing_sidebar {
                    this.sidebar_width = f32::from(event.position.x).clamp(180.0, 480.0);
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, _| {
                    this.resizing_sidebar = false;
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, _| {
                    this.resizing_sidebar = false;
                }),
            )
            .child(self.title_bar())
            .child(
                div()
                    .flex_1()
                    .w_full()
                    .min_h_0()
                    .flex()
                    .child(self.sidebar(cx))
                    .child(self.sidebar_resize_handle(cx))
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
                                            content
                                                .when(
                                                    self.selected_schema_object.is_some(),
                                                    |content| {
                                                        content.child(self.schema_object_panel())
                                                    },
                                                )
                                                .when(
                                                    self.selected_schema_object.is_none(),
                                                    |content| {
                                                        content.child(self.cluster_overview(cx))
                                                    },
                                                )
                                        }),
                                )
                            }),
                    ),
            )
            .child(self.status_bar())
    }
}

fn main() {
    Application::new().with_assets(Assets).run(|cx: &mut App| {
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
                cx.new(|cx| Workspace::new(grid, cx))
            },
        )
        .expect("failed to open window");
        cx.activate(true);
    });
}
