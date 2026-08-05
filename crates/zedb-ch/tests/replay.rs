use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use zedb_ch::replay::{
    decluster, is_access_control, restore_sentinels, sentinel_params, split_statements,
    LocalReplay, ReplaySide,
};
use zedb_core::repo::MigrationRepo;

#[test]
fn statements_split_on_top_level_semicolons_only() {
    let sql = "SELECT 'a;b'; -- trailing; comment\nSELECT 2;\n/* block; comment */ SELECT 3";
    let statements = split_statements(sql);
    assert_eq!(statements.len(), 3);
    assert!(statements[0].contains("'a;b'"));
    assert!(statements[2].contains("SELECT 3"));
}

#[test]
fn decluster_strips_coordination() {
    let sql = "CREATE TABLE t ON CLUSTER prod (x UInt8) \
               ENGINE = ReplicatedReplacingMergeTree('/zk/t', '{replica}', ver) ORDER BY x";
    let declustered = decluster(sql);
    assert!(!declustered.contains("ON CLUSTER"));
    assert!(declustered.contains("ReplacingMergeTree(ver)"));
}

#[test]
fn access_control_is_recognized_through_comments() {
    assert!(is_access_control(
        "-- grant read\nGRANT SELECT ON db.* TO reader"
    ));
    assert!(is_access_control("CREATE USER u IDENTIFIED BY 'x'"));
    assert!(!is_access_control(
        "CREATE TABLE grants (x UInt8) ENGINE = Memory"
    ));
}

#[test]
fn sentinels_round_trip() {
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../zedb-core/tests/fixtures/demo-repo");
    let repo = MigrationRepo::open_root(&fixture).unwrap();
    let params = sentinel_params(&repo.config);
    let rendered = format!(
        "CREATE TABLE {}.t TTL now() + INTERVAL {} DAY",
        params["db"], params["ttl_days"]
    );
    let restored = restore_sentinels(&rendered, &params);
    assert_eq!(
        restored,
        "CREATE TABLE ${db}.t TTL now() + INTERVAL ${ttl_days} DAY"
    );
}

fn any_cached_binary() -> Option<PathBuf> {
    let root = zedb_ch::binary_cache_dir();
    for entry in std::fs::read_dir(root).ok()?.flatten() {
        let version = entry.file_name().to_string_lossy().to_string();
        if let Some(binary) = zedb_ch::cached_binary(&version) {
            return Some(binary);
        }
    }
    None
}

/// Replays the fixture chain through a real pinned binary; skips silently
/// when no binary is cached so cold CI stays green.
#[test]
fn fixture_chain_dumps_canonical_objects() {
    let Some(binary) = any_cached_binary() else {
        eprintln!("skipping: no cached clickhouse binary (run `zedb pin`)");
        return;
    };
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../zedb-core/tests/fixtures/demo-repo");
    let repo = MigrationRepo::open_root(&fixture).unwrap();
    let texts: Vec<String> = repo
        .migrations
        .iter()
        .filter(|migration| migration.targeted.is_none())
        .map(|migration| migration.upgrade_sql().unwrap())
        .collect();

    let params = sentinel_params(&repo.config);
    let replay = LocalReplay::new(binary);
    let side = ReplaySide {
        name: params["db"].clone(),
        texts,
    };
    let dumps = replay.dump_objects(&[side], &params, &[]).unwrap();

    let objects: BTreeMap<String, String> = dumps[&params["db"]]
        .iter()
        .map(|(name, create)| {
            (
                restore_sentinels(name, &params),
                restore_sentinels(create, &params),
            )
        })
        .collect();

    // The non-targeted chain creates the events table and the daily view.
    assert!(objects.contains_key("${db}.events"), "{objects:?}");
    assert!(objects.contains_key("${db}.events_daily"), "{objects:?}");
    let events = &objects["${db}.events"];
    assert!(events.contains("toIntervalDay(${ttl_days})"), "{events}");
    let view = &objects["${db}.events_daily"];
    assert!(view.contains("CREATE VIEW"), "{view}");
}
