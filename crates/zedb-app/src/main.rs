#[path = "features/agent/mod.rs"]
mod agent_pane;
#[path = "features/fleet/author.rs"]
mod author;
#[path = "platform/clickhouse_cloud.rs"]
mod clickhouse_cloud;
#[path = "features/fleet/codegen.rs"]
mod codegen;
#[path = "features/settings/command_palette.rs"]
mod command_palette;
#[path = "features/fleet/commit.rs"]
mod commit;
#[path = "ui/components/mod.rs"]
mod components;
#[path = "features/query/explain.rs"]
mod explain_ui;
#[path = "features/query/export.rs"]
mod export;
mod features;
#[path = "features/fleet/mod.rs"]
mod fleet;
#[path = "platform/github.rs"]
mod github;
#[path = "features/query/components/grid/mod.rs"]
mod grid_spike;
#[path = "features/operations/mod.rs"]
mod ops;
#[path = "features/query/advisor.rs"]
mod query_advisor;
#[path = "features/history/mod.rs"]
mod query_history;
#[path = "platform/runtime.rs"]
mod rt;
#[path = "features/schema/intelligence.rs"]
mod schema_intelligence_ui;
#[path = "features/settings/sync.rs"]
mod settings_sync;
#[path = "shell/chrome.rs"]
mod shell_chrome;
#[path = "shell/navigation.rs"]
mod shell_navigation;
#[path = "shell/overlays.rs"]
mod shell_overlays;
#[path = "shell/render.rs"]
mod shell_render;
#[path = "shell/workspace.rs"]
mod shell_workspace;
#[path = "features/schema/advisor.rs"]
mod storage_advisor;
#[path = "features/query/tail.rs"]
mod tail;
#[path = "ui/theme.rs"]
mod theme;
#[path = "features/query/type_highlight.rs"]
mod type_highlight;
#[path = "platform/updates.rs"]
mod updates;
#[path = "features/query/vim.rs"]
mod vim;

use std::{
    borrow::Cow,
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
    Root, Theme,
};
use zedb_ch::{
    schema_cache::SchemaCache, ChClient, ChConfig, QueryStreamEvent, QueryStreamSummary,
    SchemaObjectKind, SchemaObjectMeta,
};
use zedb_core::{
    load_connections, load_preferences, save_connections, save_preferences, ConnectionConfig,
    ConnectionNode, EnvTier, Preferences,
};

use components::text_input::{self, TextInput};
use features::connections::{
    differentiating_cluster, ConnectedCluster, ConnectionDraft, ConnectionForm, ConnectionState,
    DriverSettingForm, EndpointHealth, NodeForm,
};
use features::query::{
    max_rows_from_limit, nearest_occurrence, resolve_query_variables, split_statements,
    statement_at_cursor, tab_display_name, MaxRows, QueryEstimate, QueryOutcome, QueryResizeTarget,
    QueryState, QueryTab, RunEvent, TailBatch, TailPush, TailState, TailStream, TailStreamBatch,
    TailStripInfo, TailWatch,
};
use features::schema::{
    database_nodes_from_cache, schema_object_from_cache, DatabaseNode, ObjectInspectorTab,
    SchemaState, SelectedSchemaObject,
};
use fleet::FleetState;
use grid_spike::GridSpike;
use schema_intelligence_ui::{byte_range_to_lsp, SchemaProvider};
use vim::{CommandLineSnapshot, VimController};

/// The query a fresh install starts with: valid on any ClickHouse and
/// a useful first look at what the server holds.
const DEFAULT_QUERY: &str = "select database, name, engine, total_rows\nfrom system.tables\nwhere database not in ('system', 'INFORMATION_SCHEMA', 'information_schema')\norder by total_rows desc\nlimit 100;";

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
        OpenCommandPalette,
        OpenPreferences,
        OpenSettingsFile,
        ToggleAgentPane,
        QuitZeDb,
        RunQuery,
        RunSelection,
        SaveQueryTab,
        MaxRows1k,
        MaxRows10k,
        MaxRows50k,
        MaxRows100k,
        MaxRows1m,
        MaxRowsUnlimited
    ]
);

struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        let bytes: Option<&'static [u8]> = match path {
            "icons/chevron-down.svg" => Some(include_bytes!("../assets/icons/chevron-down.svg")),
            "icons/close.svg" => Some(include_bytes!("../assets/icons/close.svg")),
            "icons/edit.svg" => Some(include_bytes!("../assets/icons/edit.svg")),
            "icons/copy.svg" => Some(include_bytes!("../assets/icons/copy.svg")),
            "icons/github.svg" => Some(include_bytes!("../assets/icons/github.svg")),
            "icons/gitlab.svg" => Some(include_bytes!("../assets/icons/gitlab.svg")),
            "icons/ops.svg" => Some(include_bytes!("../assets/icons/ops.svg")),
            "icons/history.svg" => Some(include_bytes!("../assets/icons/history.svg")),
            "icons/bookmark.svg" => Some(include_bytes!("../assets/icons/bookmark.svg")),
            "icons/star.svg" => Some(include_bytes!("../assets/icons/star.svg")),
            "icons/execute.svg" => Some(include_bytes!("../assets/icons/execute.svg")),
            "icons/check-chain.svg" => Some(include_bytes!("../assets/icons/check-chain.svg")),
            "icons/commit.svg" => Some(include_bytes!("../assets/icons/commit.svg")),
            "icons/fleet.svg" => Some(include_bytes!("../assets/icons/fleet.svg")),
            "icons/migration-plus.svg" => {
                Some(include_bytes!("../assets/icons/migration-plus.svg"))
            }
            "icons/regen.svg" => Some(include_bytes!("../assets/icons/regen.svg")),
            "icons/folder-open.svg" => Some(include_bytes!("../assets/icons/folder-open.svg")),
            "icons/lock.svg" => Some(include_bytes!("../assets/icons/lock.svg")),
            "icons/hourglass.svg" => Some(include_bytes!("../assets/icons/hourglass.svg")),
            "icons/lock-open.svg" => Some(include_bytes!("../assets/icons/lock-open.svg")),
            "icons/plug.svg" => Some(include_bytes!("../assets/icons/plug.svg")),
            "icons/plug-off.svg" => Some(include_bytes!("../assets/icons/plug-off.svg")),
            "icons/pull.svg" => Some(include_bytes!("../assets/icons/pull.svg")),
            "icons/query-plus.svg" => Some(include_bytes!("../assets/icons/query-plus.svg")),
            "icons/refresh.svg" => Some(include_bytes!("../assets/icons/refresh.svg")),
            "icons/agent-claude.svg" => Some(include_bytes!("../assets/icons/agent-claude.svg")),
            "icons/agent-codex.svg" => Some(include_bytes!("../assets/icons/agent-codex.svg")),
            "icons/send.svg" => Some(include_bytes!("../assets/icons/send.svg")),
            "icons/verify.svg" => Some(include_bytes!("../assets/icons/verify.svg")),
            "icons/advise.svg" => Some(include_bytes!("../assets/icons/advise.svg")),
            "icons/sparkle.svg" => Some(include_bytes!("../assets/icons/sparkle.svg")),
            "icons/experimental.svg" => Some(include_bytes!("../assets/icons/experimental.svg")),
            "icons/stop.svg" => Some(include_bytes!("../assets/icons/stop.svg")),
            "icons/play.svg" => Some(include_bytes!("../assets/icons/play.svg")),
            "icons/pause.svg" => Some(include_bytes!("../assets/icons/pause.svg")),
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

#[derive(Clone, PartialEq, Action)]
#[action(no_json, no_register)]
struct SelectNode {
    index: usize,
}

/// Choose the apply scope from the node selector: `Some(cluster)` runs
/// schema changes `ON CLUSTER`, `None` returns to the active node only.
#[derive(Clone, PartialEq, Action)]
#[action(no_json, no_register)]
struct SetApplyCluster {
    cluster: Option<String>,
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

/// Query-tab context-menu actions, each targeting a tab by id.
#[derive(Clone, PartialEq, Action)]
#[action(no_json, no_register)]
struct CloseQueryTab {
    tab_id: usize,
}

#[derive(Clone, PartialEq, Action)]
#[action(no_json, no_register)]
struct CloseOtherQueryTabs {
    tab_id: usize,
}

#[derive(Clone, PartialEq, Action)]
#[action(no_json, no_register)]
struct CloseQueryTabsToRight {
    tab_id: usize,
}

/// Start a live tail of a table, from the schema sidebar's table
/// context menu. `cap` is the retained-row limit the user opted into
/// (`None` = unlimited); the initial load is always small regardless.
#[derive(Clone, PartialEq, Action)]
#[action(no_json, no_register)]
struct TailTable {
    database: String,
    object: String,
    cap: Option<usize>,
}

/// Optional GitHub identity; sign-in is never required to use the app.
enum GithubAuth {
    SignedOut,
    /// Waiting for the user to approve the device code in the browser.
    Authorizing {
        user_code: String,
        verification_uri: String,
    },
    SignedIn(github::Profile),
}

/// Drag payload for reordering query tabs; also renders the drag ghost.
#[derive(Clone)]
struct DragTab {
    /// The dragged tab's position in the strip.
    index: usize,
    label: gpui::SharedString,
}

impl Render for DragTab {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_3()
            .py_1()
            .rounded(px(3.))
            .bg(theme::bg_sidebar())
            .border_1()
            .border_color(theme::accent())
            .text_color(theme::text())
            .child(self.label.clone())
    }
}

struct Workspace {
    fleet: FleetState,
    agent: agent_pane::AgentPaneState,
    author: Option<author::AuthorState>,
    regen: Option<codegen::RegenState>,
    checks: Option<codegen::ChecksState>,
    /// The last run of the chain checks passed in full and nothing in
    /// the repo changed since; tints the check-chain icon green.
    checks_clean: bool,
    /// The last regen's verdict: Some(true) current-state matches the
    /// chain (green icon), Some(false) it has drifted and a write is
    /// pending (yellow), None unknown.
    regen_status: Option<bool>,
    commit: Option<commit::CommitState>,
    show_fleet: bool,
    show_ops: bool,
    history: query_history::HistoryState,
    /// Where an error-bar ask came from: (query tab id, failed sql).
    /// An agent-proposed query replaces that statement in place.
    agent_fix_target: Option<(usize, String)>,
    /// The export dialog, when open.
    export: Option<export::ExportState>,
    ops: ops::OpsState,
    /// query_ids killed from the ops view; errors on these statements
    /// report the kill instead of a transport failure.
    ops_killed: std::collections::HashSet<String>,
    health_poll_generation: u64,
    /// Cancels a stale merges poll when the object or tab changes.
    merges_poll_generation: u64,
    connection: ConnectionState,
    schema: SchemaState,
    notice: Option<String>,
    notice_warning: bool,
    notice_flash_id: u64,
    update_available: Option<updates::UpdateInfo>,
    update_phase: UpdatePhase,
    sidebar_width: f32,
    resizing_sidebar: bool,
    connections_pane_height: f32,
    resizing_sidebar_sections: bool,
    query: QueryState,
    show_query_editor: bool,
    /// Last window-refocus health/update check, for debounce.
    last_focus_check: Option<Instant>,
    github: GithubAuth,
    github_generation: u64,
    preferences: Preferences,
    palette: command_palette::PaletteState,
    settings_sync: settings_sync::SettingsSyncState,
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
        let (mut preferences, preferences_error) = match load_preferences() {
            Ok(preferences) => (preferences, None),
            Err(error) => (
                Preferences::default(),
                Some(format!("Could not load preferences: {error}")),
            ),
        };
        // Saved queries gained a `saved_at` timestamp; anchor any that
        // predate it to now, once, so the "saved N ago" label shows for
        // them going forward (their true save time is unknowable).
        {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_secs() as i64)
                .unwrap_or(0);
            let mut stamped = false;
            for saved in &mut preferences.saved_queries {
                if saved.saved_at == 0 {
                    saved.saved_at = now;
                    stamped = true;
                }
            }
            if stamped {
                let _ = save_preferences(&preferences);
            }
        }
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
        // A stored GitHub token restores the profile quietly.
        if let Some((provider, token)) = github::stored_token_any() {
            let handle =
                rt::tokio().spawn(async move { github::fetch_profile(provider, &token).await });
            cx.spawn(async move |this, cx| {
                if let Ok(Ok(profile)) = handle.await {
                    this.update(cx, |this, cx| {
                        this.github = GithubAuth::SignedIn(profile);
                        cx.notify();
                    })
                    .ok();
                }
            })
            .detach();
        }
        // Settings sync: one reconcile pass at launch.
        cx.spawn(async move |this, cx| {
            this.update(cx, |this, cx| this.settings_sync_tick(cx)).ok();
        })
        .detach();
        // Linked Cloud orgs: fetch service states once at launch so the
        // sidebar's idle/waking markers exist before the panel is ever
        // opened (one API call per org, nothing when none are linked).
        cx.spawn(async move |this, cx| {
            this.update(cx, |this, cx| this.cloud_refresh(cx)).ok();
        })
        .detach();

        // Coming back to the window re-checks cluster health and
        // updates, debounced so focus flapping stays quiet.
        cx.observe_window_activation(window, |this, window, cx| {
            if !window.is_window_active() {
                return;
            }
            let now = Instant::now();
            if this
                .last_focus_check
                .is_some_and(|last| now.duration_since(last) < Duration::from_secs(30))
            {
                return;
            }
            this.last_focus_check = Some(now);
            this.focus_recheck(cx);
        })
        .detach();
        let schema_filter = Self::input("", "Filter schema", false, cx);
        cx.observe(&schema_filter, |this: &mut Self, _, cx| {
            // A filter auto-expands every matching database. Pull their
            // objects from the warmed cache now so they render populated
            // straight away; without this the database shows an expanded
            // chevron over an empty list and the load only fires on the
            // next click (the double-click bug). Cache reads only, so no
            // network and bounded to already-warmed databases.
            let snapshot = this.schema.cache.as_ref().map(|cache| cache.snapshot());
            for database in &mut this.schema.databases {
                database.filter_collapsed = false;
                if database.objects.is_none() {
                    if let Some(cached) = snapshot
                        .as_ref()
                        .and_then(|snap| snap.database(&database.meta.name))
                    {
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
                    // The palette chord is handled here, not via action
                    // dispatch: key equivalents die whenever the window
                    // has no live focus path, and this interceptor is the
                    // one place that provably sees every keystroke.
                    if event.keystroke.modifiers.platform
                        && event.keystroke.modifiers.shift
                        && event.keystroke.key == "p"
                    {
                        this.palette_toggle(window, cx);
                        cx.stop_propagation();
                        return;
                    }
                    // Palette keys come first: it floats above everything
                    // and its input must not feed vim or the editors.
                    if this.palette.open {
                        match event.keystroke.key.as_str() {
                            "escape" => {
                                this.palette_close(window, cx);
                                cx.stop_propagation();
                            }
                            "up" => {
                                this.palette_move(-1, cx);
                                cx.stop_propagation();
                            }
                            "down" => {
                                this.palette_move(1, cx);
                                cx.stop_propagation();
                            }
                            "enter" => {
                                this.palette_run_selected(window, cx);
                                cx.stop_propagation();
                            }
                            _ => {
                                // The filter text changes after this
                                // keystroke lands in the input; preview
                                // once it has.
                                let handle = cx.entity();
                                cx.defer(move |cx| {
                                    handle.update(cx, |this, cx| this.palette_theme_preview(cx));
                                });
                            }
                        }
                        return;
                    }
                    // cmd-i toggles the agent pane; interceptor-handled
                    // like the palette chord so focus state can't kill it.
                    if event.keystroke.modifiers.platform
                        && !event.keystroke.modifiers.shift
                        && event.keystroke.key == "i"
                    {
                        this.agent_toggle(window, cx);
                        cx.stop_propagation();
                        return;
                    }
                    // cmd-. forces the schema completion menu open in the
                    // active query editor, even with no prefix typed.
                    if event.keystroke.modifiers.platform
                        && event.keystroke.key == "."
                        && this.show_query_editor
                    {
                        if let Some(tab) = this.query.tabs.get(this.query.active_tab) {
                            let editor = tab.editor.clone();
                            editor.update(cx, |editor, cx| {
                                editor.show_completion_menu(window, cx);
                            });
                            cx.stop_propagation();
                            return;
                        }
                    }
                    // Tab cycles the connection form's fields.
                    if event.keystroke.key == "tab"
                        && this.connection.form.is_some()
                        && !event.keystroke.modifiers.platform
                        && !event.keystroke.modifiers.control
                    {
                        this.form_tab(event.keystroke.modifiers.shift, window, cx);
                        cx.stop_propagation();
                        return;
                    }
                    // cmd-n with the agent pane open: new thread with the
                    // last-used agent.
                    if event.keystroke.modifiers.platform
                        && !event.keystroke.modifiers.shift
                        && event.keystroke.key == "n"
                        && this.agent.open
                    {
                        this.agent_start_last_thread(window, cx);
                        cx.stop_propagation();
                        return;
                    }
                    // Escape closes an open filter popover regardless of
                    // where focus sits (checkbox panels hold none).
                    if event.keystroke.key == "escape" {
                        if let Some(tab) = this.query.tabs.get(this.query.active_tab) {
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
        let tab_contents: Vec<zedb_core::SavedQueryTab> = match &saved_session {
            Some(session) if !session.tabs.is_empty() => session.tabs.clone(),
            _ => vec![zedb_core::SavedQueryTab {
                id: zedb_core::new_local_id("tab"),
                saved_tab_id: None,
                name: "Tab 1".to_string(),
                sql: DEFAULT_QUERY.to_string(),
            }],
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
            .map(|(index, saved)| {
                let mut tab = Self::make_query_tab(
                    index + 1,
                    &saved.sql,
                    schema_provider.clone(),
                    window,
                    cx,
                );
                if !saved.id.is_empty() {
                    tab.persistent_id = saved.id;
                }
                tab.saved_tab_id = saved.saved_tab_id;
                if !saved.name.is_empty() {
                    tab.name = saved.name;
                }
                tab
            })
            .collect();
        match load_connections() {
            Ok(connections) => Self {
                connection: ConnectionState::new(connections),
                schema: SchemaState::new(schema_filter, schema_provider.clone()),
                notice: None,
                notice_warning: false,
                notice_flash_id: 0,
                update_available: None,
                update_phase: UpdatePhase::Available,
                sidebar_width: 240.0,
                resizing_sidebar: false,
                connections_pane_height: 430.0,
                resizing_sidebar_sections: false,
                query: QueryState::new(query_tabs, active_query_tab, next_query_tab_id),
                show_query_editor: false,
                fleet: FleetState::new(
                    fleet_repo_path.as_deref().unwrap_or(""),
                    fleet_cluster.as_deref().unwrap_or(""),
                    window,
                    cx,
                ),
                show_fleet: false,
                show_ops: false,
                history: query_history::HistoryState::new(
                    zedb_core::load_history(),
                    zedb_core::load_saved_tabs(),
                    Self::input("", "Search queries", false, cx),
                ),
                agent_fix_target: None,
                export: None,
                ops: ops::OpsState::default(),
                ops_killed: std::collections::HashSet::new(),
                agent: agent_pane::AgentPaneState::new(
                    preferences.agent_pane_width.unwrap_or(420.0),
                ),
                author: None,
                regen: None,
                checks: None,
                checks_clean: false,
                regen_status: None,
                commit: None,
                health_poll_generation: 0,
                merges_poll_generation: 0,
                last_focus_check: None,
                github: GithubAuth::SignedOut,
                github_generation: 0,
                preferences,
                palette: command_palette::PaletteState::new(cx),
                settings_sync: settings_sync::SettingsSyncState::new(cx),
                show_preferences: false,
                show_about: false,
            },
            Err(error) => Self {
                connection: ConnectionState::new(Vec::new()),
                schema: SchemaState::new(schema_filter, schema_provider),
                notice: Some(format!("Could not load connections: {error}")),
                notice_warning: false,
                notice_flash_id: 0,
                update_available: None,
                update_phase: UpdatePhase::Available,
                sidebar_width: 240.0,
                resizing_sidebar: false,
                connections_pane_height: 430.0,
                resizing_sidebar_sections: false,
                query: QueryState::new(query_tabs, active_query_tab, next_query_tab_id),
                show_query_editor: false,
                fleet: FleetState::new(
                    fleet_repo_path.as_deref().unwrap_or(""),
                    fleet_cluster.as_deref().unwrap_or(""),
                    window,
                    cx,
                ),
                show_fleet: false,
                show_ops: false,
                history: query_history::HistoryState::new(
                    zedb_core::load_history(),
                    zedb_core::load_saved_tabs(),
                    Self::input("", "Search queries", false, cx),
                ),
                agent_fix_target: None,
                export: None,
                ops: ops::OpsState::default(),
                ops_killed: std::collections::HashSet::new(),
                agent: agent_pane::AgentPaneState::new(
                    preferences.agent_pane_width.unwrap_or(420.0),
                ),
                author: None,
                regen: None,
                checks: None,
                checks_clean: false,
                regen_status: None,
                commit: None,
                health_poll_generation: 0,
                merges_poll_generation: 0,
                last_focus_check: None,
                github: GithubAuth::SignedOut,
                github_generation: 0,
                preferences,
                palette: command_palette::PaletteState::new(cx),
                settings_sync: settings_sync::SettingsSyncState::new(cx),
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
}
/// Best-effort teardown of a tail's live view, off-thread. The view only
/// exists on writable connections (watch setup skips read-only ones), and
/// a failure just leaves an orphan the server drops with the database.
fn drop_tail_view(config: ChConfig, view: String) {
    rt::tokio().spawn(async move {
        if let Ok(native) = zedb_ch::native::connect_pooled(&config).await {
            let _ = native.execute(&format!("DROP VIEW IF EXISTS {view}")).await;
        }
    });
}

/// The leading ORDER BY entry as a tailable key: a bare column identifier.
/// Returns `None` for an expression key (e.g. `toStartOfHour(at)`), which a
/// backtick-quoted predicate can't target; a column override comes later.
fn first_tail_key(order_by_entry: &str) -> Option<String> {
    let name = order_by_entry.trim().trim_matches('`').trim();
    let simple = !name.is_empty()
        && !name.starts_with(|c: char| c.is_ascii_digit())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    simple.then(|| name.to_string())
}

/// The table's real `ORDER BY` columns and `PARTITION BY` expression from
/// `system.tables`, for the query advisor. `table` is `db.name` from the
/// EXPLAIN plan. Returns `None` when the table isn't qualified or the
/// lookup fails.
async fn fetch_table_keys(client: &ChClient, table: Option<&str>) -> Option<(Vec<String>, String)> {
    let (database, name) = table?.split_once('.')?;
    let escape = |value: &str| value.replace('\'', "''");
    let sql = format!(
        "SELECT sorting_key, partition_key FROM system.tables \
         WHERE database = '{}' AND name = '{}'",
        escape(database),
        escape(name),
    );
    let result = client.query(&sql).await.ok()?;
    let row = result.rows.first()?;
    let sorting = row
        .first()
        .map(|value| value.to_string())
        .unwrap_or_default();
    let partition_key = row
        .get(1)
        .map(|value| value.to_string())
        .unwrap_or_default();
    let order_by: Vec<String> = sorting
        .split(',')
        .map(|column| column.trim().to_string())
        .filter(|column| !column.is_empty())
        .collect();
    Some((order_by, partition_key))
}

/// The filtered columns' types from `system.columns`, keyed by name, for
/// the query advisor's index-type choice. Empty when the table isn't
/// qualified or the lookup fails.
async fn fetch_column_types(
    client: &ChClient,
    table: Option<&str>,
) -> std::collections::HashMap<String, String> {
    let mut types = std::collections::HashMap::new();
    let Some((database, name)) = table.and_then(|table| table.split_once('.')) else {
        return types;
    };
    let escape = |value: &str| value.replace('\'', "''");
    let sql = format!(
        "SELECT name, type FROM system.columns WHERE database = '{}' AND table = '{}'",
        escape(database),
        escape(name),
    );
    if let Ok(result) = client.query(&sql).await {
        for row in &result.rows {
            if let (Some(name), Some(ty)) = (row.first(), row.get(1)) {
                types.insert(name.to_string(), ty.to_string());
            }
        }
    }
    types
}

/// Whether a WHERE conjunct is a range predicate (`<`, `>`, `BETWEEN`)
/// rather than an equality / `IN`. `!=` and `<>` are not ranges.
fn is_range_predicate(conjunct: &str) -> bool {
    let lower = conjunct.to_ascii_lowercase();
    if lower.contains(" between ") {
        return true;
    }
    let compacted = lower.replace("!=", "").replace("<>", "");
    compacted.contains('<') || compacted.contains('>')
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

/// The two zeDB theme configs (Zed-style JSON), embedded.
fn zedb_theme_set() -> Vec<std::rc::Rc<gpui_component::ThemeConfig>> {
    serde_json::from_str::<gpui_component::ThemeSet>(include_str!(
        "../assets/themes/zedb-themes.json"
    ))
    .expect("bundled theme JSON parses")
    .themes
    .into_iter()
    .map(std::rc::Rc::new)
    .collect()
}

/// The theme mode a preference string resolves to right now.
pub(crate) fn resolve_theme_mode(preference: Option<&str>, cx: &App) -> gpui_component::ThemeMode {
    match preference.unwrap_or("dark") {
        "light" => gpui_component::ThemeMode::Light,
        "system" => match cx.window_appearance() {
            gpui::WindowAppearance::Dark | gpui::WindowAppearance::VibrantDark => {
                gpui_component::ThemeMode::Dark
            }
            _ => gpui_component::ThemeMode::Light,
        },
        _ => gpui_component::ThemeMode::Dark,
    }
}

/// Install the zeDB themes and apply the preferred mode. Runs at
/// startup and again on every switch (mode: the fresh preference).
pub(crate) fn apply_theme_preference(
    preference: Option<&str>,
    window: Option<&mut Window>,
    cx: &mut App,
) {
    let themes = zedb_theme_set();
    let mode = resolve_theme_mode(preference, cx);
    {
        let theme = Theme::global_mut(cx);
        for config in &themes {
            match config.mode {
                gpui_component::ThemeMode::Dark => theme.dark_theme = config.clone(),
                gpui_component::ThemeMode::Light => theme.light_theme = config.clone(),
            }
        }
    }
    Theme::change(mode, window, cx);
    // Editor colors the JSON schema does not carry.
    let theme = Theme::global_mut(cx);
    if mode.is_dark() {
        theme.highlight_theme = HighlightTheme::default_dark();
        let highlight = std::sync::Arc::make_mut(&mut theme.highlight_theme);
        highlight.style.editor_background = Some(rgb(0x1e2227).into());
        highlight.style.editor_foreground = Some(rgb(0xaab2bd).into());
        highlight.style.editor_active_line = Some(rgb(0x23282f).into());
        highlight.style.editor_line_number = Some(rgb(0x6b7380).into());
        highlight.style.editor_active_line_number = Some(rgb(0xaab2bd).into());
        let theme = Theme::global_mut(cx);
        theme.colors.caret = rgb(0x9bb8d1).into();
        theme.colors.selection = rgb(0x3b5063).into();
    } else {
        theme.highlight_theme = HighlightTheme::default_light();
    }
    theme::apply(cx);
}

fn quit_ze_db(_: &QuitZeDb, cx: &mut App) {
    cx.quit();
}

/// Hidden server mode: the agent pane spawns this same executable as
/// its MCP server (the bundle ships no separate CLI). Config arrives
/// in the environment (ZEDB_MCP_*), which survives the agent runtime
/// respawning the server; a config-file argument remains supported
/// for older registrations (0600, deleted on read).
fn run_mcp_serve(config_path: &str) -> ! {
    let outcome = (|| -> Result<(), String> {
        let config: serde_json::Value = if config_path.is_empty() {
            let mut map = serde_json::Map::new();
            for (key, name) in [
                ("repo", "ZEDB_MCP_REPO"),
                ("url", "ZEDB_MCP_URL"),
                ("user", "ZEDB_MCP_USER"),
                ("password", "ZEDB_MCP_PASSWORD"),
                ("app_socket", "ZEDB_MCP_APP_SOCKET"),
                ("schema_cache", "ZEDB_MCP_SCHEMA_CACHE"),
            ] {
                if let Ok(value) = std::env::var(name) {
                    map.insert(key.into(), value.into());
                }
            }
            serde_json::Value::Object(map)
        } else {
            let raw = std::fs::read_to_string(config_path).map_err(|error| error.to_string())?;
            let _ = std::fs::remove_file(config_path);
            serde_json::from_str(&raw).map_err(|error| error.to_string())?
        };
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
                driver: Default::default(),
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
        let theme_preference = load_preferences()
            .map(|preferences| preferences.theme)
            .unwrap_or_default();
        apply_theme_preference(theme_preference.as_deref(), None, cx);
        text_input::init(cx);
        cx.on_action(quit_ze_db);
        cx.bind_keys([
            KeyBinding::new("cmd-enter", RunQuery, None),
            KeyBinding::new("ctrl-x", RunSelection, None),
            KeyBinding::new("cmd-s", SaveQueryTab, None),
            KeyBinding::new("cmd-,", OpenPreferences, None),
            // In multi-line inputs the composer sends on plain enter;
            // shift-enter keeps inserting a newline via the secondary
            // Enter action (which the composer ignores).
            KeyBinding::new(
                "shift-enter",
                gpui_component::input::Enter { secondary: true },
                Some("Input"),
            ),
        ]);
        cx.set_menus(vec![
            Menu {
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
            },
            Menu {
                name: "View".into(),
                items: vec![
                    MenuItem::action("Command Palette…", OpenCommandPalette),
                    MenuItem::separator(),
                    MenuItem::action("Open settings.json", OpenSettingsFile),
                ],
            },
        ]);
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
                let palette_workspace = workspace.clone();
                let palette_window = window.window_handle();
                cx.on_action(move |_: &OpenCommandPalette, cx| {
                    let workspace = palette_workspace.clone();
                    palette_window
                        .update(cx, |_, window, cx| {
                            workspace.update(cx, |workspace, cx| {
                                workspace.palette_toggle(window, cx);
                            });
                        })
                        .ok();
                });
                let settings_file_workspace = workspace.clone();
                cx.on_action(move |_: &OpenSettingsFile, cx| {
                    settings_file_workspace.update(cx, |workspace, cx| {
                        workspace.open_settings_file(cx);
                    });
                });
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
    use super::format_engine_definition;

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
    fn engine_definition_splits_primary_key_and_sample_by() {
        let formatted = format_engine_definition(
            "MergeTree PARTITION BY toYYYYMM(created_at) PRIMARY KEY (tenant, id) ORDER BY (tenant, id, created_at) SAMPLE BY id TTL created_at + INTERVAL 90 DAY",
        );

        assert_eq!(
            formatted,
            "ENGINE = MergeTree\nPARTITION BY toYYYYMM(created_at)\nPRIMARY KEY (tenant, id)\nORDER BY (tenant, id, created_at)\nSAMPLE BY id\nTTL created_at + INTERVAL 90 DAY"
        );
    }
}
