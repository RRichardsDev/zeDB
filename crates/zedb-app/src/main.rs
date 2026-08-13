mod agent_pane;
mod author;
mod codegen;
mod command_palette;
mod commit;
mod components;
mod explain_ui;
mod export;
mod features;
mod fleet;
mod github;
mod grid_spike;
mod ops;
mod query_advisor;
mod query_history;
mod rt;
mod schema_intelligence_ui;
mod settings_sync;
mod storage_advisor;
mod tail;
mod theme;
mod type_highlight;
mod updates;
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
    statement_at_cursor, tab_display_name, MaxRows, QueryOutcome, QueryResizeTarget, QueryState,
    QueryTab, RunEvent, TailBatch, TailPush, TailState, TailStream, TailStreamBatch, TailStripInfo,
    TailWatch,
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
        " theme::primary() KEY ",
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

/// Start a live tail of a table (Phase 10), from the schema sidebar's
/// table context menu. `cap` is the retained-row limit the user opted into
/// (`None` = unlimited); the initial load is always small regardless.
#[derive(Clone, PartialEq, Action)]
#[action(no_json, no_register)]
struct TailTable {
    database: String,
    object: String,
    cap: Option<usize>,
}

/// Optional GitHub identity (docs/PHASE-3.4.md M0).
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

    fn title_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .h(px(36.))
            .flex_none()
            .w_full()
            .bg(theme::bg_sidebar())
            .border_b_1()
            .border_color(theme::border())
            .flex()
            .items_center()
            .pl(px(90.))
            .pr_3()
            .text_sm()
            .text_color(theme::text())
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
                        .border_color(theme::text_dim())
                        .text_xs()
                        .text_color(theme::text_dim())
                        .flex()
                        .items_center()
                        .gap_1()
                        .when(phase != UpdatePhase::Installing, |pill| {
                            pill.hover(|pill| {
                                pill.bg(theme::bg())
                                    .text_color(theme::text())
                                    .cursor_pointer()
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
                            pill.child(div().text_color(theme::text()).child(version))
                        }),
                )
            })
            .when_some(
                match &self.github {
                    GithubAuth::SignedIn(profile) => Some(profile.clone()),
                    _ => None,
                },
                |toolbar, profile| {
                    let label = format!("@{} on GitHub", profile.login);
                    toolbar.child(
                        div()
                            .id("toolbar-profile")
                            .ml_2()
                            .mt(px(3.))
                            .size(px(28.))
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .map(|button| match profile.avatar.clone() {
                                Some(avatar) => button.child(
                                    gpui::img(gpui::ImageSource::Resource(gpui::Resource::Path(
                                        avatar.into(),
                                    )))
                                    .size(px(24.))
                                    .rounded_full(),
                                ),
                                None => button.child(
                                    svg()
                                        .path("icons/github.svg")
                                        .size(px(18.))
                                        .text_color(theme::text_dim()),
                                ),
                            })
                            .tooltip(move |window, cx| {
                                gpui_component::tooltip::Tooltip::new(label.clone())
                                    .build(window, cx)
                            })
                            .on_click(cx.listener(|this, _, _, cx| this.open_preferences(cx))),
                    )
                },
            )
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
                .query
                .tabs
                .iter()
                .map(|tab| zedb_core::SavedQueryTab {
                    id: tab.persistent_id.clone(),
                    saved_tab_id: tab.saved_tab_id.clone(),
                    name: tab_display_name(tab),
                    sql: tab.editor.read(cx).value().to_string(),
                })
                .collect(),
            active_tab: self.query.active_tab,
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
        self.connection.form = None;
        self.show_preferences = true;
        self.settings_sync_probe_existing(cx);
        cx.notify();
    }

    fn close_preferences(&mut self, cx: &mut Context<Self>) {
        self.show_preferences = false;
        self.settings_sync_tick(cx);
        cx.notify();
    }

    fn toggle_fleet(&mut self, cx: &mut Context<Self>) {
        self.show_fleet = !self.show_fleet;
        if self.show_fleet {
            self.show_query_editor = false;
            self.show_ops = false;
            if self.fleet.repo.is_none() && !self.fleet.repo_path.read(cx).text().trim().is_empty()
            {
                self.fleet_open_repo(cx);
            } else if self.fleet.rows.is_empty() {
                self.fleet_refresh(cx);
            }
        }
        cx.notify();
    }

    pub(crate) fn set_theme_preference(
        &mut self,
        preference: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.preferences.theme = Some(preference.to_string());
        if let Err(error) = save_preferences(&self.preferences) {
            self.notice = Some(format!("Could not save preferences: {error}"));
        }
        apply_theme_preference(Some(preference), Some(window), cx);
        self.settings_sync_tick(cx);
        cx.notify();
    }

    /// System-mode follow-up: re-resolve on window refocus so a macOS
    /// appearance change is picked up promptly.
    fn theme_recheck(&mut self, cx: &mut Context<Self>) {
        if self.preferences.theme.as_deref() == Some("system") {
            let mode = resolve_theme_mode(Some("system"), cx);
            if mode.is_dark() != Theme::global(cx).is_dark() {
                apply_theme_preference(Some("system"), None, cx);
                cx.notify();
            }
        }
    }

    fn toggle_vim_mode(&mut self, cx: &mut Context<Self>) {
        self.preferences.vim_mode = !self.preferences.vim_mode;
        if self.preferences.vim_mode {
            for tab in &mut self.query.tabs {
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

    fn toggle_experimental_streaming_queries(&mut self, cx: &mut Context<Self>) {
        self.preferences.experimental_streaming_queries =
            !self.preferences.experimental_streaming_queries;
        if let Err(error) = save_preferences(&self.preferences) {
            self.notice = Some(format!("Could not save preferences: {error}"));
            self.notice_warning = true;
        } else {
            self.settings_sync_tick(cx);
        }
        cx.notify();
    }

    /// Start the GitHub device flow: browser opens, code shows in the
    /// panel, and we poll until approved.
    fn github_sign_in(&mut self, provider: github::Provider, cx: &mut Context<Self>) {
        if !provider.configured() {
            self.flash_warning(
                format!(
                    "{} sign-in isn't wired up in this build yet",
                    provider.name()
                ),
                cx,
            );
            return;
        }
        self.github_generation += 1;
        let generation = self.github_generation;
        let handle = rt::tokio().spawn(github::start_device_flow(provider));
        cx.spawn(async move |this, cx| {
            let device = match handle.await {
                Ok(Ok(device)) => device,
                Ok(Err(error)) => {
                    this.update(cx, |this, cx| this.flash_warning(error, cx))
                        .ok();
                    return;
                }
                Err(_) => return,
            };
            let poll_device = device.clone();
            let stale = this
                .update(cx, |this, cx| {
                    if this.github_generation != generation {
                        return true;
                    }
                    this.github = GithubAuth::Authorizing {
                        user_code: device.user_code.clone(),
                        verification_uri: device.verification_uri.clone(),
                    };
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                        device.user_code.clone(),
                    ));
                    cx.open_url(&device.verification_uri);
                    cx.notify();
                    false
                })
                .unwrap_or(true);
            if stale {
                return;
            }
            let token = rt::tokio()
                .spawn(async move { github::poll_for_token(provider, &poll_device).await })
                .await;
            let token = match token {
                Ok(Ok(token)) => token,
                Ok(Err(error)) => {
                    this.update(cx, |this, cx| {
                        if this.github_generation == generation {
                            this.github = GithubAuth::SignedOut;
                            this.flash_warning(error, cx);
                        }
                    })
                    .ok();
                    return;
                }
                Err(_) => return,
            };
            if let Err(error) = github::store_token(provider, &token) {
                this.update(cx, |this, cx| {
                    this.flash_warning(format!("Could not store the token: {error}"), cx)
                })
                .ok();
            }
            let profile = rt::tokio()
                .spawn(async move { github::fetch_profile(provider, &token).await })
                .await;
            this.update(cx, |this, cx| {
                if this.github_generation != generation {
                    return;
                }
                match profile {
                    Ok(Ok(profile)) => {
                        this.notice = Some(format!(
                            "Signed in to {} as {}",
                            provider.name(),
                            profile.login
                        ));
                        this.notice_warning = false;
                        this.github = GithubAuth::SignedIn(profile);
                        this.settings_sync_identity_changed(cx);
                    }
                    Ok(Err(error)) => {
                        this.github = GithubAuth::SignedOut;
                        this.flash_warning(error, cx);
                    }
                    Err(_) => this.github = GithubAuth::SignedOut,
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn github_sign_out(&mut self, cx: &mut Context<Self>) {
        let provider = match &self.github {
            GithubAuth::SignedIn(profile) => profile.provider,
            _ => github::Provider::GitHub,
        };
        github::clear_token(provider);
        self.github_generation += 1;
        self.github = GithubAuth::SignedOut;
        self.settings_sync_identity_changed(cx);
        self.notice = Some(format!(
            "Signed out of {} on this Mac; revoke zeDB under the {} application \
             settings if you want the grant gone too",
            provider.name(),
            provider.name(),
        ));
        self.notice_warning = false;
        cx.notify();
    }

    /// GitHub-style per-character boxes for a device code; clicking
    /// re-copies the code to the clipboard.
    pub(crate) fn device_code_boxes(
        &self,
        user_code: String,
        id: &'static str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .id(id)
            .flex()
            .items_center()
            .justify_center()
            .gap_1()
            .cursor_pointer()
            .hover(|boxes| boxes.opacity(0.85))
            .on_click({
                let code = user_code.clone();
                cx.listener(move |this, _, _, cx| {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(code.clone()));
                    this.notice = Some("Code copied to the clipboard".into());
                    this.notice_warning = false;
                    cx.notify();
                })
            })
            .children(user_code.chars().map(|character| {
                if character == '-' {
                    div()
                        .px_1()
                        .text_color(theme::text_dim())
                        .child("-")
                        .into_any_element()
                } else {
                    div()
                        .w(px(38.))
                        .h(px(46.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(6.))
                        .border_1()
                        .border_color(theme::border())
                        .bg(theme::bg_sidebar())
                        .text_xl()
                        .font_family("Menlo")
                        .text_color(theme::text())
                        .child(String::from(character))
                        .into_any_element()
                }
            }))
    }

    /// The Account section of the preferences panel.
    fn account_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let row = div().py_3().border_b_1().border_color(theme::border());
        match &self.github {
            GithubAuth::SignedOut => {
                row.flex()
                    .flex_col()
                    .gap_1()
                    .child("Account")
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme::text_dim())
                            .child("Optional: shows your profile and enables settings sync later"),
                    )
                    .child(div().flex().items_center().gap_2().mt_2().children(
                        github::Provider::ALL.into_iter().map(|provider| {
                            div()
                                .id(provider.name())
                                .px_3()
                                .py_1()
                                .rounded(px(3.))
                                .border_1()
                                .border_color(theme::border())
                                .flex()
                                .items_center()
                                .gap_2()
                                .text_color(if provider.configured() {
                                    theme::text()
                                } else {
                                    theme::text_dim()
                                })
                                .hover(|button| button.bg(theme::bg_sidebar()).cursor_pointer())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.github_sign_in(provider, cx)
                                }))
                                .child(
                                    svg()
                                        .path(provider.icon())
                                        .size(px(16.))
                                        .text_color(theme::text()),
                                )
                                .child(format!("Sign in with {}", provider.name()))
                        }),
                    ))
            }
            GithubAuth::Authorizing {
                user_code,
                verification_uri,
            } => {
                let code_boxes = self.device_code_boxes(user_code.clone(), "github-code", cx);
                row.flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child("Account")
                            .child(
                                div()
                                    .id("github-cancel")
                                    .px_2()
                                    .py_1()
                                    .rounded(px(3.))
                                    .text_color(theme::text_dim())
                                    .hover(|button| {
                                        button
                                            .bg(theme::bg_sidebar())
                                            .text_color(theme::text())
                                            .cursor_pointer()
                                    })
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.github_generation += 1;
                                        this.github = GithubAuth::SignedOut;
                                        cx.notify();
                                    }))
                                    .child("Cancel"),
                            ),
                    )
                    .child(div().text_sm().text_color(theme::text_dim()).child(format!(
                        "Enter this code at {verification_uri} (opened in your browser). \
                         It's on your clipboard; click the code to copy it again."
                    )))
                    .child(code_boxes)
            }
            GithubAuth::SignedIn(profile) => {
                let display = profile
                    .name
                    .clone()
                    .unwrap_or_else(|| profile.login.clone());
                row.flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .when_some(profile.avatar.clone(), |account, avatar| {
                                account.child(
                                    gpui::img(gpui::ImageSource::Resource(gpui::Resource::Path(
                                        avatar.into(),
                                    )))
                                    .size(px(36.))
                                    .rounded_full(),
                                )
                            })
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_0p5()
                                    .child(div().text_color(theme::text()).child(display))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_1()
                                            .text_sm()
                                            .text_color(theme::text_dim())
                                            .child(
                                                svg()
                                                    .path(profile.provider.icon())
                                                    .size(px(13.))
                                                    .text_color(theme::text_dim()),
                                            )
                                            .child(format!("@{}", profile.login)),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .id("github-sign-out")
                            .px_3()
                            .py_1()
                            .rounded(px(3.))
                            .border_1()
                            .border_color(theme::border())
                            .text_color(theme::text_dim())
                            .hover(|button| {
                                button
                                    .bg(theme::bg_sidebar())
                                    .text_color(theme::text())
                                    .cursor_pointer()
                            })
                            .on_click(cx.listener(|this, _, _, cx| this.github_sign_out(cx)))
                            .child("Sign out"),
                    )
            }
        }
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
                                .border_color(theme::border())
                                .text_color(theme::text_dim())
                                .hover(|button| {
                                    button
                                        .bg(theme::bg_sidebar())
                                        .text_color(theme::text())
                                        .cursor_pointer()
                                })
                                .on_click(cx.listener(|this, _, _, cx| this.close_preferences(cx)))
                                .child("Done"),
                        ),
                )
                .child(self.account_section(cx))
                .child(self.settings_sync_section(cx))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_4()
                        .py_3()
                        .border_b_1()
                        .border_color(theme::border())
                        .child(
                            div().flex().flex_col().gap_1().child("Theme").child(
                                div()
                                    .text_sm()
                                    .text_color(theme::text_dim())
                                    .child("System follows macOS appearance."),
                            ),
                        )
                        .child(div().flex().items_center().gap_2().children(
                            [("dark", "Dark"), ("light", "Light"), ("system", "System")].map(
                                |(value, label)| {
                                    let active =
                                        self.preferences.theme.as_deref().unwrap_or("dark")
                                            == value;
                                    div()
                                        .id(label)
                                        .px_3()
                                        .py_1()
                                        .rounded(px(3.))
                                        .border_1()
                                        .border_color(if active {
                                            theme::accent()
                                        } else {
                                            theme::border()
                                        })
                                        .text_color(if active {
                                            theme::text()
                                        } else {
                                            theme::text_dim()
                                        })
                                        .hover(|button| button.bg(theme::hover()).cursor_pointer())
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.set_theme_preference(value, window, cx)
                                        }))
                                        .child(label)
                                },
                            ),
                        )),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_4()
                        .py_3()
                        .border_b_1()
                        .border_color(theme::border())
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_w_0()
                                .gap_1()
                                .child("Vim mode")
                                .child(
                                    div()
                                        .text_sm()
                                        .whitespace_normal()
                                        .text_color(theme::text_dim())
                                        .child("Use Vim keybindings in query editors."),
                                ),
                        )
                        .child(
                            div()
                                .id("toggle-vim-mode")
                                .w(px(54.))
                                .h(px(28.))
                                .flex_none()
                                .px_1()
                                .rounded_full()
                                .flex()
                                .items_center()
                                .when(self.preferences.vim_mode, |toggle| {
                                    toggle.justify_end().bg(theme::toggle_on())
                                })
                                .when(!self.preferences.vim_mode, |toggle| {
                                    toggle.justify_start().bg(theme::toggle_off())
                                })
                                .hover(|toggle| toggle.cursor_pointer())
                                .on_click(cx.listener(|this, _, _, cx| this.toggle_vim_mode(cx)))
                                .child(div().size(px(20.)).rounded_full().bg(
                                    if self.preferences.vim_mode {
                                        theme::toggle_knob_on()
                                    } else {
                                        theme::toggle_knob_off()
                                    },
                                )),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_4()
                        .py_3()
                        .border_b_1()
                        .border_color(theme::border())
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_w_0()
                                .gap_1()
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .child("Experimental STREAM tails")
                                        .child(
                                            svg()
                                                .path("icons/experimental.svg")
                                                .size(px(14.))
                                                .text_color(theme::warning()),
                                        ),
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .whitespace_normal()
                                        .text_color(theme::text_dim())
                                        .child(
                                            "Prefer ClickHouse 26.6 STREAM CURSOR for compatible instant tails. Falls back to WATCH and polling.",
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .id("toggle-experimental-streaming-queries")
                                .w(px(54.))
                                .h(px(28.))
                                .flex_none()
                                .px_1()
                                .rounded_full()
                                .flex()
                                .items_center()
                                .when(
                                    self.preferences.experimental_streaming_queries,
                                    |toggle| toggle.justify_end().bg(theme::toggle_on()),
                                )
                                .when(
                                    !self.preferences.experimental_streaming_queries,
                                    |toggle| toggle.justify_start().bg(theme::toggle_off()),
                                )
                                .hover(|toggle| toggle.cursor_pointer())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.toggle_experimental_streaming_queries(cx)
                                }))
                                .child(div().size(px(20.)).rounded_full().bg(
                                    if self.preferences.experimental_streaming_queries {
                                        theme::toggle_knob_on()
                                    } else {
                                        theme::toggle_knob_off()
                                    },
                                )),
                        ),
                ),
        )
    }

    fn tier_colors(tier: EnvTier) -> (u32, u32) {
        if theme::is_dark() {
            match tier {
                EnvTier::Dev => (0x28384b, 0x8ab4d4),
                EnvTier::Staging => (0x463b28, 0xc7a969),
                EnvTier::Production => (0x472d31, 0xd4868d),
            }
        } else {
            match tier {
                EnvTier::Dev => (0xd6e6f5, 0x2f5f8a),
                EnvTier::Staging => (0xf3e9d2, 0x8a6d1f),
                EnvTier::Production => (0xf5dcdf, 0xa03744),
            }
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

    /// Half-scale badge for the dense connections list.
    fn tier_badge_small(tier: EnvTier) -> impl IntoElement {
        let (background, foreground) = Self::tier_colors(tier);
        div()
            .px(px(4.5))
            .py(px(1.))
            .rounded(px(2.))
            .border_1()
            .border_color(rgb(foreground))
            .bg(rgb(background))
            .text_color(rgb(foreground))
            .text_size(px(8.))
            .child(tier.label().to_uppercase())
    }

    /// The connection's write posture, worn next to the tier: quiet
    /// when read-only (the safe default), loud when writes are open.
    /// Small posture badge for the dense connections list (its only
    /// current wearer; pass small=false for a full-size one).
    fn write_badge_small(read_only: bool) -> impl IntoElement {
        Self::write_badge_sized(read_only, true)
    }

    fn write_badge_sized(read_only: bool, small: bool) -> impl IntoElement {
        div()
            .map(|badge| {
                if small {
                    badge
                        .px(px(4.5))
                        .py(px(1.))
                        .rounded(px(2.))
                        .text_size(px(8.))
                } else {
                    badge.px_2().py(px(2.)).rounded(px(3.)).text_xs()
                }
            })
            .border_1()
            .map(|badge| {
                if read_only {
                    badge
                        .bg(theme::row_hover())
                        .border_color(theme::text_dim())
                        .text_color(theme::text_dim())
                        .child("READ-ONLY")
                } else {
                    badge
                        .bg(rgb(if theme::is_dark() { 0x4d2c2c } else { 0xf7dfd9 }))
                        .border_color(theme::alert())
                        .text_color(theme::alert())
                        .child("WRITE")
                }
            })
    }

    /// The at-rest environment mark: a small triangle in the tier's
    /// accent color, shown when a connection row is not hovered.
    fn tier_glyph(tier: EnvTier) -> impl IntoElement {
        let (_, foreground) = Self::tier_colors(tier);
        div()
            .text_size(px(9.))
            .text_color(rgb(foreground))
            .child("\u{25B2}")
    }

    /// The at-rest write-posture mark: a small square, dim when
    /// read-only, alert-colored when writes are open.
    fn write_glyph(read_only: bool) -> impl IntoElement {
        let color = if read_only {
            theme::text_dim()
        } else {
            theme::alert()
        };
        div().text_size(px(8.)).text_color(color).child("\u{25A0}")
    }

    fn sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self
            .connection
            .connections
            .iter()
            .enumerate()
            .map(|(index, connection)| {
                let selected = self.connection.selected == Some(index);
                let connected = self
                    .connection
                    .connected
                    .as_ref()
                    .map(|connected| connected.name.as_str())
                    == Some(connection.name.as_str());
                div()
                    .id(("connection", index))
                    .group("connection-row")
                    .w_full()
                    .px_2()
                    .py_2()
                    .rounded(px(3.))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .when(selected, |row| row.bg(theme::hover()))
                    .hover(|row| row.bg(theme::row_hover()).cursor_pointer())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        // A second click on the already-selected row
                        // brings up its Cluster connection screen (the
                        // only route back to it while connected).
                        if this.connection.selected == Some(index)
                            && (this.show_query_editor || this.show_ops)
                        {
                            this.show_query_editor = false;
                            this.show_fleet = false;
                            this.show_ops = false;
                        }
                        this.connection.selected = Some(index);
                        this.connection.pending_delete = None;
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
                            .text_color(theme::text())
                            .child(
                                // Name plus an inline muted node count "(N)"
                                // at rest; hovering hides it and reveals the
                                // full "N nodes" line below.
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1p5()
                                    .child(connection.name.clone())
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(theme::text_dim())
                                            .group_hover("connection-row", |count| {
                                                count.invisible()
                                            })
                                            .child(format!("({})", connection.nodes.len())),
                                    ),
                            )
                            .child(
                                // At rest the row wears only two small
                                // marks: a triangle in the environment
                                // color and a square in the read/write
                                // color. Hovering the row swaps in the
                                // full pills. The pills stay in flow
                                // (invisible) so they reserve the width and
                                // the layout does not shift on hover.
                                div()
                                    .relative()
                                    .flex()
                                    .items_center()
                                    .justify_end()
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_1()
                                            .invisible()
                                            .group_hover("connection-row", |pills| pills.visible())
                                            .when(connected, |pills| {
                                                pills.child(
                                                    div()
                                                        .size(px(7.))
                                                        .rounded_full()
                                                        .bg(theme::success())
                                                        .mr_1(),
                                                )
                                            })
                                            .child(Self::write_badge_small(connection.read_only))
                                            .child(Self::tier_badge_small(connection.tier)),
                                    )
                                    .child(
                                        div()
                                            .absolute()
                                            .right_0()
                                            .flex()
                                            .items_center()
                                            .gap(px(3.))
                                            .group_hover("connection-row", |marks| {
                                                marks.invisible()
                                            })
                                            .when(connected, |marks| {
                                                marks.child(
                                                    div()
                                                        .size(px(7.))
                                                        .rounded_full()
                                                        .bg(theme::success())
                                                        .mr_1(),
                                                )
                                            })
                                            .child(Self::write_glyph(connection.read_only))
                                            .child(Self::tier_glyph(connection.tier)),
                                    ),
                            ),
                    )
                    .child(
                        // The full "N nodes" line is collapsed at rest (the
                        // inline "(N)" stands in) and expands on hover.
                        div()
                            .max_h(px(0.))
                            .overflow_hidden()
                            .text_color(theme::text_dim())
                            .group_hover("connection-row", |line| line.max_h(px(20.)))
                            .child({
                                let count = connection.nodes.len();
                                format!("{count} node{}", if count == 1 { "" } else { "s" })
                            }),
                    )
            })
            .collect::<Vec<_>>();

        div()
            .w(px(self.sidebar_width))
            .flex_none()
            .h_full()
            .bg(theme::bg_sidebar())
            .flex()
            .flex_col()
            .text_sm()
            .text_color(theme::text_dim())
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
                                    .text_color(theme::text())
                                    .child("+")
                                    .hover(|button| button.bg(theme::hover()).cursor_pointer())
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
                                        .text_color(theme::text_dim())
                                        .child("No saved connections"),
                                )
                            })
                            .children(rows),
                    )
                    .when(self.connection.selected.is_some(), |sidebar| {
                        sidebar.child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .when_some(self.connection.pending_delete.as_ref(), |panel, name| {
                                    panel
                                        .child(div().text_xs().text_color(theme::danger()).child(
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
                                                        .text_color(theme::text_dim())
                                                        .child("Cancel")
                                                        .hover(|button| {
                                                            button
                                                                .bg(theme::hover())
                                                                .text_color(theme::text())
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
                                                                .text_color(theme::text_bright())
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
                                .when(self.connection.pending_delete.is_none(), |panel| {
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
                                            .border_color(theme::border())
                                            .child(
                                                div()
                                                    .id("duplicate-connection")
                                                    .size(px(24.))
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .rounded(px(3.))
                                                    .text_color(theme::text_dim())
                                                    .child(
                                                        svg()
                                                            .path("icons/copy.svg")
                                                            .size(px(14.))
                                                            .text_color(theme::text_dim()),
                                                    )
                                                    .hover(|button| {
                                                        button
                                                            .bg(theme::hover())
                                                            .text_color(theme::text())
                                                            .cursor_pointer()
                                                    })
                                                    .tooltip(|window, cx| {
                                                        gpui_component::tooltip::Tooltip::new(
                                                            "Duplicate connection",
                                                        )
                                                        .build(window, cx)
                                                    })
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        if let Some(index) = this.connection.selected {
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
                                                    .text_color(theme::text_dim())
                                                    .child(
                                                        svg()
                                                            .path("icons/edit.svg")
                                                            .size(px(14.))
                                                            .text_color(theme::text_dim()),
                                                    )
                                                    .hover(|button| {
                                                        button
                                                            .bg(theme::hover())
                                                            .text_color(theme::text())
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
                                                    .text_color(theme::text_dim())
                                                    .child(
                                                        svg()
                                                            .path("icons/trash.svg")
                                                            .size(px(14.))
                                                            .text_color(theme::text_dim()),
                                                    )
                                                    .when(self.connection.connecting.is_none(), |button| {
                                                        button
                                                            .hover(|button| {
                                                                button
                                                                    .bg(theme::danger_hover())
                                                                    .text_color(theme::danger())
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

    fn schema_kind_label(kind: SchemaObjectKind, engine: &str) -> &'static str {
        match kind {
            // A Distributed table holds no data of its own; it scatters
            // over the cluster's shard-local tables.
            SchemaObjectKind::Table if engine == "Distributed" => "DT",
            SchemaObjectKind::Table => "T",
            SchemaObjectKind::View => "V",
            SchemaObjectKind::MaterializedView => "MV",
            SchemaObjectKind::Dictionary => "D",
        }
    }

    fn schema_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let filter = self.schema.filter.read(cx).text().to_lowercase();
        let cache_status = self.schema.cache.as_ref().map(|cache| {
            let snapshot = cache.snapshot();
            format!(
                "{} of {} databases ready",
                snapshot.warmed_databases(),
                snapshot.databases.len()
            )
        });
        let selected = self
            .schema
            .selected_object
            .as_ref()
            .map(|selected| (selected.database.as_str(), selected.object.name.as_str()));
        let database_rows = self
            .schema
            .databases
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
                        let size_id = database_index.saturating_mul(100_000) + object_index;
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
                            .when(is_selected, |row| row.bg(theme::hover()))
                            .hover(|row| row.bg(theme::row_hover()).cursor_pointer())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                // Keep whatever inspector tab is open, so
                                // moving between tables stays in context.
                                let tab = this
                                    .schema
                                    .selected_object
                                    .as_ref()
                                    .map(|selected| selected.tab)
                                    .unwrap_or(ObjectInspectorTab::Overview);
                                this.select_schema_object(
                                    row_database.clone(),
                                    row_object.clone(),
                                    tab,
                                    window,
                                    cx,
                                )
                            }))
                            .context_menu({
                                let database = database_name.clone();
                                let engine = object.engine.clone();
                                let object = object.name.clone();
                                move |menu, window, cx| {
                                    let menu = menu.menu(
                                        "View DDL",
                                        Box::new(ViewObjectDdl {
                                            database: database.clone(),
                                            object: object.clone(),
                                        }),
                                    );
                                    // Tail is a MergeTree-family thing (a
                                    // monotonic key to advance on). The
                                    // submenu is the retained-row cap the
                                    // user opts into; the initial load is
                                    // always small either way.
                                    if engine.contains("MergeTree") {
                                        let database = database.clone();
                                        let object = object.clone();
                                        menu.submenu("Tail", window, cx, move |menu, _, _| {
                                            let caps: [(&str, Option<usize>); 6] = [
                                                ("20 rows", Some(20)),
                                                ("50 rows", Some(50)),
                                                ("100 rows", Some(100)),
                                                ("500 rows", Some(500)),
                                                ("1000 rows", Some(1000)),
                                                ("Unlimited", None),
                                            ];
                                            caps.into_iter().fold(menu, |menu, (label, cap)| {
                                                menu.menu(
                                                    label,
                                                    Box::new(TailTable {
                                                        database: database.clone(),
                                                        object: object.clone(),
                                                        cap,
                                                    }),
                                                )
                                            })
                                        })
                                    } else {
                                        menu
                                    }
                                }
                            })
                            .child(
                                div()
                                    .w(px(20.))
                                    .text_xs()
                                    .text_color(theme::text_dim())
                                    .child(Self::schema_kind_label(object.kind, &object.engine)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_color(theme::text())
                                    .child(object.name),
                            )
                            .when_some(object.total_bytes, |row, bytes| {
                                // Parentheses mark a derived number: a
                                // Distributed table's size is its local
                                // table summed across shards.
                                let distributed = object.engine == "Distributed";
                                let text = if distributed {
                                    format!("({})", Self::format_bytes(bytes))
                                } else {
                                    Self::format_bytes(bytes)
                                };
                                let size = div()
                                    .flex_none()
                                    .text_size(px(9.))
                                    .text_color(theme::text_dim())
                                    .child(text);
                                row.child(if distributed {
                                    size.id(("schema-object-size", size_id))
                                        .tooltip(|window, cx| {
                                            gpui_component::tooltip::Tooltip::new(
                                                "Virtual: the local table summed across shards",
                                            )
                                            .build(window, cx)
                                        })
                                        .into_any_element()
                                } else {
                                    size.into_any_element()
                                })
                            })
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
                                .hover(|row| row.bg(theme::row_hover()).cursor_pointer())
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
                                        .text_color(theme::text())
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
                                    .text_color(theme::danger())
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
                    .when(self.connection.connected.is_some(), |header| {
                        header.child(
                            div()
                                .id("refresh-schema")
                                .size(px(24.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(3.))
                                .text_color(theme::text_dim())
                                .child(
                                    svg()
                                        .path("icons/refresh.svg")
                                        .size(px(14.))
                                        .text_color(theme::text_dim()),
                                )
                                .hover(|button| {
                                    button
                                        .bg(theme::hover())
                                        .text_color(theme::text())
                                        .cursor_pointer()
                                })
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.load_schema_databases(cx)),
                                ),
                        )
                    }),
            )
            .when(self.connection.connected.is_some(), |panel| {
                panel.child(div().px_2().pb_2().child(self.schema.filter.clone()))
            })
            .when_some(cache_status, |panel, status| {
                panel.child(
                    div()
                        .px_3()
                        .pb_1()
                        .text_xs()
                        .text_color(theme::text_dim())
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
                    .when(self.connection.connected.is_none(), |tree| {
                        tree.child(
                            div()
                                .px_2()
                                .py_2()
                                .text_xs()
                                .child("Connect to browse schema"),
                        )
                    })
                    .when(self.schema.loading, |tree| {
                        tree.child(div().px_2().py_2().text_xs().child("Loading databases..."))
                    })
                    .when_some(self.schema.error.as_ref(), |tree, error| {
                        tree.child(
                            div()
                                .px_2()
                                .py_2()
                                .text_xs()
                                .text_color(theme::danger())
                                .child(error.clone()),
                        )
                    })
                    .children(database_rows),
            )
    }

    fn field(label: &'static str, input: Entity<TextInput>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(div().text_xs().text_color(theme::text_dim()).child(label))
            .child(input)
    }

    fn form_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let form = self
            .connection
            .form
            .as_ref()
            .expect("form panel requires a form");
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
                                .border_color(theme::border())
                                .child("-")
                                .hover(|button| button.bg(theme::bg_sidebar()).cursor_pointer())
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
            .bg(theme::bg())
            .p_6()
            // Centering lives on a non-scroll wrapper: a flex scroll
            // container stretches its child to the viewport height and
            // clips the overflow before scrolling ever sees it.
            .child(
                div().flex().justify_center().w_full().child(
                    div()
                        .w(px(520.))
                        .flex()
                        .flex_col()
                        .gap_4()
                        .child(div().text_lg().text_color(theme::text()).child(heading))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(div().text_xs().text_color(theme::text_dim()).child("NAME"))
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
                                                    button.bg(theme::bg_sidebar()).cursor_pointer()
                                                })
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.cycle_tier(cx)
                                                })),
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
                                                .text_color(theme::text_dim())
                                                .child("CLUSTER NODES"),
                                        )
                                        .child(
                                            div()
                                                .id("add-endpoint")
                                                .px_2()
                                                .py_1()
                                                .rounded(px(3.))
                                                .border_1()
                                                .border_color(theme::border())
                                                .child("+ Add node")
                                                .hover(|button| {
                                                    button.bg(theme::bg_sidebar()).cursor_pointer()
                                                })
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.add_endpoint(cx)
                                                })),
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
                                                .text_color(theme::text_dim())
                                                .child("DRIVER"),
                                        )
                                        .child(
                                            div()
                                                .id("add-driver-setting")
                                                .px_2()
                                                .py_0p5()
                                                .rounded(px(3.))
                                                .text_xs()
                                                .text_color(theme::text_dim())
                                                .hover(|button| {
                                                    button
                                                        .bg(theme::bg_sidebar())
                                                        .text_color(theme::text())
                                                        .cursor_pointer()
                                                })
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.add_driver_setting(cx)
                                                }))
                                                .child("+ Add setting"),
                                        ),
                                )
                                .children(form.driver_settings.iter().enumerate().map(
                                    |(index, setting)| {
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .child(
                                                div()
                                                    .w(px(220.))
                                                    .flex_none()
                                                    .child(setting.name.clone()),
                                            )
                                            .child(div().flex_1().child(setting.value.clone()))
                                            .child(
                                                div()
                                                    .id(("remove-driver-setting", index))
                                                    .w(px(30.))
                                                    .h(px(30.))
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .rounded(px(3.))
                                                    .border_1()
                                                    .border_color(theme::border())
                                                    .child("-")
                                                    .hover(|button| {
                                                        button
                                                            .bg(theme::bg_sidebar())
                                                            .cursor_pointer()
                                                    })
                                                    .on_click(cx.listener(
                                                        move |this, _, _, cx| {
                                                            this.remove_driver_setting(index, cx)
                                                        },
                                                    )),
                                            )
                                    },
                                ))
                                .when(!form.driver_settings.is_empty(), |section| {
                                    section.child(
                                        div().text_xs().text_color(theme::text_dim()).child(
                                            "Sent with every query on this cluster; \
                                         connect_timeout configures the driver instead. \
                                         Rows without a value are dropped on save.",
                                        ),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .items_center()
                                .gap_3()
                                .child("Read only")
                                .child(
                                    // Same switch as the Vim mode toggle.
                                    div()
                                        .id("toggle-read-only")
                                        .w(px(54.))
                                        .h(px(28.))
                                        .px_1()
                                        .rounded_full()
                                        .flex()
                                        .items_center()
                                        .when(form.read_only, |toggle| {
                                            toggle.justify_end().bg(theme::toggle_on())
                                        })
                                        .when(!form.read_only, |toggle| {
                                            toggle.justify_start().bg(theme::toggle_off())
                                        })
                                        .hover(|toggle| toggle.cursor_pointer())
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.toggle_read_only(cx)),
                                        )
                                        .child(div().size(px(20.)).rounded_full().bg(
                                            if form.read_only {
                                                theme::toggle_knob_on()
                                            } else {
                                                theme::toggle_knob_off()
                                            },
                                        )),
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
                                        .border_color(theme::border())
                                        .child("Cancel")
                                        .when(self.connection.connecting.is_none(), |button| {
                                            button
                                                .hover(|button| {
                                                    button.bg(theme::bg_sidebar()).cursor_pointer()
                                                })
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.cancel_form(cx)
                                                }))
                                        }),
                                )
                                .child(
                                    div()
                                        .id("save-offline")
                                        .px_4()
                                        .py_2()
                                        .rounded(px(3.))
                                        .border_1()
                                        .border_color(theme::border())
                                        .child("Save without testing")
                                        .when(self.connection.connecting.is_none(), |button| {
                                            button
                                                .hover(|button| {
                                                    button.bg(theme::bg_sidebar()).cursor_pointer()
                                                })
                                                .on_click(
                                                    cx.listener(|this, _, _, cx| {
                                                        this.save_form(cx)
                                                    }),
                                                )
                                        }),
                                )
                                .child(
                                    div()
                                        .id("save-and-connect")
                                        .px_4()
                                        .py_2()
                                        .rounded(px(3.))
                                        .bg(theme::primary())
                                        .text_color(theme::primary_foreground())
                                        .child(if self.connection.connecting.is_some() {
                                            "Testing nodes..."
                                        } else {
                                            "Save & Connect"
                                        })
                                        .when(self.connection.connecting.is_none(), |button| {
                                            button
                                                .hover(|button| {
                                                    button
                                                        .bg(theme::primary_hover())
                                                        .cursor_pointer()
                                                })
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.save_and_connect(cx)
                                                }))
                                        }),
                                ),
                        ),
                ),
            )
    }

    fn node_selector(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
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
        // Clusters the connected node belongs to. Picking one runs
        // schema-apply actions ON CLUSTER instead of just this node.
        let clusters = self.ops_cluster_options();
        let apply_cluster = connected.apply_cluster.clone();
        // In cluster scope the label reads the cluster, not the node.
        let label = match &apply_cluster {
            Some(name) => format!("Cluster: {name}"),
            None => active_name,
        };
        let action_context = self.query.tabs[self.query.active_tab]
            .editor
            .focus_handle(cx);

        Some(
            Button::new("active-node-selector")
                .label(label)
                .dropdown_caret(true)
                .compact()
                .outline()
                .dropdown_menu(move |menu: PopupMenu, _, _| {
                    let mut menu = nodes.iter().cloned().fold(
                        menu.action_context(action_context.clone()).min_w(px(180.)),
                        |menu, (index, name, reachable)| {
                            menu.menu_with_enable(name, Box::new(SelectNode { index }), reachable)
                        },
                    );
                    if !clusters.is_empty() {
                        menu = menu.separator();
                        for cluster in &clusters {
                            menu = menu.menu(
                                format!("Cluster: {cluster}"),
                                Box::new(SetApplyCluster {
                                    cluster: Some(cluster.clone()),
                                }),
                            );
                        }
                    }
                    menu
                }),
        )
    }

    fn connection_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self
            .connection
            .selected
            .and_then(|index| self.connection.connections.get(index));
        let header_connection = self
            .connection
            .connected
            .as_ref()
            .and_then(|connected| {
                self.connection
                    .connections
                    .iter()
                    .find(|connection| connection.name == connected.name)
            })
            .or(selected);
        let selected_connected = selected
            .map(|connection| {
                self.connection
                    .connected
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
            .bg(theme::bg_sidebar())
            .border_b_1()
            .border_color(theme::border())
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
                                if self.connection.connected.is_none() {
                                    // Disabled: the fleet view is per-connection.
                                    button
                                        .border_color(theme::disabled_border())
                                        .child(
                                            svg()
                                                .path("icons/fleet.svg")
                                                .size(px(14.))
                                                .text_color(theme::disabled()),
                                        )
                                        .tooltip(|window, cx| {
                                            gpui_component::tooltip::Tooltip::new(
                                                "Connect to a cluster first",
                                            )
                                            .build(window, cx)
                                        })
                                } else {
                                    button
                                        .border_color(theme::border())
                                        .when(self.show_fleet, |button| {
                                            button.bg(theme::selected())
                                        })
                                        .child(
                                            svg()
                                                .path("icons/fleet.svg")
                                                .size(px(14.))
                                                .text_color(if self.show_fleet {
                                                    theme::text()
                                                } else {
                                                    theme::text_dim()
                                                })
                                                .group_hover("btn-fleet", |icon| {
                                                    icon.text_color(theme::text())
                                                }),
                                        )
                                        .hover(|button| button.bg(theme::hover()).cursor_pointer())
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
                            .id("open-ops")
                            .group("btn-ops")
                            .size(px(28.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(3.))
                            .border_1()
                            .map(|button| {
                                if self.connection.connected.is_none() {
                                    button
                                        .border_color(theme::disabled_border())
                                        .child(
                                            svg()
                                                .path("icons/ops.svg")
                                                .size(px(14.))
                                                .text_color(theme::disabled()),
                                        )
                                        .tooltip(|window, cx| {
                                            gpui_component::tooltip::Tooltip::new(
                                                "Connect to a cluster first",
                                            )
                                            .build(window, cx)
                                        })
                                } else {
                                    button
                                        .border_color(theme::border())
                                        .when(self.show_ops, |button| button.bg(theme::selected()))
                                        .child(
                                            svg()
                                                .path("icons/ops.svg")
                                                .size(px(14.))
                                                .text_color(if self.show_ops {
                                                    theme::text()
                                                } else {
                                                    theme::text_dim()
                                                })
                                                .group_hover("btn-ops", |icon| {
                                                    icon.text_color(theme::text())
                                                }),
                                        )
                                        .hover(|button| button.bg(theme::hover()).cursor_pointer())
                                        .tooltip(|window, cx| {
                                            gpui_component::tooltip::Tooltip::new(
                                                "Ops: what the cluster is doing right now",
                                            )
                                            .build(window, cx)
                                        })
                                        .on_click(cx.listener(|this, _, _, cx| this.ops_toggle(cx)))
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
                                if self.connection.connected.is_none() {
                                    // Disabled; running from an existing tab
                                    // still gets the connect-first warning.
                                    button
                                        .border_color(theme::disabled_border())
                                        .child(
                                            svg()
                                                .path("icons/query-plus.svg")
                                                .size(px(14.))
                                                .text_color(theme::disabled()),
                                        )
                                        .tooltip(|window, cx| {
                                            gpui_component::tooltip::Tooltip::new(
                                                "Connect to a cluster first",
                                            )
                                            .build(window, cx)
                                        })
                                } else {
                                    button
                                        .border_color(theme::border())
                                        .when(!self.show_fleet, |button| {
                                            button.bg(theme::selected())
                                        })
                                        .child(
                                            svg()
                                                .path("icons/query-plus.svg")
                                                .size(px(14.))
                                                .text_color(if self.show_fleet {
                                                    theme::text_dim()
                                                } else {
                                                    theme::text()
                                                })
                                                .group_hover("btn-query", |icon| {
                                                    icon.text_color(theme::text())
                                                }),
                                        )
                                        .hover(|button| button.bg(theme::hover()).cursor_pointer())
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
                            .border_color(theme::border())
                            .when(self.agent.open, |button| button.bg(theme::selected()))
                            .child(
                                svg()
                                    .path("icons/sparkle.svg")
                                    .size(px(14.))
                                    .text_color(if self.agent.open {
                                        theme::text()
                                    } else {
                                        theme::text_dim()
                                    })
                                    .group_hover("btn-agent", |icon| {
                                        icon.text_color(theme::text())
                                    }),
                            )
                            .hover(|button| button.bg(theme::hover()).cursor_pointer())
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
                                .border_color(theme::danger())
                                .child(
                                    svg()
                                        .path("icons/plug-off.svg")
                                        .size(px(14.))
                                        .text_color(theme::danger()),
                                )
                                .hover(|button| button.bg(theme::danger_hover()).cursor_pointer())
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
                                    if self.connection.connecting.is_some() {
                                        button
                                            .border_color(theme::border())
                                            .child(
                                                svg()
                                                    .path("icons/plug.svg")
                                                    .size(px(14.))
                                                    .text_color(theme::success()),
                                            )
                                            .tooltip(|window, cx| {
                                                gpui_component::tooltip::Tooltip::new(
                                                    "Connecting...",
                                                )
                                                .build(window, cx)
                                            })
                                    } else if selected.is_some() {
                                        button
                                            .border_color(theme::border())
                                            .child(
                                                svg()
                                                    .path("icons/plug.svg")
                                                    .size(px(14.))
                                                    .text_color(theme::text_dim())
                                                    .group_hover("btn-connect", |icon| {
                                                        icon.text_color(theme::success())
                                                    }),
                                            )
                                            .hover(|button| {
                                                button
                                                    .bg(rgb(if theme::is_dark() {
                                                        0x294132
                                                    } else {
                                                        0xdcefdf
                                                    }))
                                                    .border_color(theme::success())
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
                                            .border_color(theme::disabled_border())
                                            .child(
                                                svg()
                                                    .path("icons/plug.svg")
                                                    .size(px(14.))
                                                    .text_color(theme::disabled()),
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
        let selected = self
            .connection
            .selected
            .and_then(|index| self.connection.connections.get(index));
        let nodes = selected
            .map(|connection| {
                connection
                    .nodes
                    .iter()
                    .enumerate()
                    .map(|(index, configured_node)| {
                        let reachable = self
                            .connection
                            .endpoint_health
                            .get(&connection.name)
                            .and_then(|health| {
                                health
                                    .iter()
                                    .find(|node| node.node_index == index)
                                    .map(|node| node.reachable)
                            });
                        let (label, color) = match reachable {
                            Some(true) => ("reachable", theme::success()),
                            Some(false) => ("failed", theme::danger()),
                            None => ("not tested", theme::text_dim()),
                        };
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().size(px(7.)).rounded_full().bg(color))
                            .child(configured_node.name.clone())
                            .child(
                                div()
                                    .text_color(theme::text_dim())
                                    .child(configured_node.endpoint.clone()),
                            )
                            .child(div().text_xs().text_color(theme::text_dim()).child(label))
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
                        .text_color(theme::text())
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
                        .children(self.topology_section(connection))
                })
                .when(selected.is_none(), |panel| {
                    panel.child("Add or select a cluster connection to begin.")
                }),
        )
    }

    /// Phase 5 M4: a read-only shards-and-replicas view, built entirely
    /// from the memberships each node reported about itself at connect
    /// time. Nothing here is configurable; zeDB displays what the
    /// servers said. Absent topology (never connected, LBs, Cloud)
    /// renders nothing.
    fn topology_section(&self, connection: &ConnectionConfig) -> Option<impl IntoElement> {
        let health = self.connection.endpoint_health.get(&connection.name)?;
        // cluster -> shard -> node display names, insertion-ordered.
        type Shards = Vec<(u64, Vec<String>)>;
        let mut clusters: Vec<(String, Shards)> = Vec::new();
        for node in health {
            for membership in &node.memberships {
                // Each node's implicit "default" cluster contains only
                // itself; merging them across nodes would invent a
                // cluster that does not exist.
                if membership.cluster == "default" {
                    continue;
                }
                let cluster = match clusters
                    .iter_mut()
                    .find(|(name, _)| *name == membership.cluster)
                {
                    Some((_, shards)) => shards,
                    None => {
                        clusters.push((membership.cluster.clone(), Vec::new()));
                        &mut clusters.last_mut().expect("just pushed").1
                    }
                };
                match cluster
                    .iter_mut()
                    .find(|(shard, _)| *shard == membership.shard)
                {
                    Some((_, members)) => members.push(node.name.clone()),
                    None => cluster.push((membership.shard, vec![node.name.clone()])),
                }
            }
        }
        if clusters.is_empty() {
            return None;
        }
        for (_, shards) in &mut clusters {
            shards.sort_by_key(|(shard, _)| *shard);
        }

        Some(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(div().text_color(theme::text_dim()).child("Topology"))
                .children(clusters.into_iter().map(|(cluster, shards)| {
                    let shard_count = shards.len();
                    let replicas_per_shard = shards
                        .first()
                        .map(|(_, members)| members.len())
                        .unwrap_or(0);
                    let uniform = shards
                        .iter()
                        .all(|(_, members)| members.len() == replicas_per_shard);
                    let replica_summary = if uniform {
                        format!("{shard_count} shard(s) \u{d7} {replicas_per_shard} replica(s)")
                    } else {
                        format!("{shard_count} shards")
                    };
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .p_3()
                        .rounded(px(4.))
                        .border_1()
                        .border_color(theme::border())
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(div().text_color(theme::text()).child(cluster))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme::text_dim())
                                        .child(replica_summary),
                                ),
                        )
                        .children(shards.into_iter().map(|(shard, members)| {
                            div()
                                .flex()
                                .gap_2()
                                .text_sm()
                                .child(
                                    div()
                                        .w(px(80.))
                                        .flex_none()
                                        .text_color(theme::text_dim())
                                        .child(format!("shard {shard}")),
                                )
                                .child(div().text_color(theme::text()).child(members.join(", ")))
                        }))
                })),
        )
    }

    fn query_editor_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let tab_rows = self
            .query
            .tabs
            .iter()
            .enumerate()
            .map(|(index, tab)| {
                let tab_id = tab.id;
                let multiple = self.query.tabs.len() > 1;
                let has_right = index + 1 < self.query.tabs.len();
                let active = index == self.query.active_tab;
                // Tail tabs are labelled "Tail N" and wear a steel-blue,
                // top-rounded border so they read as a distinct live view.
                let tail_number = tab.tail.as_ref().map(|state| state.number);
                let is_tail = tail_number.is_some();
                let label = tab_display_name(tab);
                div()
                    .id(("query-tab", tab_id))
                    .flex_none()
                    .h_full()
                    .px_3()
                    .flex()
                    .items_center()
                    .whitespace_nowrap()
                    .when(!is_tail, |tab| {
                        tab.border_b_2()
                            .when(active, |tab| {
                                tab.border_color(theme::accent()).text_color(theme::text())
                            })
                            .when(!active, |tab| {
                                tab.border_color(theme::bg_sidebar())
                                    .text_color(theme::text_dim())
                                    .hover(|tab| tab.text_color(theme::text()).cursor_pointer())
                            })
                    })
                    .when(is_tail, |tab| {
                        tab.border_1()
                            .border_color(rgb(0x4682b4))
                            .rounded_t(px(5.))
                            .when(active, |tab| tab.text_color(theme::text()))
                            .when(!active, |tab| {
                                tab.text_color(theme::text_dim())
                                    .hover(|tab| tab.text_color(theme::text()).cursor_pointer())
                            })
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.query.active_tab = index;
                        cx.notify();
                    }))
                    // Drag to reorder: a ghost of the label follows the
                    // cursor, the drop target shows an accent left edge.
                    .on_drag(
                        DragTab {
                            index,
                            label: label.clone().into(),
                        },
                        |drag, _, _, cx| {
                            cx.stop_propagation();
                            cx.new(|_| drag.clone())
                        },
                    )
                    .drag_over::<DragTab>(|style, _, _, _| {
                        style.border_l_2().border_color(theme::accent())
                    })
                    .on_drop(cx.listener(move |this, drag: &DragTab, _, cx| {
                        this.reorder_query_tab(drag.index, index, cx);
                    }))
                    .context_menu(move |menu, _, _| {
                        menu.menu_with_enable(
                            "Close tab",
                            Box::new(CloseQueryTab { tab_id }),
                            multiple,
                        )
                        .menu_with_enable(
                            "Close others",
                            Box::new(CloseOtherQueryTabs { tab_id }),
                            multiple,
                        )
                        .menu_with_enable(
                            "Close to the right",
                            Box::new(CloseQueryTabsToRight { tab_id }),
                            has_right,
                        )
                    })
                    .gap_2()
                    .child(label)
                    .when(self.query.tabs.len() > 1, |tab_row| {
                        tab_row.child(
                            div()
                                .id(("close-query-tab", tab_id))
                                .size(px(18.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(3.))
                                .text_color(theme::text_dim())
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
                                                    .bg(theme::hover())
                                                    .text_color(theme::text())
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
            .query
            .tabs
            .get(self.query.active_tab)
            .expect("query editor requires an active tab");
        let running = matches!(
            active.outcome,
            QueryOutcome::Running | QueryOutcome::StatementError { .. }
        );
        let statement_failed = matches!(active.outcome, QueryOutcome::StatementError { .. });
        let error_text = match &active.outcome {
            QueryOutcome::Error(error) => Some(error.clone()),
            _ => None,
        };
        // Owned snapshot of the active tab's tail, so the strip renders
        // without re-borrowing self.
        let tail_info = active.tail.as_ref().map(|state| {
            // Dirty when the editor no longer matches the adopted query, so
            // the "update tail" button can appear.
            let editor_text = active.editor.read(cx).value().to_string();
            TailStripInfo {
                tab_id: active.id,
                key: state.query.key.clone(),
                paused: state.paused,
                error: state.error.clone(),
                rows: active.result_rows,
                native_available: state.native_available == Some(true),
                push: state.push,
                experimental_streaming_enabled: self.preferences.experimental_streaming_queries,
                dirty: editor_text.trim() != state.baseline.trim(),
            }
        });
        // Ask needs a remembered agent that discovery has not ruled out.
        let ask_agent = self.preferences.last_agent.clone().filter(|name| {
            self.agent.agents.is_empty()
                || self.agent.agents.iter().any(|agent| agent.name == *name)
        });
        let ask_agent_icon = ask_agent.as_ref().map(|name| {
            self.agent
                .agents
                .iter()
                .find(|agent| agent.name == *name)
                .map(|agent| agent_pane::icon_for(&agent.id))
                .unwrap_or(match name.as_str() {
                    // Discovery may not have run yet; the built-ins
                    // are known by name.
                    "Claude Code" => "icons/agent-claude.svg",
                    "Codex" => "icons/agent-codex.svg",
                    _ => "icons/sparkle.svg",
                })
        });
        let has_result = active.has_result || active.explain.is_some();
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
                let mut text = if *columns == 0 {
                    // DDL and other resultless statements: an empty body
                    // with HTTP 200 is ClickHouse's whole success signal.
                    "OK: statement executed (no result set)".to_string()
                } else if result_capped {
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

        let editor_column = div()
            .h_full()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(36.))
                    .flex_none()
                    .flex()
                    .items_end()
                    .justify_between()
                    .bg(theme::bg_sidebar())
                    .border_b_1()
                    .border_color(theme::border())
                    .child(
                        // Tabs scroll (incl. shift-wheel) so they never push
                        // the toolbar off-screen; the toolbar is flex_none
                        // and always wins the space.
                        div()
                            .id("query-tabs-scroll")
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .flex()
                            .items_end()
                            .overflow_x_scroll()
                            .children(tab_rows)
                            .child(
                                div()
                                    .id("add-query-tab")
                                    .flex_none()
                                    .h_full()
                                    .px_3()
                                    .flex()
                                    .items_center()
                                    .text_color(theme::text_dim())
                                    .child("+")
                                    .hover(|button| {
                                        button.text_color(theme::text()).cursor_pointer()
                                    })
                                    // Dropping a dragged tab here sends it to
                                    // the very end (the one spot no tab's own
                                    // drop zone covers).
                                    .drag_over::<DragTab>(|style, _, _, _| {
                                        style.border_l_2().border_color(theme::accent())
                                    })
                                    .on_drop(cx.listener(|this, drag: &DragTab, _, cx| {
                                        let last = this.query.tabs.len().saturating_sub(1);
                                        this.reorder_query_tab(drag.index, last, cx);
                                    }))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.add_query_tab(window, cx)
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .flex_none()
                            .h_full()
                            .pr_2()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(self.max_rows_selector(running, cx))
                            .child(
                                div()
                                    .id("run-selection")
                                    .px_3()
                                    .py_1()
                                    .rounded(px(3.))
                                    .border_1()
                                    .border_color(theme::border())
                                    .text_color(theme::text_dim())
                                    .flex()
                                    .items_center()
                                    .gap_1p5()
                                    .child(
                                        svg()
                                            .path("icons/execute.svg")
                                            .size(px(13.))
                                            .text_color(theme::text_dim()),
                                    )
                                    .child("Execute")
                                    .tooltip(|window, cx| {
                                        gpui_component::tooltip::Tooltip::new(
                                            "Execute the selection, or every statement \u{b7} \u{2303}X",
                                        )
                                        .build(window, cx)
                                    })
                                    .when(!running, |button| {
                                        button
                                            .text_color(theme::text())
                                            .hover(|button| {
                                                button.bg(theme::hover()).cursor_pointer()
                                            })
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.run_selection(window, cx)
                                            }))
                                    }),
                            )
                            .child(
                                div()
                                    .id("run-query")
                                    .group("run-button")
                                    .px_3()
                                    .py_1()
                                    .rounded(px(3.))
                                    .map(|button| {
                                        if running {
                                            // Running at rest; Cancel on hover
                                            // (stacked labels: hover cannot
                                            // change text).
                                            button
                                                .relative()
                                                .bg(theme::hover())
                                                .text_color(theme::text_dim())
                                                .hover(|button| {
                                                    button
                                                        .bg(theme::danger_hover())
                                                        .text_color(theme::danger())
                                                        .cursor_pointer()
                                                })
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.cancel_query(cx)
                                                }))
                                                .child(
                                                    div()
                                                        .group_hover("run-button", |label| {
                                                            label.invisible()
                                                        })
                                                        .child("Running\u{2026}"),
                                                )
                                                .child(
                                                    div()
                                                        .absolute()
                                                        .inset_0()
                                                        .flex()
                                                        .items_center()
                                                        .justify_center()
                                                        .invisible()
                                                        .group_hover("run-button", |label| {
                                                            label.visible()
                                                        })
                                                        .child("Cancel"),
                                                )
                                        } else {
                                            button
                                                .bg(theme::primary())
                                                .text_color(theme::primary_foreground())
                                                .child("Run")
                                                .tooltip(|window, cx| {
                                                    gpui_component::tooltip::Tooltip::new(
                                                        "Run the statement at the cursor \u{b7} \u{2318}\u{21a9}",
                                                    )
                                                    .build(window, cx)
                                                })
                                                .hover(|button| {
                                                    button
                                                        .bg(theme::primary_hover())
                                                        .cursor_pointer()
                                                })
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.run_query(window, cx)
                                                }))
                                        }
                                    }),
                            )
                            .child(
                                div()
                                    .id("toggle-history")
                                    .size(px(24.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(3.))
                                    .when(self.history.open, |button| button.bg(theme::hover()))
                                    .child(
                                        svg().path("icons/history.svg").size(px(14.)).text_color(
                                            if self.history.open {
                                                theme::text()
                                            } else {
                                                theme::text_dim()
                                            },
                                        ),
                                    )
                                    .hover(|button| button.bg(theme::hover()).cursor_pointer())
                                    .tooltip(|window, cx| {
                                        gpui_component::tooltip::Tooltip::new(
                                            "Query history and saved queries",
                                        )
                                        .build(window, cx)
                                    })
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.history_toggle(cx)),
                                    ),
                            ),
                    ),
            )
            .child(
                div()
                    .when(!has_result, |editor| editor.flex_1())
                    .when(has_result, |editor| editor.h(px(editor_height)).flex_none())
                    .min_h_0()
                    .relative()
                    .bg(theme::bg())
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
            // The tail strip sits directly above the results, where the eye
            // is already resting on the newest rows.
            .when_some(tail_info, |panel, info| {
                panel.child(self.tail_strip(info, cx))
            })
            .when(has_result, |panel| {
                panel.child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .border_t_1()
                        .border_color(theme::border())
                        .map(|pane| match active.explain.as_ref() {
                            Some(plan) => pane.child(self.explain_panel(plan, cx)),
                            None => pane.child(result_grid),
                        }),
                )
            })
            .when(active.advisor.is_some() && active.explain.is_none(), |panel| {
                panel.child(self.query_advisor_panel(active, cx))
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
                    .border_color(theme::border())
                    .when(
                        matches!(
                            active.outcome,
                            QueryOutcome::Error(_) | QueryOutcome::StatementError { .. }
                        ),
                        |row| row.bg(rgb(0x2b2227)).text_color(theme::danger()),
                    )
                    .when(
                        !matches!(
                            active.outcome,
                            QueryOutcome::Error(_) | QueryOutcome::StatementError { .. }
                        ),
                        |row| row.text_color(theme::text_dim()),
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
                            .when_some(error_text, |row, error| {
                                let copy_error = error.clone();
                                row.child(
                                    div()
                                        .flex_none()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .child(
                                            div()
                                                .id("copy-error")
                                                .size(px(22.))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .rounded(px(3.))
                                                .hover(|button| {
                                                    button.bg(theme::hover()).cursor_pointer()
                                                })
                                                .tooltip(|window, cx| {
                                                    gpui_component::tooltip::Tooltip::new(
                                                        "Copy error",
                                                    )
                                                    .build(window, cx)
                                                })
                                                .on_click(cx.listener(move |_, _, _, cx| {
                                                    cx.write_to_clipboard(
                                                        ClipboardItem::new_string(
                                                            copy_error.clone(),
                                                        ),
                                                    );
                                                }))
                                                .child(
                                                    svg()
                                                        .path("icons/copy.svg")
                                                        .size(px(13.))
                                                        .text_color(theme::text_dim()),
                                                ),
                                        )
                                        .when_some(ask_agent.clone(), |actions, agent_name| {
                                            // Visible message: the error itself.
                                            // Hidden context: where it came from.
                                            let visible = format!(
                                                "This query failed, help me diagnose and fix it:\n{error}"
                                            );
                                            let mut hidden = format!(
                                                "Context (not shown to the user): the error came from zeDB query tab \"Query {}\"",
                                                active.id
                                            );
                                            match &active.failed_sql {
                                                Some(sql) => hidden.push_str(&format!(
                                                    ", which executed:\n```sql\n{sql}\n```\nIf you propose a corrected query with the propose_query tool, zeDB will replace the failed statement in that tab in place."
                                                )),
                                                None => hidden.push('.'),
                                            }
                                            let fix_target = active
                                                .failed_sql
                                                .clone()
                                                .map(|sql| (active.id, sql));
                                            actions.child(
                                                // Just the remembered agent's
                                                // logo; the tooltip names it.
                                                div()
                                                    .id("ask-agent-error")
                                                    .size(px(22.))
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .rounded(px(3.))
                                                    .hover(|button| {
                                                        button
                                                            .bg(theme::hover())
                                                            .cursor_pointer()
                                                    })
                                                    .on_click(cx.listener(
                                                        move |this, _, window, cx| {
                                                            this.agent_fix_target =
                                                                fix_target.clone();
                                                            this.agent_ask_about(
                                                                visible.clone(),
                                                                hidden.clone(),
                                                                window,
                                                                cx,
                                                            );
                                                        },
                                                    ))
                                                    .child(
                                                        svg()
                                                            .path(
                                                                ask_agent_icon
                                                                    .unwrap_or("icons/sparkle.svg"),
                                                            )
                                                            .size(px(14.))
                                                            .text_color(theme::text()),
                                                    )
                                                    .tooltip(move |window, cx| {
                                                        gpui_component::tooltip::Tooltip::new(
                                                            format!("Ask {agent_name}"),
                                                        )
                                                        .build(window, cx)
                                                    }),
                                            )
                                        }),
                                )
                            })
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
                                                .border_color(theme::border())
                                                .text_color(theme::text())
                                                .hover(|button| {
                                                    button
                                                        .bg(theme::danger_hover())
                                                        .cursor_pointer()
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
                                                .border_color(theme::border())
                                                .text_color(theme::text())
                                                .hover(|button| {
                                                    button
                                                        .bg(theme::danger_hover())
                                                        .cursor_pointer()
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
                                    div()
                                        .flex_none()
                                        .text_color(theme::text_dim())
                                        .child(elapsed),
                                )
                            }),
                    ),
            );

        div()
            .size_full()
            .flex()
            .child(editor_column)
            .when(self.history.open, |root| {
                root.child(self.history_resize_handle(cx))
                    .child(self.history_drawer(cx))
            })
    }

    fn status_bar(&self) -> impl IntoElement {
        let status = self
            .notice
            .clone()
            .unwrap_or_else(|| match &self.connection.connected {
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
            .bg(theme::bg_status())
            .border_t_1()
            .border_color(theme::border())
            .px_3()
            .flex()
            .items_center()
            .justify_between()
            .text_xs()
            .text_color(if self.notice_warning {
                theme::danger()
            } else {
                theme::text_dim()
            })
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
                                    .text_color(if normal {
                                        theme::toggle_knob_on()
                                    } else {
                                        theme::text_dim()
                                    })
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
                                        .text_color(theme::text())
                                        .child(format!("{}{text}", command_line.prompt)),
                                )
                            })
                            .when_some(recording, |row, register| {
                                row.child(
                                    div()
                                        .flex_none()
                                        .text_color(theme::warning())
                                        .child(format!("recording @{register}")),
                                )
                            })
                    }),
            )
            .child(concat!("zedb ", env!("CARGO_PKG_VERSION")))
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
        if !self.preferences.vim_mode || self.show_fleet || self.connection.connected.is_none() {
            return None;
        }
        let tab = self.query.tabs.get(self.query.active_tab)?;
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
            .bg(theme::bg())
            .text_color(theme::text())
            .font_family("Menlo")
            .text_sm()
            .on_action(cx.listener(Self::run_query_action))
            .on_action(cx.listener(Self::run_selection_action))
            .on_action(cx.listener(|this, _: &SaveQueryTab, _, cx| this.save_active_query_tab(cx)))
            .on_action(cx.listener(|this, action: &CloseQueryTab, _, cx| {
                this.close_query_tab(action.tab_id, cx);
            }))
            .on_action(cx.listener(|this, action: &CloseOtherQueryTabs, _, cx| {
                this.close_other_query_tabs(action.tab_id, cx);
            }))
            .on_action(cx.listener(|this, action: &CloseQueryTabsToRight, _, cx| {
                this.close_query_tabs_to_right(action.tab_id, cx);
            }))
            .on_action(cx.listener(|this, action: &TailTable, window, cx| {
                this.start_tail(
                    action.database.clone(),
                    action.object.clone(),
                    action.cap,
                    window,
                    cx,
                );
            }))
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
            .on_action(cx.listener(|this, action: &SetApplyCluster, _, cx| {
                this.set_apply_cluster(action.cluster.clone(), cx)
            }))
            .on_action(cx.listener(|this, action: &DuplicateConnection, _, cx| {
                this.duplicate_connection(action.index, cx)
            }))
            .on_action(cx.listener(|this, action: &EditConnection, _, cx| {
                this.connection.selected = Some(action.index);
                this.start_edit(cx)
            }))
            .on_action(cx.listener(|this, action: &DeleteConnection, _, cx| {
                this.connection.selected = Some(action.index);
                this.request_delete(cx)
            }))
            .on_action(cx.listener(|this, action: &grid_spike::HeaderSort, _, cx| {
                if let Some(tab) = this.query.tabs.get(this.query.active_tab) {
                    let grid = tab.result_grid.clone();
                    grid.update(cx, |grid, cx| grid.header_sort_action(action, cx));
                }
            }))
            // The grid's right-click Copy / Copy as CSV menu dispatches
            // to the window root, so handle it here and delegate to the
            // active tab's grid (cmd-C is handled on the grid itself).
            .on_action(cx.listener(|this, _: &grid_spike::Copy, _, cx| {
                if let Some(tab) = this.query.tabs.get(this.query.active_tab) {
                    let grid = tab.result_grid.clone();
                    grid.update(cx, |grid, cx| grid.copy_selected(cx));
                }
            }))
            .on_action(cx.listener(|this, _: &grid_spike::CopyAsCsv, _, cx| {
                if let Some(tab) = this.query.tabs.get(this.query.active_tab) {
                    let grid = tab.result_grid.clone();
                    grid.update(cx, |grid, cx| grid.copy_selected_csv(cx));
                }
            }))
            .on_action(
                cx.listener(|this, action: &grid_spike::HeaderFilter, window, cx| {
                    this.open_column_filter(action.column.clone(), window, cx)
                }),
            )
            .on_action(cx.listener(|this, action: &ops::SetOpsTopLimit, _, cx| {
                this.ops_set_top_limit(action.limit, cx)
            }))
            .on_action(cx.listener(|this, action: &ops::SetOpsScope, _, cx| {
                this.ops_set_scope(action.cluster.clone(), cx)
            }))
            .on_action(cx.listener(|this, action: &ViewObjectDdl, window, cx| {
                let object = this
                    .schema
                    .databases
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
                        this.schema.cache.as_ref().and_then(|cache| {
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
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, _, cx| {
                    // Any click that reaches the root dismisses an open
                    // filter popover; clicks inside it stop propagation.
                    if let Some(tab) = this.query.tabs.get(this.query.active_tab) {
                        let grid = tab.result_grid.clone();
                        grid.update(cx, |grid, cx| {
                            grid.close_filter_panel(cx);
                        });
                    }
                }),
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
                if let Some((start_width, start_x)) = this.history.resizing {
                    let width = start_width + (start_x - f32::from(event.position.x));
                    this.history.width = width.clamp(240.0, 640.0);
                    cx.notify();
                }
                if let Some((target, last_y)) = this.query.resize {
                    let current_y = f32::from(event.position.y);
                    let delta = current_y - last_y;
                    if let Some(tab) = this.query.tabs.get_mut(this.query.active_tab) {
                        match target {
                            QueryResizeTarget::Editor => {
                                tab.editor_height = (tab.editor_height + delta).clamp(80.0, 720.0);
                            }
                            QueryResizeTarget::Status => {
                                tab.status_height = (tab.status_height - delta).clamp(34.0, 240.0);
                            }
                        }
                    }
                    this.query.resize = Some((target, current_y));
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
                    this.query.resize = None;
                    this.history.resizing = None;
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, _| {
                    this.resizing_sidebar = false;
                    this.resizing_sidebar_sections = false;
                    this.fleet.resizing_detail = false;
                    this.agent.resizing = false;
                    this.query.resize = None;
                    this.history.resizing = None;
                }),
            )
            .when(self.export.is_some(), |root| {
                root.child(self.export_overlay(cx))
            })
            .when(self.schema.pending_apply.is_some(), |root| {
                root.child(self.apply_confirm_overlay(cx))
            })
            .when(self.palette.open, |root| {
                root.child(self.command_palette_overlay(cx))
            })
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
                            .when(
                                !self.show_preferences && self.connection.form.is_some(),
                                |main| main.child(self.form_panel(cx)),
                            )
                            .when(
                                !self.show_preferences && self.connection.form.is_none(),
                                |main| {
                                    main.child(self.connection_toolbar(cx)).child(
                                        div()
                                            .flex_1()
                                            .min_h_0()
                                            .when(self.show_ops, |content| {
                                                content.child(self.ops_panel(cx))
                                            })
                                            .when(
                                                !self.show_ops && self.show_query_editor,
                                                |content| {
                                                    content.child(self.query_editor_panel(cx))
                                                },
                                            )
                                            .when(
                                                !self.show_ops
                                                    && !self.show_query_editor
                                                    && self.show_fleet,
                                                |content| content.child(self.fleet_panel(cx)),
                                            )
                                            .when(
                                                !self.show_ops
                                                    && !self.show_query_editor
                                                    && !self.show_fleet,
                                                |content| {
                                                    content
                                                        .when(
                                                            self.schema.selected_object.is_some(),
                                                            |content| {
                                                                content.child(
                                                                    self.schema_object_panel(
                                                                        window, cx,
                                                                    ),
                                                                )
                                                            },
                                                        )
                                                        .when(
                                                            self.schema.selected_object.is_none(),
                                                            |content| {
                                                                content
                                                                    .child(self.cluster_overview())
                                                            },
                                                        )
                                                },
                                            ),
                                    )
                                },
                            ),
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
            .occlude()
            .child(
                div()
                    .w(px(560.))
                    .p_5()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(theme::border())
                    .bg(theme::bg_sidebar())
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
                            .text_color(theme::text())
                            .child(format!("zeDB {version}")),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::text_dim())
                            .mt_2()
                            .child("Commit"),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_family("Menlo")
                            .text_color(theme::text())
                            .child(commit),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::text_dim())
                            .mt_2()
                            .child("Version"),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_family("Menlo")
                            .text_color(theme::text())
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
                                    .border_color(theme::border())
                                    .text_center()
                                    .text_color(theme::text())
                                    .child("OK")
                                    .hover(|button| button.bg(theme::hover()).cursor_pointer())
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
                                    .border_color(theme::border())
                                    .text_center()
                                    .text_color(theme::text())
                                    .child("Copy")
                                    .hover(|button| button.bg(theme::hover()).cursor_pointer())
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
}
