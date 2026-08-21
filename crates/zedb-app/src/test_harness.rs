//! Scaffolding for `#[gpui::test]` window tests: a headless app themed
//! and key-bound like the real one, a Workspace built without launch
//! side effects, and fixture helpers. Window tests live next to the
//! feature they exercise (see tests/README.md); this module only holds
//! what they share.

use gpui::{prelude::*, Entity, TestAppContext, VisualTestContext, Window};
use gpui_component::input::InputState;

use crate::components::text_input;
use crate::{apply_theme_preference, Context, Workspace};

/// Point `ZEDB_CONFIG_DIR` at a per-process temp directory so no test
/// can ever read or write the user's real config files, whatever code
/// path it wanders into. The Keychain has no such override; tests must
/// simply avoid paths that store or fetch secrets (an empty password on
/// a new connection does).
fn sandbox_config_dir() {
    static SANDBOX: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    let dir = SANDBOX.get_or_init(|| tempfile::tempdir().expect("sandbox config dir"));
    std::env::set_var("ZEDB_CONFIG_DIR", dir.path());
}

/// A Workspace in a headless test window, with the real theme, key
/// bindings, and keystroke interceptor installed. The returned context
/// drives the window: `simulate_keystrokes`, `simulate_input`, focus,
/// and redraws.
pub(crate) fn workspace(cx: &mut TestAppContext) -> (Entity<Workspace>, &mut VisualTestContext) {
    sandbox_config_dir();
    cx.update(|cx| {
        gpui_component::init(cx);
        apply_theme_preference(Some("dark"), None, cx);
        text_input::init(cx);
    });
    // Mirror production: the window root is a gpui_component Root
    // wrapping the Workspace (dialogs, notifications, and input focus
    // tracking all reach for it).
    let slot = std::rc::Rc::new(std::cell::RefCell::new(None));
    let filler = slot.clone();
    let (_root, cx) = cx.add_window_view(move |window, cx| {
        let workspace = cx.new(|cx| Workspace::new_for_test(window, cx));
        *filler.borrow_mut() = Some(workspace.clone());
        gpui_component::Root::new(workspace, window, cx)
    });
    let workspace = slot.borrow_mut().take().expect("workspace built");
    (workspace, cx)
}

/// A minimal format-1 migration repo in a temp directory. Keep the
/// TempDir alive for as long as the repo is in use.
pub(crate) fn migration_repo() -> (
    tempfile::TempDir,
    std::sync::Arc<zedb_core::repo::MigrationRepo>,
) {
    migration_repo_with(&[])
}

/// A format-1 migration repo holding the given `(number, rollback)`
/// migrations, dated 2026/08. `rollback: None` means no rollback.sql
/// (irreversible at run time); `Some(class)` declares that class.
pub(crate) fn migration_repo_with(
    migrations: &[(u32, Option<&str>)],
) -> (
    tempfile::TempDir,
    std::sync::Arc<zedb_core::repo::MigrationRepo>,
) {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(
        dir.path().join("zedb.toml"),
        "format = 1\n\n[engine]\nkind = \"clickhouse\"\nversion = \"24.8\"\n",
    )
    .expect("write zedb.toml");
    std::fs::create_dir_all(dir.path().join("migrations")).expect("migrations dir");
    for (number, rollback) in migrations {
        write_migration(dir.path(), *number, *rollback);
    }
    let repo = zedb_core::repo::MigrationRepo::open_root(dir.path()).expect("open fixture repo");
    (dir, std::sync::Arc::new(repo))
}

/// Write one fixture migration into an existing repo root, dated
/// 2026/08, as a pulled commit or hand edit would leave it on disk.
pub(crate) fn write_migration(root: &std::path::Path, number: u32, rollback: Option<&str>) {
    let directory = root
        .join("migrations")
        .join("2026")
        .join("08")
        .join(format!("{number:05}"));
    std::fs::create_dir_all(&directory).expect("migration dir");
    std::fs::write(
        directory.join("upgrade.sql"),
        format!(
            "CREATE TABLE ${{db}}.fixture_{number:05} (id UInt64) \
             ENGINE = MergeTree ORDER BY id;\n"
        ),
    )
    .expect("write upgrade.sql");
    if let Some(class) = rollback {
        std::fs::write(
            directory.join("rollback.sql"),
            format!("-- rollback-class: {class}\nDROP TABLE ${{db}}.fixture_{number:05};\n"),
        )
        .expect("write rollback.sql");
    }
}

/// Port 1 is root-only to bind, so nothing listens there: if a test
/// does let an action past a gate, its runner dies on connection
/// refused instead of reaching a real ClickHouse. Never point test
/// fixtures at 8123/9000; dev machines often have live servers there.
const DEAD_ENDPOINT: &str = "http://127.0.0.1:1";

/// A ConnectedCluster for tests, aimed at a guaranteed-dead endpoint.
/// Combined with `saved_connection` under the same name it resolves to
/// that connection's tier; alone, tier resolution fails closed to
/// Production.
pub(crate) fn connected_cluster(name: &str) -> crate::ConnectedCluster {
    crate::ConnectedCluster {
        name: name.to_string(),
        active_node: 0,
        active_endpoint: DEAD_ENDPOINT.to_string(),
        client_config: zedb_ch::ChConfig {
            url: DEAD_ENDPOINT.to_string(),
            user: "default".to_string(),
            password: None,
            database: None,
            read_only: false,
            driver: Default::default(),
            native_port: None,
        },
        apply_cluster: None,
    }
}

/// A saved connection entry, for the lists tier resolution reads.
pub(crate) fn saved_connection(
    name: &str,
    tier: zedb_core::EnvTier,
    read_only: bool,
) -> zedb_core::ConnectionConfig {
    zedb_core::ConnectionConfig {
        name: name.to_string(),
        nodes: vec![zedb_core::ConnectionNode {
            name: "node1".to_string(),
            endpoint: DEAD_ENDPOINT.to_string(),
            native_port: None,
        }],
        user: "default".to_string(),
        database: None,
        driver: Default::default(),
        tier,
        read_only,
        cloud: None,
    }
}

/// A selected schema object of the given size, as the inspector would
/// hold after loading, for the in-place-apply flows.
pub(crate) fn selected_schema_object(
    database: &str,
    object: &str,
    total_bytes: u64,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> crate::SelectedSchemaObject {
    let ddl_editor = cx.new(|cx| InputState::new(window, cx).code_editor("sql"));
    let engine_editor = cx.new(|cx| InputState::new(window, cx).code_editor("sql"));
    crate::SelectedSchemaObject {
        database: database.to_string(),
        object: zedb_ch::SchemaObjectMeta {
            name: object.to_string(),
            engine: "MergeTree".to_string(),
            kind: zedb_ch::SchemaObjectKind::Table,
            total_rows: Some(1_000),
            total_bytes: Some(total_bytes),
        },
        loading: false,
        columns: Vec::new(),
        details: None,
        storage: None,
        cardinalities: None,
        cardinality_loading: false,
        cardinality_error: None,
        cardinality_confirming: false,
        measured: Default::default(),
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
        workload: None,
        workload_loading: false,
        workload_error: None,
        ddl_editor,
        engine_editor,
        tab: crate::ObjectInspectorTab::Overview,
        error: None,
    }
}

/// Poll a condition while letting the deterministic executor drain,
/// bounded by wall-clock time. For end-to-end tests whose work crosses
/// into the real tokio runtime (network I/O), where run_until_parked
/// alone cannot see pending completions.
pub(crate) fn wait_for<T>(
    cx: &mut VisualTestContext,
    timeout: std::time::Duration,
    mut poll: impl FnMut(&mut VisualTestContext) -> Option<T>,
) -> T {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        cx.run_until_parked();
        if let Some(value) = poll(cx) {
            return value;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for the polled condition"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// The rendered bounds of the element tagged `selector` (via
/// `.debug_selector(...)` in the view code, a no-op in release
/// builds). Drains pending work first so the lookup sees the current
/// state; panics when nothing painted under that tag.
pub(crate) fn bounds(
    cx: &mut VisualTestContext,
    selector: &'static str,
) -> gpui::Bounds<gpui::Pixels> {
    // Force a fresh frame: the deterministic executor's task shuffling
    // means a plain run_until_parked may or may not have repainted
    // since the last state change.
    cx.refresh().expect("schedule redraw");
    cx.run_until_parked();
    cx.debug_bounds(selector)
        .unwrap_or_else(|| panic!("no rendered element tagged {selector:?}"))
}

/// A real click at the center of the element tagged `selector`: mouse
/// down and up through the window's hitboxes, exactly as a user click
/// dispatches.
pub(crate) fn click(cx: &mut VisualTestContext, selector: &'static str) {
    let center = bounds(cx, selector).center();
    // Hover first, as a real pointer would; a bare down/up at a point
    // the window has already seen a click at can be swallowed by
    // element state from the earlier press.
    cx.simulate_mouse_move(center, None, gpui::Modifiers::default());
    cx.simulate_click(center, gpui::Modifiers::default());
}
