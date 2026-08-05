mod components;
mod grid_spike;
mod rt;
mod theme;

use std::{
    borrow::Cow,
    collections::HashMap,
    time::{Duration, Instant},
};

use gpui::{
    actions, div, point, prelude::*, px, rgb, rgba, size, svg, App, Application, AssetSource,
    Bounds, ClipboardItem, Context, Entity, EntityInputHandler, Focusable, IntoElement, KeyBinding,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, SharedString, Timer,
    TitlebarOptions, Window, WindowBounds, WindowOptions,
};
use gpui_component::{
    button::Button,
    highlighter::HighlightTheme,
    input::{Input, InputState},
    menu::{DropdownMenu, PopupMenu},
    scroll::ScrollableElement,
    Disableable, Root, Theme,
};
use tokio::task::AbortHandle;
use zedb_ch::{
    ChClient, ChConfig, ColumnInfo, DatabaseMeta, ObjectDetails, QueryStreamEvent,
    SchemaObjectKind, SchemaObjectMeta,
};
use zedb_core::{load_connections, save_connections, ConnectionConfig, EnvTier};

use components::text_input::{self, TextInput};
use grid_spike::GridSpike;
use theme::{BG, BG_SIDEBAR, BG_STATUS, BORDER, DANGER, SUCCESS, TEXT, TEXT_DIM};

actions!(
    query_editor,
    [
        RunQuery,
        MaxRows1k,
        MaxRows10k,
        MaxRows50k,
        MaxRows100k,
        MaxRows1m,
        MaxRowsUnlimited
    ]
);

#[derive(Clone, Copy, PartialEq, Eq)]
enum MaxRows {
    Rows1k,
    Rows10k,
    Rows50k,
    Rows100k,
    Rows1m,
    Unlimited,
}

impl MaxRows {
    fn label(self) -> &'static str {
        match self {
            Self::Rows1k => "1k",
            Self::Rows10k => "10k",
            Self::Rows50k => "50k",
            Self::Rows100k => "100k",
            Self::Rows1m => "1m",
            Self::Unlimited => "Unlimited",
        }
    }

    fn limit(self) -> Option<usize> {
        match self {
            Self::Rows1k => Some(1_000),
            Self::Rows10k => Some(10_000),
            Self::Rows50k => Some(50_000),
            Self::Rows100k => Some(100_000),
            Self::Rows1m => Some(1_000_000),
            Self::Unlimited => None,
        }
    }
}

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
    details: Option<ObjectDetails>,
    tab: ObjectInspectorTab,
    error: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ObjectInspectorTab {
    Overview,
    Columns,
    Ddl,
}

struct QueryTab {
    id: usize,
    editor: Entity<InputState>,
    result_grid: Entity<GridSpike>,
    result_columns: usize,
    result_rows: usize,
    has_result: bool,
    max_rows: MaxRows,
    result_capped: bool,
    editor_height: f32,
    status_height: f32,
    outcome: QueryOutcome,
    started_at: Option<Instant>,
    elapsed: Option<Duration>,
}

enum QueryOutcome {
    Idle,
    Running,
    Complete { columns: usize, rows: usize },
    Error(String),
    Cancelled,
}

#[derive(Clone, Copy)]
enum QueryResizeTarget {
    Editor,
    Status,
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
    notice_warning: bool,
    notice_flash_id: u64,
    show_grid_spike: bool,
    sidebar_width: f32,
    resizing_sidebar: bool,
    connections_pane_height: f32,
    resizing_sidebar_sections: bool,
    query_tabs: Vec<QueryTab>,
    active_query_tab: usize,
    next_query_tab_id: usize,
    show_query_editor: bool,
    query_abort: Option<AbortHandle>,
    query_run_id: u64,
    query_resize: Option<(QueryResizeTarget, f32)>,
}

impl Workspace {
    fn new(grid: Entity<GridSpike>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let schema_filter = Self::input("", "Filter schema", false, cx);
        cx.observe(&schema_filter, |_, _, cx| cx.notify()).detach();
        let query_editor = cx.new(|cx| {
            InputState::new(window, cx)
                .code_editor("sql")
                .default_value("SELECT 1;")
        });
        let query_tabs = vec![QueryTab {
            id: 1,
            editor: query_editor,
            result_grid: cx.new(GridSpike::new),
            result_columns: 0,
            result_rows: 0,
            has_result: false,
            max_rows: MaxRows::Rows100k,
            result_capped: false,
            editor_height: 220.0,
            status_height: 52.0,
            outcome: QueryOutcome::Idle,
            started_at: None,
            elapsed: None,
        }];
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
                notice_warning: false,
                notice_flash_id: 0,
                show_grid_spike: false,
                sidebar_width: 240.0,
                resizing_sidebar: false,
                connections_pane_height: 430.0,
                resizing_sidebar_sections: false,
                query_tabs,
                active_query_tab: 0,
                next_query_tab_id: 2,
                show_query_editor: false,
                query_abort: None,
                query_run_id: 0,
                query_resize: None,
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
                notice_warning: false,
                notice_flash_id: 0,
                show_grid_spike: false,
                sidebar_width: 240.0,
                resizing_sidebar: false,
                connections_pane_height: 430.0,
                resizing_sidebar_sections: false,
                query_tabs,
                active_query_tab: 0,
                next_query_tab_id: 2,
                show_query_editor: false,
                query_abort: None,
                query_run_id: 0,
                query_resize: None,
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
            .flex()
            .flex_col()
            .text_sm()
            .text_color(rgb(TEXT_DIM))
            .child(
                div()
                    .h(px(self.connections_pane_height))
                    .min_h_0()
                    .flex_none()
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_2()
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
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scrollbar()
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
                                        .child(div().text_xs().text_color(rgb(DANGER)).child(
                                            format!(
                                                "Delete {name}? This also removes its saved password."
                                            ),
                                        ))
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
                                                        .on_click(cx.listener(
                                                            |this, _, _, cx| {
                                                                this.cancel_delete(cx)
                                                            },
                                                        )),
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
                                                        .on_click(cx.listener(
                                                            |this, _, _, cx| {
                                                                this.confirm_delete(cx)
                                                            },
                                                        )),
                                                ),
                                        )
                                })
                                .when(self.pending_delete.is_none(), |panel| {
                                    panel.child(
                                        div()
                                            .h(px(32.))
                                            .mx(px(-12.))
                                            .mb(px(-12.))
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
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.start_edit(cx)
                                                    })),
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
                                                            .on_click(cx.listener(
                                                                |this, _, _, cx| {
                                                                    this.request_delete(cx)
                                                                },
                                                            ))
                                                    }),
                                            ),
                                    )
                                }),
                        )
                    }),
            )
            .child(self.sidebar_section_resize_handle(cx))
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
            .flex_1()
            .min_h_0()
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

    fn sidebar_section_resize_handle(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("sidebar-section-resize-handle")
            .h(px(8.))
            .w_full()
            .mt(px(-4.))
            .mb(px(-4.))
            .flex_none()
            .relative()
            .cursor_row_resize()
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(3.))
                    .h(px(1.))
                    .bg(rgb(BORDER)),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, _, cx| {
                    this.resizing_sidebar_sections = true;
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
            details: None,
            tab: ObjectInspectorTab::Overview,
            error: None,
        });
        self.show_grid_spike = false;
        self.show_query_editor = false;
        cx.notify();

        let task = rt::tokio().spawn({
            let database_name = database_name.clone();
            let object_name = object_name.clone();
            async move {
                let client = ChClient::new(config);
                tokio::join!(
                    client.list_columns(&database_name, &object_name),
                    client.object_details(&database_name, &object_name)
                )
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
                    Ok((Ok(columns), Ok(details))) => {
                        selected.columns = columns;
                        selected.details = Some(details);
                    }
                    Ok((Err(error), _)) | Ok((_, Err(error))) => {
                        selected.error = Some(error.to_string());
                    }
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

    fn open_query_editor(&mut self, cx: &mut Context<Self>) {
        self.show_query_editor = true;
        self.show_grid_spike = false;
        cx.notify();
    }

    fn add_query_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.next_query_tab_id;
        self.next_query_tab_id += 1;
        let editor = cx.new(|cx| InputState::new(window, cx).code_editor("sql"));
        self.query_tabs.push(QueryTab {
            id,
            editor,
            result_grid: cx.new(GridSpike::new),
            result_columns: 0,
            result_rows: 0,
            has_result: false,
            max_rows: MaxRows::Rows100k,
            result_capped: false,
            editor_height: 220.0,
            status_height: 52.0,
            outcome: QueryOutcome::Idle,
            started_at: None,
            elapsed: None,
        });
        self.active_query_tab = self.query_tabs.len() - 1;
        self.show_query_editor = true;
        cx.notify();
    }

    fn close_query_tab(&mut self, tab_id: usize, cx: &mut Context<Self>) {
        if self.query_tabs.len() == 1 {
            return;
        }
        let Some(index) = self.query_tabs.iter().position(|tab| tab.id == tab_id) else {
            return;
        };
        if matches!(self.query_tabs[index].outcome, QueryOutcome::Running) {
            return;
        }
        self.query_tabs.remove(index);
        self.active_query_tab = self
            .active_query_tab
            .min(self.query_tabs.len().saturating_sub(1));
        cx.notify();
    }

    fn run_query_action(&mut self, _: &RunQuery, window: &mut Window, cx: &mut Context<Self>) {
        self.run_query(window, cx);
    }

    fn flash_warning(&mut self, message: impl Into<String>, cx: &mut Context<Self>) {
        self.notice = Some(message.into());
        self.notice_warning = true;
        self.notice_flash_id += 1;
        let flash_id = self.notice_flash_id;
        cx.spawn(async move |this, cx| {
            Timer::after(Duration::from_secs(1)).await;
            this.update(cx, |this, cx| {
                if this.notice_flash_id == flash_id {
                    this.notice_warning = false;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn run_query(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.query_abort.is_some() {
            return;
        }
        let Some(connected) = &self.connected else {
            self.flash_warning("Connect to a cluster before running a query", cx);
            return;
        };
        let Some(editor) = self
            .query_tabs
            .get(self.active_query_tab)
            .map(|tab| tab.editor.clone())
        else {
            return;
        };
        let sql = editor.update(cx, |editor, cx| {
            let selection = EntityInputHandler::selected_text_range(editor, false, window, cx);
            selection
                .filter(|selection| !selection.range.is_empty())
                .and_then(|selection| {
                    let mut adjusted_range = None;
                    EntityInputHandler::text_for_range(
                        editor,
                        selection.range,
                        &mut adjusted_range,
                        window,
                        cx,
                    )
                })
                .unwrap_or_else(|| editor.value().to_string())
        });
        let Some(tab) = self.query_tabs.get_mut(self.active_query_tab) else {
            return;
        };
        if sql.trim().is_empty() {
            tab.outcome = QueryOutcome::Error("Query is empty".into());
            cx.notify();
            return;
        }

        let tab_id = tab.id;
        tab.outcome = QueryOutcome::Running;
        tab.result_columns = 0;
        tab.result_rows = 0;
        tab.has_result = false;
        tab.result_capped = false;
        tab.started_at = Some(Instant::now());
        tab.elapsed = None;
        let config = connected.client_config.clone();
        let row_limit = tab.max_rows.limit();
        self.query_run_id += 1;
        let run_id = self.query_run_id;
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let task = rt::tokio().spawn(async move {
            ChClient::new(config)
                .query_stream(&sql, row_limit.unwrap_or(usize::MAX), |event| {
                    let _ = sender.send(event);
                })
                .await
        });
        self.query_abort = Some(task.abort_handle());
        cx.notify();

        cx.spawn(async move |this, cx| {
            while let Some(event) = receiver.recv().await {
                let keep_receiving = this
                    .update(cx, |this, cx| {
                        if this.query_run_id != run_id {
                            return false;
                        }
                        let Some(tab) = this.query_tabs.iter_mut().find(|tab| tab.id == tab_id)
                        else {
                            return false;
                        };
                        match event {
                            QueryStreamEvent::Columns(columns) => {
                                tab.result_columns = columns.len();
                                tab.has_result = true;
                                tab.result_grid.update(cx, |grid, cx| {
                                    grid.begin_result(columns, row_limit, cx)
                                });
                            }
                            QueryStreamEvent::Rows(rows) => {
                                tab.result_rows += rows.len();
                                tab.result_grid
                                    .update(cx, |grid, cx| grid.append_rows(rows, cx));
                            }
                        }
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !keep_receiving {
                    break;
                }
            }
            let result = task.await;
            this.update(cx, |this, cx| {
                if this.query_run_id != run_id {
                    return;
                }
                this.query_abort = None;
                let Some(tab) = this.query_tabs.iter_mut().find(|tab| tab.id == tab_id) else {
                    return;
                };
                tab.elapsed = tab.started_at.take().map(|started| started.elapsed());
                tab.outcome = match result {
                    Ok(Ok(summary)) => {
                        tab.result_capped = summary.capped;
                        tab.result_grid
                            .update(cx, |grid, cx| grid.finish_result(summary.capped, cx));
                        QueryOutcome::Complete {
                            columns: tab.result_columns,
                            rows: tab.result_rows,
                        }
                    }
                    Ok(Err(error)) => QueryOutcome::Error(error.to_string()),
                    Err(error) => QueryOutcome::Error(error.to_string()),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn select_max_rows(&mut self, max_rows: MaxRows, cx: &mut Context<Self>) {
        if let Some(tab) = self.query_tabs.get_mut(self.active_query_tab) {
            tab.max_rows = max_rows;
        }
        cx.notify();
    }

    fn max_rows_selector(&self, running: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let active = &self.query_tabs[self.active_query_tab];
        let selected = active.max_rows;
        let action_context = active.editor.focus_handle(cx);
        Button::new("query-max-rows")
            .label(format!("Max rows: {}", selected.label()))
            .dropdown_caret(true)
            .compact()
            .outline()
            .disabled(running)
            .dropdown_menu(move |menu: PopupMenu, _, _| {
                menu.action_context(action_context.clone())
                    .min_w(px(164.))
                    .menu("1,000", Box::new(MaxRows1k))
                    .menu("10,000", Box::new(MaxRows10k))
                    .menu("50,000", Box::new(MaxRows50k))
                    .menu("100,000", Box::new(MaxRows100k))
                    .menu("1,000,000", Box::new(MaxRows1m))
                    .menu("Unlimited", Box::new(MaxRowsUnlimited))
            })
    }

    fn query_resize_handle(
        &self,
        id: &'static str,
        target: QueryResizeTarget,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .id(id)
            .h(px(8.))
            .w_full()
            .mt(px(-4.))
            .mb(px(-4.))
            .flex_none()
            .relative()
            .cursor_row_resize()
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(3.))
                    .h(px(1.))
                    .bg(rgb(BORDER)),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.query_resize = Some((target, f32::from(event.position.y)));
                    cx.notify();
                }),
            )
    }

    fn cancel_query(&mut self, cx: &mut Context<Self>) {
        let Some(abort) = self.query_abort.take() else {
            return;
        };
        abort.abort();
        self.query_run_id += 1;
        if let Some(tab) = self
            .query_tabs
            .iter_mut()
            .find(|tab| matches!(tab.outcome, QueryOutcome::Running))
        {
            tab.elapsed = tab.started_at.take().map(|started| started.elapsed());
            tab.outcome = QueryOutcome::Cancelled;
        }
        cx.notify();
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
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .id("open-query-editor")
                            .px_3()
                            .py_1()
                            .rounded(px(3.))
                            .border_1()
                            .border_color(rgb(BORDER))
                            .text_color(rgb(TEXT))
                            .child("New query")
                            .hover(|button| button.bg(rgb(0x303640)).cursor_pointer())
                            .on_click(cx.listener(|this, _, _, cx| this.open_query_editor(cx))),
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
                    ),
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

    fn schema_object_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
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

        let loading = selected.loading;
        let error = selected.error.clone();
        let details = selected.details.clone();
        let tab = selected.tab;
        let tab_bar = div()
            .h(px(34.))
            .flex_none()
            .px_3()
            .flex()
            .items_end()
            .gap_4()
            .border_b_1()
            .border_color(rgb(BORDER))
            .children(
                [
                    (ObjectInspectorTab::Overview, "Overview"),
                    (ObjectInspectorTab::Columns, "Columns"),
                    (ObjectInspectorTab::Ddl, "DDL"),
                ]
                .into_iter()
                .enumerate()
                .map(|(index, (button_tab, label))| {
                    div()
                        .id(("object-inspector-tab", index))
                        .h_full()
                        .px_1()
                        .flex()
                        .items_center()
                        .border_b_2()
                        .when(tab == button_tab, |button| {
                            button.border_color(rgb(0x6f8fac)).text_color(rgb(TEXT))
                        })
                        .when(tab != button_tab, |button| {
                            button
                                .border_color(rgb(BG))
                                .text_color(rgb(TEXT_DIM))
                                .hover(|button| button.text_color(rgb(TEXT)).cursor_pointer())
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(selected) = &mut this.selected_schema_object {
                                selected.tab = button_tab;
                                cx.notify();
                            }
                        }))
                        .child(label)
                }),
            );

        let content = match tab {
            ObjectInspectorTab::Overview => {
                let engine_definition = details.as_ref().map(|details| details.engine_full.clone());
                let metadata = details.as_ref().map(|details| {
                    [
                        ("Partition key", details.partition_key.clone()),
                        ("Order by", details.sorting_key.clone()),
                        ("Primary key", details.primary_key.clone()),
                    ]
                    .into_iter()
                    .map(|(label, value)| {
                        div()
                            .py_3()
                            .border_b_1()
                            .border_color(rgb(BORDER))
                            .flex()
                            .gap_4()
                            .child(
                                div()
                                    .w(px(150.))
                                    .flex_none()
                                    .text_color(rgb(TEXT_DIM))
                                    .child(label),
                            )
                            .child(div().flex_1().min_w_0().text_color(rgb(TEXT)).child(
                                if value.is_empty() {
                                    "None".to_string()
                                } else {
                                    value
                                },
                            ))
                    })
                    .collect::<Vec<_>>()
                });
                div().flex_1().min_h_0().child(
                    div()
                        .id("object-overview")
                        .size_full()
                        .overflow_y_scroll()
                        .px_4()
                        .py_2()
                        .when(loading, |panel| {
                            panel.child(
                                div()
                                    .py_3()
                                    .text_color(rgb(TEXT_DIM))
                                    .child("Loading details..."),
                            )
                        })
                        .when_some(error.as_ref(), |panel, error| {
                            panel.child(div().py_3().text_color(rgb(DANGER)).child(error.clone()))
                        })
                        .when_some(engine_definition, |panel, engine| {
                            panel.child(
                                div()
                                    .py_3()
                                    .border_b_1()
                                    .border_color(rgb(BORDER))
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .child(
                                        div().text_color(rgb(TEXT_DIM)).child("Engine definition"),
                                    )
                                    .child(
                                        div()
                                            .id("engine-definition")
                                            .w_full()
                                            .px_3()
                                            .py_2()
                                            .rounded(px(3.))
                                            .border_1()
                                            .border_color(rgb(BORDER))
                                            .bg(rgb(0x191c21))
                                            .whitespace_normal()
                                            .text_color(rgb(TEXT))
                                            .child(engine),
                                    ),
                            )
                        })
                        .when_some(metadata, |panel, rows| panel.children(rows)),
                )
            }
            ObjectInspectorTab::Columns => div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
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
                        .when(loading, |columns| {
                            columns.child(
                                div()
                                    .p_3()
                                    .text_color(rgb(TEXT_DIM))
                                    .child("Loading columns..."),
                            )
                        })
                        .when_some(error.as_ref(), |columns, error| {
                            columns.child(div().p_3().text_color(rgb(DANGER)).child(error.clone()))
                        })
                        .when(
                            !loading && error.is_none() && selected.columns.is_empty(),
                            |columns| {
                                columns.child(
                                    div().p_3().text_color(rgb(TEXT_DIM)).child("No columns"),
                                )
                            },
                        )
                        .children(column_rows),
                ),
            ObjectInspectorTab::Ddl => {
                let ddl = details
                    .as_ref()
                    .map(|details| details.create_table_query.clone())
                    .unwrap_or_default();
                let clipboard_ddl = ddl.clone();
                let lines = ddl
                    .lines()
                    .enumerate()
                    .map(|(index, line)| {
                        div()
                            .id(("ddl-line", index))
                            .min_h(px(22.))
                            .flex()
                            .whitespace_nowrap()
                            .child(
                                div()
                                    .w(px(42.))
                                    .pr_3()
                                    .flex_none()
                                    .text_right()
                                    .text_color(rgb(0x5f6672))
                                    .child((index + 1).to_string()),
                            )
                            .child(div().flex_1().min_w_0().child(line.to_string()))
                    })
                    .collect::<Vec<_>>();
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .h(px(34.))
                            .flex_none()
                            .px_3()
                            .flex()
                            .items_center()
                            .justify_end()
                            .border_b_1()
                            .border_color(rgb(BORDER))
                            .child(
                                div()
                                    .id("copy-object-ddl")
                                    .px_2()
                                    .py_1()
                                    .rounded(px(3.))
                                    .text_xs()
                                    .text_color(rgb(TEXT_DIM))
                                    .when(!ddl.is_empty(), |button| {
                                        button
                                            .hover(|button| {
                                                button
                                                    .bg(rgb(BG_SIDEBAR))
                                                    .text_color(rgb(TEXT))
                                                    .cursor_pointer()
                                            })
                                            .on_click(cx.listener(move |_, _, _, cx| {
                                                cx.write_to_clipboard(ClipboardItem::new_string(
                                                    clipboard_ddl.clone(),
                                                ));
                                            }))
                                    })
                                    .child("Copy DDL"),
                            ),
                    )
                    .child(
                        div()
                            .id("object-ddl")
                            .flex_1()
                            .min_h_0()
                            .m_3()
                            .overflow_scroll()
                            .p_3()
                            .rounded(px(3.))
                            .border_1()
                            .border_color(rgb(BORDER))
                            .bg(rgb(0x191c21))
                            .text_color(rgb(TEXT))
                            .when(loading, |panel| panel.child("Loading DDL..."))
                            .when_some(error.as_ref(), |panel, error| {
                                panel.child(div().text_color(rgb(DANGER)).child(error.clone()))
                            })
                            .when(!loading && error.is_none() && ddl.is_empty(), |panel| {
                                panel
                                    .child(div().text_color(rgb(TEXT_DIM)).child("DDL unavailable"))
                            })
                            .children(lines),
                    )
            }
        };

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
            .child(tab_bar)
            .child(content)
    }

    fn query_editor_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let tab_rows = self
            .query_tabs
            .iter()
            .enumerate()
            .map(|(index, tab)| {
                let tab_id = tab.id;
                div()
                    .id(("query-tab", tab_id))
                    .h_full()
                    .px_3()
                    .flex()
                    .items_center()
                    .border_b_2()
                    .when(index == self.active_query_tab, |tab| {
                        tab.border_color(rgb(0x6f8fac)).text_color(rgb(TEXT))
                    })
                    .when(index != self.active_query_tab, |tab| {
                        tab.border_color(rgb(BG_SIDEBAR))
                            .text_color(rgb(TEXT_DIM))
                            .hover(|tab| tab.text_color(rgb(TEXT)).cursor_pointer())
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.active_query_tab = index;
                        cx.notify();
                    }))
                    .gap_2()
                    .child(format!("Query {tab_id}"))
                    .when(self.query_tabs.len() > 1, |tab_row| {
                        tab_row.child(
                            div()
                                .id(("close-query-tab", tab_id))
                                .size(px(18.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(3.))
                                .text_color(rgb(TEXT_DIM))
                                .child("×")
                                .when(!matches!(tab.outcome, QueryOutcome::Running), |close| {
                                    close
                                        .hover(|close| {
                                            close
                                                .bg(rgb(0x303640))
                                                .text_color(rgb(TEXT))
                                                .cursor_pointer()
                                        })
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.close_query_tab(tab_id, cx);
                                        }))
                                }),
                        )
                    })
            })
            .collect::<Vec<_>>();
        let active = self
            .query_tabs
            .get(self.active_query_tab)
            .expect("query editor requires an active tab");
        let running = matches!(active.outcome, QueryOutcome::Running);
        let has_result = active.has_result;
        let result_capped = active.result_capped;
        let editor_height = active.editor_height;
        let status_height = active.status_height;
        let result_grid = active.result_grid.clone();
        let status = match &active.outcome {
            QueryOutcome::Idle => "Ready".to_string(),
            QueryOutcome::Running => format!("Running: {} row(s)", active.result_rows),
            QueryOutcome::Complete { columns, rows } => {
                if result_capped {
                    format!("Showing first {rows} row(s), {columns} column(s)")
                } else {
                    format!("Complete: {rows} row(s), {columns} column(s)")
                }
            }
            QueryOutcome::Error(error) => error.clone(),
            QueryOutcome::Cancelled => "Query cancelled".to_string(),
        };
        let elapsed = active.elapsed.map(format_query_duration);

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(36.))
                    .flex_none()
                    .flex()
                    .items_end()
                    .justify_between()
                    .bg(rgb(BG_SIDEBAR))
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(
                        div().h_full().flex().items_end().children(tab_rows).child(
                            div()
                                .id("add-query-tab")
                                .h_full()
                                .px_3()
                                .flex()
                                .items_center()
                                .text_color(rgb(TEXT_DIM))
                                .child("+")
                                .hover(|button| button.text_color(rgb(TEXT)).cursor_pointer())
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.add_query_tab(window, cx)
                                })),
                        ),
                    )
                    .child(
                        div()
                            .h_full()
                            .pr_2()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(self.max_rows_selector(running, cx))
                            .child(
                                div()
                                    .id("cancel-query")
                                    .px_3()
                                    .py_1()
                                    .rounded(px(3.))
                                    .text_color(rgb(TEXT_DIM))
                                    .when(running, |button| {
                                        button
                                            .text_color(rgb(DANGER))
                                            .hover(|button| {
                                                button.bg(rgb(0x3d2528)).cursor_pointer()
                                            })
                                            .on_click(
                                                cx.listener(|this, _, _, cx| this.cancel_query(cx)),
                                            )
                                    })
                                    .child("Cancel"),
                            )
                            .child(
                                div()
                                    .id("run-query")
                                    .px_3()
                                    .py_1()
                                    .rounded(px(3.))
                                    .bg(rgb(0x2f6f9f))
                                    .text_color(rgb(0xffffff))
                                    .child("Run  ⌘↵")
                                    .when(!running, |button| {
                                        button
                                            .hover(|button| {
                                                button.bg(rgb(0x3884bd)).cursor_pointer()
                                            })
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.run_query(window, cx)
                                            }))
                                    }),
                            ),
                    ),
            )
            .child(
                div()
                    .when(!has_result, |editor| editor.flex_1())
                    .when(has_result, |editor| editor.h(px(editor_height)).flex_none())
                    .min_h_0()
                    .relative()
                    .bg(rgb(BG))
                    .child(
                        Input::new(&active.editor)
                            .appearance(false)
                            .bordered(false)
                            .focus_bordered(false)
                            .pl(px(4.))
                            .h_full(),
                    )
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .top_0()
                            .bottom_0()
                            .w(px(50.))
                            .bg(rgba(0x15181c48))
                            .border_r_1()
                            .border_color(rgb(0x2b3037)),
                    ),
            )
            .when(has_result, |panel| {
                panel.child(self.query_resize_handle(
                    "query-editor-resize-handle",
                    QueryResizeTarget::Editor,
                    cx,
                ))
            })
            .when(has_result, |panel| {
                panel.child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .border_t_1()
                        .border_color(rgb(BORDER))
                        .child(result_grid),
                )
            })
            .child(self.query_resize_handle(
                "query-status-resize-handle",
                QueryResizeTarget::Status,
                cx,
            ))
            .child(
                div()
                    .h(px(status_height))
                    .flex_none()
                    .px_3()
                    .py_2()
                    .overflow_y_scrollbar()
                    .border_t_1()
                    .border_color(rgb(BORDER))
                    .when(matches!(active.outcome, QueryOutcome::Error(_)), |row| {
                        row.bg(rgb(0x2b2227)).text_color(rgb(DANGER))
                    })
                    .when(!matches!(active.outcome, QueryOutcome::Error(_)), |row| {
                        row.text_color(rgb(TEXT_DIM))
                    })
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .items_start()
                            .justify_between()
                            .gap_4()
                            .child(div().flex_1().min_w_0().child(status))
                            .when_some(elapsed, |row, elapsed| {
                                row.child(
                                    div().flex_none().text_color(rgb(TEXT_DIM)).child(elapsed),
                                )
                            }),
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
            .text_color(rgb(if self.notice_warning {
                DANGER
            } else {
                TEXT_DIM
            }))
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
            .on_action(cx.listener(Self::run_query_action))
            .on_action(
                cx.listener(|this, _: &MaxRows1k, _, cx| this.select_max_rows(MaxRows::Rows1k, cx)),
            )
            .on_action(
                cx.listener(|this, _: &MaxRows10k, _, cx| {
                    this.select_max_rows(MaxRows::Rows10k, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &MaxRows50k, _, cx| {
                    this.select_max_rows(MaxRows::Rows50k, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &MaxRows100k, _, cx| {
                this.select_max_rows(MaxRows::Rows100k, cx)
            }))
            .on_action(
                cx.listener(|this, _: &MaxRows1m, _, cx| this.select_max_rows(MaxRows::Rows1m, cx)),
            )
            .on_action(cx.listener(|this, _: &MaxRowsUnlimited, _, cx| {
                this.select_max_rows(MaxRows::Unlimited, cx)
            }))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                if this.resizing_sidebar {
                    this.sidebar_width = f32::from(event.position.x).clamp(180.0, 480.0);
                    cx.notify();
                }
                if this.resizing_sidebar_sections {
                    let viewport_height = f32::from(window.viewport_size().height);
                    let maximum = (viewport_height - 220.0).max(140.0);
                    this.connections_pane_height =
                        (f32::from(event.position.y) - 36.0).clamp(140.0, maximum);
                    cx.notify();
                }
                if let Some((target, last_y)) = this.query_resize {
                    let current_y = f32::from(event.position.y);
                    let delta = current_y - last_y;
                    if let Some(tab) = this.query_tabs.get_mut(this.active_query_tab) {
                        match target {
                            QueryResizeTarget::Editor => {
                                tab.editor_height = (tab.editor_height + delta).clamp(80.0, 720.0);
                            }
                            QueryResizeTarget::Status => {
                                tab.status_height = (tab.status_height - delta).clamp(34.0, 240.0);
                            }
                        }
                    }
                    this.query_resize = Some((target, current_y));
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, _| {
                    this.resizing_sidebar = false;
                    this.resizing_sidebar_sections = false;
                    this.query_resize = None;
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, _| {
                    this.resizing_sidebar = false;
                    this.resizing_sidebar_sections = false;
                    this.query_resize = None;
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
                                        .when(self.show_query_editor, |content| {
                                            content.child(self.query_editor_panel(cx))
                                        })
                                        .when(
                                            !self.show_query_editor && self.show_grid_spike,
                                            |content| content.child(self.grid_spike_panel(cx)),
                                        )
                                        .when(
                                            !self.show_query_editor && !self.show_grid_spike,
                                            |content| {
                                                content
                                                    .when(
                                                        self.selected_schema_object.is_some(),
                                                        |content| {
                                                            content
                                                                .child(self.schema_object_panel(cx))
                                                        },
                                                    )
                                                    .when(
                                                        self.selected_schema_object.is_none(),
                                                        |content| {
                                                            content.child(self.cluster_overview(cx))
                                                        },
                                                    )
                                            },
                                        ),
                                )
                            }),
                    ),
            )
            .child(self.status_bar())
    }
}

fn format_query_duration(duration: Duration) -> String {
    if duration.as_secs() >= 60 {
        let minutes = duration.as_secs() / 60;
        let seconds = duration.as_secs() % 60;
        format!("{minutes}m {seconds:02}s")
    } else if duration.as_millis() >= 1_000 {
        format!("{:.2} s", duration.as_secs_f64())
    } else if duration.as_millis() < 1 {
        format!("{:.3} ms", duration.as_secs_f64() * 1_000.)
    } else if duration.as_millis() < 100 {
        format!("{:.1} ms", duration.as_secs_f64() * 1_000.)
    } else {
        format!("{} ms", duration.as_millis())
    }
}

fn configure_component_theme(cx: &mut App) {
    let theme = Theme::global_mut(cx);
    theme.font_family = "Menlo".into();
    theme.mono_font_family = "Menlo".into();
    theme.font_size = px(14.);
    theme.mono_font_size = px(14.);
    theme.colors.background = rgb(BG).into();
    theme.colors.foreground = rgb(TEXT).into();
    theme.colors.popover = rgb(BG_SIDEBAR).into();
    theme.colors.popover_foreground = rgb(TEXT_DIM).into();
    theme.colors.accent = rgb(0x303640).into();
    theme.colors.accent_foreground = rgb(TEXT).into();
    theme.colors.secondary_foreground = rgb(TEXT_DIM).into();
    theme.colors.border = rgb(BORDER).into();
    theme.colors.input = rgb(BORDER).into();
    theme.colors.muted = rgb(BG_SIDEBAR).into();
    theme.colors.muted_foreground = rgb(TEXT_DIM).into();
    theme.colors.caret = rgb(0x9bb8d1).into();
    theme.colors.selection = rgb(0x3b5063).into();
    theme.colors.ring = rgb(0x6f8fac).into();
    theme.highlight_theme = HighlightTheme::default_dark();
    let highlight = std::sync::Arc::make_mut(&mut theme.highlight_theme);
    highlight.style.editor_background = Some(rgb(BG).into());
    highlight.style.editor_foreground = Some(rgb(TEXT).into());
    highlight.style.editor_active_line = Some(rgb(0x23282f).into());
    highlight.style.editor_line_number = Some(rgb(TEXT_DIM).into());
    highlight.style.editor_active_line_number = Some(rgb(TEXT).into());
}

fn main() {
    Application::new().with_assets(Assets).run(|cx: &mut App| {
        gpui_component::init(cx);
        configure_component_theme(cx);
        text_input::init(cx);
        cx.bind_keys([KeyBinding::new("cmd-enter", RunQuery, None)]);
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
            |window, cx| {
                let grid = cx.new(GridSpike::new);
                let workspace = cx.new(|cx| Workspace::new(grid, window, cx));
                cx.new(|cx| Root::new(workspace, window, cx))
            },
        )
        .expect("failed to open window");
        cx.activate(true);
    });
}
