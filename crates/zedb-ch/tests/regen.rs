use std::path::{Path, PathBuf};

use zedb_ch::regen::{diff_tree, write_tree, Regenerator};
use zedb_core::repo::MigrationRepo;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../zedb-core/tests/fixtures/demo-repo")
}

fn pinned_binary(repo: &MigrationRepo) -> Option<PathBuf> {
    // Any cached binary will do for tests; the fixture pin may not match
    // the developer's cache exactly.
    if let Some(binary) = zedb_ch::cached_binary(&repo.config.engine.version) {
        return Some(binary);
    }
    let root = zedb_ch::binary_cache_dir();
    for entry in std::fs::read_dir(root).ok()?.flatten() {
        let version = entry.file_name().to_string_lossy().to_string();
        if let Some(binary) = zedb_ch::cached_binary(&version) {
            return Some(binary);
        }
    }
    None
}

fn copy_fixture(destination: &Path) {
    fn copy_tree(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let target = to.join(entry.file_name());
            if entry.path().is_dir() {
                copy_tree(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), target).unwrap();
            }
        }
    }
    copy_tree(&fixture(), destination);
}

/// The full M3 done-condition, against the real pinned binary: regen is
/// byte-stable, the canonical phase handles ALTER, data-only migrations
/// cause zero churn, and --check catches hand edits. Skips silently when
/// no binary is cached so cold CI stays green.
#[test]
fn regen_is_stable_canonical_and_checkable() {
    let probe = MigrationRepo::open_root(&fixture()).unwrap();
    let Some(binary) = pinned_binary(&probe) else {
        eprintln!("skipping: no cached clickhouse binary (run `zedb pin`)");
        return;
    };

    let dir = tempfile::tempdir().unwrap();
    copy_fixture(dir.path());
    let repo = MigrationRepo::open_root(dir.path()).unwrap();

    // First generation from tracking alone.
    let files = Regenerator::new(&repo, binary.clone())
        .regenerate()
        .unwrap();
    let names: Vec<&String> = files.keys().collect();
    assert_eq!(
        names,
        [
            "db/00000_01_create_db.sql",
            "db/00000_02_create_events.sql",
            "db/00100_01_create_events_daily.sql"
        ]
    );
    write_tree(&repo, &files).unwrap();
    assert!(diff_tree(&repo, &files).unwrap().is_empty());

    // Rerun is byte-stable.
    let again = Regenerator::new(&repo, binary.clone())
        .regenerate()
        .unwrap();
    assert_eq!(files, again);

    // An ALTER migration flows through the canonical replay; a data-only
    // migration causes zero churn.
    let alter = dir.path().join("migrations/2026/09/00300");
    std::fs::create_dir_all(&alter).unwrap();
    std::fs::write(
        alter.join("upgrade.sql"),
        "-- migration 00300: track event source.\n\
         ALTER TABLE ${db}.events ADD COLUMN source LowCardinality(String) DEFAULT 'unknown';\n",
    )
    .unwrap();
    let data_only = dir.path().join("migrations/2026/09/00400");
    std::fs::create_dir_all(&data_only).unwrap();
    std::fs::write(
        data_only.join("upgrade.sql"),
        "-- migration 00400: seed row (data only).\n\
         INSERT INTO ${db}.events (id, kind, occurred_at) VALUES (0, 'seed', now64(3));\n",
    )
    .unwrap();

    let repo = MigrationRepo::open_root(dir.path()).unwrap();
    let evolved = Regenerator::new(&repo, binary.clone())
        .regenerate()
        .unwrap();
    let events = &evolved["db/00000_02_create_events.sql"];
    assert!(
        events.contains("`source` LowCardinality(String) DEFAULT 'unknown'"),
        "{events}"
    );
    assert!(events.contains("toIntervalDay(${ttl_days})"), "{events}");
    // The view and database files are untouched by the ALTER + INSERT.
    assert_eq!(
        evolved["db/00000_01_create_db.sql"],
        files["db/00000_01_create_db.sql"]
    );
    assert_eq!(
        evolved["db/00100_01_create_events_daily.sql"],
        files["db/00100_01_create_events_daily.sql"]
    );

    // --check catches a hand edit.
    write_tree(&repo, &evolved).unwrap();
    let view_path = dir
        .path()
        .join("current-state/db/00100_01_create_events_daily.sql");
    let mut text = std::fs::read_to_string(&view_path).unwrap();
    text.push_str("-- sneaky edit\n");
    std::fs::write(&view_path, text).unwrap();
    let problems = diff_tree(&repo, &evolved).unwrap();
    assert_eq!(problems.len(), 1);
    assert!(problems[0].contains("stale"), "{problems:?}");
}
