mod agent_pane;
mod author;
mod codegen;
mod command_palette;
mod commit;
mod components;
mod explain_ui;
mod export;
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
    QueryStreamSummary, SchemaObjectKind, SchemaObjectMeta, TableStorage,
};
use zedb_core::{
    load_connections, load_preferences, save_connections, save_preferences, ConnectionConfig,
    ConnectionNode, EnvTier, Preferences,
};

use components::text_input::{self, TextInput};
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
    /// Driver settings rows; rows with a blank name or value are
    /// dropped on save.
    driver_settings: Vec<DriverSettingForm>,
}

struct DriverSettingForm {
    name: Entity<TextInput>,
    value: Entity<TextInput>,
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
    /// When set, schema-changing actions (applying a storage suggestion)
    /// run `ON CLUSTER <name>` so they reach every node, not just the
    /// active one. None = the active node only. Chosen from the node
    /// selector, defaults to node scope.
    apply_cluster: Option<String>,
}

#[derive(Clone)]
struct EndpointHealth {
    node_index: usize,
    name: String,
    endpoint: String,
    reachable: bool,
    /// This node's shard/replica memberships from its own
    /// system.clusters (empty when unreachable or unknown).
    memberships: Vec<zedb_ch::ClusterMembership>,
}

/// The first cluster in which the two nodes sit on different shards:
/// switching between them changes which slice local tables show.
fn differentiating_cluster(
    a: &[zedb_ch::ClusterMembership],
    b: &[zedb_ch::ClusterMembership],
) -> Option<String> {
    a.iter().find_map(|membership| {
        b.iter()
            .find(|other| other.cluster == membership.cluster)
            .filter(|other| other.shard != membership.shard)
            .map(|_| membership.cluster.clone())
    })
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
        total_bytes: object.total_bytes,
    }
}

struct SelectedSchemaObject {
    database: String,
    object: SchemaObjectMeta,
    loading: bool,
    columns: Vec<ColumnInfo>,
    details: Option<ObjectDetails>,
    /// Table-wide compression totals; None until loaded, or for objects
    /// with no parts (views, dictionaries).
    storage: Option<TableStorage>,
    /// Per-column approximate distinct counts (aligned to `columns`),
    /// filled on demand by the opt-in cardinality probe. None until run.
    cardinalities: Option<Vec<u64>>,
    /// The cardinality probe is scanning the table.
    cardinality_loading: bool,
    /// The probe failed (e.g. a column type `uniqCombined` rejects).
    cardinality_error: Option<String>,
    /// Waiting for the user to confirm that analysing may write temporary
    /// tables (only asked on writable connections, where measurement runs).
    cardinality_confirming: bool,
    /// Measured savings per column index (Tier 3): how many times smaller
    /// the suggested definition is than the current one. Filled in the
    /// background after analysis, only on writable connections.
    measured: HashMap<usize, f64>,
    /// The column index whose suggestion is currently being applied.
    applying: Option<usize>,
    /// The in-flight apply has run long enough to show a spinner.
    applying_slow: bool,
    /// Active parts grouped by partition (Phase 9, Part B). None until the
    /// Parts tab loads them off-thread.
    partitions: Option<Vec<zedb_ch::PartitionStats>>,
    partitions_loading: bool,
    partitions_error: Option<String>,
    /// Merges in progress for this object, refreshed on a poll while the
    /// Parts tab is open (Phase 9, Part B).
    merges: Vec<zedb_ch::MergeInfo>,
    /// Materialized-view lineage for this object (Phase 9, Part C). None
    /// until the Dependencies tab loads it.
    dependencies: Option<zedb_ch::ObjectDependencies>,
    dependencies_loading: bool,
    dependencies_error: Option<String>,
    /// Projections attached to this table (Phase 9, Part C). None until the
    /// Projections tab loads them.
    projections: Option<Vec<zedb_ch::ProjectionInfo>>,
    projections_loading: bool,
    projections_error: Option<String>,
    ddl_editor: Entity<InputState>,
    engine_editor: Entity<InputState>,
    tab: ObjectInspectorTab,
    error: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ObjectInspectorTab {
    Overview,
    Columns,
    Parts,
    Projections,
    Dependencies,
    Ddl,
}

struct QueryTab {
    /// Stable across snapshots and relaunches. Display names are not keys.
    persistent_id: String,
    /// The machine-local saved item this editor updates with cmd-s.
    saved_tab_id: Option<String>,
    name: String,
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
    /// An EXPLAIN plan shown in place of the results grid.
    explain: Option<zedb_ch::explain::ExplainNode>,
    /// Query advisor result for the displayed statement, shown only when
    /// explicitly requested (the saved-query Advise button). `None` hides
    /// the lane; `Some(empty)` shows a "looks fine" note; `Some(findings)`
    /// lists them. Computed off-thread from the run stats + EXPLAIN.
    advisor: Option<Vec<query_advisor::QueryFinding>>,
    /// Set when this run was launched by Advise, so completion computes
    /// findings; a plain Run leaves it false and shows no lane.
    advise_pending: bool,
    /// Generation guarding a late advisor result against a newer run.
    advisor_generation: u64,
    /// What the failed run executed, for the error bar's Ask button.
    failed_sql: Option<String>,
    /// The last successfully executed statement, i.e. the one whose
    /// result the grid is showing; header sorts rewrite and re-run it.
    displayed_statement: Option<String>,
    /// Byte offset of the displayed statement in the editor at run
    /// time, so rewrites target the right one among identical twins.
    displayed_statement_offset: Option<usize>,
    /// Server-side id of the currently streaming statement, for
    /// recognizing kills initiated from the ops view.
    running_query_id: Option<String>,
    /// Live tail state when this tab is tailing a table (Phase 10); `None`
    /// for an ordinary query tab.
    tail: Option<TailState>,
}

/// A running live tail bound to a query tab: the table, the monotonic key
/// it advances on, and the last key seen. The poll loop is guarded by
/// `generation` so a stopped or restarted tail's late polls are ignored.
struct TailState {
    /// Display number for the tab label ("Tail 1", "Tail 2", ...).
    number: usize,
    /// The editable definition: table, key, filter, LIMIT. The tab editor
    /// shows [`tail::base_sql`] of this; editing and applying re-parses it.
    query: tail::TailQuery,
    /// The editor text the tail last adopted, for dirty-detection (the
    /// "update tail" button shows when the editor differs from this).
    baseline: String,
    /// SQL literal of the last-seen key; `None` until the seed runs.
    last: Option<String>,
    /// The key's column index in the result, for reading each batch's max.
    key_index: usize,
    /// Retained-row cap the user chose (`None` = unlimited).
    cap: Option<usize>,
    /// Native-protocol (TCP) push discovery: `None` while probing, then
    /// whether a native port (9440 TLS / 9000) is reachable, so we can offer
    /// "instant updates" only when a switch is actually possible.
    native_available: Option<bool>,
    /// How new rows arrive: HTTP-cadence polling, fast polling over the
    /// native connection, or a direct ClickHouse streaming query.
    push: TailPush,
    /// Last server-native cursor emitted by `STREAM CURSOR`, retained across
    /// reconnects. The monotonic `last` key remains the fallback until this is
    /// available.
    stream_cursor: Option<tail::StreamCursor>,
    /// The dedicated native query backing active streaming, including its
    /// abort handle and epoch for rejecting late batches.
    stream: Option<TailStream>,
    /// The Live View backing an active `WATCH`, retained for servers and
    /// query shapes where direct streaming is not selected or supported.
    watch: Option<TailWatch>,
    /// Do not retry STREAM continuously after it fails for this tail. The
    /// remaining instant ladder, WATCH then fast polling, stays available.
    stream_rejected: bool,
    generation: u64,
    paused: bool,
    /// A transient error from the last poll, shown without stopping.
    error: Option<String>,
}

/// The tail's delivery mechanism. `Poll` is the universal baseline; the
/// other two are the "instant updates" upgrade over the native (TCP)
/// protocol, and both silently fall back to `Poll` when the native
/// connection goes away (docs/PHASE-10.md).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TailPush {
    /// `poll_sql` every [`tail::TAIL_INTERVAL_MS`] over HTTP.
    Poll,
    /// `poll_sql` every [`tail::TAIL_INTERVAL_FAST_MS`]; each poll rides
    /// the pooled native connection. Used when streaming is unsupported for
    /// the server or edited query shape.
    Fast,
    /// A dedicated native connection returns inserted rows from ClickHouse
    /// 26.6 `STREAM CURSOR` directly.
    Stream,
    /// A dedicated native connection holds `WATCH <live view> EVENTS` open;
    /// each notification triggers the existing keyed fetch.
    Watch,
}

/// An active direct stream. Aborting its task drops the only handle to the
/// dedicated native connection, which closes the server query.
#[derive(Clone, Debug)]
struct TailStream {
    epoch: u64,
    abort: tokio::task::AbortHandle,
}

impl Drop for TailStream {
    fn drop(&mut self) {
        self.abort.abort();
    }
}

#[derive(Clone, Debug)]
struct TailWatch {
    view: String,
    epoch: u64,
    abort: tokio::task::AbortHandle,
}

impl Drop for TailWatch {
    fn drop(&mut self) {
        self.abort.abort();
    }
}

type TailStreamBatch = (Vec<zedb_core::ColumnMeta>, Vec<Vec<zedb_core::Value>>);

/// One appendable tail batch: the columns (only on the priming poll, to
/// install the header) and the rows.
type TailBatch = (
    Option<Vec<zedb_core::ColumnMeta>>,
    Vec<Vec<zedb_core::Value>>,
);

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

/// Owned view of a tab's tail for rendering the status strip.
struct TailStripInfo {
    tab_id: usize,
    key: String,
    paused: bool,
    error: Option<String>,
    rows: usize,
    /// A native port is reachable, so "instant updates" (server-push) is on
    /// the table; the button only shows when this is true.
    native_available: bool,
    /// The active delivery mode, for the instant badge and for hiding the
    /// upgrade button once instant updates are on.
    push: TailPush,
    /// Whether experimental STREAM is opted in, for the adjacent flask icon.
    experimental_streaming_enabled: bool,
    /// The editor differs from the adopted query, so "update tail" shows.
    dirty: bool,
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
    show_ops: bool,
    /// Query history + saved queries drawer beside the editor.
    show_history: bool,
    history: Vec<zedb_core::HistoryEntry>,
    history_search: Entity<TextInput>,
    /// A saved query being renamed inline: (original name, input).
    history_renaming: Option<(String, Entity<TextInput>)>,
    /// A saved tab being renamed inline: (id, name, input).
    saved_tab_renaming: Option<(String, String, Entity<TextInput>)>,
    saved_tabs: Vec<zedb_core::SavedTab>,
    /// Clear-history asked once; the next click clears.
    history_clear_armed: bool,
    /// Where an error-bar ask came from: (query tab id, failed sql).
    /// An agent-proposed query replaces that statement in place.
    agent_fix_target: Option<(usize, String)>,
    /// The export dialog, when open.
    export: Option<export::ExportState>,
    history_width: f32,
    /// An active drawer-edge drag: (start width, start mouse x).
    history_resizing: Option<(f32, f32)>,
    history_tab: query_history::HistoryTab,
    ops: ops::OpsState,
    /// query_ids killed from the ops view; errors on these statements
    /// report the kill instead of a transport failure.
    ops_killed: std::collections::HashSet<String>,
    health_poll_generation: u64,
    /// Cancels a stale merges poll when the object or tab changes.
    merges_poll_generation: u64,
    /// Monotonic source for per-tail generation ids; a stopped or restarted
    /// tail bumps past its old loop so late polls are dropped.
    next_tail_generation: u64,
    /// Display counter for tail tabs ("Tail 1", "Tail 2", ...).
    next_tail_number: usize,
    /// Monotonic source for STREAM and WATCH epochs. WATCH also uses it for
    /// unique Live View names.
    next_stream_epoch: u64,
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
    /// Cardinality-probe results kept for this session, keyed by
    /// (connection, database, object). Once a table has been analyzed
    /// its distinct counts auto-load on reopen without re-prompting.
    cardinality_cache: HashMap<(String, String, String), Vec<u64>>,
    /// Measured codec savings for the session (Tier 3), keyed like the
    /// cardinality cache: (connection, database, object) -> {column
    /// index -> times-smaller}. Auto-loads on reopen.
    measured_cache: HashMap<(String, String, String), HashMap<usize, f64>>,
    /// A suggestion (column index + statements) awaiting confirmation
    /// before it runs, because the table is large enough that applying
    /// rewrites a lot of data. Rendered as a confirm overlay.
    pending_apply: Option<(usize, Vec<String>)>,
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
    /// Last window-refocus health/update check, for debounce.
    last_focus_check: Option<Instant>,
    github: GithubAuth,
    github_generation: u64,
    rerun_generation: u64,
    query_error_decision: Option<tokio::sync::oneshot::Sender<bool>>,
    query_run_id: u64,
    query_resize: Option<(QueryResizeTarget, f32)>,
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
            let snapshot = this.schema_cache.as_ref().map(|cache| cache.snapshot());
            for database in &mut this.schema_databases {
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
                        if let Some(tab) = this.query_tabs.get(this.active_query_tab) {
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
                        && this.form.is_some()
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
                cardinality_cache: HashMap::new(),
                measured_cache: HashMap::new(),
                pending_apply: None,
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
                show_ops: false,
                show_history: false,
                history: zedb_core::load_history(),
                history_search: Self::input("", "Search queries", false, cx),
                history_renaming: None,
                saved_tab_renaming: None,
                saved_tabs: zedb_core::load_saved_tabs(),
                history_clear_armed: false,
                agent_fix_target: None,
                export: None,
                history_width: 320.0,
                history_resizing: None,
                history_tab: query_history::HistoryTab::default(),
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
                next_tail_generation: 0,
                next_tail_number: 0,
                next_stream_epoch: 0,
                query_abort: None,
                rerun_pending: None,
                last_focus_check: None,
                github: GithubAuth::SignedOut,
                github_generation: 0,
                rerun_generation: 0,
                query_error_decision: None,
                query_run_id: 0,
                query_resize: None,
                preferences,
                palette: command_palette::PaletteState::new(cx),
                settings_sync: settings_sync::SettingsSyncState::new(cx),
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
                cardinality_cache: HashMap::new(),
                measured_cache: HashMap::new(),
                pending_apply: None,
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
                show_ops: false,
                show_history: false,
                history: zedb_core::load_history(),
                history_search: Self::input("", "Search queries", false, cx),
                history_renaming: None,
                saved_tab_renaming: None,
                saved_tabs: zedb_core::load_saved_tabs(),
                history_clear_armed: false,
                agent_fix_target: None,
                export: None,
                history_width: 320.0,
                history_resizing: None,
                history_tab: query_history::HistoryTab::default(),
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
                next_tail_generation: 0,
                next_tail_number: 0,
                next_stream_epoch: 0,
                query_abort: None,
                rerun_pending: None,
                last_focus_check: None,
                github: GithubAuth::SignedOut,
                github_generation: 0,
                rerun_generation: 0,
                query_error_decision: None,
                query_run_id: 0,
                query_resize: None,
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
                .query_tabs
                .iter()
                .map(|tab| zedb_core::SavedQueryTab {
                    id: tab.persistent_id.clone(),
                    saved_tab_id: tab.saved_tab_id.clone(),
                    name: tab_display_name(tab),
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
                        if this.selected == Some(index) && (this.show_query_editor || this.show_ops)
                        {
                            this.show_query_editor = false;
                            this.show_fleet = false;
                            this.show_ops = false;
                        }
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
                    .when(self.selected.is_some(), |sidebar| {
                        sidebar.child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .when_some(self.pending_delete.as_ref(), |panel, name| {
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
                                                    .when(self.connecting.is_none(), |button| {
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
                                    .selected_schema_object
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
                    .when(self.connected.is_some(), |header| {
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
            .when(self.connected.is_some(), |panel| {
                panel.child(div().px_2().pb_2().child(self.schema_filter.clone()))
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
                                .text_color(theme::danger())
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
            driver_settings: Self::seeded_driver_settings(&[], cx),
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
            driver_settings: Self::seeded_driver_settings(&connection.driver.settings, cx),
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
        self.settings_sync_tick(cx);
        cx.notify();
    }

    /// The connection form's inputs in visual order, for tab cycling.
    fn form_focus_order(&self) -> Vec<Entity<components::text_input::TextInput>> {
        let Some(form) = &self.form else {
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
    fn form_tab(&mut self, backwards: bool, window: &mut Window, cx: &mut Context<Self>) {
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

    /// The saved driver settings as form rows, with the two well-known
    /// names seeded (empty, so they drop on save unless filled) when
    /// absent. They behave exactly like manually added rows.
    fn seeded_driver_settings(
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

    fn remove_driver_setting(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(form) = &mut self.form {
            if index < form.driver_settings.len() {
                form.driver_settings.remove(index);
                cx.notify();
            }
        }
    }

    fn add_driver_setting(&mut self, cx: &mut Context<Self>) {
        let setting = DriverSettingForm {
            name: Self::input("", "setting", false, cx),
            value: Self::input("", "value", false, cx),
        };
        if let Some(form) = &mut self.form {
            form.driver_settings.push(setting);
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
                self.settings_sync_tick(cx);
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

    fn sidebar_section_resize_handle(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
                self.settings_sync_tick(cx);
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
        let driver = connection.driver.clone();
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
                        driver: connection.driver.clone(),
                    },
                    apply_cluster: None,
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
    fn focus_recheck(&mut self, cx: &mut Context<Self>) {
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
        let Some(connected) = &self.connected else {
            return;
        };
        let config = connected.client_config.clone();
        let name = connected.name.clone();
        let node_index = connected.active_node;
        let schema_cache = self.schema_cache.clone();
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
                    .connected
                    .as_ref()
                    .is_some_and(|connected| connected.name == name);
                if !still_here {
                    return;
                }
                this.connected = None;
                this.schema_cache = None;
                this.schema_provider.set_context(None, None);
                this.fleet.write_unlocked = false;
                if let Some(health) = this.endpoint_health.get_mut(&name) {
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
        self.ops_reset(cx);
        cx.notify();
    }

    /// Set (or clear) the cluster the schema-apply actions target with
    /// `ON CLUSTER`. Chosen from the node selector.
    fn set_apply_cluster(&mut self, cluster: Option<String>, cx: &mut Context<Self>) {
        if let Some(connected) = self.connected.as_mut() {
            connected.apply_cluster = cluster;
            cx.notify();
        }
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
        let previous_memberships = self
            .connected
            .as_ref()
            .map(|connected| connected.active_node)
            .and_then(|active| {
                self.endpoint_health
                    .get(&connected_name)
                    .and_then(|health| health.iter().find(|node| node.node_index == active))
            })
            .map(|node| node.memberships.clone())
            .unwrap_or_default();
        let Some(connected) = self.connected.as_mut() else {
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
                // The schema context just changed; refresh open editors'
                // diagnostics so a now-known database drops its stale
                // "unknown" squiggly without waiting for the next edit.
                this.refresh_schema_diagnostics(cx);
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

    /// Entry point for the Analyse button. On a writable connection the
    /// measurement step writes temporary tables, so ask for confirmation
    /// first; on a read-only connection nothing is written, so run the
    /// (read-only) scan straight away.
    fn request_analyze(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let writable = self
            .connected
            .as_ref()
            .map(|cluster| cluster.name.clone())
            .map(|name| self.connection_is_writable(&name))
            .unwrap_or(false);
        if writable {
            if let Some(selected) = &mut self.selected_schema_object {
                selected.cardinality_confirming = true;
            }
            cx.notify();
        } else {
            self.analyze_cardinality(window, cx);
        }
    }

    /// The user confirmed the write; clear the prompt and run.
    fn confirm_analyze(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(selected) = &mut self.selected_schema_object {
            selected.cardinality_confirming = false;
        }
        self.analyze_cardinality(window, cx);
    }

    fn cancel_analyze(&mut self, cx: &mut Context<Self>) {
        if let Some(selected) = &mut self.selected_schema_object {
            selected.cardinality_confirming = false;
        }
        cx.notify();
    }

    /// Opt-in cardinality probe (Phase 8, Tier 2): scan the selected
    /// table once for each column's approximate distinct count, off the
    /// main thread, and store it on the selection. Feeds the codec
    /// advisor. Guarded so a stale result (selection changed while the
    /// scan ran) is dropped.
    fn analyze_cardinality(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let (connection_name, config, database_name, object_name, column_names) = {
            let Some(selected) = &self.selected_schema_object else {
                return;
            };
            if selected.cardinality_loading || selected.columns.is_empty() {
                return;
            }
            let Some(connected) = &self.connected else {
                return;
            };
            (
                connected.name.clone(),
                connected.client_config.clone(),
                selected.database.clone(),
                selected.object.name.clone(),
                selected
                    .columns
                    .iter()
                    .map(|column| column.name.clone())
                    .collect::<Vec<_>>(),
            )
        };

        if let Some(selected) = &mut self.selected_schema_object {
            selected.cardinality_loading = true;
            selected.cardinality_error = None;
        }
        cx.notify();

        let task = rt::tokio().spawn({
            let database_name = database_name.clone();
            let object_name = object_name.clone();
            async move {
                ChClient::new(config)
                    .column_cardinalities(&database_name, &object_name, &column_names)
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
                selected.cardinality_loading = false;
                let to_cache = match result {
                    Ok(Ok(cardinalities)) => {
                        selected.cardinalities = Some(cardinalities.clone());
                        Some(cardinalities)
                    }
                    Ok(Err(error)) => {
                        selected.cardinality_error = Some(error.to_string());
                        None
                    }
                    Err(error) => {
                        selected.cardinality_error = Some(error.to_string());
                        None
                    }
                };
                // The `selected` borrow ends here; keep the result for the
                // session so reopening this table auto-loads it.
                if let Some(cardinalities) = to_cache {
                    this.cardinality_cache
                        .insert((connection_name, database_name, object_name), cardinalities);
                    // Cardinality is known; measure the actual savings of
                    // the actionable suggestions (Tier 3), writable
                    // connections only.
                    this.measure_suggestions(cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Load active parts grouped by partition for the selected object
    /// (Phase 9, Part B), off-thread. No-op if already loaded or loading;
    /// pass `force` to reload. Guards against a stale connection/object.
    fn load_partitions(&mut self, cx: &mut Context<Self>) {
        let (connection_name, config, database_name, object_name) = {
            let Some(selected) = &self.selected_schema_object else {
                return;
            };
            if selected.partitions.is_some() || selected.partitions_loading {
                return;
            }
            let Some(connected) = &self.connected else {
                return;
            };
            (
                connected.name.clone(),
                connected.client_config.clone(),
                selected.database.clone(),
                selected.object.name.clone(),
            )
        };

        if let Some(selected) = &mut self.selected_schema_object {
            selected.partitions_loading = true;
            selected.partitions_error = None;
        }
        cx.notify();

        let task = rt::tokio().spawn({
            let database_name = database_name.clone();
            let object_name = object_name.clone();
            async move {
                ChClient::new(config)
                    .table_partitions(&database_name, &object_name)
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
                selected.partitions_loading = false;
                match result {
                    Ok(Ok(partitions)) => selected.partitions = Some(partitions),
                    Ok(Err(error)) => selected.partitions_error = Some(error.to_string()),
                    Err(error) => selected.partitions_error = Some(error.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Poll `system.merges` for the selected object while the Parts tab is
    /// open, so in-progress merges and their progress update live. A
    /// generation guard stops the loop when the object or tab changes.
    fn start_merges_poll(&mut self, cx: &mut Context<Self>) {
        self.merges_poll_generation += 1;
        let generation = self.merges_poll_generation;
        let Some(selected) = &self.selected_schema_object else {
            return;
        };
        let Some(connected) = &self.connected else {
            return;
        };
        let connection_name = connected.name.clone();
        let database = selected.database.clone();
        let object = selected.object.name.clone();
        self.merges_fetch(generation, cx);
        cx.spawn(async move |this, cx| loop {
            Timer::after(Duration::from_secs(2)).await;
            let live = this
                .update(cx, |this, cx| {
                    let live = this.merges_poll_generation == generation
                        && this.connected.as_ref().map(|cluster| cluster.name.as_str())
                            == Some(connection_name.as_str())
                        && this
                            .selected_schema_object
                            .as_ref()
                            .is_some_and(|selected| {
                                selected.tab == ObjectInspectorTab::Parts
                                    && selected.database == database
                                    && selected.object.name == object
                            });
                    if live {
                        this.merges_fetch(generation, cx);
                    }
                    live
                })
                .unwrap_or(false);
            if !live {
                break;
            }
        })
        .detach();
    }

    /// One off-thread read of the selected object's in-progress merges.
    fn merges_fetch(&mut self, generation: u64, cx: &mut Context<Self>) {
        let (connection_name, config, database, object) = {
            let Some(selected) = &self.selected_schema_object else {
                return;
            };
            let Some(connected) = &self.connected else {
                return;
            };
            (
                connected.name.clone(),
                connected.client_config.clone(),
                selected.database.clone(),
                selected.object.name.clone(),
            )
        };
        let guard_database = database.clone();
        let guard_object = object.clone();
        let task = rt::tokio().spawn(async move {
            ChClient::new(config)
                .active_merges(&database, &object)
                .await
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if this.merges_poll_generation != generation {
                    return;
                }
                if this.connected.as_ref().map(|cluster| cluster.name.as_str())
                    != Some(connection_name.as_str())
                {
                    return;
                }
                let Some(selected) = &mut this.selected_schema_object else {
                    return;
                };
                if selected.database != guard_database || selected.object.name != guard_object {
                    return;
                }
                // Keep the last snapshot on a transient error; a live poll
                // shouldn't blank out or spam.
                if let Ok(Ok(merges)) = result {
                    selected.merges = merges;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// The Parts tab: active parts grouped by partition, with a "too many
    /// parts" warning. Reads the connected node's `system.parts`.
    fn parts_panel(&self, selected: &SelectedSchemaObject, cx: &mut Context<Self>) -> gpui::Div {
        // A single partition with this many active parts is worth flagging
        // (ClickHouse delays inserts around 150, throws around 300).
        const TOO_MANY_PARTS: u64 = 100;

        let loading = selected.partitions_loading;
        let error = selected.partitions_error.clone();
        let partitions = selected.partitions.clone().unwrap_or_default();
        let total_parts: u64 = partitions.iter().map(|partition| partition.parts).sum();
        let total_rows: u64 = partitions.iter().map(|partition| partition.rows).sum();
        let total_compressed: u64 = partitions
            .iter()
            .map(|partition| partition.compressed_bytes)
            .sum();
        let busiest = partitions.iter().map(|p| p.parts).max().unwrap_or(0);

        let num_cell = |width: f32, text: String, dim: bool| {
            div()
                .w(px(width))
                .flex_none()
                .text_right()
                .when(dim, |cell| cell.text_color(theme::text_dim()))
                .child(text)
        };
        let header_cell = |width: f32, text: &'static str| {
            div()
                .w(px(width))
                .flex_none()
                .text_right()
                .text_color(theme::text_dim())
                .child(text)
        };

        let rows: Vec<_> = partitions
            .iter()
            .map(|partition| {
                let ratio = if partition.compressed_bytes > 0 {
                    format!(
                        "{:.1}x",
                        partition.uncompressed_bytes as f64 / partition.compressed_bytes as f64
                    )
                } else {
                    "-".to_string()
                };
                let label = if partition.partition == "tuple()" {
                    "(unpartitioned)".to_string()
                } else {
                    partition.partition.clone()
                };
                let hot = partition.parts >= TOO_MANY_PARTS;
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .py_1()
                    .border_b_1()
                    .border_color(theme::border())
                    .text_sm()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_color(theme::text())
                            .child(label),
                    )
                    .child(
                        num_cell(70.0, Self::format_count(partition.parts), false)
                            .when(hot, |cell| cell.text_color(theme::warning())),
                    )
                    .child(num_cell(110.0, Self::format_count(partition.rows), true))
                    .child(num_cell(
                        110.0,
                        Self::format_bytes(partition.compressed_bytes),
                        true,
                    ))
                    .child(num_cell(
                        110.0,
                        Self::format_bytes(partition.uncompressed_bytes),
                        true,
                    ))
                    .child(num_cell(60.0, ratio, true))
                    .child(num_cell(
                        60.0,
                        Self::format_count(partition.max_level),
                        true,
                    ))
            })
            .collect();

        // Compact count: 42_401_792 -> "42.4M", so a live merge line never
        // wraps.
        let compact = |n: u64| -> String {
            let (value, suffix) = if n >= 1_000_000_000 {
                (n as f64 / 1e9, "B")
            } else if n >= 1_000_000 {
                (n as f64 / 1e6, "M")
            } else if n >= 1_000 {
                (n as f64 / 1e3, "K")
            } else {
                return n.to_string();
            };
            let text = format!("{value:.1}");
            format!("{}{suffix}", text.strip_suffix(".0").unwrap_or(&text))
        };

        // Live merges (auto-refreshed by the poll): a thin progress bar and
        // a compact, dot-separated status line. Mutations are tagged.
        let merge_rows: Vec<_> = selected
            .merges
            .iter()
            .map(|merge| {
                let pct = merge.progress_pct.min(100);
                let partition = if merge.partition_id.is_empty() || merge.partition_id == "all" {
                    "(unpartitioned)".to_string()
                } else {
                    merge.partition_id.clone()
                };
                // The stats that ride the right edge; the progress (bar + %)
                // stays grouped with the label on the left.
                let stats = format!(
                    "{}\u{2192}1 \u{b7} {} rows \u{b7} {} \u{b7} {}s",
                    merge.num_parts,
                    compact(merge.rows_written),
                    Self::format_bytes(merge.memory_usage),
                    merge.elapsed_secs,
                );
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .py_1p5()
                    .border_b_1()
                    .border_color(theme::border())
                    .text_sm()
                    .whitespace_nowrap()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(div().text_color(theme::text()).child(partition))
                            .when(merge.is_mutation, |row| {
                                row.child(
                                    div()
                                        .text_xs()
                                        .text_color(theme::text_dim())
                                        .child("mutation"),
                                )
                            })
                            .child(
                                div()
                                    .w(px(120.))
                                    .h(px(6.))
                                    .rounded(px(3.))
                                    .bg(theme::border())
                                    .child(
                                        div()
                                            .h_full()
                                            .w(px(120. * pct as f32 / 100.))
                                            .rounded(px(3.))
                                            .bg(theme::accent()),
                                    ),
                            )
                            .child(
                                div()
                                    .w(px(40.))
                                    .text_color(theme::text_dim())
                                    .child(format!("{pct}%")),
                            ),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_xs()
                            .text_color(theme::text_dim())
                            .child(stats),
                    )
            })
            .collect();
        let has_merges = !merge_rows.is_empty();

        let body = div()
            .id("object-parts")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px_4()
            .py_2()
            .map(|panel| {
                if loading && partitions.is_empty() {
                    panel.child(
                        div()
                            .py_3()
                            .text_color(theme::text_dim())
                            .child("Loading parts\u{2026}"),
                    )
                } else if let Some(error) = error {
                    panel.child(div().py_3().text_color(theme::danger()).child(error))
                } else if partitions.is_empty() {
                    panel.child(div().py_3().text_color(theme::text_dim()).child(
                        "No active parts. This object stores nothing on disk (a view or \
                             dictionary), or the table is empty.",
                    ))
                } else {
                    panel
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_3()
                                .py_1()
                                .border_b_1()
                                .border_color(theme::border())
                                .text_xs()
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .text_color(theme::text_dim())
                                        .child("Partition"),
                                )
                                .child(header_cell(70.0, "Parts"))
                                .child(header_cell(110.0, "Rows"))
                                .child(header_cell(110.0, "Compressed"))
                                .child(header_cell(110.0, "Uncompressed"))
                                .child(header_cell(60.0, "Ratio"))
                                .child(header_cell(60.0, "Level")),
                        )
                        .children(rows)
                }
            });

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            // Summary + refresh.
            .child(
                div()
                    .flex_none()
                    .px_4()
                    .py_2()
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(theme::border())
                    .child(div().text_xs().text_color(theme::text_dim()).child(
                        if partitions.is_empty() {
                            String::new()
                        } else {
                            format!(
                                "{} partition(s) \u{b7} {} active parts \u{b7} {} rows \u{b7} {}",
                                Self::format_count(partitions.len() as u64),
                                Self::format_count(total_parts),
                                Self::format_count(total_rows),
                                Self::format_bytes(total_compressed),
                            )
                        },
                    ))
                    .child(
                        div()
                            .id("refresh-parts")
                            .size(px(22.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(3.))
                            .child(
                                svg()
                                    .path("icons/refresh.svg")
                                    .size(px(13.))
                                    .text_color(theme::text_dim()),
                            )
                            .hover(|button| button.bg(theme::hover()).cursor_pointer())
                            .tooltip(|window, cx| {
                                gpui_component::tooltip::Tooltip::new("Refresh").build(window, cx)
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(selected) = &mut this.selected_schema_object {
                                    selected.partitions = None;
                                }
                                this.load_partitions(cx);
                            })),
                    ),
            )
            // Live merges (shown only while something is merging).
            .when(has_merges, |panel| {
                panel.child(
                    div()
                        .flex_none()
                        .px_4()
                        .py_2()
                        .border_b_1()
                        .border_color(theme::border())
                        .bg(theme::bg_sunken())
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme::text_dim())
                                .pb_1()
                                .child("Merges in progress"),
                        )
                        .children(merge_rows),
                )
            })
            // A single partition with too many parts slows reads and inserts.
            .when(busiest >= TOO_MANY_PARTS, |panel| {
                panel.child(
                    div()
                        .flex_none()
                        .px_4()
                        .py_2()
                        .border_b_1()
                        .border_color(theme::border())
                        .bg(theme::bg_status())
                        .text_xs()
                        .text_color(theme::warning())
                        .child(format!(
                            "A partition has {} active parts. Many small parts slow reads \
                             and inserts; consider OPTIMIZE, fewer partitions, or larger \
                             inserts.",
                            Self::format_count(busiest)
                        )),
                )
            })
            .child(body)
    }

    /// Load the materialized-view lineage for the selected object
    /// (Phase 9, Part C), off-thread. No-op if already loaded or loading.
    fn load_dependencies(&mut self, cx: &mut Context<Self>) {
        let (connection_name, config, database_name, object_name) = {
            let Some(selected) = &self.selected_schema_object else {
                return;
            };
            if selected.dependencies.is_some() || selected.dependencies_loading {
                return;
            }
            let Some(connected) = &self.connected else {
                return;
            };
            (
                connected.name.clone(),
                connected.client_config.clone(),
                selected.database.clone(),
                selected.object.name.clone(),
            )
        };

        if let Some(selected) = &mut self.selected_schema_object {
            selected.dependencies_loading = true;
            selected.dependencies_error = None;
        }
        cx.notify();

        let task = rt::tokio().spawn({
            let database_name = database_name.clone();
            let object_name = object_name.clone();
            async move {
                ChClient::new(config)
                    .object_dependencies(&database_name, &object_name)
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
                selected.dependencies_loading = false;
                match result {
                    Ok(Ok(dependencies)) => selected.dependencies = Some(dependencies),
                    Ok(Err(error)) => selected.dependencies_error = Some(error.to_string()),
                    Err(error) => selected.dependencies_error = Some(error.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Load the projections attached to the selected object (Phase 9,
    /// Part C), off-thread. No-op if already loaded or loading.
    fn load_projections(&mut self, cx: &mut Context<Self>) {
        let (connection_name, config, database_name, object_name) = {
            let Some(selected) = &self.selected_schema_object else {
                return;
            };
            if selected.projections.is_some() || selected.projections_loading {
                return;
            }
            let Some(connected) = &self.connected else {
                return;
            };
            (
                connected.name.clone(),
                connected.client_config.clone(),
                selected.database.clone(),
                selected.object.name.clone(),
            )
        };

        if let Some(selected) = &mut self.selected_schema_object {
            selected.projections_loading = true;
            selected.projections_error = None;
        }
        cx.notify();

        let task = rt::tokio().spawn({
            let database_name = database_name.clone();
            let object_name = object_name.clone();
            async move {
                ChClient::new(config)
                    .table_projections(&database_name, &object_name)
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
                selected.projections_loading = false;
                match result {
                    Ok(Ok(projections)) => selected.projections = Some(projections),
                    Ok(Err(error)) => selected.projections_error = Some(error.to_string()),
                    Err(error) => selected.projections_error = Some(error.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Select the `db.table` a dependency node points at, landing on the
    /// Dependencies tab so the lineage can be walked node by node. Uses the
    /// loaded schema meta when available, else a minimal placeholder the
    /// detail load fills in.
    fn navigate_to_dependency(
        &mut self,
        full: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((database, name)) = full.split_once('.') else {
            return;
        };
        let meta = self
            .schema_databases
            .iter()
            .find(|node| node.meta.name == database)
            .and_then(|node| node.objects.as_ref())
            .and_then(|objects| objects.iter().find(|object| object.name == name).cloned())
            .unwrap_or_else(|| SchemaObjectMeta {
                name: name.to_string(),
                engine: String::new(),
                kind: SchemaObjectKind::Table,
                total_rows: None,
                total_bytes: None,
            });
        self.select_schema_object(
            database.to_string(),
            meta,
            ObjectInspectorTab::Dependencies,
            window,
            cx,
        );
    }

    /// The Dependencies tab: the object's materialized-view lineage as
    /// clickable source -> view -> target chains (walk the graph), plus its
    /// projections. This object is emphasized; missing tables are flagged.
    fn dependencies_panel(
        &self,
        selected: &SelectedSchemaObject,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let this = format!("{}.{}", selected.database, selected.object.name);
        let missing: Vec<String> = selected
            .dependencies
            .as_ref()
            .map(|deps| deps.missing_tables.clone())
            .unwrap_or_default();

        // Unique element-id base per chain so clickable nodes don't collide.
        let mut chain_base = 0usize;
        let section = |title: &'static str| {
            div()
                .text_xs()
                .text_color(theme::text_dim())
                .pt_3()
                .pb_1()
                .child(title)
        };

        let mut body = div()
            .id("object-graph")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px_4()
            .py_2();

        if selected.dependencies_loading && selected.dependencies.is_none() {
            return div().flex_1().min_h_0().child(
                body.child(
                    div()
                        .py_3()
                        .text_color(theme::text_dim())
                        .child("Loading lineage\u{2026}"),
                ),
            );
        }
        if let Some(error) = &selected.dependencies_error {
            return div().flex_1().min_h_0().child(
                body.child(
                    div()
                        .py_3()
                        .text_color(theme::danger())
                        .child(error.clone()),
                ),
            );
        }

        let deps = selected.dependencies.clone().unwrap_or_default();
        let has_lineage =
            deps.is_materialized_view || !deps.feeds.is_empty() || !deps.written_by.is_empty();

        if !has_lineage {
            body = body.child(
                div()
                    .py_3()
                    .text_color(theme::text_dim())
                    .child("No materialized views read from or write to this object."),
            );
        } else {
            if !deps.missing_tables.is_empty() {
                body = body.child(
                    div()
                        .mt_1()
                        .px_3()
                        .py_2()
                        .rounded(px(3.))
                        .border_1()
                        .border_color(theme::danger())
                        .text_xs()
                        .text_color(theme::danger())
                        .child(
                            "A referenced table no longer exists: this pipeline is broken and \
                             may be silently dropping inserts.",
                        ),
                );
            }
            if deps.is_materialized_view {
                body = body.child(section("This materialized view"));
                let mut nodes = Vec::new();
                if let Some(source) = deps.reads_from.clone() {
                    nodes.push((source, false));
                }
                nodes.push((this.clone(), true));
                if let Some(target) = deps.writes_to.clone() {
                    nodes.push((target, false));
                }
                body = body.child(self.dep_chain(nodes, &missing, chain_base, cx));
                chain_base += 10;
            }
            if !deps.written_by.is_empty() {
                body = body.child(section("Written by"));
                for mv in &deps.written_by {
                    let mut nodes = Vec::new();
                    if let Some(source) = mv.source.clone() {
                        nodes.push((source, false));
                    }
                    nodes.push((mv.view.clone(), false));
                    nodes.push((this.clone(), true));
                    body = body.child(self.dep_chain(nodes, &missing, chain_base, cx));
                    chain_base += 10;
                }
            }
            if !deps.feeds.is_empty() {
                body = body.child(section("Feeds"));
                for mv in &deps.feeds {
                    let mut nodes = vec![(this.clone(), true), (mv.view.clone(), false)];
                    if let Some(target) = mv.target.clone() {
                        nodes.push((target, false));
                    }
                    body = body.child(self.dep_chain(nodes, &missing, chain_base, cx));
                    chain_base += 10;
                }
            }
        }

        div().flex_1().min_h_0().child(body)
    }

    /// The Projections tab: the alternate sorted / pre-aggregated copies of
    /// this table's data that ClickHouse keeps in sync (Phase 9, Part C).
    fn projections_panel(&self, selected: &SelectedSchemaObject) -> gpui::Div {
        let mut body = div()
            .id("object-projections")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px_4()
            .py_2()
            // A one-line explanation of what a projection is.
            .child(
                div()
                    .pt_1()
                    .pb_2()
                    .text_xs()
                    .text_color(theme::text_dim())
                    .child(
                        "A projection is a hidden, always-in-sync copy of this table's data in a \
                         different sort order (or pre-aggregated). Queries that match it read far \
                         less, at the cost of extra storage and write work.",
                    ),
            );

        if selected.projections_loading && selected.projections.is_none() {
            return div().flex_1().min_h_0().child(
                body.child(
                    div()
                        .py_3()
                        .text_color(theme::text_dim())
                        .child("Loading projections\u{2026}"),
                ),
            );
        }
        if let Some(error) = &selected.projections_error {
            return div().flex_1().min_h_0().child(
                body.child(
                    div()
                        .py_3()
                        .text_color(theme::danger())
                        .child(error.clone()),
                ),
            );
        }

        let projections = selected.projections.clone().unwrap_or_default();
        if projections.is_empty() {
            body = body.child(
                div()
                    .py_3()
                    .text_color(theme::text_dim())
                    .child("This table has no projections."),
            );
        } else {
            for projection in &projections {
                body = body.child(
                    div()
                        .py_1p5()
                        .border_b_1()
                        .border_color(theme::border())
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap_3()
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .text_sm()
                                        .child(
                                            div()
                                                .text_color(theme::text())
                                                .child(projection.name.clone()),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(theme::text_dim())
                                                .child(projection.kind.clone()),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex_none()
                                        .text_xs()
                                        .text_color(theme::text_dim())
                                        .child(format!(
                                            "{} rows \u{b7} {} \u{b7} {} parts",
                                            Self::format_count(projection.rows),
                                            Self::format_bytes(projection.compressed_bytes),
                                            Self::format_count(projection.parts),
                                        )),
                                ),
                        )
                        .child(
                            div()
                                .text_xs()
                                .font_family("Menlo")
                                .text_color(theme::text_dim())
                                .overflow_hidden()
                                .whitespace_nowrap()
                                // An Aggregate projection's value is what it
                                // aggregates; a Normal one, its order.
                                .child(
                                    if projection.kind == "Aggregate"
                                        || projection.sorting_key.is_empty()
                                    {
                                        projection.query.clone()
                                    } else {
                                        format!("ORDER BY {}", projection.sorting_key)
                                    },
                                ),
                        ),
                );
            }
        }

        div().flex_1().min_h_0().child(body)
    }

    /// One arrow-joined lineage chain. `this` node is emphasized, missing
    /// tables are flagged in danger, and every other node is clickable to
    /// navigate to it. `base` seeds unique element ids.
    fn dep_chain(
        &self,
        nodes: Vec<(String, bool)>,
        missing: &[String],
        base: usize,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let group = SharedString::from(format!("dep-chain-{base}"));
        let mut row = div()
            .group(group.clone())
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .py_1p5()
            .text_sm();
        for (index, (label, is_self)) in nodes.into_iter().enumerate() {
            if index > 0 {
                row = row.child(
                    div()
                        .flex_none()
                        .text_color(theme::text_dim())
                        .child("\u{2192}"),
                );
            }
            let is_missing = missing.contains(&label);
            let mut pill = div()
                .id(("dep-node", base + index))
                .flex_none()
                .px_2()
                .py_0p5()
                .rounded(px(3.))
                .border_1()
                .when(is_missing, |pill| {
                    pill.border_color(theme::danger())
                        .text_color(theme::danger())
                })
                .when(is_self && !is_missing, |pill| {
                    // Accent border by default, but it yields to whichever
                    // node in the chain is under the cursor.
                    pill.border_color(theme::accent())
                        .bg(theme::selected())
                        .text_color(theme::text())
                        .group_hover(group.clone(), |pill| pill.border_color(theme::border()))
                })
                .when(!is_self && !is_missing, |pill| {
                    pill.border_color(theme::border())
                        .text_color(theme::text_dim())
                })
                .child(if is_missing {
                    format!("{label}  (missing)")
                } else {
                    label.clone()
                });
            if !is_self && !is_missing {
                let target = label.clone();
                pill = pill
                    .hover(|pill| {
                        pill.border_color(theme::accent())
                            .bg(theme::hover())
                            .cursor_pointer()
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.navigate_to_dependency(target.clone(), window, cx)
                    }));
            }
            row = row.child(pill);
        }
        row
    }

    /// True when the named connection allows writes (needed for the
    /// Tier 3 measurement, which builds a throwaway table).
    fn connection_is_writable(&self, name: &str) -> bool {
        self.connections
            .iter()
            .find(|connection| connection.name.as_str() == name)
            .map(|connection| !connection.read_only)
            .unwrap_or(false)
    }

    /// Measure the actual size savings of each actionable suggestion for
    /// the current selection (Phase 8, Tier 3). Runs one throwaway-table
    /// trial per suggested column, off the main thread, and stores the
    /// result. Only on writable connections; a no-op otherwise.
    fn measure_suggestions(&mut self, cx: &mut Context<Self>) {
        let (connection_name, config, database, table) = {
            let Some(selected) = &self.selected_schema_object else {
                return;
            };
            let Some(connected) = &self.connected else {
                return;
            };
            (
                connected.name.clone(),
                connected.client_config.clone(),
                selected.database.clone(),
                selected.object.name.clone(),
            )
        };
        if !self.connection_is_writable(&connection_name) {
            return;
        }

        // Collect the actionable columns and how to build their trials.
        let jobs: Vec<(usize, String, String, String)> = {
            let Some(selected) = &self.selected_schema_object else {
                return;
            };
            let Some(cardinalities) = &selected.cardinalities else {
                return;
            };
            let total_rows = selected.object.total_rows.unwrap_or(0);
            selected
                .columns
                .iter()
                .enumerate()
                .filter(|(index, _)| !selected.measured.contains_key(index))
                .filter_map(|(index, column)| {
                    let distinct = cardinalities.get(index).copied().unwrap_or(0);
                    let advice = storage_advisor::advise(
                        &storage_advisor::ColumnFacts {
                            name: &column.name,
                            type_name: &column.type_name,
                            codec: &column.codec,
                            distinct,
                            total_rows,
                            compressed_bytes: column.compressed_bytes,
                            uncompressed_bytes: column.uncompressed_bytes,
                        },
                        &database,
                        &table,
                        // Trial defs are cluster-independent.
                        None,
                    );
                    if let storage_advisor::Advice::Suggest {
                        base_def, cand_def, ..
                    } = advice
                    {
                        Some((index, column.name.clone(), base_def, cand_def))
                    } else {
                        None
                    }
                })
                .collect()
        };

        for (index, column_name, base_def, cand_def) in jobs {
            let config = config.clone();
            let database = database.clone();
            let table = table.clone();
            let connection_name = connection_name.clone();
            // Globally-unique trial-table name so concurrent trials (even
            // two tables in the same database) never drop each other's.
            static TRIAL_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let seq = TRIAL_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let trial_name = format!("_zedb_codec_trial_{seq}");
            let task_database = database.clone();
            let task_table = table.clone();
            let task = rt::tokio().spawn(async move {
                ChClient::new(config)
                    .measure_codec_savings(
                        &task_database,
                        &task_table,
                        &column_name,
                        &base_def,
                        &cand_def,
                        &trial_name,
                    )
                    .await
            });
            cx.spawn(async move |this, cx| {
                let result = task.await;
                this.update(cx, |this, cx| {
                    if let Ok(Ok(Some(ratio))) = result {
                        this.measured_cache
                            .entry((connection_name.clone(), database.clone(), table.clone()))
                            .or_default()
                            .insert(index, ratio);
                        if this.connected.as_ref().map(|cluster| cluster.name.as_str())
                            == Some(connection_name.as_str())
                        {
                            if let Some(selected) = &mut this.selected_schema_object {
                                if selected.database == database && selected.object.name == table {
                                    selected.measured.insert(index, ratio);
                                }
                            }
                        }
                        cx.notify();
                    }
                })
                .ok();
            })
            .detach();
        }
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
        // Auto-load cardinality if this table was analyzed earlier this
        // session, so it shows without re-prompting.
        let cache_key = (
            connection_name.clone(),
            database_name.clone(),
            object_name.clone(),
        );
        let cached_cardinalities = self.cardinality_cache.get(&cache_key).cloned();
        let cached_measured = self
            .measured_cache
            .get(&cache_key)
            .cloned()
            .unwrap_or_default();
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
            storage: None,
            cardinalities: cached_cardinalities,
            cardinality_loading: false,
            cardinality_error: None,
            cardinality_confirming: false,
            measured: cached_measured,
            applying: None,
            applying_slow: false,
            partitions: None,
            partitions_loading: false,
            partitions_error: None,
            merges: Vec::new(),
            dependencies: None,
            dependencies_loading: false,
            dependencies_error: None,
            projections: None,
            projections_loading: false,
            projections_error: None,
            ddl_editor: ddl_editor.clone(),
            engine_editor: engine_editor.clone(),
            tab,
            error: None,
        });
        self.show_query_editor = false;
        cx.notify();

        // A carried-over tab must load its own data (tab clicks normally do).
        match tab {
            ObjectInspectorTab::Parts => {
                self.load_partitions(cx);
                self.start_merges_poll(cx);
            }
            ObjectInspectorTab::Dependencies => self.load_dependencies(cx),
            ObjectInspectorTab::Projections => self.load_projections(cx),
            _ => {}
        }

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
                    client.object_details(&database_name, &object_name),
                    client.table_storage(&database_name, &object_name)
                )
            }
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            if let Ok((_, Ok(details), _)) = &result {
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
                    Ok((Ok(columns), Ok(details), storage)) => {
                        selected.columns = columns;
                        selected.details = Some(details);
                        // Storage is supplementary; a failure here (e.g.
                        // no access to system.parts) must not fail the
                        // whole load, so drop it to None rather than error.
                        selected.storage = storage.ok().flatten();
                    }
                    Ok((Err(error), _, _)) | Ok((_, Err(error), _)) => {
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
            .child(div().text_xs().text_color(theme::text_dim()).child(label))
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
                                        .when(self.connecting.is_none(), |button| {
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
                                        .when(self.connecting.is_none(), |button| {
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
                                        .child(if self.connecting.is_some() {
                                            "Testing nodes..."
                                        } else {
                                            "Save & Connect"
                                        })
                                        .when(self.connecting.is_none(), |button| {
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

    fn open_query_editor(&mut self, cx: &mut Context<Self>) {
        self.show_query_editor = true;
        self.show_fleet = false;
        self.show_ops = false;
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
            persistent_id: zedb_core::new_local_id("tab"),
            saved_tab_id: None,
            name: format!("Tab {id}"),
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
            explain: None,
            advisor: None,
            advise_pending: false,
            advisor_generation: 0,
            failed_sql: None,
            displayed_statement: None,
            displayed_statement_offset: None,
            running_query_id: None,
            tail: None,
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

    /// The environment tier of the active connection (prod/staging/dev).
    fn active_tier(&self) -> Option<EnvTier> {
        let name = self
            .connected
            .as_ref()
            .map(|cluster| cluster.name.as_str())?;
        self.connections
            .iter()
            .find(|connection| connection.name == name)
            .map(|connection| connection.tier)
    }

    /// Left-click on an advice icon. Applying rewrites data, so the policy
    /// is: never apply in place on **production** (open the editor to run
    /// deliberately); on a read-only connection there is nowhere to apply
    /// (open the editor); on writable staging/dev apply in place, but if
    /// the table is large first confirm, since it rewrites a lot of data.
    fn request_apply(
        &mut self,
        index: usize,
        apply: Vec<String>,
        editor_sql: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let is_prod = self.active_tier() == Some(EnvTier::Production);
        let writable = self
            .connected
            .as_ref()
            .map(|cluster| cluster.name.clone())
            .map(|name| self.connection_is_writable(&name))
            .unwrap_or(false);
        if is_prod || !writable {
            self.open_query_tab_with(&editor_sql, window, cx);
            return;
        }
        const LARGE_TABLE_BYTES: u64 = 1_000_000_000; // ~1 GB
        let large = self
            .selected_schema_object
            .as_ref()
            .and_then(|selected| selected.object.total_bytes)
            .is_some_and(|bytes| bytes > LARGE_TABLE_BYTES);
        if large {
            self.pending_apply = Some((index, apply));
            cx.notify();
        } else {
            self.apply_suggestion(index, apply, window, cx);
        }
    }

    fn confirm_apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((index, apply)) = self.pending_apply.take() {
            self.apply_suggestion(index, apply, window, cx);
        }
    }

    fn cancel_apply(&mut self, cx: &mut Context<Self>) {
        self.pending_apply = None;
        cx.notify();
    }

    /// Right-click on an advice icon: open the suggestion in the query
    /// editor. Does nothing on production (per the apply policy).
    fn open_suggestion_in_editor(
        &mut self,
        editor_sql: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.active_tier() == Some(EnvTier::Production) {
            return;
        }
        self.open_query_tab_with(&editor_sql, window, cx);
    }

    /// A small spinning indicator for a slow in-place apply, a rotating
    /// refresh icon (gpui-component's Spinner needs an asset the app does
    /// not serve, so this reuses the whitelisted icon).
    fn advice_spinner() -> impl IntoElement {
        use gpui::{percentage, Animation, AnimationExt as _, Transformation};
        use gpui_component::Sizable as _;
        gpui_component::Icon::empty()
            .path("icons/refresh.svg")
            .with_size(gpui_component::Size::Small)
            .text_color(theme::text_dim())
            .with_animation(
                "advice-spin",
                Animation::new(Duration::from_secs(1)).repeat(),
                |icon, delta| icon.transform(Transformation::rotate(percentage(delta))),
            )
    }

    /// The large-table apply confirmation (Phase 8, Tier 3). Deferred so
    /// it paints above everything, with an occluding backdrop that dims
    /// the window and dismisses on an outside click.
    fn apply_confirm_overlay(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let size = self
            .selected_schema_object
            .as_ref()
            .and_then(|selected| selected.object.total_bytes)
            .map(Self::format_bytes)
            .unwrap_or_default();
        gpui::deferred(
            div()
                .id("apply-confirm")
                .occlude()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::rgba(0x00000088))
                .on_click(cx.listener(|this, _, _, cx| this.cancel_apply(cx)))
                .child(
                    div()
                        .id("apply-dialog")
                        .occlude()
                        .w(px(440.))
                        .p_4()
                        .rounded(px(6.))
                        .bg(theme::bg())
                        .border_1()
                        .border_color(theme::border())
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(div().text_color(theme::text()).child("Apply this change?"))
                        .child(div().text_xs().text_color(theme::text_dim()).child(format!(
                            "This rewrites the whole table (about {size}). It can take a while \
                         and use significant resources. Continue?"
                        )))
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap_2()
                                .child(
                                    div()
                                        .id("apply-cancel")
                                        .px_3()
                                        .py_1()
                                        .rounded(px(4.))
                                        .text_xs()
                                        .text_color(theme::text_dim())
                                        .cursor_pointer()
                                        .hover(|button| {
                                            button.bg(theme::hover()).text_color(theme::text())
                                        })
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.cancel_apply(cx)),
                                        )
                                        .child("Cancel"),
                                )
                                .child(
                                    div()
                                        .id("apply-continue")
                                        .group("apply-continue")
                                        .px_3()
                                        .py_1()
                                        .rounded(px(4.))
                                        .border_1()
                                        .border_color(theme::warning())
                                        .text_xs()
                                        .text_color(theme::warning())
                                        .cursor_pointer()
                                        .hover(|button| {
                                            button
                                                .bg(theme::warning())
                                                .border_color(theme::warning())
                                        })
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.confirm_apply(window, cx)
                                        }))
                                        .child(
                                            div()
                                                .group_hover("apply-continue", |label| {
                                                    label.text_color(rgb(0x14171c))
                                                })
                                                .child("Continue"),
                                        ),
                                ),
                        ),
                ),
        )
    }

    /// Run a suggestion's statements in order on the current connection,
    /// off the main thread, then re-fetch just this table's columns and
    /// storage and update them in place. Updating in place (rather than
    /// re-selecting the object) keeps cardinality/measurement and avoids
    /// flashing the whole pane through a loading state: only the changed
    /// column's numbers and advice repaint.
    fn apply_suggestion(
        &mut self,
        index: usize,
        apply: Vec<String>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(connected) = &self.connected else {
            return;
        };
        let connection_name = connected.name.clone();
        let config = connected.client_config.clone();
        let Some(selected) = &mut self.selected_schema_object else {
            return;
        };
        let database = selected.database.clone();
        let object_name = selected.object.name.clone();
        selected.applying = Some(index);
        selected.applying_slow = false;
        cx.notify();

        // Show a spinner only if the apply runs past this, so quick ones
        // do not flicker.
        cx.spawn(async move |this, cx| {
            Timer::after(Duration::from_secs(3)).await;
            this.update(cx, |this, cx| {
                if let Some(selected) = &mut this.selected_schema_object {
                    if selected.applying == Some(index) {
                        selected.applying_slow = true;
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();

        let task = rt::tokio().spawn({
            let database = database.clone();
            let object_name = object_name.clone();
            async move {
                let client = ChClient::new(config);
                for statement in &apply {
                    client.execute(statement).await?;
                }
                let columns = client.list_columns(&database, &object_name).await?;
                let storage = client
                    .table_storage(&database, &object_name)
                    .await
                    .ok()
                    .flatten();
                Ok::<_, zedb_ch::ChError>((columns, storage))
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
                if let Some(selected) = &mut this.selected_schema_object {
                    selected.applying = None;
                    selected.applying_slow = false;
                }
                match result {
                    Ok(Ok((columns, storage))) => {
                        if let Some(selected) = &mut this.selected_schema_object {
                            if selected.database == database && selected.object.name == object_name
                            {
                                selected.columns = columns;
                                selected.storage = storage;
                            }
                        }
                        this.flash_notice("Applied", cx);
                    }
                    _ => this.flash_warning("Could not apply the change", cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
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

    /// Re-run schema analysis on every open editor against the current
    /// snapshot, refreshing diagnostics. Called when the schema context
    /// changes (node / cluster switch, schema reload) so a stale "unknown
    /// database/table" squiggly clears once the object is known again.
    /// Unlike [`Self::schedule_schema_analysis`] it does not warm column
    /// metadata, so it needs no window; column-level hints refresh on the
    /// next edit.
    fn refresh_schema_diagnostics(&mut self, cx: &mut Context<Self>) {
        let jobs: Vec<(usize, u64, Entity<InputState>)> = self
            .query_tabs
            .iter_mut()
            .map(|tab| {
                tab.schema_analysis_generation += 1;
                (tab.id, tab.schema_analysis_generation, tab.editor.clone())
            })
            .collect();
        let snapshot = self.schema_provider.snapshot();
        for (tab_id, generation, editor) in jobs {
            let Some((snapshot, default_database)) = snapshot.clone() else {
                // No schema context: clear any stale diagnostics outright.
                editor.update(cx, |editor, cx| {
                    if let Some(diagnostics) = editor.diagnostics_mut() {
                        diagnostics.clear();
                    }
                    cx.notify();
                });
                continue;
            };
            let sql = editor.read(cx).value().to_string();
            let task = rt::tokio().spawn(async move {
                let issues = zedb_ch::schema_intelligence::analyze_sql(
                    &snapshot,
                    default_database.as_deref(),
                    &sql,
                );
                (sql, issues)
            });
            cx.spawn(async move |this, cx| {
                let Ok((sql, issues)) = task.await else {
                    return;
                };
                this.update(cx, |this, cx| {
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
        self.stop_tail(tab_id, cx);
        let closed = self.query_tabs.remove(index);
        // A closed tab may hold a huge result; clear it into a
        // background drop before the entity goes away.
        closed.result_grid.update(cx, |grid, _| grid.release_rows());
        drop(closed);
        self.active_query_tab = self
            .active_query_tab
            .min(self.query_tabs.len().saturating_sub(1));
        cx.notify();
    }

    /// Close every tab whose id is in `close_ids`, keeping `focus_id`
    /// active. Running / errored tabs are protected (never closed), and the
    /// focus tab always survives, so at least one tab remains.
    fn close_query_tab_ids(
        &mut self,
        close_ids: &[usize],
        focus_id: usize,
        cx: &mut Context<Self>,
    ) {
        if close_ids.is_empty() {
            return;
        }
        let tail_ids: Vec<usize> = self
            .query_tabs
            .iter()
            .filter(|tab| {
                tab.id != focus_id
                    && close_ids.contains(&tab.id)
                    && !matches!(
                        tab.outcome,
                        QueryOutcome::Running | QueryOutcome::StatementError { .. }
                    )
            })
            .map(|tab| tab.id)
            .collect();
        for tab_id in tail_ids {
            self.stop_tail(tab_id, cx);
        }
        let mut kept = Vec::with_capacity(self.query_tabs.len());
        let mut dropped = Vec::new();
        for tab in self.query_tabs.drain(..) {
            let closable = !matches!(
                tab.outcome,
                QueryOutcome::Running | QueryOutcome::StatementError { .. }
            );
            if tab.id != focus_id && closable && close_ids.contains(&tab.id) {
                dropped.push(tab);
            } else {
                kept.push(tab);
            }
        }
        self.query_tabs = kept;
        // A closed tab may hold a huge result; drop it in the background.
        for tab in &dropped {
            tab.result_grid.update(cx, |grid, _| grid.release_rows());
        }
        drop(dropped);
        self.active_query_tab = self
            .query_tabs
            .iter()
            .position(|tab| tab.id == focus_id)
            .unwrap_or_else(|| {
                self.active_query_tab
                    .min(self.query_tabs.len().saturating_sub(1))
            });
        cx.notify();
    }

    /// Move a query tab from one strip position to another (drag reorder),
    /// keeping the currently-active tab active.
    fn reorder_query_tab(&mut self, from: usize, to: usize, cx: &mut Context<Self>) {
        let len = self.query_tabs.len();
        if from == to || from >= len || to >= len {
            return;
        }
        let active_id = self.query_tabs.get(self.active_query_tab).map(|tab| tab.id);
        let tab = self.query_tabs.remove(from);
        self.query_tabs.insert(to, tab);
        if let Some(id) = active_id {
            if let Some(pos) = self.query_tabs.iter().position(|tab| tab.id == id) {
                self.active_query_tab = pos;
            }
        }
        cx.notify();
    }

    fn close_other_query_tabs(&mut self, keep_id: usize, cx: &mut Context<Self>) {
        let ids: Vec<usize> = self
            .query_tabs
            .iter()
            .filter(|tab| tab.id != keep_id)
            .map(|tab| tab.id)
            .collect();
        self.close_query_tab_ids(&ids, keep_id, cx);
    }

    fn close_query_tabs_to_right(&mut self, from_id: usize, cx: &mut Context<Self>) {
        let Some(pos) = self.query_tabs.iter().position(|tab| tab.id == from_id) else {
            return;
        };
        let ids: Vec<usize> = self.query_tabs[pos + 1..]
            .iter()
            .map(|tab| tab.id)
            .collect();
        self.close_query_tab_ids(&ids, from_id, cx);
    }

    /// Begin a live tail of a table (Phase 10): open a fresh tab, resolve
    /// the monotonic key (the table's leading ORDER BY column), and start
    /// polling `WHERE key > :last` off the main thread.
    fn start_tail(
        &mut self,
        database: String,
        object: String,
        cap: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(connected) = self.connected.as_ref() else {
            self.flash_warning("Connect before tailing a table", cx);
            return;
        };
        let config = connected.client_config.clone();
        let connection_name = connected.name.clone();

        // A dedicated tab hosts the tail so it never fights a real query.
        self.add_query_tab(window, cx);
        let tab_id = self.query_tabs[self.active_query_tab].id;

        self.next_tail_generation += 1;
        let generation = self.next_tail_generation;
        self.next_tail_number += 1;
        let number = self.next_tail_number;
        let qualified = format!("{database}.{object}");
        let task = rt::tokio().spawn(async move {
            let client = ChClient::new(config);
            fetch_table_keys(&client, Some(&qualified))
                .await
                .and_then(|(order_by, _)| order_by.into_iter().next())
                .and_then(|first| first_tail_key(&first))
        });
        cx.spawn_in(window, async move |this, cx| {
            let key = task.await.ok().flatten();
            this.update_in(cx, |this, window, cx| {
                let Some(key) = key else {
                    this.flash_warning(
                        format!("{database}.{object} has no simple ORDER BY key to tail on"),
                        cx,
                    );
                    // Leave the empty tab in place; the user can close it.
                    return;
                };
                if let Some(tab) = this.query_tabs.iter_mut().find(|tab| tab.id == tab_id) {
                    let query = tail::TailQuery {
                        body: tail::table_body(&database, &object),
                        key,
                        limit: tail::TAIL_BATCH,
                    };
                    // Show the runnable base query in the tab editor; editing
                    // it and pressing "update tail" re-parses it.
                    let baseline = tail::base_sql(&query);
                    let editor = tab.editor.clone();
                    let display = baseline.clone();
                    editor.update(cx, |editor, cx| editor.set_value(display, window, cx));
                    tab.tail = Some(TailState {
                        number,
                        query,
                        baseline,
                        last: None,
                        key_index: 0,
                        cap,
                        native_available: None,
                        push: TailPush::Poll,
                        stream_cursor: None,
                        stream: None,
                        watch: None,
                        stream_rejected: false,
                        generation,
                        paused: false,
                        error: None,
                    });
                    tab.has_result = true;
                }
                // One immediate poll to prime, then the timer loop.
                this.tail_poll_once(tab_id, generation, connection_name.clone(), cx);
                this.start_tail_loop(tab_id, generation, connection_name.clone(), cx);
                // Discover whether a native (TCP) port is reachable, to
                // offer the "instant updates" upgrade only when possible.
                this.probe_native_push(tab_id, generation, connection_name, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The timer loop: every cadence, while the tab still hosts this tail
    /// generation on the same connection and isn't paused, run one poll.
    /// The cadence follows the delivery mode ([`TailPush`]): fast over a
    /// live native connection, baseline otherwise; while a direct stream
    /// drives the tail, the timer only idles as its watchdog.
    fn start_tail_loop(
        &mut self,
        tab_id: usize,
        generation: u64,
        connection_name: String,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            let mut interval = tail::TAIL_INTERVAL_MS;
            loop {
                Timer::after(Duration::from_millis(interval)).await;
                let alive = this
                    .update(cx, |this, cx| {
                        let on_connection = this.connected.as_ref().map(|c| c.name.as_str())
                            == Some(connection_name.as_str());
                        let config = this.connected.as_ref().map(|c| c.client_config.clone());
                        let mut lost_native = false;
                        let (live, paused, push) = {
                            let state = this
                                .query_tabs
                                .iter_mut()
                                .find(|tab| tab.id == tab_id)
                                .and_then(|tab| tab.tail.as_mut());
                            match state {
                                Some(state) if on_connection && state.generation == generation => {
                                    // Fast mode needs the native connection;
                                    // when it is gone the polls are silently
                                    // riding HTTP already, so drop back to
                                    // the HTTP cadence and re-offer the
                                    // upgrade once the port answers again.
                                    if state.push == TailPush::Fast {
                                        let native_up = config.as_ref().is_some_and(|config| {
                                            zedb_ch::native::pooled(config).is_some()
                                        });
                                        if !native_up {
                                            state.push = TailPush::Poll;
                                            state.native_available = None;
                                            lost_native = true;
                                        }
                                    }
                                    (true, state.paused, state.push)
                                }
                                _ => (false, true, TailPush::Poll),
                            }
                        };
                        if !live {
                            return None;
                        }
                        if lost_native {
                            this.flash_notice(
                                "Native connection lost; tail back to HTTP polling",
                                cx,
                            );
                            this.probe_native_push(tab_id, generation, connection_name.clone(), cx);
                        }
                        if !paused && !matches!(push, TailPush::Stream | TailPush::Watch) {
                            this.tail_poll_once(tab_id, generation, connection_name.clone(), cx);
                        }
                        Some(match push {
                            TailPush::Fast => tail::TAIL_INTERVAL_FAST_MS,
                            _ => tail::TAIL_INTERVAL_MS,
                        })
                    })
                    .ok()
                    .flatten();
                match alive {
                    Some(next) => interval = next,
                    None => break,
                }
            }
        })
        .detach();
    }

    /// One off-thread poll: `seed_sql` while unprimed (grab the newest rows
    /// and install the header), then `poll_sql` for everything after the
    /// last seen key. New rows append and follow the bottom.
    fn tail_poll_once(
        &mut self,
        tab_id: usize,
        generation: u64,
        connection_name: String,
        cx: &mut Context<Self>,
    ) {
        let (config, sql, key) = {
            let Some(connected) = self.connected.as_ref() else {
                return;
            };
            if connected.name != connection_name {
                return;
            }
            let Some(state) = self
                .query_tabs
                .iter()
                .find(|tab| tab.id == tab_id)
                .and_then(|tab| tab.tail.as_ref())
            else {
                return;
            };
            if state.generation != generation {
                return;
            }
            let sql = match &state.last {
                None => tail::seed_sql(&state.query, tail::TAIL_SEED),
                Some(last) => tail::poll_sql(&state.query, last, state.query.limit),
            };
            (
                connected.client_config.clone(),
                sql,
                state.query.key.clone(),
            )
        };
        let priming = self
            .query_tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .and_then(|tab| tab.tail.as_ref())
            .map(|state| state.last.is_none())
            .unwrap_or(false);
        let cap = self
            .query_tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .and_then(|tab| tab.tail.as_ref())
            .and_then(|state| state.cap)
            .unwrap_or(usize::MAX);
        let Some(grid) = self
            .query_tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .map(|tab| tab.result_grid.clone())
        else {
            return;
        };

        let task = rt::tokio().spawn(async move { ChClient::new(config).query(&sql).await });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                let mut batch: Option<TailBatch> = None;
                {
                    let Some(state) = this
                        .query_tabs
                        .iter_mut()
                        .find(|tab| tab.id == tab_id)
                        .and_then(|tab| tab.tail.as_mut())
                    else {
                        return;
                    };
                    if state.generation != generation {
                        return;
                    }
                    match result {
                        Ok(Ok(res)) => {
                            state.error = None;
                            if !res.rows.is_empty() {
                                let Some(idx) =
                                    res.columns.iter().position(|column| column.name == key)
                                else {
                                    state.error =
                                        Some(format!("tail key `{key}` is not in the result"));
                                    cx.notify();
                                    return;
                                };
                                state.key_index = idx;
                                if let Some(next) = tail::last_key(&res.rows, idx) {
                                    state.last = Some(next);
                                }
                                let columns = priming.then(|| res.columns.clone());
                                batch = Some((columns, res.rows));
                            }
                        }
                        Ok(Err(error)) => state.error = Some(error.to_string()),
                        Err(error) => state.error = Some(error.to_string()),
                    }
                }
                if let Some((columns, rows)) = batch {
                    let columns_len = columns.as_ref().map(|columns| columns.len());
                    if let Some(columns) = columns {
                        grid.update(cx, |grid, cx| grid.begin_result(columns, None, cx));
                    }
                    grid.update(cx, |grid, cx| grid.prepend_tail(rows, cap, cx));
                    let count = grid.read(cx).row_count();
                    if let Some(tab) = this.query_tabs.iter_mut().find(|tab| tab.id == tab_id) {
                        tab.result_rows = count;
                        tab.has_result = true;
                        if let Some(len) = columns_len {
                            tab.result_columns = len;
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Stop the tail on a tab (its loop notices the cleared/renumbered
    /// generation and exits). The tab and its rows stay.
    fn stop_tail(&mut self, tab_id: usize, cx: &mut Context<Self>) {
        let config = self.connected.as_ref().map(|c| c.client_config.clone());
        if let Some(tab) = self.query_tabs.iter_mut().find(|tab| tab.id == tab_id) {
            if let Some(stream) = tab.tail.as_mut().and_then(|state| state.stream.take()) {
                stream.abort.abort();
            }
            if let (Some(config), Some(watch)) = (
                config,
                tab.tail.as_mut().and_then(|state| state.watch.take()),
            ) {
                drop_tail_view(config, watch.view.clone());
            }
            tab.tail = None;
            cx.notify();
        }
    }

    fn toggle_tail_pause(&mut self, tab_id: usize, cx: &mut Context<Self>) {
        let mut resume_stream = false;
        let connection_name = self.connected.as_ref().map(|c| c.name.clone());
        let mut resume_watch_poll = None;
        if let Some(state) = self
            .query_tabs
            .iter_mut()
            .find(|tab| tab.id == tab_id)
            .and_then(|tab| tab.tail.as_mut())
        {
            state.paused = !state.paused;
            if state.push == TailPush::Stream {
                if state.paused {
                    // Do not buffer an unbounded result while paused. Closing
                    // the dedicated connection leaves the saved cursor in
                    // place for an exact resume.
                    if let Some(stream) = state.stream.take() {
                        stream.abort.abort();
                    }
                } else if state.stream.is_none() {
                    state.push = TailPush::Poll;
                    resume_stream = true;
                }
            } else if !state.paused && state.push == TailPush::Watch {
                resume_watch_poll = Some(state.generation);
            }
            cx.notify();
        }
        if resume_stream {
            self.upgrade_tail_instant(tab_id, cx);
        }
        if let (Some(generation), Some(connection_name)) = (resume_watch_poll, connection_name) {
            self.tail_poll_once(tab_id, generation, connection_name, cx);
        }
    }

    /// Adopt the tab editor's edited query as the tail's new definition. The
    /// edit is validated by running its seed once; if that errors (or the
    /// text isn't a tailable `SELECT ... FROM ... ORDER BY key`), the tail is
    /// left running unchanged and the reason is flashed.
    fn update_tail_from_editor(&mut self, tab_id: usize, cx: &mut Context<Self>) {
        let Some(connection_name) = self.connected.as_ref().map(|c| c.name.clone()) else {
            return;
        };
        let config = self.connected.as_ref().map(|c| c.client_config.clone());
        let (Some(config), Some(tab)) =
            (config, self.query_tabs.iter().find(|tab| tab.id == tab_id))
        else {
            return;
        };
        let Some(state) = tab.tail.as_ref() else {
            return;
        };
        let generation = state.generation;
        let edited = tab.editor.read(cx).value().to_string();
        let Some(parsed) = tail::parse_tail_query(&edited, tail::TAIL_BATCH) else {
            self.flash_warning(
                "That isn't a tailable query (need SELECT … FROM db.table … ORDER BY key); tail unchanged",
                cx,
            );
            return;
        };

        // Validate by running the seed once before switching over.
        let probe_sql = tail::seed_sql(&parsed, tail::TAIL_SEED);
        let task = rt::tokio().spawn(async move { ChClient::new(config).query(&probe_sql).await });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if this.connected.as_ref().map(|c| c.name.as_str())
                    != Some(connection_name.as_str())
                {
                    return;
                }
                match result {
                    // The probe IS the seed, so its rows are the newest ones:
                    // adopt the query and repaint the grid from them right
                    // now, without waiting for the next poll or new inserts.
                    Ok(Ok(res)) => {
                        let grid = this
                            .query_tabs
                            .iter()
                            .find(|tab| tab.id == tab_id)
                            .map(|tab| tab.result_grid.clone());
                        let cap = this
                            .query_tabs
                            .iter()
                            .find(|tab| tab.id == tab_id)
                            .and_then(|tab| tab.tail.as_ref())
                            .and_then(|state| state.cap)
                            .unwrap_or(usize::MAX);
                        let key_index = res
                            .columns
                            .iter()
                            .position(|column| column.name == parsed.key);
                        // The cursor needs the ORDER BY key in the output;
                        // if the projection dropped it, keep the old tail.
                        if key_index.is_none() {
                            this.flash_warning(
                                format!(
                                    "The ORDER BY key `{}` must be in the SELECT to tail; tail unchanged",
                                    parsed.key
                                ),
                                cx,
                            );
                            cx.notify();
                            return;
                        }
                        let Some(state) = this
                            .query_tabs
                            .iter_mut()
                            .find(|tab| tab.id == tab_id)
                            .and_then(|tab| tab.tail.as_mut())
                        else {
                            return;
                        };
                        if state.generation != generation {
                            return;
                        }
                        state.query = parsed.clone();
                        state.baseline = edited.clone();
                        state.error = None;
                        state.key_index = key_index.unwrap_or(0);
                        state.last = key_index.and_then(|idx| tail::last_key(&res.rows, idx));
                        // A stream is bound to the old body and cursor. Stop
                        // it and re-negotiate against the edited query.
                        let stale_stream = state.stream.take();
                        let stale_watch = state.watch.take();
                        state.stream_cursor = None;
                        state.stream_rejected = false;
                        if stale_stream.is_some() || stale_watch.is_some() {
                            state.push = TailPush::Poll;
                        }

                        if let Some(grid) = grid {
                            let columns = res.columns.clone();
                            let columns_len = columns.len();
                            let rows = res.rows;
                            grid.update(cx, |grid, cx| grid.clear_rows(cx));
                            if !rows.is_empty() {
                                grid.update(cx, |grid, cx| grid.begin_result(columns, None, cx));
                                grid.update(cx, |grid, cx| grid.prepend_tail(rows, cap, cx));
                            }
                            let count = grid.read(cx).row_count();
                            if let Some(tab) =
                                this.query_tabs.iter_mut().find(|tab| tab.id == tab_id)
                            {
                                tab.result_rows = count;
                                tab.has_result = true;
                                tab.result_columns = columns_len;
                            }
                        }
                        if let Some(stream) = stale_stream {
                            stream.abort.abort();
                            this.upgrade_tail_instant(tab_id, cx);
                        }
                        if let Some(watch) = stale_watch {
                            if let Some(config) =
                                this.connected.as_ref().map(|c| c.client_config.clone())
                            {
                                drop_tail_view(config, watch.view.clone());
                            }
                            this.upgrade_tail_instant(tab_id, cx);
                        }
                        this.flash_notice("Tail updated", cx);
                    }
                    Ok(Err(error)) => {
                        this.flash_warning(format!("Query failed, tail unchanged: {error}"), cx);
                    }
                    Err(error) => {
                        this.flash_warning(format!("Query failed, tail unchanged: {error}"), cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Discover whether this connection's server is reachable over the
    /// native (TCP) protocol, by actually establishing the pooled native
    /// connection: the server names its own ports, the socket is proven
    /// to be the same server, and general reads start riding it right
    /// away. Success surfaces the "instant updates" button; poll-over-HTTP
    /// tail works everywhere regardless.
    fn probe_native_push(
        &mut self,
        tab_id: usize,
        generation: u64,
        connection_name: String,
        cx: &mut Context<Self>,
    ) {
        let Some(config) = self
            .connected
            .as_ref()
            .map(|connected| connected.client_config.clone())
        else {
            return;
        };
        let task = rt::tokio().spawn(async move { zedb_ch::native::connect_pooled(&config).await });
        cx.spawn(async move |this, cx| {
            let reachable = task.await.is_ok_and(|connected| connected.is_ok());
            this.update(cx, |this, cx| {
                if this.connected.as_ref().map(|c| c.name.as_str())
                    != Some(connection_name.as_str())
                {
                    return;
                }
                if let Some(state) = this
                    .query_tabs
                    .iter_mut()
                    .find(|tab| tab.id == tab_id)
                    .and_then(|tab| tab.tail.as_mut())
                {
                    if state.generation == generation {
                        state.native_available = Some(reachable);
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    /// Switch a tail to "instant updates" over native TCP. Experimental
    /// STREAM is opt-in; WATCH remains the normal push path on versions that
    /// support Live Views, followed by native fast polling.
    fn upgrade_tail_instant(&mut self, tab_id: usize, cx: &mut Context<Self>) {
        let Some(connected) = self.connected.as_ref() else {
            return;
        };
        let config = connected.client_config.clone();
        let connection_name = connected.name.clone();
        let Some(state) = self
            .query_tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .and_then(|tab| tab.tail.as_ref())
        else {
            return;
        };
        if state.push != TailPush::Poll {
            return;
        }
        let generation = state.generation;
        let body = state.query.body.clone();
        let stream_sql = self
            .preferences
            .experimental_streaming_queries
            .then(|| tail::stream_sql(&state.query, state.stream_cursor, state.last.as_deref()))
            .flatten()
            .filter(|_| !state.stream_rejected);
        let stream_requested = stream_sql.is_some();
        self.next_stream_epoch += 1;
        let epoch = self.next_stream_epoch;
        let view = format!("zedb_tail_{epoch}");
        self.flash_notice("Connecting for instant updates…", cx);

        enum Instant {
            Stream {
                receiver: tokio::sync::mpsc::UnboundedReceiver<TailStreamBatch>,
                abort: tokio::task::AbortHandle,
            },
            Watch {
                receiver: tokio::sync::mpsc::UnboundedReceiver<()>,
                abort: tokio::task::AbortHandle,
                stream_rejected: bool,
            },
            FastPoll {
                stream_rejected: bool,
            },
        }
        let read_only = config.read_only;
        let setup_config = config.clone();
        let setup_view = view.clone();
        let task = rt::tokio().spawn(async move {
            let pooled = zedb_ch::native::connect_pooled(&setup_config).await?;
            let mut stream_rejected = false;
            if let Some(stream_sql) = stream_sql {
                let version = pooled
                    .query("SELECT version()")
                    .await
                    .ok()
                    .and_then(|result| result.rows.first().cloned())
                    .and_then(|row| row.first().map(ToString::to_string));
                if version
                    .as_deref()
                    .is_some_and(tail::supports_streaming_version)
                {
                    if let Ok(streamer) =
                        zedb_ch::native::NativeClient::connect(&setup_config).await
                    {
                        let preflight = streamer
                            .query(&format!("EXPLAIN SYNTAX {stream_sql}"))
                            .await;
                        if preflight.is_ok() {
                            let (sender, receiver) =
                                tokio::sync::mpsc::unbounded_channel::<TailStreamBatch>();
                            let stream_task = tokio::spawn(async move {
                                let _ = streamer
                                    .stream_blocks(&stream_sql, |columns, rows| {
                                        sender.send((columns, rows)).is_ok()
                                    })
                                    .await;
                            });
                            let abort = stream_task.abort_handle();
                            return Ok::<Instant, zedb_ch::ChError>(Instant::Stream {
                                receiver,
                                abort,
                            });
                        }
                    }
                }
                stream_rejected = true;
            }
            if read_only {
                return Ok(Instant::FastPoll { stream_rejected });
            }
            // The WATCH holds its own connection open indefinitely: the
            // native protocol runs one query at a time per connection, so
            // it must never share the pooled one.
            let Ok(watcher) = zedb_ch::native::NativeClient::connect(&setup_config).await else {
                return Ok(Instant::FastPoll { stream_rejected });
            };
            let experimental = watcher
                .execute("SET allow_experimental_live_view = 1")
                .await;
            let created = match experimental {
                Ok(()) => {
                    watcher
                        .execute(&format!(
                            "CREATE LIVE VIEW {setup_view} AS SELECT count() FROM ({body})"
                        ))
                        .await
                }
                Err(error) => Err(error),
            };
            if created.is_err() {
                // Live views are experimental and semi-deprecated; any
                // refusal (setting locked, feature removed, no grant)
                // lands here and fast polling takes over.
                return Ok(Instant::FastPoll { stream_rejected });
            }
            let (sender, receiver) = tokio::sync::mpsc::unbounded_channel::<()>();
            let watch_task = tokio::spawn(async move {
                // Runs until the server ends the stream, the connection
                // drops, or the consumer goes away (send fails).
                let _ = watcher
                    .stream_blocks(&format!("WATCH {setup_view} EVENTS"), |_, _| {
                        sender.send(()).is_ok()
                    })
                    .await;
            });
            let abort = watch_task.abort_handle();
            Ok::<Instant, zedb_ch::ChError>(Instant::Watch {
                receiver,
                abort,
                stream_rejected,
            })
        });
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            this.update(cx, |this, cx| {
                let alive = this.connected.as_ref().map(|c| c.name.as_str())
                    == Some(connection_name.as_str());
                let Some(state) = this
                    .query_tabs
                    .iter_mut()
                    .find(|tab| tab.id == tab_id)
                    .and_then(|tab| tab.tail.as_mut())
                    .filter(|state| alive && state.generation == generation)
                else {
                    // The tail is gone; if a watch got set up, tear its
                    // view down (the stream ends when the receiver drops).
                    if let Ok(Ok(Instant::Watch { abort, .. })) = &outcome {
                        abort.abort();
                        drop_tail_view(config.clone(), view.clone());
                    }
                    if let Ok(Ok(Instant::Stream { abort, .. })) = &outcome {
                        abort.abort();
                    }
                    return;
                };
                match outcome {
                    Ok(Ok(Instant::Stream { receiver, abort })) => {
                        state.push = TailPush::Stream;
                        state.stream = Some(TailStream { epoch, abort });
                        this.flash_notice("Instant updates on: experimental STREAM over TCP", cx);
                        this.start_tail_stream_consumer(
                            tab_id,
                            generation,
                            epoch,
                            connection_name.clone(),
                            receiver,
                            cx,
                        );
                    }
                    Ok(Ok(Instant::Watch {
                        receiver,
                        abort,
                        stream_rejected,
                    })) => {
                        state.stream_rejected |= stream_rejected;
                        state.push = TailPush::Watch;
                        state.watch = Some(TailWatch {
                            view: view.clone(),
                            epoch,
                            abort,
                        });
                        this.flash_notice("Instant updates on: server push over TCP", cx);
                        this.start_tail_watch_consumer(
                            tab_id,
                            generation,
                            epoch,
                            connection_name.clone(),
                            receiver,
                            cx,
                        );
                    }
                    Ok(Ok(Instant::FastPoll { stream_rejected })) => {
                        state.stream_rejected |= stream_rejected;
                        state.push = TailPush::Fast;
                        this.flash_notice(
                            if stream_requested && stream_rejected {
                                "STREAM unavailable; using fast native polling"
                            } else {
                                "Instant updates on: fast polling over the native connection"
                            },
                            cx,
                        );
                    }
                    Ok(Err(error)) => {
                        state.native_available = Some(false);
                        this.flash_warning(
                            format!("Couldn't connect to the native port: {error}"),
                            cx,
                        );
                    }
                    Err(error) => {
                        state.native_available = Some(false);
                        this.flash_warning(format!("Instant updates failed: {error}"), cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Consume rows returned directly by ClickHouse `STREAM CURSOR`. The two
    /// private leading columns advance the resumable server cursor and are
    /// removed before rows reach the grid.
    fn start_tail_stream_consumer(
        &mut self,
        tab_id: usize,
        generation: u64,
        epoch: u64,
        connection_name: String,
        mut receiver: tokio::sync::mpsc::UnboundedReceiver<TailStreamBatch>,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            while let Some((mut columns, mut rows)) = receiver.recv().await {
                let alive = this
                    .update(cx, |this, cx| {
                        let on_connection = this.connected.as_ref().map(|c| c.name.as_str())
                            == Some(connection_name.as_str());
                        let Some(state) = this
                            .query_tabs
                            .iter_mut()
                            .find(|tab| tab.id == tab_id)
                            .and_then(|tab| tab.tail.as_mut())
                            .filter(|state| {
                                on_connection
                                    && state.generation == generation
                                    && state
                                        .stream
                                        .as_ref()
                                        .is_some_and(|stream| stream.epoch == epoch)
                            })
                        else {
                            return false;
                        };
                        if columns.len() < 2
                            || columns[0].name != tail::STREAM_BLOCK_COLUMN
                            || columns[1].name != tail::STREAM_OFFSET_COLUMN
                        {
                            state.error = Some("STREAM did not return a resumable cursor".into());
                            return false;
                        }
                        if let Some(cursor) = rows.last().and_then(|row| tail::stream_cursor(row)) {
                            state.stream_cursor = Some(cursor);
                        }
                        columns.drain(..2);
                        for row in &mut rows {
                            if row.len() >= 2 {
                                row.drain(..2);
                            }
                        }
                        let Some(key_index) = columns
                            .iter()
                            .position(|column| column.name == state.query.key)
                        else {
                            state.error = Some(format!(
                                "tail key `{}` is not in the streamed result",
                                state.query.key
                            ));
                            return false;
                        };
                        state.key_index = key_index;
                        if let Some(last) = tail::last_key(&rows, key_index) {
                            state.last = Some(last);
                        }
                        state.error = None;
                        let cap = state.cap.unwrap_or(usize::MAX);
                        let grid = this
                            .query_tabs
                            .iter()
                            .find(|tab| tab.id == tab_id)
                            .map(|tab| tab.result_grid.clone());
                        if let Some(grid) = grid {
                            grid.update(cx, |grid, cx| grid.prepend_tail(rows, cap, cx));
                            let count = grid.read(cx).row_count();
                            if let Some(tab) =
                                this.query_tabs.iter_mut().find(|tab| tab.id == tab_id)
                            {
                                tab.result_rows = count;
                                tab.has_result = true;
                                tab.result_columns = columns.len();
                            }
                        }
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !alive {
                    break;
                }
            }

            this.update(cx, |this, cx| {
                let on_connection = this.connected.as_ref().map(|c| c.name.as_str())
                    == Some(connection_name.as_str());
                let ended = this
                    .query_tabs
                    .iter_mut()
                    .find(|tab| tab.id == tab_id)
                    .and_then(|tab| tab.tail.as_mut())
                    .filter(|state| {
                        on_connection
                            && state.generation == generation
                            && state
                                .stream
                                .as_ref()
                                .is_some_and(|stream| stream.epoch == epoch)
                    })
                    .map(|state| {
                        state.stream = None;
                        state.stream_rejected = true;
                        state.push = TailPush::Poll;
                    })
                    .is_some();
                if ended {
                    this.flash_notice("Experimental STREAM ended; trying WATCH", cx);
                    this.upgrade_tail_instant(tab_id, cx);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Consume one watch's events: each server-pushed event triggers one
    /// poll (which rides the pooled native connection). When the stream
    /// ends, the tail drops back to plain polling and the upgrade is
    /// re-offered once the native port answers again.
    fn start_tail_watch_consumer(
        &mut self,
        tab_id: usize,
        generation: u64,
        epoch: u64,
        connection_name: String,
        mut receiver: tokio::sync::mpsc::UnboundedReceiver<()>,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            loop {
                let event = receiver.recv().await;
                // Coalesce a burst of events into one poll.
                while receiver.try_recv().is_ok() {}
                let ended = event.is_none();
                let alive = this
                    .update(cx, |this, cx| {
                        let on_connection = this.connected.as_ref().map(|c| c.name.as_str())
                            == Some(connection_name.as_str());
                        let config = this.connected.as_ref().map(|c| c.client_config.clone());
                        let mut downgraded = false;
                        let (live, paused) = {
                            let state = this
                                .query_tabs
                                .iter_mut()
                                .find(|tab| tab.id == tab_id)
                                .and_then(|tab| tab.tail.as_mut());
                            match state {
                                Some(state)
                                    if on_connection
                                        && state.generation == generation
                                        && state
                                            .watch
                                            .as_ref()
                                            .is_some_and(|watch| watch.epoch == epoch) =>
                                {
                                    if ended {
                                        // Server-push ended (connection
                                        // drop, live view gone): back to
                                        // polling, silently resuming from
                                        // the last-seen key.
                                        let view =
                                            state.watch.take().map(|watch| watch.view.clone());
                                        state.push = TailPush::Poll;
                                        state.native_available = None;
                                        downgraded = true;
                                        if let (Some(config), Some(view)) = (config, view) {
                                            drop_tail_view(config, view);
                                        }
                                    }
                                    (true, state.paused)
                                }
                                _ => (false, true),
                            }
                        };
                        if downgraded {
                            this.flash_notice("Instant updates ended; tail back to polling", cx);
                            this.probe_native_push(tab_id, generation, connection_name.clone(), cx);
                            cx.notify();
                        }
                        if live && !ended && !paused {
                            this.tail_poll_once(tab_id, generation, connection_name.clone(), cx);
                        }
                        live
                    })
                    .unwrap_or(false);
                if ended || !alive {
                    break;
                }
            }
        })
        .detach();
    }

    /// The live-tail status strip above the editor: what's tailing, the
    /// retained row count, and Pause / Stop.
    fn tail_strip(&self, info: TailStripInfo, cx: &mut Context<Self>) -> impl IntoElement {
        let TailStripInfo {
            tab_id,
            key,
            paused,
            error,
            rows,
            native_available,
            push,
            experimental_streaming_enabled,
            dirty,
        } = info;
        let icon_button = |id: &'static str, icon: &'static str, color: gpui::Hsla| {
            div()
                .id(id)
                .size(px(22.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(3.))
                .hover(|button| button.bg(theme::hover()).cursor_pointer())
                .child(svg().path(icon).size(px(13.)).text_color(color))
        };
        div()
            .flex_none()
            .h(px(30.))
            .px_3()
            .flex()
            .items_center()
            .gap_2()
            .bg(theme::bg_sidebar())
            // An orange outline on the whole strip while the query is edited
            // (unapplied), alongside the green Update Tail button.
            .when(dirty, |strip| {
                strip.border_1().border_color(theme::warning())
            })
            .child(
                // A live dot: accent when following, dim when paused.
                div().size(px(7.)).rounded_full().bg(if paused {
                    theme::text_dim()
                } else {
                    theme::accent()
                }),
            )
            .child(div().text_xs().text_color(theme::text()).child(if paused {
                format!("Tail paused · advancing on {key}")
            } else {
                format!("Tailing · advancing on {key}")
            }))
            .child(
                div()
                    .text_xs()
                    .text_color(theme::text_dim())
                    .child(format!("· {rows} rows")),
            )
            .when(push != TailPush::Poll, |row| {
                // Instant updates active: name the mechanism.
                row.child(
                    div()
                        .text_xs()
                        .text_color(theme::accent())
                        .child(match push {
                            TailPush::Stream => "· instant (STREAM)",
                            TailPush::Watch => "· instant (WATCH)",
                            _ => "· instant (native)",
                        }),
                )
            })
            .when_some(error, |row, error| {
                row.child(
                    div()
                        .text_xs()
                        .text_color(theme::danger())
                        .child(format!("· {error}")),
                )
            })
            .child(div().flex_1())
            .when(dirty, |row| {
                // The editor query was edited; a green-outlined text button
                // (left of "Get instant updates") that reads as "apply your
                // changes".
                row.child(
                    div()
                        .id("tail-update")
                        .px_2()
                        .py_0p5()
                        .rounded(px(3.))
                        .border_1()
                        .border_color(theme::success())
                        .text_xs()
                        .text_color(theme::success())
                        .child("Update Tail")
                        .hover(|button| button.bg(theme::hover()).cursor_pointer())
                        .tooltip(|window, cx| {
                            gpui_component::tooltip::Tooltip::new(
                                "Apply the edited query to the tail",
                            )
                            .build(window, cx)
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.update_tail_from_editor(tab_id, cx)
                        })),
                )
            })
            .when(native_available && push == TailPush::Poll, |row| {
                row.child(
                    icon_button(
                        "tail-experimental-settings",
                        "icons/experimental.svg",
                        if experimental_streaming_enabled {
                            theme::warning()
                        } else {
                            theme::text_dim()
                        },
                    )
                    .tooltip(move |window, cx| {
                        gpui_component::tooltip::Tooltip::new(if experimental_streaming_enabled {
                            "Experimental STREAM tails enabled. Open Preferences"
                        } else {
                            "Experimental STREAM tails disabled. Open Preferences"
                        })
                        .build(window, cx)
                    })
                    .on_click(cx.listener(|this, _, _, cx| this.open_preferences(cx))),
                )
            })
            .when(native_available && push == TailPush::Poll, |row| {
                // Discovery found a native port: offer the server-push
                // upgrade, accent-tinted so it reads as an offer.
                row.child(
                    div()
                        .id("tail-instant")
                        .px_2()
                        .py_0p5()
                        .rounded(px(3.))
                        .border_1()
                        .border_color(theme::accent())
                        .text_xs()
                        .text_color(theme::accent())
                        .child("Get instant updates")
                        .hover(|button| button.bg(theme::hover()).cursor_pointer())
                        .tooltip(|window, cx| {
                            gpui_component::tooltip::Tooltip::new(
                                "Switch to the native (TCP) connection for instant updates",
                            )
                            .build(window, cx)
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.upgrade_tail_instant(tab_id, cx);
                        })),
                )
            })
            .child(
                // Paused shows green Play (resume); running shows orange
                // Pause. Stop is always red.
                if paused {
                    icon_button("tail-play", "icons/play.svg", theme::success()).tooltip(
                        |window, cx| {
                            gpui_component::tooltip::Tooltip::new("Resume").build(window, cx)
                        },
                    )
                } else {
                    icon_button("tail-pause", "icons/pause.svg", theme::warning()).tooltip(
                        |window, cx| {
                            gpui_component::tooltip::Tooltip::new("Pause").build(window, cx)
                        },
                    )
                }
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_tail_pause(tab_id, cx))),
            )
            .child(
                icon_button("tail-stop", "icons/stop.svg", theme::danger())
                    .tooltip(|window, cx| {
                        gpui_component::tooltip::Tooltip::new("Stop").build(window, cx)
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.stop_tail(tab_id, cx))),
            )
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

    /// A neutral, self-clearing status message (e.g. "Applied").
    fn flash_notice(&mut self, message: impl Into<String>, cx: &mut Context<Self>) {
        self.notice = Some(message.into());
        self.notice_warning = false;
        self.notice_flash_id += 1;
        let flash_id = self.notice_flash_id;
        cx.spawn(async move |this, cx| {
            Timer::after(Duration::from_secs(2)).await;
            this.update(cx, |this, cx| {
                if this.notice_flash_id == flash_id {
                    this.notice = None;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
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
        // Run means run: a press during an in-flight query cancels it
        // and starts this one, instead of being silently swallowed.
        if self.query_abort.is_some() {
            self.cancel_query(cx);
        }
        let raw_sql = self.run_target_sql(window, cx);
        let full_text = self
            .query_tabs
            .get(self.active_query_tab)
            .map(|tab| tab.editor.read(cx).value().to_string())
            .unwrap_or_default();
        let sql = match resolve_query_variables(&raw_sql, &full_text) {
            Ok(sql) => sql,
            Err(error) => {
                self.flash_warning(error, cx);
                return;
            }
        };
        let offset = if sql.trim() == raw_sql.trim() {
            self.query_tabs.get(self.active_query_tab).and_then(|tab| {
                let editor = tab.editor.read(cx);
                nearest_occurrence(editor.value().as_ref(), raw_sql.trim(), editor.cursor())
            })
        } else {
            None
        };
        self.start_statements(vec![(sql.trim().to_string(), offset)], cx);
    }

    /// Run every statement in the selection (or the whole buffer when nothing
    /// is selected) one after another.
    fn run_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.query_abort.is_some() {
            self.cancel_query(cx);
        }
        let selection = self.selected_text(window, cx);
        let (full_text, cursor) = self
            .query_tabs
            .get(self.active_query_tab)
            .map(|tab| {
                let editor = tab.editor.read(cx);
                (editor.value().to_string(), editor.cursor())
            })
            .unwrap_or_default();
        // Offsets are absolute editor positions; a selection anchors its
        // relative offsets at the occurrence nearest the cursor.
        let (raw_text, base) = match selection {
            Some(selection) => {
                let base = nearest_occurrence(&full_text, &selection, cursor);
                (selection, base)
            }
            None => (full_text.clone(), Some(0)),
        };
        let text = match resolve_query_variables(&raw_text, &full_text) {
            Ok(text) => text,
            Err(error) => {
                self.flash_warning(error, cx);
                return;
            }
        };
        let transformed = text != raw_text;
        let statements = split_statements(&text)
            .into_iter()
            .filter_map(|(start, end)| {
                let raw = &text[start..end.min(text.len())];
                let statement = raw.trim();
                if statement.is_empty() {
                    return None;
                }
                let leading = raw.len() - raw.trim_start().len();
                let offset = if transformed {
                    None
                } else {
                    base.map(|base| base + start + leading)
                };
                Some((statement.to_string(), offset))
            })
            .collect();
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
        let offset = tab.displayed_statement_offset;
        let editor = tab.editor.clone();
        let value = editor.read(cx).value().to_string();
        // Position first: identical statements elsewhere in the buffer
        // must not swallow the rewrite. Text match is the fallback for
        // a buffer edited since the run.
        let position_match = offset
            .filter(|&offset| value.get(offset..offset + statement.len()) == Some(&statement[..]));
        // Fallback resolves by the occurrence nearest the last known
        // position (never blindly the first), so a drifted offset still
        // lands on the right twin.
        let splice_at =
            position_match.or_else(|| nearest_occurrence(&value, &statement, offset.unwrap_or(0)));
        if let Some(splice_at) = splice_at {
            if let Some(tab) = self.query_tabs.get_mut(self.active_query_tab) {
                tab.displayed_statement_offset = Some(splice_at);
            }
            let updated = format!(
                "{}{}{}",
                &value[..splice_at],
                rewritten,
                &value[splice_at + statement.len()..]
            );
            editor.update(cx, |editor, cx| editor.set_value(updated, window, cx));
        } else {
            if let Some(tab) = self.query_tabs.get_mut(self.active_query_tab) {
                tab.displayed_statement_offset = None;
            }
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
                let offset = this
                    .query_tabs
                    .get(this.active_query_tab)
                    .and_then(|tab| tab.displayed_statement_offset);
                this.start_statements(vec![(statement, offset)], cx);
            })
            .ok();
        })
        .detach();
    }

    fn start_statements(
        &mut self,
        mut statements: Vec<(String, Option<usize>)>,
        cx: &mut Context<Self>,
    ) {
        if self.query_abort.is_some() {
            return;
        }
        let Some(connected) = &self.connected else {
            self.flash_warning("Connect to a cluster before running a query", cx);
            return;
        };
        statements.retain(|(statement, _)| !statement.trim().is_empty());
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
        tab.explain = None;
        tab.advisor = None;
        tab.advise_pending = false;
        tab.advisor_generation += 1;
        tab.failed_sql = None;
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
        // For the history record on completion; `statements` moves
        // into the runner task.
        let run_sqls: Vec<String> = statements.iter().map(|(sql, _)| sql.clone()).collect();
        self.query_run_id += 1;
        let run_id = self.query_run_id;
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let task = rt::tokio().spawn(async move {
            let client = ChClient::new(config);
            let total = statements.len();
            let mut summary: Option<QueryStreamSummary> = None;
            let mut skipped = 0usize;
            let mut succeeded = Vec::new();
            for (index, (sql, offset)) in statements.iter().enumerate() {
                let outcome = client
                    .query_stream(sql, row_limit.unwrap_or(usize::MAX), |event| {
                        let _ = sender.send(RunEvent::Stream(event));
                    })
                    .await;
                match outcome {
                    Ok(current) => {
                        summary = Some(current);
                        succeeded.push((sql.clone(), *offset));
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
                            RunEvent::Stream(QueryStreamEvent::Started { query_id }) => {
                                tab.running_query_id = Some(query_id);
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
                let advise_pending = std::mem::take(&mut tab.advise_pending);
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
                    Ok(Err(error)) => {
                        // A kill from the ops view tears the stream; the
                        // transport error would otherwise be misleading.
                        let killed = tab
                            .running_query_id
                            .as_ref()
                            .map(|id| this.ops_killed.contains(id))
                            .unwrap_or(false);
                        if killed {
                            QueryOutcome::Error("Query killed from the ops view".into())
                        } else if error.contains("Query was cancelled")
                            || error.contains("(394)")
                            || error.contains("code 394")
                        {
                            QueryOutcome::Error(
                                "Query was cancelled (KILL QUERY on the server)".into(),
                            )
                        } else {
                            QueryOutcome::Error(error)
                        }
                    }
                    Err(error) => QueryOutcome::Error(error.to_string()),
                };
                // Re-sync the sort indicator with reality: the executed
                // SQL on success, or the still-displayed old result's SQL
                // when the run failed after an optimistic indicator.
                if let Some((statement, offset)) = successful_statements.last() {
                    tab.displayed_statement = Some(statement.clone());
                    tab.displayed_statement_offset = *offset;
                }
                if let Some(statement) = tab.displayed_statement.clone() {
                    let sort = zedb_ch::schema_intelligence::top_level_order_by(&statement);
                    let filters = zedb_ch::schema_intelligence::column_filters(&statement);
                    tab.result_grid.update(cx, |grid, cx| {
                        grid.set_sort(sort, cx);
                        grid.set_filters(filters, cx);
                    });
                }
                let duration_ms = tab.elapsed.map(|elapsed| elapsed.as_millis() as u64);
                let result_rows = tab.result_rows as u64;
                let run_error = match &tab.outcome {
                    QueryOutcome::Error(error) => Some(error.clone()),
                    _ => None,
                };
                tab.failed_sql = run_error.is_some().then(|| run_sqls.join(";\n\n"));
                let successful_sql: Vec<String> = successful_statements
                    .iter()
                    .map(|(sql, _)| sql.clone())
                    .collect();
                if run_error.is_none() {
                    if !successful_sql.is_empty() {
                        this.history_record(&successful_sql, duration_ms, Some(result_rows), None);
                    }
                    if advise_pending {
                        this.run_query_advisor(tab_id, cx);
                    }
                } else if run_sqls.len() == 1 {
                    // Failed single-statement runs are history too: the
                    // statement you need to fix is the one you want back.
                    this.history_record(&run_sqls, duration_ms, None, run_error.as_deref());
                }
                this.refresh_schema_after_statements(&successful_sql);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Run the query behind a saved query and, on completion, advise on
    /// it. Opens the SQL in a fresh tab (so the results and advice are
    /// visible together), runs it, and flags the run for advising.
    pub(crate) fn advise_saved_query(
        &mut self,
        sql: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.connected.is_none() {
            self.flash_warning("Connect to a cluster before advising a query", cx);
            return;
        }
        self.open_query_tab_with(&sql, window, cx);
        self.start_statements(vec![(sql, None)], cx);
        if let Some(tab) = self.query_tabs.get_mut(self.active_query_tab) {
            tab.advise_pending = true;
        }
    }

    /// Compute the advisor result for the displayed statement off-thread:
    /// EXPLAIN it (reusing the plan parser), turn the plan + run stats into
    /// facts, and store the ranked findings. Always stores `Some` when
    /// invoked, so an empty result shows a "looks fine" note rather than
    /// nothing. A generation + connection guard drops a stale result.
    fn run_query_advisor(&mut self, tab_id: usize, cx: &mut Context<Self>) {
        let Some(connected) = self.connected.as_ref() else {
            return;
        };
        let connection_name = connected.name.clone();
        let config = connected.client_config.clone();
        let Some(tab) = self.query_tabs.iter().find(|tab| tab.id == tab_id) else {
            return;
        };
        let Some(sql) = tab.displayed_statement.clone() else {
            return;
        };
        let read_rows = tab.read_rows.unwrap_or(0);
        let result_rows = tab.result_rows as u64;
        let read_bytes = tab.read_bytes.unwrap_or(0);
        let capped = tab.result_capped;
        let generation = tab.advisor_generation;

        // Only a read can be EXPLAINed; anything else gets an empty (looks
        // fine) result so the invoked lane still gives feedback.
        if !query_advisor::is_advisable_select(&sql) {
            if let Some(tab) = self.query_tabs.iter_mut().find(|tab| tab.id == tab_id) {
                tab.advisor = Some(Vec::new());
                cx.notify();
            }
            return;
        }

        // The WHERE columns with whether each is a range filter, so the fix
        // DDL names the real column and picks the right index type; the
        // column's type is fetched below once we know the table.
        let filters: Vec<(String, bool)> = zedb_ch::schema_intelligence::column_filters(&sql)
            .into_iter()
            .map(|(name, conjunct)| (name, is_range_predicate(&conjunct)))
            .collect();
        // A top-level GROUP BY marks a rollup the advisor can suggest a
        // projection / materialized view for (vs a global aggregate); when
        // present we also rebuild the projection body from the SQL so the
        // fix is copyable DDL, not just prose.
        let has_group_by = zedb_ch::schema_intelligence::has_group_by(&sql);
        let aggregate_projection = zedb_ch::schema_intelligence::aggregate_projection(&sql);
        let explain_sql = zedb_ch::explain::explain_statement(&sql);
        // Compute the findings off-thread: EXPLAIN, then (once we know the
        // table) fetch its true sorting key and the filtered columns' types
        // from system.*. EXPLAIN's PrimaryKey "Keys" only lists the keys the
        // WHERE touched, not the table's full ORDER BY, so we can't tell the
        // leading key from it.
        let task = rt::tokio().spawn(async move {
            let client = ChClient::new(config);
            let raw = client
                .query(&explain_sql)
                .await
                .ok()?
                .rows
                .first()
                .and_then(|row| row.first())
                .map(|value| value.to_string())
                .unwrap_or_default();
            let plan = zedb_ch::explain::parse_explain_json(&raw).ok()?;
            let mut facts =
                query_advisor::facts_from_plan(&plan, read_rows, result_rows, read_bytes, capped);
            facts.has_group_by = has_group_by;
            facts.aggregate_projection = aggregate_projection;
            if let Some((order_by, partition_key)) =
                fetch_table_keys(&client, facts.table.as_deref()).await
            {
                if !order_by.is_empty() {
                    facts.order_by = order_by;
                }
                facts.partition_key = partition_key;
            }
            let types = fetch_column_types(&client, facts.table.as_deref()).await;
            let mut filter_columns: Vec<query_advisor::FilterColumn> = filters
                .into_iter()
                .map(|(name, is_range)| query_advisor::FilterColumn {
                    base_type: types.get(&name).cloned().unwrap_or_default(),
                    name,
                    is_range,
                    distinct: None,
                })
                .collect();
            // For equality filters whose cardinality we can't infer from the
            // type, probe uniqCombined so the index choice (set vs bloom,
            // and the bloom rate) fits the data. One batched query.
            if let Some((database, table)) = facts
                .table
                .as_deref()
                .and_then(|table| table.split_once('.'))
            {
                let probe: Vec<String> = filter_columns
                    .iter()
                    .filter(|column| query_advisor::needs_cardinality_probe(column))
                    .map(|column| column.name.clone())
                    .collect();
                if !probe.is_empty() {
                    if let Ok(distincts) =
                        client.column_cardinalities(database, table, &probe).await
                    {
                        for (name, distinct) in probe.iter().zip(distincts) {
                            if let Some(column) = filter_columns
                                .iter_mut()
                                .find(|column| &column.name == name)
                            {
                                column.distinct = Some(distinct);
                            }
                        }
                    }
                }
            }
            facts.filter_columns = filter_columns;
            Some(query_advisor::advise(&facts))
        });

        cx.spawn(async move |this, cx| {
            let findings = task.await.ok().flatten().unwrap_or_default();
            this.update(cx, |this, cx| {
                if this.connected.as_ref().map(|c| c.name.as_str())
                    != Some(connection_name.as_str())
                {
                    return;
                }
                let Some(tab) = this.query_tabs.iter_mut().find(|tab| tab.id == tab_id) else {
                    return;
                };
                if tab.advisor_generation != generation {
                    return;
                }
                tab.advisor = Some(findings);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The query-advisor lane under the results: ranked findings, each a
    /// plain-language diagnosis plus a copyable fix. Kept visually quiet
    /// (an accent rule, not an alarm) and dismissible.
    fn query_advisor_panel(&self, tab: &QueryTab, cx: &mut Context<Self>) -> impl IntoElement {
        let tab_id = tab.id;
        let advised_sql = tab.displayed_statement.clone().unwrap_or_default();
        // The optional agent hand-off, exactly like the error bar: only
        // when a usable agent is remembered, and it rides silent context.
        let ask_agent_icon: Option<String> = self
            .preferences
            .last_agent
            .clone()
            .filter(|name| {
                self.agent.agents.is_empty()
                    || self.agent.agents.iter().any(|agent| agent.name == *name)
            })
            .map(|name| {
                self.agent
                    .agents
                    .iter()
                    .find(|agent| agent.name == name)
                    .map(|agent| agent_pane::icon_for(&agent.id))
                    .unwrap_or(match name.as_str() {
                        "Claude Code" => "icons/agent-claude.svg",
                        "Codex" => "icons/agent-codex.svg",
                        _ => "icons/sparkle.svg",
                    })
                    .to_string()
            });
        // A small square icon action for the fix line (copy / open / ask).
        let advisor_action = |id: (&'static str, usize), icon: &str| {
            div()
                .id(id)
                .size(px(18.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(3.))
                .hover(|button| button.bg(theme::hover()).cursor_pointer())
                .child(
                    svg()
                        .path(icon.to_string())
                        .size(px(12.))
                        .text_color(theme::text_dim()),
                )
        };
        let mut panel = div()
            .flex_none()
            .flex()
            .flex_col()
            .gap_2()
            .px_3()
            .py_2()
            .border_t_1()
            .border_color(theme::border())
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(theme::warning())
                            .child("QUERY ADVISOR"),
                    )
                    .child(
                        div()
                            .id("advisor-dismiss")
                            .px_1()
                            .rounded(px(3.))
                            .text_color(theme::text_dim())
                            .child("x")
                            .hover(|button| button.bg(theme::hover()).cursor_pointer())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(tab) =
                                    this.query_tabs.iter_mut().find(|tab| tab.id == tab_id)
                                {
                                    tab.advisor = None;
                                    cx.notify();
                                }
                            })),
                    ),
            );
        let findings = tab.advisor.as_deref().unwrap_or(&[]);
        if findings.is_empty() {
            panel = panel.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .border_l_2()
                    .border_color(theme::success())
                    .pl_2()
                    .child(
                        svg()
                            .path("icons/verify.svg")
                            .size(px(12.))
                            .text_color(theme::success()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::text_dim())
                            .child("No advice — the primary key is serving this query's filter."),
                    ),
            );
        }
        for (index, finding) in findings.iter().enumerate() {
            let editor_sql = finding.editor_sql.clone();
            // Findings without copyable DDL (e.g. the partition and
            // aggregate advice) still get a copy button: it copies the
            // suggestion prose, so every row has a copy action.
            let copy_fix_text = editor_sql.is_none().then(|| finding.fix.clone());
            panel = panel.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .border_l_2()
                    .border_color(theme::warning())
                    .pl_2()
                    .child(div().text_color(theme::text()).child(finding.title.clone()))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::text_dim())
                            .child(finding.detail.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(
                                // min-width: 0 lets the flex child shrink
                                // below its content width so the fix text
                                // wraps instead of overflowing (and being
                                // clipped) when the panel narrows.
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_xs()
                                    .text_color(theme::text_dim())
                                    .child(finding.fix.clone()),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .when_some(editor_sql, |actions, sql| {
                                        let copy_sql = sql.clone();
                                        actions
                                            .child(
                                                advisor_action(("advisor-copy", index), "icons/copy.svg")
                                                    .tooltip(|window, cx| {
                                                        gpui_component::tooltip::Tooltip::new(
                                                            "Copy fix",
                                                        )
                                                        .build(window, cx)
                                                    })
                                                    .on_click(cx.listener(move |_, _, _, cx| {
                                                        cx.write_to_clipboard(
                                                            gpui::ClipboardItem::new_string(
                                                                copy_sql.clone(),
                                                            ),
                                                        );
                                                    })),
                                            )
                                            .child(
                                                advisor_action(
                                                    ("advisor-open", index),
                                                    "icons/query-plus.svg",
                                                )
                                                .tooltip(|window, cx| {
                                                    gpui_component::tooltip::Tooltip::new(
                                                        "Open fix in editor",
                                                    )
                                                    .build(window, cx)
                                                })
                                                .on_click(cx.listener(move |this, _, window, cx| {
                                                    this.open_query_tab_with(&sql, window, cx);
                                                })),
                                            )
                                    })
                                    .when_some(copy_fix_text, |actions, fix| {
                                        actions.child(
                                            advisor_action(("advisor-copy", index), "icons/copy.svg")
                                                .tooltip(|window, cx| {
                                                    gpui_component::tooltip::Tooltip::new(
                                                        "Copy suggestion",
                                                    )
                                                    .build(window, cx)
                                                })
                                                .on_click(cx.listener(move |_, _, _, cx| {
                                                    cx.write_to_clipboard(
                                                        gpui::ClipboardItem::new_string(fix.clone()),
                                                    );
                                                })),
                                        )
                                    })
                                    .when_some(ask_agent_icon.clone(), |actions, icon| {
                                        // Silent hand-off, mirroring the error
                                        // bar: a plain ask, the finding + query
                                        // rides as hidden context.
                                        let visible =
                                            "This query isn't using the primary key — help me make it faster."
                                                .to_string();
                                        let mut hidden = format!(
                                            "Context (not shown to the user): from the zeDB query advisor. Finding: {}\nSuggested fix: {}",
                                            finding.detail, finding.fix,
                                        );
                                        if let Some(ddl) = &finding.editor_sql {
                                            hidden.push_str(&format!(
                                                "\nSuggested DDL:\n```sql\n{ddl}\n```"
                                            ));
                                        }
                                        if !advised_sql.is_empty() {
                                            hidden.push_str(&format!(
                                                "\nThe query was:\n```sql\n{advised_sql}\n```"
                                            ));
                                        }
                                        hidden.push_str(
                                            "\nDo not open a migration for this. Put the DDL in the query editor with propose_query so the user can review and run it, or explain the trade-offs.",
                                        );
                                        actions.child(
                                            advisor_action(("advisor-agent", index), &icon)
                                                .tooltip(|window, cx| {
                                                    gpui_component::tooltip::Tooltip::new(
                                                        "Ask your agent",
                                                    )
                                                    .build(window, cx)
                                                })
                                                .on_click(cx.listener(move |this, _, window, cx| {
                                                    this.agent_ask_about(
                                                        visible.clone(),
                                                        hidden.clone(),
                                                        window,
                                                        cx,
                                                    );
                                                })),
                                        )
                                    }),
                            ),
                    ),
            );
        }
        panel
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
        gpui::deferred(
            div()
                .id(id)
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
                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                        this.query_resize = Some((target, f32::from(event.position.y)));
                        cx.notify();
                    }),
                ),
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
        let action_context = self.query_tabs[self.active_query_tab]
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
                                if self.connected.is_none() {
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
                                if self.connected.is_none() {
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
                                if self.connected.is_none() {
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
                                    if self.connecting.is_some() {
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
        let health = self.endpoint_health.get(&connection.name)?;
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
        let cardinalities = selected.cardinalities.clone();
        let measured = selected.measured.clone();
        let applying = selected.applying;
        let applying_slow = selected.applying_slow;
        let advisor_db = selected.database.clone();
        let advisor_table = selected.object.name.clone();
        let advisor_rows = selected.object.total_rows.unwrap_or(0);
        // In cluster scope the generated statements run ON CLUSTER.
        let advisor_cluster = self
            .connected
            .as_ref()
            .and_then(|connected| connected.apply_cluster.clone());
        let column_rows = selected
            .columns
            .iter()
            .enumerate()
            .map(|(index, column)| {
                // Per-column storage (Phase 8, Tier 1). 0 compressed
                // bytes means the object holds no data (a view, or an
                // empty table); show a dash rather than "0 B / NaNx".
                let has_data = column.compressed_bytes > 0;
                let compressed = if has_data {
                    Self::format_bytes(column.compressed_bytes)
                } else {
                    "\u{2014}".to_string()
                };
                let uncompressed = if column.uncompressed_bytes > 0 {
                    Self::format_bytes(column.uncompressed_bytes)
                } else {
                    "\u{2014}".to_string()
                };
                let ratio = if has_data {
                    format!(
                        "{:.1}x",
                        column.uncompressed_bytes as f64 / column.compressed_bytes as f64
                    )
                } else {
                    "\u{2014}".to_string()
                };
                let codec = if column.codec.is_empty() {
                    "\u{2014}".to_string()
                } else {
                    column.codec.clone()
                };
                // Storage advice (Phase 8, Tier 2), computed once the
                // cardinality probe has run. None before then.
                let distinct_count = cardinalities
                    .as_ref()
                    .and_then(|values| values.get(index))
                    .copied();
                let advice = distinct_count.map(|distinct| {
                    storage_advisor::advise(
                        &storage_advisor::ColumnFacts {
                            name: &column.name,
                            type_name: &column.type_name,
                            codec: &column.codec,
                            distinct,
                            total_rows: advisor_rows,
                            compressed_bytes: column.compressed_bytes,
                            uncompressed_bytes: column.uncompressed_bytes,
                        },
                        &advisor_db,
                        &advisor_table,
                        advisor_cluster.as_deref(),
                    )
                });
                div()
                    .id(("schema-column", index))
                    .h(px(30.))
                    .flex_none()
                    .px_3()
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(theme::border())
                    .when(index % 2 == 1, |row| row.bg(rgb(0x1f2329)))
                    .child(
                        // Preferred width, but allowed to shrink (and
                        // truncate) on a narrow window so the storage
                        // columns and codec are not pushed off the edge.
                        div()
                            .w(px(220.))
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_color(theme::text())
                            .child(column.name.clone()),
                    )
                    .child(
                        div()
                            .w(px(300.))
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_color(theme::text_dim())
                            .child(type_highlight::styled(&column.type_name)),
                    )
                    .child(
                        div()
                            .w(px(100.))
                            .flex_none()
                            .pl_4()
                            .text_right()
                            .text_color(theme::text())
                            .child(compressed),
                    )
                    .child(
                        div()
                            .w(px(100.))
                            .flex_none()
                            .pl_4()
                            .text_right()
                            .text_color(theme::text_dim())
                            .child(uncompressed),
                    )
                    .child(
                        div()
                            .w(px(80.))
                            .flex_none()
                            .pl_4()
                            .text_right()
                            .text_color(theme::text_dim())
                            .child(ratio),
                    )
                    .child(
                        div()
                            .flex_1()
                            // A floor so CODEC keeps a usable width and it
                            // is COLUMN/TYPE that shrink on a narrow window,
                            // rather than the codec clipping off the edge.
                            .min_w(px(230.))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .pl_4()
                            .text_color(theme::text_dim())
                            // Codec expressions read like nested type
                            // calls (CODEC(Delta(8), ZSTD(1))), so the
                            // type colorer highlights them the same way;
                            // the plain "—" placeholder passes through dim.
                            .child(type_highlight::styled(&codec)),
                    )
                    // SUGGESTION (Phase 8, Tier 2): the advisor's verdict.
                    // Actionable ones are clickable and copy their ALTER;
                    // the distinct count and reason ride along in a tooltip.
                    .when_some(advice, |row, advice| {
                        // The analysis lane: a green tick (hover = why it is
                        // fine) when there is nothing to do, or an action
                        // icon that opens the query editor with the ALTER
                        // (hover = what it will do and why) when there is.
                        let base = || {
                            div()
                                .w(px(110.))
                                .flex_none()
                                .border_l_1()
                                .border_color(theme::border())
                                .flex()
                                .items_center()
                                .justify_center()
                        };
                        let evidence = distinct_count
                            .map(|distinct| {
                                format!("{} distinct values", Self::format_count(distinct))
                            })
                            .unwrap_or_default();
                        let cell = match advice {
                            storage_advisor::Advice::Good(reason)
                            | storage_advisor::Advice::Leave(reason) => base()
                                .id(("advice", index))
                                .child(
                                    svg()
                                        .path("icons/verify.svg")
                                        .size(px(14.))
                                        .text_color(theme::success()),
                                )
                                .tooltip(move |window, cx| {
                                    gpui_component::tooltip::Tooltip::new(reason.clone())
                                        .build(window, cx)
                                })
                                .into_any_element(),
                            storage_advisor::Advice::Suggest {
                                label,
                                reason,
                                apply,
                                editor_sql,
                                ..
                            } if applying == Some(index) && applying_slow => {
                                // A slow in-place apply is in progress.
                                let _ = (label, reason, apply, editor_sql);
                                base().child(Self::advice_spinner()).into_any_element()
                            }
                            storage_advisor::Advice::Suggest {
                                label,
                                reason,
                                apply,
                                editor_sql,
                                ..
                            } => {
                                // The measured savings from the Tier 3 trial,
                                // once it lands (writable connections only).
                                let saving = measured.get(&index).copied();
                                let saving_label = saving
                                    .map(|ratio| format!("{ratio:.1}\u{00d7}"))
                                    .unwrap_or_default();
                                let measured_line = saving
                                    .map(|ratio| {
                                        format!("\nMeasured {ratio:.1}x smaller on a sample.")
                                    })
                                    .unwrap_or_default();
                                let tooltip = format!(
                                    "Suggest {label}: {reason} ({evidence}).{measured_line}\n\
                                     Left-click to apply \u{00b7} right-click to open in the \
                                     query editor:\n{editor_sql}"
                                );
                                let editor_left = editor_sql.clone();
                                base()
                                    .id(("advice", index))
                                    .gap_1()
                                    .cursor_pointer()
                                    .rounded(px(3.))
                                    .hover(|cell| cell.bg(theme::hover()))
                                    .child(
                                        svg()
                                            .path("icons/query-plus.svg")
                                            .size(px(14.))
                                            .text_color(theme::accent()),
                                    )
                                    .when(!saving_label.is_empty(), |cell| {
                                        cell.child(
                                            div()
                                                .text_xs()
                                                .text_color(theme::accent())
                                                .child(saving_label),
                                        )
                                    })
                                    // Left-click: apply in place (staging/dev),
                                    // or open the editor (prod / read-only), or
                                    // confirm first on a large table.
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.request_apply(
                                            index,
                                            apply.clone(),
                                            editor_left.clone(),
                                            window,
                                            cx,
                                        );
                                    }))
                                    // Right-click: open in the query editor
                                    // (nothing on production).
                                    .on_mouse_down(
                                        MouseButton::Right,
                                        cx.listener(move |this, _, window, cx| {
                                            this.open_suggestion_in_editor(
                                                editor_sql.clone(),
                                                window,
                                                cx,
                                            );
                                        }),
                                    )
                                    .tooltip(move |window, cx| {
                                        gpui_component::tooltip::Tooltip::new(tooltip.clone())
                                            .build(window, cx)
                                    })
                                    .into_any_element()
                            }
                            storage_advisor::Advice::Unknown => base().into_any_element(),
                        };
                        row.child(cell)
                    })
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
            .border_color(theme::border())
            .children(
                [
                    (ObjectInspectorTab::Overview, "Overview"),
                    (ObjectInspectorTab::Columns, "Columns"),
                    (ObjectInspectorTab::Parts, "Parts"),
                    (ObjectInspectorTab::Projections, "Projections"),
                    (ObjectInspectorTab::Dependencies, "Dependencies"),
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
                            button
                                .border_color(theme::accent())
                                .text_color(theme::text())
                        })
                        .when(tab != button_tab, |button| {
                            button
                                .border_color(theme::bg())
                                .text_color(theme::text_dim())
                                .hover(|button| button.text_color(theme::text()).cursor_pointer())
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(selected) = &mut this.selected_schema_object {
                                selected.tab = button_tab;
                                cx.notify();
                            }
                            if button_tab == ObjectInspectorTab::Parts {
                                this.load_partitions(cx);
                                this.start_merges_poll(cx);
                            }
                            if button_tab == ObjectInspectorTab::Dependencies {
                                this.load_dependencies(cx);
                            }
                            if button_tab == ObjectInspectorTab::Projections {
                                this.load_projections(cx);
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
                    let sharding_key = zedb_ch::distributed_sharding_key(&details.engine_full)
                        .map(|key| ("Sharding key", key));
                    [
                        ("Partition key", details.partition_key.clone()),
                        ("Order by", details.sorting_key.clone()),
                        ("Primary key", details.primary_key.clone()),
                    ]
                    .into_iter()
                    .chain(sharding_key)
                    .map(|(label, value)| {
                        div()
                            .py_3()
                            .border_b_1()
                            .border_color(theme::border())
                            .flex()
                            .gap_4()
                            .child(
                                div()
                                    .w(px(150.))
                                    .flex_none()
                                    .text_color(theme::text_dim())
                                    .child(label),
                            )
                            .child(div().flex_1().min_w_0().text_color(theme::text()).child(
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
                                    .text_color(theme::text_dim())
                                    .child("Loading details..."),
                            )
                        })
                        .when_some(error.as_ref(), |panel, error| {
                            panel.child(
                                div()
                                    .py_3()
                                    .text_color(theme::danger())
                                    .child(error.clone()),
                            )
                        })
                        .when(has_engine_definition, |panel| {
                            panel.child(
                                div()
                                    .py_3()
                                    .border_b_1()
                                    .border_color(theme::border())
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_color(theme::text_dim())
                                            .child("Engine definition"),
                                    )
                                    .child(
                                        div()
                                            .id("engine-definition")
                                            .w_full()
                                            .h(px(132.))
                                            .rounded(px(3.))
                                            .border_1()
                                            .border_color(theme::border())
                                            .bg(theme::bg_sunken())
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
                // Opt-in cardinality prompt (Phase 8, Tier 2). Shows until
                // the probe has run; the result is cached for the session,
                // so a re-opened table skips the prompt and auto-loads.
                .when(
                    selected.cardinalities.is_none() && !selected.columns.is_empty(),
                    |body| {
                        let loading = selected.cardinality_loading;
                        let confirming = selected.cardinality_confirming;
                        // On a writable connection the measurement step
                        // writes a temporary table, so the prompt asks
                        // before running. The ghost green primary button:
                        // green outline that fills on hover. The label sits
                        // in a child driven by group-hover, so it flips to
                        // dark on the green fill (a plain hover text_color
                        // does not repaint the text child in this gpui).
                        let ghost = |id: &'static str, label: &'static str| {
                            div()
                                .id(id)
                                .group(id)
                                .px_3()
                                .py_1()
                                .rounded(px(4.))
                                .border_1()
                                .border_color(theme::success())
                                .text_xs()
                                .text_color(theme::success())
                                .cursor_pointer()
                                .hover(|button| {
                                    button.bg(theme::success()).border_color(theme::success())
                                })
                                .child(
                                    div()
                                        .group_hover(id, |label| label.text_color(rgb(0x14171c)))
                                        .child(label),
                                )
                        };
                        let message = if confirming {
                            "Measuring the suggestions creates a temporary table on the \
                             server (dropped afterwards). Continue?"
                        } else {
                            "Analyse per-column cardinality to suggest better codecs and \
                             types. Scans the table once."
                        };
                        body.child(
                            div()
                                .flex_none()
                                .px_3()
                                .py_2()
                                .flex()
                                .items_center()
                                .justify_between()
                                .border_b_1()
                                .border_color(theme::border())
                                .bg(theme::bg_sunken())
                                .child(div().text_xs().text_color(theme::text_dim()).child(message))
                                .child(if loading {
                                    div()
                                        .px_3()
                                        .py_1()
                                        .text_xs()
                                        .text_color(theme::text_dim())
                                        .child("Analysing\u{2026}")
                                        .into_any_element()
                                } else if confirming {
                                    div()
                                        .flex()
                                        .gap_2()
                                        .child(
                                            div()
                                                .id("cancel-analyze")
                                                .px_3()
                                                .py_1()
                                                .rounded(px(4.))
                                                .text_xs()
                                                .text_color(theme::text_dim())
                                                .cursor_pointer()
                                                .hover(|button| {
                                                    button
                                                        .bg(theme::hover())
                                                        .text_color(theme::text())
                                                })
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.cancel_analyze(cx)
                                                }))
                                                .child("Cancel"),
                                        )
                                        .child(ghost("confirm-analyze", "Continue").on_click(
                                            cx.listener(|this, _, window, cx| {
                                                this.confirm_analyze(window, cx)
                                            }),
                                        ))
                                        .into_any_element()
                                } else {
                                    ghost("analyze-cardinality", "Analyse")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.request_analyze(window, cx)
                                        }))
                                        .into_any_element()
                                }),
                        )
                    },
                )
                .child(
                    div()
                        .h(px(28.))
                        .flex_none()
                        .px_3()
                        .flex()
                        .items_center()
                        .bg(theme::bg_sidebar())
                        .border_b_1()
                        .border_color(theme::border())
                        .text_xs()
                        .text_color(theme::text_dim())
                        .child(
                            div()
                                .w(px(220.))
                                .min_w_0()
                                .overflow_hidden()
                                .child("COLUMN"),
                        )
                        .child(div().w(px(300.)).min_w_0().overflow_hidden().child("TYPE"))
                        .child(
                            div()
                                .w(px(100.))
                                .flex_none()
                                .pl_4()
                                .text_right()
                                .child("COMPRESSED"),
                        )
                        .child(
                            div()
                                .w(px(100.))
                                .flex_none()
                                .pl_4()
                                .text_right()
                                .child("UNCOMP."),
                        )
                        .child(
                            div()
                                .w(px(80.))
                                .flex_none()
                                .pl_4()
                                .text_right()
                                .child("RATIO"),
                        )
                        .child(div().flex_1().min_w(px(230.)).pl_4().child("CODEC"))
                        // ADVICE is the analysis lane, set off from the
                        // data columns by a divider and the accent color so
                        // "the data" and "the analysis" read as separate.
                        // Appears once the probe has run.
                        .when(selected.cardinalities.is_some(), |header| {
                            header.child(
                                div()
                                    .w(px(110.))
                                    .flex_none()
                                    .border_l_1()
                                    .border_color(theme::border())
                                    .flex()
                                    .justify_center()
                                    .text_color(theme::accent())
                                    .child("ADVICE"),
                            )
                        }),
                )
                // Per-column bytes only exist for Wide parts; when the
                // table is entirely Compact, explain the empty columns
                // rather than leaving a wall of dashes (Phase 8).
                .when_some(
                    selected
                        .storage
                        .as_ref()
                        .filter(|storage| storage.wide_parts == 0 && storage.compact_parts > 0),
                    |body, storage| {
                        body.child(
                            div()
                                .flex_none()
                                .px_3()
                                .py_1()
                                .bg(theme::bg_sidebar())
                                .border_b_1()
                                .border_color(theme::border())
                                .text_xs()
                                .text_color(theme::text_dim())
                                .child(format!(
                                    "Per-column sizes need Wide parts; all {} parts are Compact \
                                     (see the table ratio above).",
                                    storage.compact_parts
                                )),
                        )
                    },
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
                                    .text_color(theme::text_dim())
                                    .child("Loading columns..."),
                            )
                        })
                        .when_some(error.as_ref(), |columns, error| {
                            columns
                                .child(div().p_3().text_color(theme::danger()).child(error.clone()))
                        })
                        .when(
                            !loading && error.is_none() && selected.columns.is_empty(),
                            |columns| {
                                columns.child(
                                    div()
                                        .p_3()
                                        .text_color(theme::text_dim())
                                        .child("No columns"),
                                )
                            },
                        )
                        .children(column_rows),
                ),
            ObjectInspectorTab::Parts => self.parts_panel(selected, cx),
            ObjectInspectorTab::Dependencies => self.dependencies_panel(selected, cx),
            ObjectInspectorTab::Projections => self.projections_panel(selected),
            ObjectInspectorTab::Ddl => {
                let ddl = details
                    .as_ref()
                    .map(|details| details.create_table_query.clone())
                    .unwrap_or_default();
                let clipboard_ddl = ddl.clone();
                let has_ddl = !loading && error.is_none() && !ddl.is_empty();
                div().flex_1().min_h_0().flex().flex_col().child(
                    div()
                        .id("object-ddl")
                        .group("ddl-editor")
                        .relative()
                        .flex_1()
                        .min_h_0()
                        .m_3()
                        .overflow_hidden()
                        .rounded(px(3.))
                        .border_1()
                        .border_color(theme::border())
                        .bg(theme::bg_sunken())
                        .text_color(theme::text())
                        .when(loading, |panel| panel.child("Loading DDL..."))
                        .when_some(error.as_ref(), |panel, error| {
                            panel.child(div().text_color(theme::danger()).child(error.clone()))
                        })
                        .when(!loading && error.is_none() && ddl.is_empty(), |panel| {
                            panel
                                .p_3()
                                .child(div().text_color(theme::text_dim()).child("DDL unavailable"))
                        })
                        .when(has_ddl, |panel| {
                            panel.child(
                                Input::new(&ddl_editor)
                                    .appearance(false)
                                    .bordered(false)
                                    .focus_bordered(false)
                                    .disabled(true)
                                    .tab_index(-1)
                                    .h_full(),
                            )
                        })
                        // Copy sits over the top-right of the editor,
                        // revealed on hover, instead of a dedicated bar.
                        .when(has_ddl, |panel| {
                            panel.child(
                                div()
                                    .id("copy-object-ddl")
                                    .absolute()
                                    .top(px(6.))
                                    .right(px(6.))
                                    .size(px(22.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(3.))
                                    .bg(theme::bg_sidebar())
                                    .invisible()
                                    .group_hover("ddl-editor", |icon| icon.visible())
                                    .hover(|icon| icon.cursor_pointer())
                                    .tooltip(|window, cx| {
                                        gpui_component::tooltip::Tooltip::new("Copy DDL")
                                            .build(window, cx)
                                    })
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            clipboard_ddl.clone(),
                                        ));
                                    }))
                                    .child(
                                        svg()
                                            .path("icons/copy.svg")
                                            .size(px(13.))
                                            .text_color(theme::text_dim()),
                                    ),
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
                    .border_color(theme::border())
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div().text_lg().text_color(theme::text()).child(format!(
                                    "{}.{}",
                                    selected.database, selected.object.name
                                )),
                            )
                            .child(
                                div()
                                    .px_2()
                                    .py(px(2.))
                                    .rounded(px(3.))
                                    .bg(theme::hover())
                                    .text_xs()
                                    .text_color(theme::text_dim())
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
                                    .child(div().text_color(theme::text()).child("Engine:"))
                                    .child(
                                        div()
                                            .text_color(theme::text_dim())
                                            .child(selected.object.engine.clone()),
                                    ),
                            )
                            .when_some(selected.object.total_rows, |row, rows| {
                                row.child(
                                    div()
                                        .flex()
                                        .gap_1()
                                        .child(div().text_color(theme::text()).child("Rows:"))
                                        .child(
                                            div()
                                                .text_color(theme::text_dim())
                                                .child(Self::format_count(rows)),
                                        ),
                                )
                            })
                            .when_some(selected.object.total_bytes, |row, bytes| {
                                let distributed = selected.object.engine == "Distributed";
                                let text = if distributed {
                                    format!("({})", Self::format_bytes(bytes))
                                } else {
                                    Self::format_bytes(bytes)
                                };
                                let size = div()
                                    .flex()
                                    .gap_1()
                                    .child(div().text_color(theme::text()).child("Size:"))
                                    .child(div().text_color(theme::text_dim()).child(text));
                                row.child(if distributed {
                                    size.id("object-size-virtual")
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
                            .when_some(selected.storage.as_ref(), |row, storage| {
                                // Table-wide compression, always available
                                // (Phase 8). Per-column sizes need Wide parts;
                                // this ratio does not.
                                let ratio = storage.uncompressed_bytes as f64
                                    / storage.compressed_bytes.max(1) as f64;
                                let raw = Self::format_bytes(storage.uncompressed_bytes);
                                row.child(
                                    div()
                                        .id("object-compression")
                                        .flex()
                                        .gap_1()
                                        .child(div().text_color(theme::text()).child("Ratio:"))
                                        .child(
                                            div()
                                                .text_color(theme::text_dim())
                                                .child(format!("{ratio:.2}x")),
                                        )
                                        .tooltip(move |window, cx| {
                                            gpui_component::tooltip::Tooltip::new(format!(
                                                "{raw} uncompressed"
                                            ))
                                            .build(window, cx)
                                        }),
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
                let multiple = self.query_tabs.len() > 1;
                let has_right = index + 1 < self.query_tabs.len();
                let active = index == self.active_query_tab;
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
                        this.active_query_tab = index;
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
                    .when(self.query_tabs.len() > 1, |tab_row| {
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
            .query_tabs
            .get(self.active_query_tab)
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
                                        let last = this.query_tabs.len().saturating_sub(1);
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
                                    .when(self.show_history, |button| button.bg(theme::hover()))
                                    .child(
                                        svg().path("icons/history.svg").size(px(14.)).text_color(
                                            if self.show_history {
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
            .when(self.show_history, |root| {
                root.child(self.history_resize_handle(cx))
                    .child(self.history_drawer(cx))
            })
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
            // The grid's right-click Copy / Copy as CSV menu dispatches
            // to the window root, so handle it here and delegate to the
            // active tab's grid (cmd-C is handled on the grid itself).
            .on_action(cx.listener(|this, _: &grid_spike::Copy, _, cx| {
                if let Some(tab) = this.query_tabs.get(this.active_query_tab) {
                    let grid = tab.result_grid.clone();
                    grid.update(cx, |grid, cx| grid.copy_selected(cx));
                }
            }))
            .on_action(cx.listener(|this, _: &grid_spike::CopyAsCsv, _, cx| {
                if let Some(tab) = this.query_tabs.get(this.active_query_tab) {
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
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, _, cx| {
                    // Any click that reaches the root dismisses an open
                    // filter popover; clicks inside it stop propagation.
                    if let Some(tab) = this.query_tabs.get(this.active_query_tab) {
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
                if let Some((start_width, start_x)) = this.history_resizing {
                    let width = start_width + (start_x - f32::from(event.position.x));
                    this.history_width = width.clamp(240.0, 640.0);
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
                    this.history_resizing = None;
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
                    this.history_resizing = None;
                }),
            )
            .when(self.export.is_some(), |root| {
                root.child(self.export_overlay(cx))
            })
            .when(self.pending_apply.is_some(), |root| {
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
                            .when(!self.show_preferences && self.form.is_some(), |main| {
                                main.child(self.form_panel(cx))
                            })
                            .when(!self.show_preferences && self.form.is_none(), |main| {
                                main.child(self.connection_toolbar(cx)).child(
                                    div()
                                        .flex_1()
                                        .min_h_0()
                                        .when(self.show_ops, |content| {
                                            content.child(self.ops_panel(cx))
                                        })
                                        .when(!self.show_ops && self.show_query_editor, |content| {
                                            content.child(self.query_editor_panel(cx))
                                        })
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

/// Resolve editor-local `@set name=value` declarations and `${name}` uses.
/// Declarations come from the full editor buffer, while only declarations in
/// the execution target are removed from the SQL sent to ClickHouse.
fn resolve_query_variables(text: &str, editor_text: &str) -> Result<String, String> {
    let mut variables = HashMap::new();
    for (line_index, line) in editor_text.lines().enumerate() {
        let trimmed = line.trim();
        let directive = if trimmed == "@set" {
            Some("")
        } else {
            trimmed
                .strip_prefix("@set")
                .filter(|rest| rest.starts_with(char::is_whitespace))
        };
        let Some(directive) = directive else {
            continue;
        };
        let Some((name, value)) = directive.trim().split_once('=') else {
            return Err(format!(
                "Invalid query variable on line {}: use @set name=value",
                line_index + 1
            ));
        };
        let name = name.trim();
        let value = value.trim();
        let valid_name = name
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
            && name
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '_');
        if !valid_name {
            return Err(format!(
                "Invalid query variable name `{name}` on line {}",
                line_index + 1
            ));
        }
        if value.is_empty() {
            return Err(format!(
                "Query variable `{name}` has no value on line {}",
                line_index + 1
            ));
        }
        variables.insert(name.to_string(), value.to_string());
    }

    let mut sql = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let content = line.strip_suffix('\n').unwrap_or(line);
        let trimmed = content.trim();
        let is_directive = trimmed == "@set"
            || trimmed
                .strip_prefix("@set")
                .is_some_and(|rest| rest.starts_with(char::is_whitespace));
        if is_directive {
            if line.ends_with('\n') {
                sql.push('\n');
            }
            continue;
        }

        let mut remaining = line;
        while let Some(start) = remaining.find("${") {
            sql.push_str(&remaining[..start]);
            let placeholder = &remaining[start + 2..];
            let Some(end) = placeholder.find('}') else {
                return Err("Unclosed query variable placeholder".to_string());
            };
            let name = &placeholder[..end];
            let Some(value) = variables.get(name) else {
                return Err(format!(
                    "Query variable `${{{name}}}` is not set; add @set {name}=value"
                ));
            };
            sql.push_str(value);
            remaining = &placeholder[end + 1..];
        }
        sql.push_str(remaining);
    }
    Ok(sql)
}

/// Return the statement containing the byte offset `cursor`. When the cursor
/// sits in blank space between statements, prefer the nearest non-empty
/// statement before it, then after it.
/// The occurrence of `needle` in `text` closest to `cursor`, so a
/// statement resolved by content lands on the instance the user acted
/// on when the buffer holds identical twins.
fn nearest_occurrence(text: &str, needle: &str, cursor: usize) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    text.match_indices(needle)
        .map(|(start, _)| {
            let end = start + needle.len();
            let distance = if cursor < start {
                start - cursor
            } else {
                cursor.saturating_sub(end)
            };
            (distance, start)
        })
        .min()
        .map(|(_, start)| start)
}

fn statement_at_cursor(text: &str, cursor: usize) -> Option<&str> {
    let segments = split_statements(text);
    let mut idx = segments
        .iter()
        .position(|&(start, end)| cursor >= start && cursor <= end)
        .unwrap_or(segments.len() - 1);
    // A caret just after a statement's semicolon is byte-wise the
    // first position of the NEXT segment, but visually still on the
    // finished statement's line (End, arrow-up to line end, click
    // past the semicolon). If everything on this line behind the
    // caret is a finished statement, it is the one meant.
    if idx > 0 && cursor <= text.len() {
        let line_start = text[..cursor].rfind('\n').map(|pos| pos + 1).unwrap_or(0);
        let line_before_cursor = text[line_start..cursor].trim_end();
        if line_before_cursor.ends_with(';') && segments[idx].0 >= line_start {
            idx -= 1;
        }
    }
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

fn max_rows_from_limit(limit: Option<usize>) -> MaxRows {
    match limit {
        Some(1_000) => MaxRows::Rows1k,
        Some(10_000) => MaxRows::Rows10k,
        Some(50_000) => MaxRows::Rows50k,
        Some(1_000_000) => MaxRows::Rows1m,
        None => MaxRows::Unlimited,
        _ => MaxRows::Rows100k,
    }
}

fn tab_display_name(tab: &QueryTab) -> String {
    tab.tail
        .as_ref()
        .map(|tail| format!("Tail {}", tail.number))
        .unwrap_or_else(|| tab.name.clone())
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
    use super::{
        format_engine_definition, max_rows_from_limit, resolve_query_variables, split_statements,
        statement_at_cursor, MaxRows,
    };

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

    fn membership(cluster: &str, shard: u64, replica: u64) -> zedb_ch::ClusterMembership {
        zedb_ch::ClusterMembership {
            cluster: cluster.into(),
            shard,
            replica,
        }
    }

    #[test]
    fn differentiating_cluster_finds_shard_splits_only() {
        // Replicas of the same shard: interchangeable.
        let node_a = vec![membership("default", 1, 1), membership("main", 1, 1)];
        let node_b = vec![membership("default", 1, 1), membership("main", 1, 2)];
        assert_eq!(super::differentiating_cluster(&node_a, &node_b), None);

        // Same nodes also form a sharded cluster: that one differentiates,
        // even though they replicate each other elsewhere.
        let node_a = vec![membership("main", 1, 1), membership("sharded", 1, 1)];
        let node_b = vec![membership("main", 1, 2), membership("sharded", 2, 1)];
        assert_eq!(
            super::differentiating_cluster(&node_a, &node_b),
            Some("sharded".into())
        );

        // Unknown topology (empty memberships) never differentiates.
        assert_eq!(super::differentiating_cluster(&[], &node_b), None);
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
    fn query_variables_are_removed_and_substituted() {
        let text = "@set db=KPARTS\nselect count() from ${db}.ContactsDim";
        assert_eq!(
            resolve_query_variables(text, text).unwrap(),
            "\nselect count() from KPARTS.ContactsDim"
        );
    }

    #[test]
    fn query_variables_apply_to_a_selected_statement() {
        let editor = "@set db=KPARTS\nselect 1;\nselect count() from ${db}.ContactsDim";
        let selected = "select count() from ${db}.ContactsDim";
        assert_eq!(
            resolve_query_variables(selected, editor).unwrap(),
            "select count() from KPARTS.ContactsDim"
        );
    }

    #[test]
    fn query_variables_report_invalid_or_missing_values() {
        assert_eq!(
            resolve_query_variables("select ${db}.table", "select ${db}.table").unwrap_err(),
            "Query variable `${db}` is not set; add @set db=value"
        );
        assert_eq!(
            resolve_query_variables("@set db\nselect 1", "@set db\nselect 1").unwrap_err(),
            "Invalid query variable on line 1: use @set name=value"
        );
    }

    #[test]
    fn saved_tab_row_limits_restore_to_the_matching_choice() {
        assert!(matches!(max_rows_from_limit(Some(1_000)), MaxRows::Rows1k));
        assert!(matches!(
            max_rows_from_limit(Some(100_000)),
            MaxRows::Rows100k
        ));
        assert!(matches!(max_rows_from_limit(None), MaxRows::Unlimited));
        assert!(matches!(max_rows_from_limit(Some(123)), MaxRows::Rows100k));
    }

    #[test]
    fn statement_at_cursor_picks_statement_under_cursor() {
        let text = "SELECT 1;\nSELECT 2;\nSELECT 3";
        assert_eq!(statement_at_cursor(text, 3), Some("SELECT 1"));
        assert_eq!(statement_at_cursor(text, 12), Some("SELECT 2"));
        assert_eq!(statement_at_cursor(text, text.len()), Some("SELECT 3"));
    }

    #[test]
    fn statement_at_cursor_end_of_line_stays_on_that_line() {
        // The caret after a statement's semicolon (End, arrow-up to a
        // shorter line's end) is byte-wise inside the next segment but
        // visually on the finished statement's line; it must pick the
        // statement on its own line, not the neighbor below.
        let text = "DESCRIBE sat.arrayValues;\ndescribe sat.complexTypes;";
        let after_first_semicolon = text.find(';').unwrap() + 1;
        assert_eq!(
            statement_at_cursor(text, after_first_semicolon),
            Some("DESCRIBE sat.arrayValues")
        );
        // Same-line trailing whitespace after the semicolon too.
        let text = "SELECT 1;  \nSELECT 2;";
        assert_eq!(statement_at_cursor(text, 10), Some("SELECT 1"));
        assert_eq!(statement_at_cursor(text, 11), Some("SELECT 1"));
        // But the start of the NEXT line belongs to the next statement.
        assert_eq!(statement_at_cursor(text, 12), Some("SELECT 2"));
        // And a second statement beginning on the same line after the
        // semicolon still wins once the caret is inside it.
        let text = "SELECT 1; SELECT 2;";
        assert_eq!(statement_at_cursor(text, 14), Some("SELECT 2"));
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
