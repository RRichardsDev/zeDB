//! Scaffolding for `#[gpui::test]` window tests: a headless app themed
//! and key-bound like the real one, a Workspace built without launch
//! side effects, and fixture helpers. Window tests live next to the
//! feature they exercise (see tests/README.md); this module only holds
//! what they share.

use gpui::{Entity, TestAppContext, VisualTestContext};

use crate::components::text_input;
use crate::{apply_theme_preference, Workspace};

/// A Workspace in a headless test window, with the real theme and
/// text-input key bindings installed. The returned context drives the
/// window: `simulate_keystrokes`, `simulate_input`, focus, and redraws.
pub(crate) fn workspace(cx: &mut TestAppContext) -> (Entity<Workspace>, &mut VisualTestContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
        apply_theme_preference(Some("dark"), None, cx);
        text_input::init(cx);
    });
    cx.add_window_view(|window, cx| Workspace::new_for_test(window, cx))
}

/// A minimal format-1 migration repo in a temp directory. Keep the
/// TempDir alive for as long as the repo is in use.
pub(crate) fn migration_repo() -> (
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
    let repo = zedb_core::repo::MigrationRepo::open_root(dir.path()).expect("open fixture repo");
    (dir, std::sync::Arc::new(repo))
}

/// A ConnectedCluster pointing at a name that is absent from the saved
/// connections list, so tier resolution fails closed to Production and
/// the confirmation ladder demands its typed phrase.
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
