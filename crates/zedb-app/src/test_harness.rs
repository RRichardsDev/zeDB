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
        let directory = dir
            .path()
            .join("migrations")
            .join("2026")
            .join("08")
            .join(format!("{number:05}"));
        std::fs::create_dir_all(&directory).expect("migration dir");
        std::fs::write(
            directory.join("upgrade.sql"),
            "CREATE TABLE fixture (id UInt64) ENGINE = MergeTree ORDER BY id;\n",
        )
        .expect("write upgrade.sql");
        if let Some(class) = rollback {
            std::fs::write(
                directory.join("rollback.sql"),
                format!("-- rollback-class: {class}\nDROP TABLE fixture;\n"),
            )
            .expect("write rollback.sql");
        }
    }
    let repo = zedb_core::repo::MigrationRepo::open_root(dir.path()).expect("open fixture repo");
    (dir, std::sync::Arc::new(repo))
}

/// A ConnectedCluster for tests. Combined with `saved_connection` under
/// the same name it resolves to that connection's tier; alone, tier
/// resolution fails closed to Production.
pub(crate) fn connected_cluster(name: &str) -> crate::ConnectedCluster {
    crate::ConnectedCluster {
        name: name.to_string(),
        active_node: 0,
        active_endpoint: "http://127.0.0.1:8123".to_string(),
        client_config: zedb_ch::ChConfig {
            url: "http://127.0.0.1:8123".to_string(),
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
            endpoint: "http://127.0.0.1:8123".to_string(),
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
