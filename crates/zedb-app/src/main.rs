mod agent_pane;
mod author;
mod codegen;
mod commit;
mod components;
mod fleet;
mod grid_spike;
mod rt;
mod schema_intelligence_ui;
mod theme;
mod updates;
mod vim;

use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    rc::Rc,
    time::{Duration, Instant},
};

use gpui::{
    actions, div, img, point, prelude::*, px, rgb, rgba, size, svg, Action, App, Application,
    AssetSource, Bounds, ClipboardItem, Context, Entity, EntityInputHandler, Focusable,
    IntoElement, KeyBinding, Keystroke, Menu, MenuItem, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, SharedString, SystemMenuType, Timer, TitlebarOptions, Window,
    WindowBounds, WindowOptions,
};
use gpui_component::{
    button::Button,
    highlighter::{Diagnostic, DiagnosticSeverity, HighlightTheme},
    input::{Input, InputEvent, InputState, Position},
    menu::{ContextMenuExt, DropdownMenu, PopupMenu},
    scroll::ScrollableElement,
    Disableable, Root, Theme,
};
use tokio::task::AbortHandle;
use zedb_ch::{
    schema_cache::{CachedObjectKind, SchemaCache},
    ChClient, ChConfig, ColumnInfo, DatabaseMeta, ObjectDetails, QueryStreamEvent,
    QueryStreamSummary, SchemaObjectKind, SchemaObjectMeta,
};
use zedb_core::{
    load_connections, load_preferences, save_connections, save_preferences, ConnectionConfig,
    ConnectionNode, EnvTier, Preferences,
};

use components::text_input::{self, TextInput};
use fleet::FleetState;
use grid_spike::GridSpike;
use schema_intelligence_ui::{byte_range_to_lsp, SchemaProvider};
use theme::{BG, BG_SIDEBAR, BG_STATUS, BORDER, DANGER, SUCCESS, TEXT, TEXT_DIM};
use vim::{CommandLineSnapshot, VimController};

/// The query a fresh install starts with.
const DEFAULT_QUERY: &str = "select * from PERFOMANCE.ActivityFacts;";

fn format_engine_definition(engine: &str) -> String {
    let mut formatted = format!("ENGINE = {engine}");
    for clause in [
        " PARTITION BY ",
        " PRIMARY KEY ",
        " ORDER BY ",
        " SAMPLE BY ",
        " TTL ",
        " SETTINGS ",
    ] {
        formatted = formatted.replace(clause, &format!("\n{} ", clause.trim()));
    }
    formatted
}

actions!(
    query_editor,
    [
        OpenAbout,
        CheckForUpdates,
        OpenPreferences,
        ToggleAgentPane,
        QuitZeDb,
        RunQuery,
        RunSelection,
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
            "icons/chevron-down.svg" => Some(include_bytes!("../assets/icons/chevron-down.svg")),
            "icons/close.svg" => Some(include_bytes!("../assets/icons/close.svg")),
            "icons/edit.svg" => Some(include_bytes!("../assets/icons/edit.svg")),
            "icons/copy.svg" => Some(include_bytes!("../assets/icons/copy.svg")),
            "icons/check-chain.svg" => Some(include_bytes!("../assets/icons/check-chain.svg")),
            "icons/commit.svg" => Some(include_bytes!("../assets/icons/commit.svg")),
            "icons/fleet.svg" => Some(include_bytes!("../assets/icons/fleet.svg")),
            "icons/migration-plus.svg" => {
                Some(include_bytes!("../assets/icons/migration-plus.svg"))
            }
            "icons/regen.svg" => Some(include_bytes!("../assets/icons/regen.svg")),
            "icons/folder-open.svg" => Some(include_bytes!("../assets/icons/folder-open.svg")),
            "icons/lock.svg" => Some(include_bytes!("../assets/icons/lock.svg")),
            "icons/lock-open.svg" => Some(include_bytes!("../assets/icons/lock-open.svg")),
            "icons/plug.svg" => Some(include_bytes!("../assets/icons/plug.svg")),
            "icons/pull.svg" => Some(include_bytes!("../assets/icons/pull.svg")),
            "icons/query-plus.svg" => Some(include_bytes!("../assets/icons/query-plus.svg")),
            "icons/refresh.svg" => Some(include_bytes!("../assets/icons/refresh.svg")),
            "icons/agent-claude.svg" => Some(include_bytes!("../assets/icons/agent-claude.svg")),
            "icons/agent-codex.svg" => Some(include_bytes!("../assets/icons/agent-codex.svg")),
            "icons/send.svg" => Some(include_bytes!("../assets/icons/send.svg")),
            "icons/verify.svg" => Some(include_bytes!("../assets/icons/verify.svg")),
            "icons/sparkle.svg" => Some(include_bytes!("../assets/icons/sparkle.svg")),
            "icons/stop.svg" => Some(include_bytes!("../assets/icons/stop.svg")),
            "icons/trash.svg" => Some(include_bytes!("../assets/icons/trash.svg")),
            "about-logo.png" => Some(include_bytes!("../assets/about-logo.png")),
            _ => None,
        };
        Ok(bytes.map(Cow::Borrowed))
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        Ok(match path {
            "icons" => vec![
                "chevron-down.svg".into(),
                "close.svg".into(),
                "edit.svg".into(),
                "refresh.svg".into(),
                "trash.svg".into(),
            ],
            _ => Vec::new(),
        })
    }
}

struct ConnectionForm {
    editing: Option<usize>,
    original_name: Option<String>,
    name: Entity<TextInput>,
    nodes: Vec<NodeForm>,
    user: Entity<TextInput>,
    database: Entity<TextInput>,
    password: Entity<TextInput>,
    tier: EnvTier,
    read_only: bool,
}

struct NodeForm {
    name: Entity<TextInput>,
    endpoint: Entity<TextInput>,
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
    active_node: usize,
    active_endpoint: String,
    client_config: ChConfig,
}

#[derive(Clone)]
struct EndpointHealth {
    node_index: usize,
    name: String,
    endpoint: String,
    reachable: bool,
}

#[derive(Clone, PartialEq, Action)]
#[action(no_json, no_register)]
struct SelectNode {
    index: usize,
}

#[derive(Clone, PartialEq, Action)]
#[action(no_json, no_register)]
struct DuplicateConnection {
    index: usize,
}

#[derive(Clone, PartialEq, Action)]
#[action(no_json, no_register)]
struct EditConnection {
    index: usize,
}

#[derive(Clone, PartialEq, Action)]
#[action(no_json, no_register)]
struct ViewObjectDdl {
    database: String,
    object: String,
}

#[derive(Clone, PartialEq, Action)]
#[action(no_json, no_register)]
struct DeleteConnection {
    index: usize,
}

struct DatabaseNode {
    meta: DatabaseMeta,
    expanded: bool,
    /// While a filter is active every matching database auto-expands;
    /// this records an explicit collapse under that filter. Reset
    /// whenever the filter text changes.
    filter_collapsed: bool,
    loading: bool,
    objects: Option<Vec<SchemaObjectMeta>>,
    error: Option<String>,
}

fn database_nodes_from_cache(cache: &SchemaCache) -> Vec<DatabaseNode> {
    let snapshot = cache.snapshot();
    let mut databases: Vec<_> = snapshot
        .databases
        .values()
        .map(|database| DatabaseNode {
            meta: DatabaseMeta {
                name: database.name.clone(),
            },
            expanded: false,
            filter_collapsed: false,
            loading: false,
            objects: None,
            error: None,
        })
        .collect();
    databases.sort_by(|left, right| left.meta.name.cmp(&right.meta.name));
    databases
}

fn schema_object_from_cache(object: &zedb_ch::schema_cache::CachedObject) -> SchemaObjectMeta {
    SchemaObjectMeta {
        name: object.name.clone(),
        engine: object.engine.clone(),
        kind: match object.kind {
            CachedObjectKind::Table => SchemaObjectKind::Table,
            CachedObjectKind::View => SchemaObjectKind::View,
            CachedObjectKind::MaterializedView => SchemaObjectKind::MaterializedView,
            CachedObjectKind::Dictionary => SchemaObjectKind::Dictionary,
        },
        total_rows: object.total_rows,
        total_bytes: None,
    }
}

struct SelectedSchemaObject {
    database: String,
    object: SchemaObjectMeta,
    loading: bool,
    columns: Vec<ColumnInfo>,
    details: Option<ObjectDetails>,
    ddl_editor: Entity<InputState>,
    engine_editor: Entity<InputState>,
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
    read_rows: Option<u64>,
    read_bytes: Option<u64>,
    total_rows: Option<u64>,
    received_bytes: u64,
    editor_height: f32,
    status_height: f32,
    outcome: QueryOutcome,
    started_at: Option<Instant>,
    elapsed: Option<Duration>,
    vim: VimController,
    vim_command_line: Option<CommandLineSnapshot>,
    vim_recording: Option<char>,
    schema_analysis_generation: u64,
    /// The last successfully executed statement, i.e. the one whose
    /// result the grid is showing; header sorts rewrite and re-run it.
    displayed_statement: Option<String>,
}

enum QueryOutcome {
    Idle,
    Running,
    Complete {
        columns: usize,
        rows: usize,
        skipped: usize,
    },
    Error(String),
    /// A statement in a multi-statement run failed and the run is paused
    /// waiting for the user to skip it or cancel the rest.
    StatementError {
        index: usize,
        total: usize,
        message: String,
    },
    Cancelled,
}

enum RunEvent {
    Stream(QueryStreamEvent),
    StatementFailed {
        index: usize,
        total: usize,
        message: String,
        decision: tokio::sync::oneshot::Sender<bool>,
    },
}

#[derive(Clone, Copy)]
enum QueryResizeTarget {
    Editor,
    Status,
}

struct Workspace {
    fleet: FleetState,
    agent: agent_pane::AgentPaneState,
    author: Option<author::AuthorState>,
    regen: Option<codegen::RegenState>,
    checks: Option<codegen::ChecksState>,
    commit: Option<commit::CommitState>,
    show_fleet: bool,
    health_poll_generation: u64,
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
    schema_cache: Option<SchemaCache>,
    schema_provider: Rc<SchemaProvider>,
    /// Databases with a column fetch in flight, so warm-up paths
    /// (sidebar, object clicks, editor references) never double-fetch.
    schema_warming: HashSet<String>,
    schema_loading: bool,
    schema_databases: Vec<DatabaseNode>,
    schema_error: Option<String>,
    selected_schema_object: Option<SelectedSchemaObject>,
    notice: Option<String>,
    notice_warning: bool,
    notice_flash_id: u64,
    update_available: Option<updates::UpdateInfo>,
    update_phase: UpdatePhase,
    sidebar_width: f32,
    resizing_sidebar: bool,
    connections_pane_height: f32,
    resizing_sidebar_sections: bool,
    query_tabs: Vec<QueryTab>,
    active_query_tab: usize,
    next_query_tab_id: usize,
    show_query_editor: bool,
    query_abort: Option<AbortHandle>,
    /// The latest header-driven rewrite awaiting its debounced re-run.
    rerun_pending: Option<String>,
    rerun_generation: u64,
    query_error_decision: Option<tokio::sync::oneshot::Sender<bool>>,
    query_run_id: u64,
    query_resize: Option<(QueryResizeTarget, f32)>,
    preferences: Preferences,
    show_preferences: bool,
    show_about: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum UpdatePhase {
    Available,
    Installing,
    Ready,
}

impl Workspace {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (preferences, preferences_error) = match load_preferences() {
            Ok(preferences) => (preferences, None),
            Err(error) => (
                Preferences::default(),
                Some(format!("Could not load preferences: {error}")),
            ),
        };
        let fleet_repo_path = preferences.fleet_repo.clone();
        let fleet_cluster = preferences.fleet_cluster.clone();
        // Check for updates now and every five minutes (the health-poll
        // cadence), so long-running instances learn about releases too.
        cx.spawn(async move |this, cx| loop {
            let update = rt::tokio().spawn(updates::check()).await.ok().flatten();
            if let Some(update) = update {
                this.update(cx, |this, cx| {
                    let fresh = this
                        .update_available
                        .as_ref()
                        .map(|current| current.version != update.version)
                        .unwrap_or(true);
                    // Never disturb an install already in progress or
                    // waiting on a restart.
                    if fresh && this.update_phase == UpdatePhase::Available {
                        this.update_available = Some(update);
                        cx.notify();
                    }
                })
                .ok();
            }
            Timer::after(Duration::from_secs(300)).await;
        })
        .detach();
        let schema_filter = Self::input("", "Filter schema", false, cx);
        cx.observe(&schema_filter, |this: &mut Self, _, cx| {
            for database in &mut this.schema_databases {
                database.filter_collapsed = false;
            }
            cx.notify()
        })
        .detach();
        // Vim keys must be intercepted before action dispatch: the editor's
        // own key bindings (Enter, Backspace, arrows) run before any key-down
        // listener and would edit the buffer behind modalkit's back.
        let workspace = cx.entity().downgrade();
        cx.intercept_keystrokes(move |event, window, cx| {
            if let Some(workspace) = workspace.upgrade() {
                workspace.update(cx, |this, cx| {
                    // Escape closes an open filter popover regardless of
                    // where focus sits (checkbox panels hold none).
                    if event.keystroke.key == "escape" {
                        if let Some(tab) = this.query_tabs.get(this.active_query_tab) {
                            let closed = tab
                                .result_grid
                                .update(cx, |grid, cx| grid.close_filter_panel(cx));
                            if closed {
                                cx.stop_propagation();
                                return;
                            }
                        }
                    }
                    this.vim_keystroke(&event.keystroke, window, cx)
                });
            }
        })
        .detach();
        // Tabs from the previous session come back; first launch starts with
        // the sample query.
        let saved_session = zedb_core::take_session();
        cx.on_app_quit(|this: &mut Self, cx| {
            let session = this.session_snapshot(cx);
            async move {
                let _ = zedb_core::save_session(&session);
            }
        })
        .detach();
        let tab_contents: Vec<String> = match &saved_session {
            Some(session) if !session.tabs.is_empty() => {
                session.tabs.iter().map(|tab| tab.sql.clone()).collect()
            }
            _ => vec![DEFAULT_QUERY.to_string()],
        };
        let active_query_tab = saved_session
            .as_ref()
            .map(|session| session.active_tab.min(tab_contents.len() - 1))
            .unwrap_or(0);
        let next_query_tab_id = tab_contents.len() + 1;
        let schema_provider = SchemaProvider::new();
        let query_tabs: Vec<QueryTab> = tab_contents
            .into_iter()
            .enumerate()
            .map(|(index, sql)| {
                Self::make_query_tab(index + 1, &sql, schema_provider.clone(), window, cx)
            })
            .collect();
        match load_connections() {
            Ok(connections) => Self {
                selected: (!connections.is_empty()).then_some(0),
                connections,
                connected: None,
                connecting: None,
                endpoint_health: HashMap::new(),
                password_cache: HashMap::new(),
                form: None,
                pending_delete: None,
                schema_filter,
                schema_connection: None,
                schema_cache: None,
                schema_provider: schema_provider.clone(),
                schema_warming: HashSet::new(),
                schema_loading: false,
                schema_databases: Vec::new(),
                schema_error: None,
                selected_schema_object: None,
                notice: None,
                notice_warning: false,
                notice_flash_id: 0,
                update_available: None,
                update_phase: UpdatePhase::Available,
                sidebar_width: 240.0,
                resizing_sidebar: false,
                connections_pane_height: 430.0,
                resizing_sidebar_sections: false,
                query_tabs,
                active_query_tab,
                next_query_tab_id,
                show_query_editor: false,
                fleet: FleetState::new(
                    fleet_repo_path.as_deref().unwrap_or(""),
                    fleet_cluster.as_deref().unwrap_or(""),
                    window,
                    cx,
                ),
                show_fleet: false,
                agent: agent_pane::AgentPaneState::new(
                    preferences.agent_pane_width.unwrap_or(420.0),
                ),
                author: None,
                regen: None,
                checks: None,
                commit: None,
                health_poll_generation: 0,
                query_abort: None,
                rerun_pending: None,
                rerun_generation: 0,
                query_error_decision: None,
                query_run_id: 0,
                query_resize: None,
                preferences,
                show_preferences: false,
                show_about: false,
            },
            Err(error) => Self {
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
                schema_cache: None,
                schema_provider,
                schema_warming: HashSet::new(),
                schema_loading: false,
                schema_databases: Vec::new(),
                schema_error: None,
                selected_schema_object: None,
                notice: Some(format!("Could not load connections: {error}")),
                notice_warning: false,
                notice_flash_id: 0,
                update_available: None,
                update_phase: UpdatePhase::Available,
                sidebar_width: 240.0,
                resizing_sidebar: false,
                connections_pane_height: 430.0,
                resizing_sidebar_sections: false,
                query_tabs,
                active_query_tab,
                next_query_tab_id,
                show_query_editor: false,
                fleet: FleetState::new(
                    fleet_repo_path.as_deref().unwrap_or(""),
                    fleet_cluster.as_deref().unwrap_or(""),
                    window,
                    cx,
                ),
                show_fleet: false,
                agent: agent_pane::AgentPaneState::new(
                    preferences.agent_pane_width.unwrap_or(420.0),
                ),
                author: None,
                regen: None,
                checks: None,
                commit: None,
                health_poll_generation: 0,
                query_abort: None,
                rerun_pending: None,
                rerun_generation: 0,
                query_error_decision: None,
                query_run_id: 0,
                query_resize: None,
                preferences,
                show_preferences: false,
                show_about: false,
            },
        }
        .with_startup_notice(preferences_error)
    }

    fn with_startup_notice(mut self, notice: Option<String>) -> Self {
        if notice.is_some() {
            self.notice = notice;
        }
        self
    }

    fn title_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
            .child(div().flex_1())
            .when_some(self.update_available.clone(), |bar, update| {
                let phase = self.update_phase;
                let (prefix, version) = match phase {
                    UpdatePhase::Available => {
                        ("Update available:", Some(format!("v{}", update.version)))
                    }
                    UpdatePhase::Installing => {
                        ("Downloading", Some(format!("v{}...", update.version)))
                    }
                    UpdatePhase::Ready => ("Restart to update", None),
                };
                bar.child(
                    div()
                        .id("update-available")
                        .px_2()
                        .py_0p5()
                        .rounded(px(3.))
                        .border_1()
                        .border_color(rgb(TEXT_DIM))
                        .text_xs()
                        .text_color(rgb(TEXT_DIM))
                        .flex()
                        .items_center()
                        .gap_1()
                        .when(phase != UpdatePhase::Installing, |pill| {
                            pill.hover(|pill| {
                                pill.bg(rgb(BG)).text_color(rgb(TEXT)).cursor_pointer()
                            })
                            .on_click(cx.listener(
                                move |this, _, _, cx| match this.update_phase {
                                    UpdatePhase::Available => this.start_update_install(cx),
                                    UpdatePhase::Ready => this.relaunch_updated(cx),
                                    UpdatePhase::Installing => {}
                                },
                            ))
                        })
                        .child(prefix)
                        .when_some(version, |pill, version| {
                            pill.child(div().text_color(rgb(TEXT)).child(version))
                        }),
                )
            })
    }

    fn start_update_install(&mut self, cx: &mut Context<Self>) {
        let Some(update) = self.update_available.clone() else {
            return;
        };
        // Bare-binary runs (cargo run) have nothing to swap; hand over to the
        // release page instead.
        if updates::current_bundle().is_none() || update.asset.is_none() {
            cx.open_url(&update.url);
            return;
        }
        self.update_phase = UpdatePhase::Installing;
        cx.notify();
        let task = rt::tokio().spawn(async move { updates::download_and_install(&update).await });
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .unwrap_or_else(|error| Err(format!("update task failed: {error}")));
            this.update(cx, |this, cx| {
                match result {
                    Ok(()) => this.update_phase = UpdatePhase::Ready,
                    Err(error) => {
                        this.update_phase = UpdatePhase::Available;
                        this.flash_warning(format!("Update failed: {error}"), cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn session_snapshot(&self, cx: &Context<Self>) -> zedb_core::SavedSession {
        zedb_core::SavedSession {
            tabs: self
                .query_tabs
                .iter()
                .map(|tab| zedb_core::SavedQueryTab {
                    sql: tab.editor.read(cx).value().to_string(),
                })
                .collect(),
            active_tab: self.active_query_tab,
        }
    }

    fn relaunch_updated(&mut self, cx: &mut Context<Self>) {
        // The quit hook would save too, but saving up front means a failure
        // can stop the restart instead of losing tabs.
        let session = self.session_snapshot(cx);
        if let Err(error) = zedb_core::save_session(&session) {
            // Losing open tabs is worse than delaying the restart.
            self.flash_warning(format!("Could not save open tabs: {error}"), cx);
            return;
        }
        if let Some(bundle) = updates::current_bundle() {
            let _ = std::process::Command::new("open")
                .arg("-n")
                .arg(bundle)
                .spawn();
        }
        cx.quit();
    }

    fn open_preferences(&mut self, cx: &mut Context<Self>) {
        self.form = None;
        self.show_preferences = true;
        cx.notify();
    }

    fn close_preferences(&mut self, cx: &mut Context<Self>) {
        self.show_preferences = false;
        cx.notify();
    }

    fn toggle_fleet(&mut self, cx: &mut Context<Self>) {
        self.show_fleet = !self.show_fleet;
        if self.show_fleet {
            self.show_query_editor = false;
            if self.fleet.repo.is_none() && !self.fleet.repo_path.read(cx).text().trim().is_empty()
            {
                self.fleet_open_repo(cx);
            } else if self.fleet.rows.is_empty() {
                self.fleet_refresh(cx);
            }
        }
        cx.notify();
    }

    fn toggle_vim_mode(&mut self, cx: &mut Context<Self>) {
        self.preferences.vim_mode = !self.preferences.vim_mode;
        if self.preferences.vim_mode {
            for tab in &mut self.query_tabs {
                let editor = tab.editor.read(cx);
                let cursor = editor.cursor_position();
                tab.vim.reset(
                    editor.value().as_ref(),
                    cursor.line as usize,
                    cursor.character as usize,
                );
                tab.vim_command_line = None;
                tab.vim_recording = None;
            }
        }
        if let Err(error) = save_preferences(&self.preferences) {
            self.notice = Some(format!("Could not save preferences: {error}"));
        }
        cx.notify();
    }

    fn preferences_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().flex().justify_center().child(
            div()
                .w(px(680.))
                .max_w_full()
                .p_6()
                .flex()
                .flex_col()
                .gap_5()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(div().text_xl().child("Preferences"))
                        .child(
                            div()
                                .id("close-preferences")
                                .px_3()
                                .py_1()
                                .rounded(px(3.))
                                .border_1()
                                .border_color(rgb(BORDER))
                                .text_color(rgb(TEXT_DIM))
                                .hover(|button| {
                                    button
                                        .bg(rgb(BG_SIDEBAR))
                                        .text_color(rgb(TEXT))
                                        .cursor_pointer()
                                })
                                .on_click(cx.listener(|this, _, _, cx| this.close_preferences(cx)))
                                .child("Done"),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .py_3()
                        .border_b_1()
                        .border_color(rgb(BORDER))
                        .child(
                            div().flex().flex_col().gap_1().child("Vim mode").child(
                                div()
                                    .text_sm()
                                    .text_color(rgb(TEXT_DIM))
                                    .child("Use Vim keybindings in query editors."),
                            ),
                        )
                        .child(
                            div()
                                .id("toggle-vim-mode")
                                .w(px(54.))
                                .h(px(28.))
                                .px_1()
                                .rounded_full()
                                .flex()
                                .items_center()
                                .when(self.preferences.vim_mode, |toggle| {
                                    toggle.justify_end().bg(rgb(0x3f6650))
                                })
                                .when(!self.preferences.vim_mode, |toggle| {
                                    toggle.justify_start().bg(rgb(0x343941))
                                })
                                .hover(|toggle| toggle.cursor_pointer())
                                .on_click(cx.listener(|this, _, _, cx| this.toggle_vim_mode(cx)))
                                .child(div().size(px(20.)).rounded_full().bg(rgb(
                                    if self.preferences.vim_mode {
                                        0x9ab7a1
                                    } else {
                                        0x777e88
                                    },
                                ))),
                        ),
                ),
        )
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
            .border_1()
            .border_color(rgb(foreground))
            .bg(rgb(background))
            .text_color(rgb(foreground))
            .text_xs()
            .child(tier.label().to_uppercase())
    }

    /// The connection's write posture, worn next to the tier: quiet
    /// when read-only (the safe default), loud when writes are open.
    fn write_badge(read_only: bool) -> impl IntoElement {
        div()
            .px_2()
            .py(px(2.))
            .rounded(px(3.))
            .border_1()
            .text_xs()
            .map(|badge| {
                if read_only {
                    badge
                        .bg(rgb(0x2a2f37))
                        .border_color(rgb(TEXT_DIM))
                        .text_color(rgb(TEXT_DIM))
                        .child("READ-ONLY")
                } else {
                    badge
                        .bg(rgb(0x4d2c2c))
                        .border_color(rgb(0xe0806f))
                        .text_color(rgb(0xe0806f))
                        .child("WRITE")
                }
            })
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
                    .context_menu(move |menu, _, _| {
                        menu.menu("Edit", Box::new(EditConnection { index }))
                            .menu("Duplicate", Box::new(DuplicateConnection { index }))
                            .menu("Delete", Box::new(DeleteConnection { index }))
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .text_color(rgb(TEXT))
                            .child(connection.name.clone())
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .child(Self::write_badge(connection.read_only))
                                    .child(Self::tier_badge(connection.tier)),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(format!("{} node(s)", connection.nodes.len()))
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
                                                    .id("duplicate-connection")
                                                    .size(px(24.))
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .rounded(px(3.))
                                                    .text_color(rgb(TEXT_DIM))
                                                    .child(
                                                        svg()
                                                            .path("icons/copy.svg")
                                                            .size(px(14.))
                                                            .text_color(rgb(TEXT_DIM)),
                                                    )
                                                    .hover(|button| {
                                                        button
                                                            .bg(rgb(0x303640))
                                                            .text_color(rgb(TEXT))
                                                            .cursor_pointer()
                                                    })
                                                    .tooltip(|window, cx| {
                                                        gpui_component::tooltip::Tooltip::new(
                                                            "Duplicate connection",
                                                        )
                                                        .build(window, cx)
                                                    })
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        if let Some(index) = this.selected {
                                                            this.duplicate_connection(index, cx)
                                                        }
                                                    })),
                                            )
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
                                                    .tooltip(|window, cx| {
                                                        gpui_component::tooltip::Tooltip::new(
                                                            "Edit connection",
                                                        )
                                                        .build(window, cx)
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
                                                            .tooltip(|window, cx| {
                                                                gpui_component::tooltip::Tooltip::new(
                                                                    "Delete connection",
                                                                )
                                                                .build(window, cx)
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
        let cache_status = self.schema_cache.as_ref().map(|cache| {
            let snapshot = cache.snapshot();
            format!(
                "{} of {} databases ready",
                snapshot.warmed_databases(),
                snapshot.databases.len()
            )
        });
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
                let show_objects = if filter.is_empty() {
                    database.expanded
                } else {
                    !database.filter_collapsed
                };
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
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.select_schema_object(
                                    row_database.clone(),
                                    row_object.clone(),
                                    ObjectInspectorTab::Overview,
                                    window,
                                    cx,
                                )
                            }))
                            .context_menu({
                                let database = database_name.clone();
                                let object = object.name.clone();
                                move |menu, _, _| {
                                    menu.menu(
                                        "View DDL",
                                        Box::new(ViewObjectDdl {
                                            database: database.clone(),
                                            object: object.clone(),
                                        }),
                                    )
                                }
                            })
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
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.toggle_schema_database(database_index, window, cx)
                                }))
                                .child(if show_objects { "▾" } else { "▸" })
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
            .when_some(cache_status, |panel, status| {
                panel.child(
                    div()
                        .px_3()
                        .pb_1()
                        .text_xs()
                        .text_color(rgb(TEXT_DIM))
                        .child(status),
                )
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
            nodes: vec![NodeForm {
                name: Self::input("Node 1", "Node 1", false, cx),
                endpoint: Self::input("http://localhost:8123", "http://host:8123", false, cx),
            }],
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
            self.fleet.write_unlocked = false;
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
        let next_number = self
            .form
            .as_ref()
            .map(|form| form.nodes.len() + 1)
            .unwrap_or(1);
        let node = NodeForm {
            name: Self::input(format!("Node {next_number}"), "Node name", false, cx),
            endpoint: Self::input("", "http://host:8123", false, cx),
        };
        if let Some(form) = &mut self.form {
            form.nodes.push(node);
            cx.notify();
        }
    }

    fn remove_endpoint(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(form) = &mut self.form {
            if form.nodes.len() > 1 && index < form.nodes.len() {
                form.nodes.remove(index);
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
                nodes,
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

    /// Duplicate a saved connection under a fresh name. Passwords live
    /// in the keychain keyed by connection name, so the copy has no
    /// credentials until its first connect asks for them.
    fn duplicate_connection(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(original) = self.connections.get(index) else {
            return;
        };
        let mut copy = original.clone();
        let base = format!("{} copy", copy.name);
        let mut name = base.clone();
        let mut suffix = 2;
        while self.connections.iter().any(|c| c.name == name) {
            name = format!("{base} {suffix}");
            suffix += 1;
        }
        copy.name = name.clone();
        self.connections.push(copy);
        match save_connections(&self.connections) {
            Ok(()) => {
                self.selected = Some(self.connections.len() - 1);
                self.notice = Some(format!(
                    "Duplicated as \"{name}\"; the password is not copied, connecting will ask for it"
                ));
                self.notice_warning = false;
            }
            Err(error) => {
                self.connections.pop();
                self.notice = Some(format!("Could not save connections: {error}"));
                self.notice_warning = true;
                self.notice_flash_id += 1;
            }
        }
        cx.notify();
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
                self.fleet.write_unlocked = false;
                self.fleet.write_unlocked = false;
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
        let nodes = connection.nodes.clone();
        let user = connection.user.clone();
        let database = connection.database.clone();
        let read_only = connection.read_only;
        let connected_password = password.clone();
        self.connecting = Some(name.clone());
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
                });
                health.push(EndpointHealth {
                    node_index,
                    name: node.name,
                    endpoint: node.endpoint,
                    reachable: client.test_connection().await.is_ok(),
                });
            }
            (health, schema_cache)
        });
        cx.spawn(async move |this, cx| {
            let Ok((health, schema_cache)) = task.await else {
                this.update(cx, |this, cx| {
                    this.connecting = None;
                    this.flash_warning("Connection task stopped unexpectedly", cx);
                })
                .ok();
                return;
            };
            this.update(cx, |this, cx| {
                this.connecting = None;
                let active_node = health.iter().find(|node| node.reachable).cloned();
                let reachable = health.iter().filter(|node| node.reachable).count();
                let total = health.len();

                let Some(active_node) = active_node else {
                    this.endpoint_health.insert(name.clone(), health);
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
                    this.form = None;
                }
                this.endpoint_health.insert(name.clone(), health);
                this.fleet.write_unlocked = false;
                this.connected = Some(ConnectedCluster {
                    name: name.clone(),
                    active_node: active_node.node_index,
                    active_endpoint: active_node.endpoint.clone(),
                    client_config: ChConfig {
                        url: active_node.endpoint.clone(),
                        user: connection.user.clone(),
                        password: connected_password.clone(),
                        database: connection.database.clone(),
                        read_only: connection.read_only,
                    },
                });
                this.password_cache
                    .insert(name.clone(), connected_password.clone());
                match schema_cache {
                    Ok(cache) => this.schema_cache = Some(cache),
                    Err(error) => {
                        this.schema_cache = None;
                        this.flash_warning(
                            format!("Connected, but the schema cache could not open: {error}"),
                            cx,
                        );
                    }
                }
                this.schema_provider
                    .set_context(this.schema_cache.clone(), connection.database.clone());
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
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Health poll: every five minutes run SELECT 1 through the active
    /// node; on failure flip to disconnected and mark the node unhealthy,
    /// so the next query attempt gets the usual connect-first warning.
    fn start_health_poll(&mut self, cx: &mut Context<Self>) {
        self.health_poll_generation += 1;
        let generation = self.health_poll_generation;
        let Some(connected) = &self.connected else {
            return;
        };
        let config = connected.client_config.clone();
        let name = connected.name.clone();
        let node_index = connected.active_node;
        let schema_cache = self.schema_cache.clone();
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
                        .connected
                        .as_ref()
                        .is_some_and(|connected| connected.name == name);
                    if !still_here {
                        return true;
                    }
                    this.connected = None;
                    this.schema_cache = None;
                    this.schema_provider.set_context(None, None);
                    this.fleet.write_unlocked = false;
                    if let Some(health) = this.endpoint_health.get_mut(&name) {
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

    fn disconnect(&mut self, cx: &mut Context<Self>) {
        self.health_poll_generation += 1;
        if let Some(connected) = self.connected.take() {
            self.notice = Some(format!("Disconnected from {}", connected.name));
        }
        self.schema_cache = None;
        self.schema_provider.set_context(None, None);
        self.clear_schema();
        cx.notify();
    }

    fn select_node(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(connected_name) = self
            .connected
            .as_ref()
            .map(|connected| connected.name.clone())
        else {
            return;
        };
        let Some(node) = self
            .endpoint_health
            .get(&connected_name)
            .and_then(|health| health.iter().find(|node| node.node_index == index))
            .filter(|node| node.reachable)
            .cloned()
        else {
            return;
        };
        let Some(connected) = self.connected.as_mut() else {
            return;
        };
        if connected.active_node == node.node_index {
            return;
        }

        connected.active_node = node.node_index;
        connected.active_endpoint = node.endpoint.clone();
        connected.client_config.url = node.endpoint;
        self.notice = Some(format!("Using {} for {connected_name}", node.name));
        self.load_schema_databases(cx);
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
        if let Some(cache) = &self.schema_cache {
            self.schema_databases = database_nodes_from_cache(cache);
        }
        cx.notify();

        let cache = self.schema_cache.clone();
        let task = rt::tokio().spawn(async move {
            let client = ChClient::new(config);
            if let Some(cache) = cache {
                cache
                    .refresh_tables(&client)
                    .await
                    .map_err(|error| error.to_string())?;
                Ok(database_nodes_from_cache(&cache))
            } else {
                client
                    .list_databases()
                    .await
                    .map(|databases| {
                        databases
                            .into_iter()
                            .map(|meta| DatabaseNode {
                                meta,
                                expanded: false,
                                filter_collapsed: false,
                                loading: false,
                                objects: None,
                                error: None,
                            })
                            .collect()
                    })
                    .map_err(|error| error.to_string())
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
                this.schema_loading = false;
                match result {
                    Ok(Ok(databases)) => this.schema_databases = databases,
                    Ok(Err(error)) => this.schema_error = Some(error),
                    Err(error) => this.schema_error = Some(error.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Fetch a database's column metadata in the background if the cache
    /// is missing it; on success, re-run analysis so open editors update.
    fn warm_schema_columns(
        &mut self,
        database: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(cache), Some(connected)) = (self.schema_cache.clone(), self.connected.as_ref())
        else {
            return;
        };
        if !cache.needs_columns(&database) || !self.schema_warming.insert(database.clone()) {
            return;
        }
        let config = connected.client_config.clone();
        let task = rt::tokio().spawn({
            let database = database.clone();
            async move {
                let client = ChClient::new(config);
                cache.refresh_columns(&client, &database).await.is_ok()
            }
        });
        cx.spawn_in(window, async move |this, cx| {
            let warmed = task.await.unwrap_or(false);
            this.update_in(cx, |this, window, cx| {
                this.schema_warming.remove(&database);
                if warmed {
                    let editors: Vec<(usize, Entity<InputState>)> = this
                        .query_tabs
                        .iter()
                        .map(|tab| (tab.id, tab.editor.clone()))
                        .collect();
                    for (id, editor) in editors {
                        this.schedule_schema_analysis(id, editor, window, cx);
                    }
                    // If the user already typed the trigger (say `e.`)
                    // while columns were cold, reopen the popup now.
                    if let Some(tab) = this.query_tabs.get(this.active_query_tab) {
                        let editor = tab.editor.clone();
                        if editor.read(cx).focus_handle(cx).is_focused(window) {
                            editor.update(cx, |editor, cx| {
                                editor.retrigger_completion(window, cx);
                            });
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn toggle_schema_database(
        &mut self,
        database_index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let filter_active = !self.schema_filter.read(cx).text().trim().is_empty();
        let Some(database) = self.schema_databases.get_mut(database_index) else {
            return;
        };
        let shown = if filter_active {
            database.filter_collapsed = !database.filter_collapsed;
            !database.filter_collapsed
        } else {
            database.expanded = !database.expanded;
            database.expanded
        };
        if shown {
            if let Some(cache) = &self.schema_cache {
                cache.touch_database(&database.meta.name);
                if let Some(cached) = cache.snapshot().database(&database.meta.name) {
                    database.objects = Some(
                        cached
                            .objects
                            .values()
                            .map(schema_object_from_cache)
                            .collect(),
                    );
                }
            }
        }
        let needs_object_load = shown && database.objects.is_none() && !database.loading;
        let database_name = database.meta.name.clone();
        if shown {
            self.warm_schema_columns(database_name.clone(), window, cx);
        }
        if !needs_object_load {
            cx.notify();
            return;
        }
        let Some(connected) = &self.connected else {
            return;
        };
        let connection_name = connected.name.clone();
        let config = connected.client_config.clone();
        let Some(database) = self.schema_databases.get_mut(database_index) else {
            return;
        };
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
        tab: ObjectInspectorTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(connected) = &self.connected else {
            return;
        };
        let connection_name = connected.name.clone();
        let config = connected.client_config.clone();
        let object_name = object.name.clone();
        let window_handle = window.window_handle();
        let ddl_editor = cx.new(|cx| InputState::new(window, cx).code_editor("sql"));
        let engine_editor = cx.new(|cx| {
            InputState::new(window, cx)
                .code_editor("sql")
                .line_number(false)
        });
        self.selected_schema_object = Some(SelectedSchemaObject {
            database: database_name.clone(),
            object,
            loading: true,
            columns: Vec::new(),
            details: None,
            ddl_editor: ddl_editor.clone(),
            engine_editor: engine_editor.clone(),
            tab,
            error: None,
        });
        self.show_query_editor = false;
        cx.notify();

        if let Some(cache) = &self.schema_cache {
            cache.touch_database(&database_name);
        }
        self.warm_schema_columns(database_name.clone(), window, cx);

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
            if let Ok((_, Ok(details))) = &result {
                let ddl = details.create_table_query.clone();
                let engine = format_engine_definition(&details.engine_full);
                cx.update_window(window_handle, |_, window, cx| {
                    ddl_editor.update(cx, |editor, cx| editor.set_value(ddl, window, cx));
                    engine_editor.update(cx, |editor, cx| editor.set_value(engine, window, cx));
                })
                .ok();
            }
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
        let endpoint_count = form.nodes.len();
        let endpoint_rows = form
            .nodes
            .iter()
            .enumerate()
            .map(|(index, node)| {
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().w(px(150.)).flex_none().child(node.name.clone()))
                    .child(div().flex_1().child(node.endpoint.clone()))
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
        self.show_fleet = false;
        cx.notify();
    }

    fn make_query_tab(
        id: usize,
        sql: &str,
        schema_provider: Rc<SchemaProvider>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> QueryTab {
        let default_value = sql.to_string();
        let editor = cx.new(|cx| {
            let mut editor = InputState::new(window, cx)
                .code_editor("sql")
                .default_value(default_value);
            editor.lsp.completion_provider = Some(schema_provider.clone());
            editor.lsp.hover_provider = Some(schema_provider.clone());
            // Right-clicking a recognized table adds "View DDL" to the
            // editor's context menu.
            editor.context_menu_extension = Some(Rc::new(move |text, offset, menu| {
                let Some((snapshot, default_database)) = schema_provider.snapshot() else {
                    return menu;
                };
                let sql = text.to_string();
                match zedb_ch::schema_intelligence::object_at(
                    &snapshot,
                    default_database.as_deref(),
                    &sql,
                    offset,
                ) {
                    Some((database, object)) => menu
                        .separator()
                        .menu("View DDL", Box::new(ViewObjectDdl { database, object })),
                    None => menu,
                }
            }));
            editor
        });
        cx.subscribe_in(
            &editor,
            window,
            move |this, state, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Change) {
                    // Text changed outside modalkit (completion accepted,
                    // programmatic insert): resync vim's shadow buffer or
                    // the next vim keystroke would revert the edit. Costs
                    // vim undo history for that buffer, which beats losing
                    // the text itself.
                    if this.preferences.vim_mode {
                        let value = state.read(cx).value().to_string();
                        let cursor = state.read(cx).cursor_position();
                        if let Some(tab) = this.query_tabs.iter_mut().find(|tab| tab.id == id) {
                            if tab.vim.text() != value {
                                tab.vim.reset(
                                    &value,
                                    cursor.line as usize,
                                    cursor.character as usize,
                                );
                            }
                        }
                    }
                    this.schedule_schema_analysis(id, state.clone(), window, cx);
                }
            },
        )
        .detach();
        let result_grid = cx.new(GridSpike::new);
        cx.subscribe_in(
            &result_grid,
            window,
            move |this, _, event: &grid_spike::GridEvent, window, cx| match event {
                grid_spike::GridEvent::SortRequested { sort } => {
                    this.grid_sort_requested(id, sort.clone(), window, cx);
                }
                grid_spike::GridEvent::FilterRequested { column, predicate } => {
                    this.grid_filter_requested(id, column.clone(), predicate.clone(), window, cx);
                }
            },
        )
        .detach();
        QueryTab {
            id,
            editor,
            result_grid: result_grid.clone(),
            result_columns: 0,
            result_rows: 0,
            has_result: false,
            max_rows: MaxRows::Rows100k,
            result_capped: false,
            read_rows: None,
            read_bytes: None,
            total_rows: None,
            received_bytes: 0,
            editor_height: 220.0,
            status_height: 52.0,
            outcome: QueryOutcome::Idle,
            started_at: None,
            elapsed: None,
            vim: VimController::new(sql),
            vim_command_line: None,
            vim_recording: None,
            schema_analysis_generation: 0,
            displayed_statement: None,
        }
    }

    /// Agent-facing: show the query editor, creating a tab if none.
    pub(crate) fn open_query_editor_for_agent(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.query_tabs.is_empty() {
            self.add_query_tab(window, cx);
        } else {
            self.show_query_editor = true;
            self.show_fleet = false;
            cx.notify();
        }
    }

    /// Agent-facing: a new query tab pre-filled with SQL, focused.
    pub(crate) fn open_query_tab_with(
        &mut self,
        sql: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = self.next_query_tab_id;
        self.next_query_tab_id += 1;
        let tab = Self::make_query_tab(id, sql, self.schema_provider.clone(), window, cx);
        self.query_tabs.push(tab);
        self.active_query_tab = self.query_tabs.len() - 1;
        self.show_query_editor = true;
        self.show_fleet = false;
        cx.notify();
    }

    fn schedule_schema_analysis(
        &mut self,
        tab_id: usize,
        editor: Entity<InputState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.query_tabs.iter_mut().find(|tab| tab.id == tab_id) else {
            return;
        };
        tab.schema_analysis_generation += 1;
        let generation = tab.schema_analysis_generation;
        let sql = editor.read(cx).value().to_string();
        let Some((snapshot, default_database)) = self.schema_provider.snapshot() else {
            editor.update(cx, |editor, cx| {
                if let Some(diagnostics) = editor.diagnostics_mut() {
                    diagnostics.clear();
                }
                cx.notify();
            });
            return;
        };
        let task = rt::tokio().spawn(async move {
            tokio::time::sleep(Duration::from_millis(180)).await;
            let issues = zedb_ch::schema_intelligence::analyze_sql(
                &snapshot,
                default_database.as_deref(),
                &sql,
            );
            let referenced = zedb_ch::schema_intelligence::referenced_databases(
                &snapshot,
                default_database.as_deref(),
                &sql,
            );
            (sql, issues, referenced)
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok((sql, issues, referenced)) = task.await else {
                return;
            };
            this.update_in(cx, |this, window, cx| {
                for database in referenced {
                    this.warm_schema_columns(database, window, cx);
                }
                let Some(tab) = this.query_tabs.iter().find(|tab| tab.id == tab_id) else {
                    return;
                };
                if tab.schema_analysis_generation != generation {
                    return;
                }
                editor.update(cx, |editor, cx| {
                    let Some(diagnostics) = editor.diagnostics_mut() else {
                        return;
                    };
                    diagnostics.clear();
                    diagnostics.extend(issues.into_iter().map(|issue| {
                        let range = byte_range_to_lsp(&sql, issue.range);
                        Diagnostic {
                            range: range.start..range.end,
                            severity: DiagnosticSeverity::Hint,
                            source: Some("zeDB schema".into()),
                            message: issue.message.into(),
                            ..Default::default()
                        }
                    }));
                    cx.notify();
                });
            })
            .ok();
        })
        .detach();
    }

    fn add_query_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.next_query_tab_id;
        self.next_query_tab_id += 1;
        let tab = Self::make_query_tab(id, "", self.schema_provider.clone(), window, cx);
        self.query_tabs.push(tab);
        self.active_query_tab = self.query_tabs.len() - 1;
        self.show_query_editor = true;
        self.show_fleet = false;
        cx.notify();
    }

    fn close_query_tab(&mut self, tab_id: usize, cx: &mut Context<Self>) {
        if self.query_tabs.len() == 1 {
            return;
        }
        let Some(index) = self.query_tabs.iter().position(|tab| tab.id == tab_id) else {
            return;
        };
        if matches!(
            self.query_tabs[index].outcome,
            QueryOutcome::Running | QueryOutcome::StatementError { .. }
        ) {
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

    fn run_selection_action(
        &mut self,
        _: &RunSelection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_selection(window, cx);
    }

    fn modalkit_key(keystroke: &Keystroke) -> Option<String> {
        if keystroke.modifiers.platform || keystroke.modifiers.function {
            return None;
        }

        let special = match keystroke.key.as_str() {
            "escape" => Some("Esc"),
            "enter" => Some("Enter"),
            "backspace" => Some("BS"),
            "delete" => Some("Del"),
            "tab" => Some("Tab"),
            "space" => Some("Space"),
            "left" => Some("Left"),
            "right" => Some("Right"),
            "up" => Some("Up"),
            "down" => Some("Down"),
            "home" => Some("Home"),
            "end" => Some("End"),
            "pageup" => Some("PageUp"),
            "pagedown" => Some("PageDown"),
            _ => None,
        };
        let key = special
            .map(str::to_string)
            .or_else(|| keystroke.key_char.clone())
            .unwrap_or_else(|| keystroke.key.clone());

        if keystroke.modifiers.control || keystroke.modifiers.alt || special.is_some() {
            let mut modifiers = String::new();
            if keystroke.modifiers.control {
                modifiers.push_str("C-");
            }
            if keystroke.modifiers.alt {
                modifiers.push_str("A-");
            }
            if keystroke.modifiers.shift && special.is_some() {
                modifiers.push_str("S-");
            }
            Some(format!("<{modifiers}{key}>"))
        } else {
            Some(key)
        }
    }

    fn vim_keystroke(
        &mut self,
        keystroke: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.preferences.vim_mode {
            return;
        }
        let Some(key) = Self::modalkit_key(keystroke) else {
            return;
        };
        // Reserved for RunSelection; must reach action dispatch instead of
        // modalkit (where it would decrement numbers in normal mode).
        if key == "<C-x>" {
            return;
        }
        let Some(tab) = self.query_tabs.get_mut(self.active_query_tab) else {
            return;
        };
        if !tab.editor.focus_handle(cx).is_focused(window) {
            return;
        }
        // While the completion popup is open, arrows, enter, and escape
        // belong to it: skip modalkit so the editor's own bindings route
        // these keys into the menu (navigate, confirm, dismiss).
        if !keystroke.modifiers.modified()
            && matches!(keystroke.key.as_str(), "up" | "down" | "enter" | "escape")
            && tab.editor.read(cx).completion_menu_open(cx)
        {
            return;
        }
        // In visual mode the editor cursor tracks the selection end, not the
        // Vim head, so feeding it back would drift the selection; in command
        // mode keystrokes edit the command line, not the buffer.
        if !matches!(
            tab.vim.mode(),
            modalkit::env::vim::VimMode::Visual
                | modalkit::env::vim::VimMode::Select
                | modalkit::env::vim::VimMode::Command
        ) {
            let cursor = tab.editor.read(cx).cursor_position();
            tab.vim
                .set_cursor(cursor.line as usize, cursor.character as usize);
        }
        let mut snapshot = match tab.vim.input(&key) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.notice = Some(format!("Vim input error: {error}"));
                cx.stop_propagation();
                cx.notify();
                return;
            }
        };
        tab.vim_command_line = snapshot.command_line.take();
        tab.vim_recording = snapshot.recording;
        let editor = tab.editor.clone();
        editor.update(cx, |state, cx| {
            let text_changed = state.value().as_ref() != snapshot.text;
            let old_utf16_len = state.value().encode_utf16().count();
            EntityInputHandler::replace_text_in_range(
                state,
                Some(0..old_utf16_len),
                &snapshot.text,
                window,
                cx,
            );
            state.set_cursor_position(
                Position::new(snapshot.line as u32, snapshot.column as u32),
                window,
                cx,
            );
            // set_cursor_position deliberately closes editor popovers. In
            // Vim Insert mode that happens after every modalkit edit, so
            // request completion again from the final cursor position.
            if text_changed && tab.vim.mode() == modalkit::env::vim::VimMode::Insert {
                state.retrigger_completion(window, cx);
            }
            if let Some(selection) = &snapshot.selection {
                let start = Self::utf16_offset(&snapshot.text, selection.start);
                let end = Self::utf16_offset(&snapshot.text, selection.end);
                if start < end {
                    let mut adjusted_range = None;
                    let selected = EntityInputHandler::text_for_range(
                        state,
                        start..end,
                        &mut adjusted_range,
                        window,
                        cx,
                    );
                    if let Some(selected) = selected.filter(|selected| !selected.is_empty()) {
                        // InputState has no public API for setting an arbitrary
                        // selection; a same-text IME replace positions one
                        // without changing the buffer.
                        EntityInputHandler::replace_and_mark_text_in_range(
                            state,
                            Some(start..end),
                            &selected,
                            Some(0..0),
                            window,
                            cx,
                        );
                        EntityInputHandler::unmark_text(state, window, cx);
                    }
                }
            }
        });
        if let Some(unsupported) = snapshot.unsupported {
            self.notice = Some(format!("Vim action not available here: {unsupported}"));
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn utf16_offset(text: &str, (line, column): (usize, usize)) -> usize {
        let mut offset = 0;
        for (index, line_text) in text.split('\n').enumerate() {
            if index == line {
                return offset
                    + line_text
                        .chars()
                        .take(column)
                        .map(char::len_utf16)
                        .sum::<usize>();
            }
            offset += line_text.encode_utf16().count() + 1;
        }
        // `line` is past the last line (e.g. a line-wise selection ending at
        // the final line); the loop overcounts by one virtual newline.
        offset.saturating_sub(1)
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

    /// Text the user has highlighted in the active editor, if any.
    fn selected_text(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<String> {
        let editor = self
            .query_tabs
            .get(self.active_query_tab)
            .map(|tab| tab.editor.clone())?;
        editor.update(cx, |editor, cx| {
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
        })
    }

    /// Run the selection as a single query, or the statement under the cursor
    /// when nothing is selected.
    fn run_query(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.query_abort.is_some() {
            return;
        }
        let sql = self.selected_text(window, cx).unwrap_or_else(|| {
            self.query_tabs
                .get(self.active_query_tab)
                .map(|tab| {
                    tab.editor.update(cx, |editor, _| {
                        let text = editor.value().to_string();
                        statement_at_cursor(&text, editor.cursor())
                            .map(str::to_string)
                            .unwrap_or_default()
                    })
                })
                .unwrap_or_default()
        });
        self.start_statements(vec![sql], cx);
    }

    /// Run every statement in the selection (or the whole buffer when nothing
    /// is selected) one after another.
    fn run_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.query_abort.is_some() {
            return;
        }
        let text = self.selected_text(window, cx).or_else(|| {
            self.query_tabs
                .get(self.active_query_tab)
                .map(|tab| tab.editor.read(cx).value().to_string())
        });
        let statements = text
            .map(|text| {
                split_statements(&text)
                    .into_iter()
                    .filter_map(|(start, end)| {
                        let statement = text[start..end].trim();
                        (!statement.is_empty()).then(|| statement.to_string())
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.start_statements(statements, cx);
    }

    /// A grid header was clicked: rewrite the displayed statement's
    /// top-level ORDER BY, mirror it into the editor, and re-run it.
    fn grid_sort_requested(
        &mut self,
        tab_id: usize,
        sort: Vec<(String, bool)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.query_tabs.iter_mut().find(|tab| tab.id == tab_id) else {
            return;
        };
        let Some(statement) = tab.displayed_statement.clone() else {
            return;
        };
        let rewritten = zedb_ch::schema_intelligence::set_order_by(&statement, &sort);
        self.apply_rewritten_statement(statement, rewritten, window, cx);
    }

    /// Open the filter popover for a column, probing the server for its
    /// distinct values (capped, short-circuiting past ten) so even
    /// non-dictionary columns get checkboxes when they are small.
    fn open_column_filter(&mut self, column: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.query_tabs.get(self.active_query_tab) else {
            return;
        };
        let statement = tab.displayed_statement.clone();
        let prefill = statement
            .as_deref()
            .and_then(|statement| zedb_ch::schema_intelligence::column_filter(statement, &column));
        let grid = tab.result_grid.clone();
        let needs_probe = grid.update(cx, |grid, cx| {
            grid.begin_filter_panel(column.clone(), prefill, cx)
        });
        if !needs_probe {
            return;
        }
        let (Some(statement), Some(connected)) = (statement, self.connected.as_ref()) else {
            grid.update(cx, |grid, cx| {
                grid.finish_filter_panel(&column, None, window, cx)
            });
            return;
        };
        // Distinct within the query's other filters, unbounded by its
        // LIMIT, ignoring this column's own filter and the sort.
        let base = zedb_ch::schema_intelligence::set_column_filter(&statement, &column, None);
        let base = zedb_ch::schema_intelligence::set_order_by(&base, &[]);
        let base = zedb_ch::schema_intelligence::strip_top_level_limit(&base);
        let base = base.trim_end().trim_end_matches(';').to_string();
        let probe = format!(
            "SELECT DISTINCT `{}` AS value FROM ({base}) LIMIT 11",
            column.replace('`', "")
        );
        let config = connected.client_config.clone();
        let task = rt::tokio().spawn(async move {
            zedb_ch::ChClient::new(config)
                .query_guarded(&probe, 5, 32, 10 * 1024 * 1024 * 1024)
                .await
        });
        cx.spawn_in(window, async move |this, cx| {
            let values = match task.await {
                Ok(Ok(result)) => {
                    let has_null = result
                        .rows
                        .iter()
                        .any(|row| matches!(row.first(), Some(zedb_core::Value::Null)));
                    Some((
                        result
                            .rows
                            .into_iter()
                            .filter_map(|row| {
                                row.first().and_then(|value| match value {
                                    zedb_core::Value::Null => None,
                                    other => Some(other.to_string()),
                                })
                            })
                            .collect::<Vec<_>>(),
                        has_null,
                    ))
                }
                _ => None,
            };
            this.update_in(cx, |_, window, cx| {
                grid.update(cx, |grid, cx| {
                    grid.finish_filter_panel(&column, values, window, cx)
                });
            })
            .ok();
        })
        .detach();
    }

    /// A grid header asked for a filter change on the displayed statement.
    fn grid_filter_requested(
        &mut self,
        tab_id: usize,
        column: String,
        predicate: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.query_tabs.iter_mut().find(|tab| tab.id == tab_id) else {
            return;
        };
        let Some(statement) = tab.displayed_statement.clone() else {
            return;
        };
        let rewritten = zedb_ch::schema_intelligence::set_column_filter(
            &statement,
            &column,
            predicate.as_deref(),
        );
        self.apply_rewritten_statement(statement, rewritten, window, cx);
    }

    /// Mirror a rewritten statement into the editor and re-run it.
    fn apply_rewritten_statement(
        &mut self,
        statement: String,
        rewritten: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if rewritten == statement {
            return;
        }
        let Some(tab) = self.query_tabs.get_mut(self.active_query_tab) else {
            return;
        };
        // Later header interactions compose on this rewrite even before
        // it has run.
        tab.displayed_statement = Some(rewritten.clone());
        let editor = tab.editor.clone();
        let value = editor.read(cx).value().to_string();
        if value.contains(&statement) {
            let updated = value.replacen(&statement, &rewritten, 1);
            editor.update(cx, |editor, cx| editor.set_value(updated, window, cx));
        } else {
            self.flash_warning(
                "Query changed since it ran; rewriting the last executed statement",
                cx,
            );
        }
        // Coalesce rapid interactions into one run: debounce a beat and
        // cancel-and-restart anything in flight.
        self.rerun_generation += 1;
        let generation = self.rerun_generation;
        self.rerun_pending = Some(rewritten);
        cx.spawn(async move |this, cx| {
            Timer::after(Duration::from_millis(150)).await;
            this.update(cx, |this, cx| {
                if this.rerun_generation != generation {
                    return;
                }
                let Some(statement) = this.rerun_pending.take() else {
                    return;
                };
                if this.query_abort.is_some() {
                    this.cancel_query(cx);
                }
                this.start_statements(vec![statement], cx);
            })
            .ok();
        })
        .detach();
    }

    fn start_statements(&mut self, mut statements: Vec<String>, cx: &mut Context<Self>) {
        if self.query_abort.is_some() {
            return;
        }
        let Some(connected) = &self.connected else {
            self.flash_warning("Connect to a cluster before running a query", cx);
            return;
        };
        statements.retain(|statement| !statement.trim().is_empty());
        let Some(tab) = self.query_tabs.get_mut(self.active_query_tab) else {
            return;
        };
        if statements.is_empty() {
            tab.outcome = QueryOutcome::Error("Query is empty".into());
            cx.notify();
            return;
        }

        let tab_id = tab.id;
        tab.outcome = QueryOutcome::Running;
        tab.result_columns = 0;
        tab.result_rows = 0;
        // has_result stays as it was: an already-displayed result keeps
        // its pane (and its rows, via the grid's deferred swap) until
        // the replacement streams in.
        tab.result_capped = false;
        tab.read_rows = None;
        tab.read_bytes = None;
        tab.total_rows = None;
        tab.received_bytes = 0;
        tab.started_at = Some(Instant::now());
        tab.elapsed = None;
        let config = connected.client_config.clone();
        let row_limit = tab.max_rows.limit();
        self.query_run_id += 1;
        let run_id = self.query_run_id;
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let task = rt::tokio().spawn(async move {
            let client = ChClient::new(config);
            let total = statements.len();
            let mut summary: Option<QueryStreamSummary> = None;
            let mut skipped = 0usize;
            let mut succeeded = Vec::new();
            for (index, sql) in statements.iter().enumerate() {
                let outcome = client
                    .query_stream(sql, row_limit.unwrap_or(usize::MAX), |event| {
                        let _ = sender.send(RunEvent::Stream(event));
                    })
                    .await;
                match outcome {
                    Ok(current) => {
                        summary = Some(current);
                        succeeded.push(sql.clone());
                    }
                    Err(error) => {
                        let message = if total > 1 {
                            format!("Statement {} of {total} failed: {error}", index + 1)
                        } else {
                            error.to_string()
                        };
                        if index + 1 == total {
                            return Err(message);
                        }
                        let (decision, wait) = tokio::sync::oneshot::channel();
                        let _ = sender.send(RunEvent::StatementFailed {
                            index,
                            total,
                            message: error.to_string(),
                            decision,
                        });
                        // Pause until the user skips this statement or cancels
                        // the rest of the run. A dropped sender cancels.
                        if wait.await.unwrap_or(false) {
                            skipped += 1;
                        } else {
                            return Err(message);
                        }
                    }
                }
            }
            Ok((summary, skipped, succeeded))
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
                            RunEvent::StatementFailed {
                                index,
                                total,
                                message,
                                decision,
                            } => {
                                this.query_error_decision = Some(decision);
                                tab.outcome = QueryOutcome::StatementError {
                                    index,
                                    total,
                                    message,
                                };
                            }
                            RunEvent::Stream(QueryStreamEvent::Columns(columns)) => {
                                tab.result_columns = columns.len();
                                tab.result_rows = 0;
                                tab.has_result = true;
                                // Each statement reports its own progress;
                                // never let one statement's totals stand
                                // for the next.
                                tab.read_rows = None;
                                tab.read_bytes = None;
                                tab.total_rows = None;
                                tab.received_bytes = 0;
                                tab.result_grid.update(cx, |grid, cx| {
                                    grid.begin_result(columns, row_limit, cx)
                                });
                            }
                            RunEvent::Stream(QueryStreamEvent::Rows(rows)) => {
                                tab.result_rows += rows.len();
                                tab.result_grid
                                    .update(cx, |grid, cx| grid.append_rows(rows, cx));
                            }
                            RunEvent::Stream(QueryStreamEvent::Progress(progress)) => {
                                if progress.read_rows.is_some() {
                                    tab.read_rows = progress.read_rows;
                                }
                                if progress.read_bytes.is_some() {
                                    tab.read_bytes = progress.read_bytes;
                                }
                                if progress.total_rows.is_some() {
                                    tab.total_rows = progress.total_rows;
                                }
                                tab.received_bytes = progress.received_bytes;
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
                this.query_error_decision = None;
                let Some(tab) = this.query_tabs.iter_mut().find(|tab| tab.id == tab_id) else {
                    return;
                };
                tab.elapsed = tab.started_at.take().map(|started| started.elapsed());
                let mut successful_statements = Vec::new();
                tab.outcome = match result {
                    Ok(Ok((summary, skipped, succeeded))) => {
                        let capped = summary.map(|summary| summary.capped).unwrap_or(false);
                        tab.result_capped = capped;
                        tab.result_grid
                            .update(cx, |grid, cx| grid.finish_result(capped, cx));
                        let outcome = QueryOutcome::Complete {
                            columns: tab.result_columns,
                            rows: tab.result_rows,
                            skipped,
                        };
                        successful_statements = succeeded;
                        outcome
                    }
                    Ok(Err(error)) => QueryOutcome::Error(error),
                    Err(error) => QueryOutcome::Error(error.to_string()),
                };
                // Re-sync the sort indicator with reality: the executed
                // SQL on success, or the still-displayed old result's SQL
                // when the run failed after an optimistic indicator.
                if let Some(statement) = successful_statements.last() {
                    tab.displayed_statement = Some(statement.clone());
                }
                if let Some(statement) = tab.displayed_statement.clone() {
                    let sort = zedb_ch::schema_intelligence::top_level_order_by(&statement);
                    let filters = zedb_ch::schema_intelligence::column_filters(&statement);
                    tab.result_grid.update(cx, |grid, cx| {
                        grid.set_sort(sort, cx);
                        grid.set_filters(filters, cx);
                    });
                }
                this.refresh_schema_after_statements(&successful_statements);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn refresh_schema_after_statements(&self, statements: &[String]) {
        let (Some(cache), Some(connected)) = (self.schema_cache.clone(), self.connected.as_ref())
        else {
            return;
        };
        let mut databases = statements
            .iter()
            .flat_map(|statement| {
                zedb_ch::schema_intelligence::touched_databases(
                    statement,
                    connected.client_config.database.as_deref(),
                )
            })
            .collect::<Vec<_>>();
        databases.sort();
        databases.dedup();
        if databases.is_empty() {
            return;
        }
        let config = connected.client_config.clone();
        rt::tokio().spawn(async move {
            for database in &databases {
                let _ = cache.invalidate_database(database);
            }
            let client = ChClient::new(config);
            if cache.refresh_tables(&client).await.is_ok() {
                for database in databases {
                    let _ = cache.refresh_columns(&client, &database).await;
                }
            }
        });
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

    /// Resume a run paused on a failed statement: skip it and continue, or
    /// cancel the remaining statements.
    fn resolve_statement_failure(&mut self, skip: bool, cx: &mut Context<Self>) {
        let Some(decision) = self.query_error_decision.take() else {
            return;
        };
        let _ = decision.send(skip);
        if skip {
            if let Some(tab) = self
                .query_tabs
                .iter_mut()
                .find(|tab| matches!(tab.outcome, QueryOutcome::StatementError { .. }))
            {
                tab.outcome = QueryOutcome::Running;
            }
        }
        cx.notify();
    }

    fn cancel_query(&mut self, cx: &mut Context<Self>) {
        let Some(abort) = self.query_abort.take() else {
            return;
        };
        abort.abort();
        self.query_error_decision = None;
        self.query_run_id += 1;
        if let Some(tab) = self.query_tabs.iter_mut().find(|tab| {
            matches!(
                tab.outcome,
                QueryOutcome::Running | QueryOutcome::StatementError { .. }
            )
        }) {
            tab.elapsed = tab.started_at.take().map(|started| started.elapsed());
            tab.outcome = QueryOutcome::Cancelled;
        }
        cx.notify();
    }

    fn node_selector(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let connected = self.connected.as_ref()?;
        let connection = self
            .connections
            .iter()
            .find(|connection| connection.name == connected.name)?;
        let health = self.endpoint_health.get(&connected.name);
        let nodes = connection
            .nodes
            .iter()
            .enumerate()
            .map(|(index, node)| {
                let reachable = health
                    .and_then(|health| health.iter().find(|item| item.node_index == index))
                    .map(|item| item.reachable)
                    .unwrap_or(false);
                (index, node.name.clone(), reachable)
            })
            .collect::<Vec<_>>();
        let active_name = connection
            .nodes
            .get(connected.active_node)
            .map(|node| node.name.clone())
            .unwrap_or_else(|| "Select node".into());
        let action_context = self.query_tabs[self.active_query_tab]
            .editor
            .focus_handle(cx);

        Some(
            Button::new("active-node-selector")
                .label(active_name)
                .dropdown_caret(true)
                .compact()
                .outline()
                .dropdown_menu(move |menu: PopupMenu, _, _| {
                    nodes.iter().cloned().fold(
                        menu.action_context(action_context.clone()).min_w(px(180.)),
                        |menu, (index, name, reachable)| {
                            menu.menu_with_enable(name, Box::new(SelectNode { index }), reachable)
                        },
                    )
                }),
        )
    }

    fn connection_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.selected.and_then(|index| self.connections.get(index));
        let header_connection = self
            .connected
            .as_ref()
            .and_then(|connected| {
                self.connections
                    .iter()
                    .find(|connection| connection.name == connected.name)
            })
            .or(selected);
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
                    .when_some(header_connection, |row, connection| {
                        row.child(connection.name.clone())
                            .child(Self::tier_badge(connection.tier))
                            .when_some(self.node_selector(cx), |row, selector| row.child(selector))
                    })
                    .when(header_connection.is_none(), |row| {
                        row.child("Select a connection")
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .id("open-fleet")
                            .group("btn-fleet")
                            .size(px(28.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(3.))
                            .border_1()
                            .map(|button| {
                                if self.connected.is_none() {
                                    // Disabled: the fleet view is per-connection.
                                    button
                                        .border_color(rgb(0x22262c))
                                        .child(
                                            svg()
                                                .path("icons/fleet.svg")
                                                .size(px(14.))
                                                .text_color(rgb(0x454b55)),
                                        )
                                        .tooltip(|window, cx| {
                                            gpui_component::tooltip::Tooltip::new(
                                                "Connect to a cluster first",
                                            )
                                            .build(window, cx)
                                        })
                                } else {
                                    button
                                        .border_color(rgb(BORDER))
                                        .when(self.show_fleet, |button| button.bg(rgb(0x2c3a4d)))
                                        .child(
                                            svg()
                                                .path("icons/fleet.svg")
                                                .size(px(14.))
                                                .text_color(rgb(if self.show_fleet {
                                                    TEXT
                                                } else {
                                                    TEXT_DIM
                                                }))
                                                .group_hover("btn-fleet", |icon| {
                                                    icon.text_color(rgb(TEXT))
                                                }),
                                        )
                                        .hover(|button| button.bg(rgb(0x303640)).cursor_pointer())
                                        .tooltip(|window, cx| {
                                            gpui_component::tooltip::Tooltip::new("Fleet view")
                                                .build(window, cx)
                                        })
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.toggle_fleet(cx)),
                                        )
                                }
                            }),
                    )
                    .child(
                        div()
                            .id("open-query-editor")
                            .group("btn-query")
                            .size(px(28.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(3.))
                            .border_1()
                            .map(|button| {
                                if self.connected.is_none() {
                                    // Disabled; running from an existing tab
                                    // still gets the connect-first warning.
                                    button
                                        .border_color(rgb(0x22262c))
                                        .child(
                                            svg()
                                                .path("icons/query-plus.svg")
                                                .size(px(14.))
                                                .text_color(rgb(0x454b55)),
                                        )
                                        .tooltip(|window, cx| {
                                            gpui_component::tooltip::Tooltip::new(
                                                "Connect to a cluster first",
                                            )
                                            .build(window, cx)
                                        })
                                } else {
                                    button
                                        .border_color(rgb(BORDER))
                                        .when(!self.show_fleet, |button| button.bg(rgb(0x2c3a4d)))
                                        .child(
                                            svg()
                                                .path("icons/query-plus.svg")
                                                .size(px(14.))
                                                .text_color(rgb(if self.show_fleet {
                                                    TEXT_DIM
                                                } else {
                                                    TEXT
                                                }))
                                                .group_hover("btn-query", |icon| {
                                                    icon.text_color(rgb(TEXT))
                                                }),
                                        )
                                        .hover(|button| button.bg(rgb(0x303640)).cursor_pointer())
                                        .tooltip(|window, cx| {
                                            gpui_component::tooltip::Tooltip::new("New query")
                                                .build(window, cx)
                                        })
                                        .on_click(
                                            cx.listener(|this, _, _, cx| {
                                                this.open_query_editor(cx)
                                            }),
                                        )
                                }
                            }),
                    )
                    .child(
                        div()
                            .id("open-agent-pane")
                            .group("btn-agent")
                            .size(px(28.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(3.))
                            .border_1()
                            .border_color(rgb(BORDER))
                            .when(self.agent.open, |button| button.bg(rgb(0x2c3a4d)))
                            .child(
                                svg()
                                    .path("icons/sparkle.svg")
                                    .size(px(14.))
                                    .text_color(rgb(if self.agent.open { TEXT } else { TEXT_DIM }))
                                    .group_hover("btn-agent", |icon| icon.text_color(rgb(TEXT))),
                            )
                            .hover(|button| button.bg(rgb(0x303640)).cursor_pointer())
                            .tooltip(|window, cx| {
                                gpui_component::tooltip::Tooltip::new(
                                    "Agent pane: AI threads with your installed agents",
                                )
                                .build(window, cx)
                            })
                            .on_click(
                                cx.listener(|this, _, window, cx| this.agent_toggle(window, cx)),
                            ),
                    )
                    .when(selected_connected, |toolbar| {
                        toolbar.child(
                            div()
                                .id("disconnect")
                                .size(px(28.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(3.))
                                .border_1()
                                .border_color(rgb(DANGER))
                                .child(
                                    svg()
                                        .path("icons/stop.svg")
                                        .size(px(12.))
                                        .text_color(rgb(DANGER)),
                                )
                                .hover(|button| button.bg(rgb(0x3d2528)).cursor_pointer())
                                .tooltip(|window, cx| {
                                    gpui_component::tooltip::Tooltip::new("Disconnect")
                                        .build(window, cx)
                                })
                                .on_click(cx.listener(|this, _, _, cx| this.disconnect(cx))),
                        )
                    })
                    .when(!selected_connected, |toolbar| {
                        toolbar.child(
                            div()
                                .id("connect-toggle")
                                .group("btn-connect")
                                .size(px(28.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(3.))
                                .border_1()
                                .map(|button| {
                                    if self.connecting.is_some() {
                                        button
                                            .border_color(rgb(BORDER))
                                            .child(
                                                svg()
                                                    .path("icons/plug.svg")
                                                    .size(px(14.))
                                                    .text_color(rgb(SUCCESS)),
                                            )
                                            .tooltip(|window, cx| {
                                                gpui_component::tooltip::Tooltip::new(
                                                    "Connecting...",
                                                )
                                                .build(window, cx)
                                            })
                                    } else if selected.is_some() {
                                        button
                                            .border_color(rgb(BORDER))
                                            .child(
                                                svg()
                                                    .path("icons/plug.svg")
                                                    .size(px(14.))
                                                    .text_color(rgb(TEXT_DIM))
                                                    .group_hover("btn-connect", |icon| {
                                                        icon.text_color(rgb(SUCCESS))
                                                    }),
                                            )
                                            .hover(|button| {
                                                button
                                                    .bg(rgb(0x294132))
                                                    .border_color(rgb(SUCCESS))
                                                    .cursor_pointer()
                                            })
                                            .tooltip(|window, cx| {
                                                gpui_component::tooltip::Tooltip::new("Connect")
                                                    .build(window, cx)
                                            })
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.connect_selected(cx)
                                            }))
                                    } else {
                                        // Disabled: nothing selected to connect to.
                                        button
                                            .border_color(rgb(0x22262c))
                                            .child(
                                                svg()
                                                    .path("icons/plug.svg")
                                                    .size(px(14.))
                                                    .text_color(rgb(0x454b55)),
                                            )
                                            .tooltip(|window, cx| {
                                                gpui_component::tooltip::Tooltip::new(
                                                    "Select a connection first",
                                                )
                                                .build(window, cx)
                                            })
                                    }
                                }),
                        )
                    }),
            )
    }

    fn cluster_overview(&self) -> impl IntoElement {
        let selected = self.selected.and_then(|index| self.connections.get(index));
        let nodes = selected
            .map(|connection| {
                connection
                    .nodes
                    .iter()
                    .enumerate()
                    .map(|(index, configured_node)| {
                        let reachable =
                            self.endpoint_health
                                .get(&connection.name)
                                .and_then(|health| {
                                    health
                                        .iter()
                                        .find(|node| node.node_index == index)
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
                            .child(configured_node.name.clone())
                            .child(
                                div()
                                    .text_color(rgb(TEXT_DIM))
                                    .child(configured_node.endpoint.clone()),
                            )
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
                }),
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

    fn schema_object_panel(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
        let ddl_editor = selected.ddl_editor.clone();
        let engine_editor = selected.engine_editor.clone();
        if ddl_editor.focus_handle(cx).is_focused(window)
            || engine_editor.focus_handle(cx).is_focused(window)
        {
            window.blur();
        }
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
                let has_engine_definition = details
                    .as_ref()
                    .map(|details| !details.engine_full.is_empty())
                    .unwrap_or(false);
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
                        .when(has_engine_definition, |panel| {
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
                                            .h(px(132.))
                                            .rounded(px(3.))
                                            .border_1()
                                            .border_color(rgb(BORDER))
                                            .bg(rgb(0x191c21))
                                            .overflow_hidden()
                                            .child(
                                                Input::new(&engine_editor)
                                                    .appearance(false)
                                                    .bordered(false)
                                                    .focus_bordered(false)
                                                    .disabled(true)
                                                    .tab_index(-1)
                                                    .h_full(),
                                            ),
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
                            .overflow_hidden()
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
                                    .p_3()
                                    .child(div().text_color(rgb(TEXT_DIM)).child("DDL unavailable"))
                            })
                            .when(!loading && error.is_none() && !ddl.is_empty(), |panel| {
                                panel.child(
                                    Input::new(&ddl_editor)
                                        .appearance(false)
                                        .bordered(false)
                                        .focus_bordered(false)
                                        .disabled(true)
                                        .tab_index(-1)
                                        .h_full(),
                                )
                            }),
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
                            .child(
                                div()
                                    .flex()
                                    .gap_1()
                                    .child(div().text_color(rgb(TEXT)).child("Engine:"))
                                    .child(
                                        div()
                                            .text_color(rgb(TEXT_DIM))
                                            .child(selected.object.engine.clone()),
                                    ),
                            )
                            .when_some(selected.object.total_rows, |row, rows| {
                                row.child(
                                    div()
                                        .flex()
                                        .gap_1()
                                        .child(div().text_color(rgb(TEXT)).child("Rows:"))
                                        .child(
                                            div()
                                                .text_color(rgb(TEXT_DIM))
                                                .child(Self::format_count(rows)),
                                        ),
                                )
                            })
                            .when_some(selected.object.total_bytes, |row, bytes| {
                                row.child(
                                    div()
                                        .flex()
                                        .gap_1()
                                        .child(div().text_color(rgb(TEXT)).child("Size:"))
                                        .child(
                                            div()
                                                .text_color(rgb(TEXT_DIM))
                                                .child(Self::format_bytes(bytes)),
                                        ),
                                )
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
                                .when(
                                    !matches!(
                                        tab.outcome,
                                        QueryOutcome::Running | QueryOutcome::StatementError { .. }
                                    ),
                                    |close| {
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
                                    },
                                ),
                        )
                    })
            })
            .collect::<Vec<_>>();
        let active = self
            .query_tabs
            .get(self.active_query_tab)
            .expect("query editor requires an active tab");
        let running = matches!(
            active.outcome,
            QueryOutcome::Running | QueryOutcome::StatementError { .. }
        );
        let statement_failed = matches!(active.outcome, QueryOutcome::StatementError { .. });
        let has_result = active.has_result;
        let result_capped = active.result_capped;
        let editor_height = active.editor_height;
        let status_height = active.status_height;
        let result_grid = active.result_grid.clone();
        let mut status = match &active.outcome {
            QueryOutcome::Idle => "Ready".to_string(),
            QueryOutcome::Running => format!("Running: {} row(s)", active.result_rows),
            QueryOutcome::Complete {
                columns,
                rows,
                skipped,
            } => {
                let mut text = if result_capped {
                    format!("Showing first {rows} row(s), {columns} column(s)")
                } else {
                    format!("Complete: {rows} row(s), {columns} column(s)")
                };
                if *skipped > 0 {
                    text.push_str(&format!("  {skipped} statement(s) skipped"));
                }
                text
            }
            QueryOutcome::Error(error) => error.clone(),
            QueryOutcome::StatementError {
                index,
                total,
                message,
            } => {
                format!("Statement {} of {total} failed: {message}", index + 1)
            }
            QueryOutcome::Cancelled => "Query cancelled".to_string(),
        };
        if let Some(read_rows) = active.read_rows {
            if let Some(total_rows) = active.total_rows {
                status.push_str(&format!(
                    "  Read {} of {} rows",
                    Self::format_count(read_rows),
                    Self::format_count(total_rows)
                ));
            } else {
                status.push_str(&format!("  Read {} rows", Self::format_count(read_rows)));
            }
        }
        if let Some(read_bytes) = active.read_bytes {
            status.push_str(&format!("  {} read", Self::format_bytes(read_bytes)));
        } else if active.received_bytes > 0 {
            status.push_str(&format!(
                "  {} received",
                Self::format_bytes(active.received_bytes)
            ));
        }
        let elapsed = active
            .elapsed
            .or_else(|| active.started_at.map(|started| started.elapsed()))
            .map(format_query_duration);

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
                                    .id("run-selection")
                                    .px_3()
                                    .py_1()
                                    .rounded(px(3.))
                                    .border_1()
                                    .border_color(rgb(BORDER))
                                    .text_color(rgb(TEXT_DIM))
                                    .child("Run all  ⌃X")
                                    .when(!running, |button| {
                                        button
                                            .text_color(rgb(TEXT))
                                            .hover(|button| {
                                                button.bg(rgb(0x2c3d4a)).cursor_pointer()
                                            })
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.run_selection(window, cx)
                                            }))
                                    }),
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
                    .when(
                        matches!(
                            active.outcome,
                            QueryOutcome::Error(_) | QueryOutcome::StatementError { .. }
                        ),
                        |row| row.bg(rgb(0x2b2227)).text_color(rgb(DANGER)),
                    )
                    .when(
                        !matches!(
                            active.outcome,
                            QueryOutcome::Error(_) | QueryOutcome::StatementError { .. }
                        ),
                        |row| row.text_color(rgb(TEXT_DIM)),
                    )
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .items_start()
                            .justify_between()
                            .gap_4()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .items_center()
                                    .gap_3()
                                    .child(div().flex_1().min_w_0().child(status)),
                            )
                            .when(statement_failed, |row| {
                                row.child(
                                    div()
                                        .flex_none()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .child(
                                            div()
                                                .id("skip-failed-statement")
                                                .px_2()
                                                .rounded(px(3.))
                                                .border_1()
                                                .border_color(rgb(BORDER))
                                                .text_color(rgb(TEXT))
                                                .hover(|button| {
                                                    button.bg(rgb(0x3d2528)).cursor_pointer()
                                                })
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.resolve_statement_failure(true, cx)
                                                }))
                                                .child("Skip"),
                                        )
                                        .child(
                                            div()
                                                .id("cancel-remaining-statements")
                                                .px_2()
                                                .rounded(px(3.))
                                                .border_1()
                                                .border_color(rgb(BORDER))
                                                .text_color(rgb(TEXT))
                                                .hover(|button| {
                                                    button.bg(rgb(0x3d2528)).cursor_pointer()
                                                })
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.resolve_statement_failure(false, cx)
                                                }))
                                                .child("Cancel rest"),
                                        ),
                                )
                            })
                            .when_some(elapsed, |row, elapsed| {
                                row.child(
                                    div().flex_none().text_color(rgb(TEXT_DIM)).child(elapsed),
                                )
                            }),
                    ),
            )
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
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .min_w_0()
                    .child(div().overflow_hidden().whitespace_nowrap().child(status))
                    .when_some(self.footer_vim_state(), |row, state| {
                        let (normal, label, command_line, recording) = state;
                        row.child(div().flex_none().child("|"))
                            .child(
                                div()
                                    .flex_none()
                                    .text_color(rgb(if normal { 0x9ab7a1 } else { TEXT_DIM }))
                                    .child(format!("-- {label} --")),
                            )
                            .when_some(command_line, |row, command_line| {
                                let mut text = command_line.text;
                                let cursor = command_line.cursor.min(text.chars().count());
                                let byte = text
                                    .char_indices()
                                    .nth(cursor)
                                    .map(|(index, _)| index)
                                    .unwrap_or(text.len());
                                text.insert(byte, '\u{258c}');
                                row.child(
                                    div()
                                        .flex_none()
                                        .text_color(rgb(TEXT))
                                        .child(format!("{}{text}", command_line.prompt)),
                                )
                            })
                            .when_some(recording, |row, register| {
                                row.child(
                                    div()
                                        .flex_none()
                                        .text_color(rgb(0xd7a65f))
                                        .child(format!("recording @{register}")),
                                )
                            })
                    }),
            )
            .child(concat!("zedb ", env!("CARGO_PKG_VERSION"), " | M8"))
    }

    /// Vim state for the bottom bar: mode, command line, and recording
    /// register of the active query tab, when vim mode is on and the
    /// query editor is the active view.
    #[allow(clippy::type_complexity)]
    fn footer_vim_state(
        &self,
    ) -> Option<(
        bool,
        &'static str,
        Option<CommandLineSnapshot>,
        Option<char>,
    )> {
        if !self.preferences.vim_mode || self.show_fleet || self.connected.is_none() {
            return None;
        }
        let tab = self.query_tabs.get(self.active_query_tab)?;
        Some((
            tab.vim.mode() == modalkit::env::vim::VimMode::Normal,
            tab.vim.mode_label(),
            tab.vim_command_line.clone(),
            tab.vim_recording,
        ))
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Agent-requested effects need a Window; apply them here.
        self.agent_drain_effects(window, cx);
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(BG))
            .text_color(rgb(TEXT))
            .font_family("Menlo")
            .text_sm()
            .on_action(cx.listener(Self::run_query_action))
            .on_action(cx.listener(Self::run_selection_action))
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
            .on_action(
                cx.listener(|this, action: &SelectNode, _, cx| this.select_node(action.index, cx)),
            )
            .on_action(cx.listener(|this, action: &DuplicateConnection, _, cx| {
                this.duplicate_connection(action.index, cx)
            }))
            .on_action(cx.listener(|this, action: &EditConnection, _, cx| {
                this.selected = Some(action.index);
                this.start_edit(cx)
            }))
            .on_action(cx.listener(|this, action: &DeleteConnection, _, cx| {
                this.selected = Some(action.index);
                this.request_delete(cx)
            }))
            .on_action(cx.listener(|this, action: &grid_spike::HeaderSort, _, cx| {
                if let Some(tab) = this.query_tabs.get(this.active_query_tab) {
                    let grid = tab.result_grid.clone();
                    grid.update(cx, |grid, cx| grid.header_sort_action(action, cx));
                }
            }))
            .on_action(
                cx.listener(|this, action: &grid_spike::HeaderFilter, window, cx| {
                    this.open_column_filter(action.column.clone(), window, cx)
                }),
            )
            .on_action(cx.listener(|this, action: &ViewObjectDdl, window, cx| {
                let object = this
                    .schema_databases
                    .iter()
                    .find_map(|database| {
                        if database.meta.name != action.database {
                            return None;
                        }
                        database.objects.as_ref()?.iter().find_map(|object| {
                            (object.name == action.object).then(|| object.clone())
                        })
                    })
                    .or_else(|| {
                        // The sidebar may not have loaded that database's
                        // objects yet; the schema cache has them.
                        this.schema_cache.as_ref().and_then(|cache| {
                            cache
                                .snapshot()
                                .object(&action.database, &action.object)
                                .map(schema_object_from_cache)
                        })
                    });
                if let Some(object) = object {
                    this.select_schema_object(
                        action.database.clone(),
                        object,
                        ObjectInspectorTab::Ddl,
                        window,
                        cx,
                    )
                }
            }))
            .on_action(
                cx.listener(|this, action: &agent_pane::StartAgentThread, window, cx| {
                    if action.index != usize::MAX {
                        this.agent_start_thread(action.index, window, cx);
                    }
                }),
            )
            .on_action(
                cx.listener(|this, _: &agent_pane::OpenAddAgent, window, cx| {
                    this.agent_open_add_form(window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &ToggleAgentPane, window, cx| this.agent_toggle(window, cx)),
            )
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
                if this.agent.resizing {
                    let viewport_width = f32::from(window.viewport_size().width);
                    this.agent.width = (viewport_width - f32::from(event.position.x))
                        .clamp(300.0, (viewport_width - 500.0).max(300.0));
                    cx.notify();
                }
                if this.fleet.resizing_detail {
                    let viewport_width = f32::from(window.viewport_size().width);
                    this.fleet.detail_width = (viewport_width - f32::from(event.position.x))
                        .clamp(280.0, (viewport_width - 400.0).max(280.0));
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
                    this.fleet.resizing_detail = false;
                    if this.agent.resizing {
                        this.agent.resizing = false;
                        this.preferences.agent_pane_width = Some(this.agent.width);
                        let _ = save_preferences(&this.preferences);
                    }
                    this.query_resize = None;
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, _| {
                    this.resizing_sidebar = false;
                    this.resizing_sidebar_sections = false;
                    this.fleet.resizing_detail = false;
                    this.agent.resizing = false;
                    this.query_resize = None;
                }),
            )
            .child(self.title_bar(cx))
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
                            .when(self.show_preferences, |main| {
                                main.child(self.preferences_panel(cx))
                            })
                            .when(!self.show_preferences && self.form.is_some(), |main| {
                                main.child(self.form_panel(cx))
                            })
                            .when(!self.show_preferences && self.form.is_none(), |main| {
                                main.child(self.connection_toolbar(cx)).child(
                                    div()
                                        .flex_1()
                                        .min_h_0()
                                        .when(self.show_query_editor, |content| {
                                            content.child(self.query_editor_panel(cx))
                                        })
                                        .when(
                                            !self.show_query_editor && self.show_fleet,
                                            |content| content.child(self.fleet_panel(cx)),
                                        )
                                        .when(
                                            !self.show_query_editor && !self.show_fleet,
                                            |content| {
                                                content
                                                    .when(
                                                        self.selected_schema_object.is_some(),
                                                        |content| {
                                                            content.child(
                                                                self.schema_object_panel(
                                                                    window, cx,
                                                                ),
                                                            )
                                                        },
                                                    )
                                                    .when(
                                                        self.selected_schema_object.is_none(),
                                                        |content| {
                                                            content.child(self.cluster_overview())
                                                        },
                                                    )
                                            },
                                        ),
                                )
                            }),
                    )
                    .when(self.agent.open, |row| {
                        row.child(self.agent_panel(window, cx))
                    }),
            )
            .child(self.status_bar())
            .when(self.show_about, |root| root.child(self.about_panel(cx)))
    }
}

impl Workspace {
    /// Check for updates on demand (the menu item); the periodic loop
    /// stays quiet when nothing is newer, this says so out loud.
    fn check_for_updates_now(&mut self, cx: &mut Context<Self>) {
        self.notice = Some("Checking for updates...".into());
        self.notice_warning = false;
        cx.notify();
        let handle = rt::tokio().spawn(updates::check());
        cx.spawn(async move |this, cx| {
            let update = handle.await.ok().flatten();
            this.update(cx, |this, cx| {
                match update {
                    Some(update) => {
                        this.notice = Some(format!(
                            "v{} is available; click the update pill in the title bar to install",
                            update.version
                        ));
                        if this.update_phase == UpdatePhase::Available {
                            this.update_available = Some(update);
                        }
                    }
                    None => {
                        this.notice = Some(format!(
                            "No newer release found; you are on v{}",
                            env!("CARGO_PKG_VERSION")
                        ));
                    }
                }
                this.notice_warning = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn about_panel(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let version = env!("CARGO_PKG_VERSION");
        let commit = option_env!("ZEDB_BUILD_COMMIT").unwrap_or("dev");
        let build = option_env!("ZEDB_BUILD_NUMBER").unwrap_or("0");
        let full_version = format!("{version}+build.{build}.{commit}");
        let copy_text = full_version.clone();
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::rgba(0x00000088))
            .child(
                div()
                    .w(px(560.))
                    .p_5()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(rgb(BORDER))
                    .bg(rgb(BG_SIDEBAR))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_2()
                    .child(
                        // Explicit Embedded: a bare filename parses as
                        // a relative URI, so From<&str> routes it to
                        // the HTTP client and it never loads.
                        img(gpui::ImageSource::Resource(gpui::Resource::Embedded(
                            "about-logo.png".into(),
                        )))
                        .size(px(96.)),
                    )
                    .child(
                        div()
                            .text_xl()
                            .text_color(rgb(TEXT))
                            .child(format!("zeDB {version}")),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(TEXT_DIM))
                            .mt_2()
                            .child("Commit"),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_family("Menlo")
                            .text_color(rgb(TEXT))
                            .child(commit),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(TEXT_DIM))
                            .mt_2()
                            .child("Version"),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_family("Menlo")
                            .text_color(rgb(TEXT))
                            .child(full_version),
                    )
                    .child(
                        div()
                            .mt_4()
                            .w_full()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .id("about-ok")
                                    .flex_1()
                                    .py_1()
                                    .rounded(px(4.))
                                    .border_1()
                                    .border_color(rgb(BORDER))
                                    .text_center()
                                    .text_color(rgb(TEXT))
                                    .child("OK")
                                    .hover(|button| button.bg(rgb(0x303640)).cursor_pointer())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.show_about = false;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .id("about-copy")
                                    .flex_1()
                                    .py_1()
                                    .rounded(px(4.))
                                    .border_1()
                                    .border_color(rgb(BORDER))
                                    .text_center()
                                    .text_color(rgb(TEXT))
                                    .child("Copy")
                                    .hover(|button| button.bg(rgb(0x303640)).cursor_pointer())
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                            copy_text.clone(),
                                        ));
                                        this.notice = Some("Version copied".into());
                                        this.notice_warning = false;
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
    }
}

/// Split `text` into statement byte ranges on top-level semicolons, ignoring
/// semicolons inside strings and comments. Ranges exclude the semicolon and may
/// be blank; always returns at least one range.
fn split_statements(text: &str) -> Vec<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut segments: Vec<(usize, usize)> = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            quote @ (b'\'' | b'"' | b'`') => {
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b'\\' && quote != b'`' {
                        i += 2;
                    } else if bytes[i] == quote {
                        i += 1;
                        break;
                    } else {
                        i += 1;
                    }
                }
            }
            b'-' if bytes.get(i + 1) == Some(&b'-') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i < bytes.len() {
                    if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/') {
                        i += 2;
                        break;
                    }
                    i += 1;
                }
            }
            b';' => {
                segments.push((start, i));
                i += 1;
                start = i;
            }
            _ => i += 1,
        }
    }
    segments.push((start, text.len()));
    segments
}

/// Return the statement containing the byte offset `cursor`. When the cursor
/// sits in blank space between statements, prefer the nearest non-empty
/// statement before it, then after it.
fn statement_at_cursor(text: &str, cursor: usize) -> Option<&str> {
    let segments = split_statements(text);
    let idx = segments
        .iter()
        .position(|&(start, end)| cursor >= start && cursor <= end)
        .unwrap_or(segments.len() - 1);
    let pick = |i: usize| {
        let (start, end) = segments[i];
        let statement = text[start..end.min(text.len())].trim();
        (!statement.is_empty()).then_some(statement)
    };
    pick(idx)
        .or_else(|| (0..idx).rev().find_map(pick))
        .or_else(|| ((idx + 1)..segments.len()).find_map(pick))
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

fn quit_ze_db(_: &QuitZeDb, cx: &mut App) {
    cx.quit();
}

/// Hidden server mode: the agent pane spawns this same executable as
/// its MCP server (the bundle ships no separate CLI). The config file
/// carries the connection credentials at 0600 and is deleted on read.
fn run_mcp_serve(config_path: &str) -> ! {
    let outcome = (|| -> Result<(), String> {
        let raw = std::fs::read_to_string(config_path).map_err(|error| error.to_string())?;
        let _ = std::fs::remove_file(config_path);
        let config: serde_json::Value =
            serde_json::from_str(&raw).map_err(|error| error.to_string())?;
        let repo = config
            .get("repo")
            .and_then(|value| value.as_str())
            .and_then(|path| zedb_core::repo::MigrationRepo::open(std::path::Path::new(path)).ok());
        let connection = config
            .get("url")
            .and_then(|value| value.as_str())
            .map(|url| zedb_ch::ChConfig {
                url: url.to_string(),
                user: config
                    .get("user")
                    .and_then(|value| value.as_str())
                    .unwrap_or("default")
                    .to_string(),
                password: config
                    .get("password")
                    .and_then(|value| value.as_str())
                    .filter(|password| !password.is_empty())
                    .map(str::to_string),
                database: None,
                read_only: true,
            });
        let mut server = zedb_ch::mcp::McpServer::new(repo, connection, Default::default());
        if let Some(socket) = config.get("app_socket").and_then(|value| value.as_str()) {
            server = server.with_app_bridge(std::path::PathBuf::from(socket));
        }
        if let Some(cache) = config.get("schema_cache").and_then(|value| value.as_str()) {
            server = server.with_schema_cache(std::path::PathBuf::from(cache));
        }
        let runtime = tokio::runtime::Runtime::new().map_err(|error| error.to_string())?;
        runtime
            .block_on(zedb_ch::mcp::serve_stdio(server))
            .map_err(|error| error.to_string())
    })();
    match outcome {
        Ok(()) => std::process::exit(0),
        Err(error) => {
            eprintln!("zedb-mcp-serve: {error}");
            std::process::exit(1);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("zedb-mcp-serve") {
        let config_path = args.get(2).cloned().unwrap_or_default();
        run_mcp_serve(&config_path);
    }
    // ZEDB_LOG=1 surfaces log-crate records (gpui swallows asset and
    // image errors into log::error, which is silence without a logger).
    if std::env::var_os("ZEDB_LOG").is_some() {
        struct StderrLogger;
        impl log::Log for StderrLogger {
            fn enabled(&self, _: &log::Metadata) -> bool {
                true
            }
            fn log(&self, record: &log::Record) {
                eprintln!("[{}] {}", record.level(), record.args());
            }
            fn flush(&self) {}
        }
        let _ = log::set_logger(&StderrLogger);
        log::set_max_level(log::LevelFilter::Warn);
    }
    Application::new().with_assets(Assets).run(|cx: &mut App| {
        gpui_component::init(cx);
        configure_component_theme(cx);
        text_input::init(cx);
        cx.on_action(quit_ze_db);
        cx.bind_keys([
            KeyBinding::new("cmd-enter", RunQuery, None),
            KeyBinding::new("ctrl-x", RunSelection, None),
            KeyBinding::new("cmd-,", OpenPreferences, None),
            KeyBinding::new("cmd-i", ToggleAgentPane, None),
            // In multi-line inputs the composer sends on plain enter;
            // shift-enter keeps inserting a newline via the secondary
            // Enter action (which the composer ignores).
            KeyBinding::new(
                "shift-enter",
                gpui_component::input::Enter { secondary: true },
                Some("Input"),
            ),
        ]);
        cx.set_menus(vec![Menu {
            name: "zeDB".into(),
            items: vec![
                MenuItem::action("About zeDB", OpenAbout),
                MenuItem::action("Check for Updates…", CheckForUpdates),
                MenuItem::separator(),
                MenuItem::action("Preferences…", OpenPreferences),
                MenuItem::separator(),
                MenuItem::os_submenu("Services", SystemMenuType::Services),
                MenuItem::separator(),
                MenuItem::action("Quit zeDB", QuitZeDb),
            ],
        }]);
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
                let workspace = cx.new(|cx| Workspace::new(window, cx));
                let preferences_workspace = workspace.clone();
                cx.on_action(move |_: &OpenPreferences, cx| {
                    preferences_workspace.update(cx, |workspace, cx| {
                        workspace.open_preferences(cx);
                    });
                });
                let about_workspace = workspace.clone();
                cx.on_action(move |_: &OpenAbout, cx| {
                    about_workspace.update(cx, |workspace, cx| {
                        workspace.show_about = true;
                        cx.notify();
                    });
                });
                let updates_workspace = workspace.clone();
                cx.on_action(move |_: &CheckForUpdates, cx| {
                    updates_workspace.update(cx, |workspace, cx| {
                        workspace.check_for_updates_now(cx);
                    });
                });
                cx.new(|cx| Root::new(workspace, window, cx))
            },
        )
        .expect("failed to open window");
        cx.activate(true);
    });
}

#[cfg(test)]
mod tests {
    use super::{format_engine_definition, split_statements, statement_at_cursor};

    #[test]
    fn engine_definition_is_split_at_top_level_clauses() {
        let formatted = format_engine_definition(
            "MergeTree ORDER BY id PARTITION BY toYYYYMM(created_at) SETTINGS index_granularity = 8192",
        );

        assert_eq!(
            formatted,
            "ENGINE = MergeTree\nORDER BY id\nPARTITION BY toYYYYMM(created_at)\nSETTINGS index_granularity = 8192"
        );
    }

    #[test]
    fn split_statements_yields_every_statement_in_order() {
        let text = "SELECT 1;\n-- a; comment\nSELECT ';';\n\nSELECT 3";
        let statements: Vec<&str> = split_statements(text)
            .into_iter()
            .map(|(start, end)| text[start..end].trim())
            .filter(|statement| !statement.is_empty())
            .collect();
        assert_eq!(
            statements,
            vec!["SELECT 1", "-- a; comment\nSELECT ';'", "SELECT 3"]
        );
    }

    #[test]
    fn statement_at_cursor_picks_statement_under_cursor() {
        let text = "SELECT 1;\nSELECT 2;\nSELECT 3";
        assert_eq!(statement_at_cursor(text, 3), Some("SELECT 1"));
        assert_eq!(statement_at_cursor(text, 12), Some("SELECT 2"));
        assert_eq!(statement_at_cursor(text, text.len()), Some("SELECT 3"));
    }

    #[test]
    fn statement_at_cursor_handles_single_statement() {
        assert_eq!(statement_at_cursor("SELECT 1", 4), Some("SELECT 1"));
        assert_eq!(statement_at_cursor("", 0), None);
        assert_eq!(statement_at_cursor("  \n ; ; ", 3), None);
    }

    #[test]
    fn statement_at_cursor_ignores_semicolons_in_strings_and_comments() {
        let text = "SELECT ';' AS a; -- trailing; comment\nSELECT /* not; here */ 2";
        assert_eq!(statement_at_cursor(text, 4), Some("SELECT ';' AS a"));
        assert_eq!(
            statement_at_cursor(text, text.len()),
            Some("-- trailing; comment\nSELECT /* not; here */ 2")
        );
    }

    #[test]
    fn statement_at_cursor_falls_back_to_nearest_statement_from_blank_space() {
        let text = "SELECT 1;\n\n  \nSELECT 2;\n\n";
        assert_eq!(statement_at_cursor(text, text.len()), Some("SELECT 2"));
        let leading = ";\nSELECT 9";
        assert_eq!(statement_at_cursor(leading, 0), Some("SELECT 9"));
    }
}
