use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use zedb_ch::checks::{check_equivalence, check_sql};
use zedb_ch::ephemeral::EphemeralServer;
use zedb_ch::regen::{diff_tree, write_tree, Regenerator};
use zedb_ch::runner::{Runner, RunnerOptions, Targets};
use zedb_ch::ChConfig;
use zedb_core::repo::{import_repo, MigrationRepo};

fn ancestor() -> Option<PathBuf> {
    let path = dirs::home_dir()?.join("code/analytics/analytics-clickhouse-ddl");
    path.join("migrations").is_dir().then_some(path)
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

/// Proves the real ancestor repo imports without hand editing, that
/// regen is stable over it, and that sql + equivalence checks pass on the
/// imported repo. Requires both a local analytics-clickhouse-ddl checkout
/// and a cached ClickHouse binary (`zedb pin`); skips silently otherwise,
/// leaving import against real-world SQL unverified.
#[test]
fn ancestor_repo_imports_and_passes_all_checks() {
    let Some(ancestor) = ancestor() else {
        eprintln!("skipping: no analytics-clickhouse-ddl checkout");
        return;
    };
    let Some(binary) = any_cached_binary() else {
        eprintln!("skipping: no cached clickhouse binary (run `zedb pin`)");
        return;
    };

    let dir = tempfile::tempdir().unwrap();
    let destination = dir.path().join("imported");
    let report = import_repo(&ancestor, &destination).unwrap();
    assert!(report.migrations >= 2, "{}", report.migrations);
    assert!(!report.engine_version.is_empty());

    let repo = MigrationRepo::open_root(&destination).unwrap();
    let files = Regenerator::new(&repo, binary.clone())
        .regenerate()
        .unwrap();
    write_tree(&repo, &files).unwrap();
    assert!(diff_tree(&repo, &files).unwrap().is_empty());

    // Rerun is byte-stable on the real chain.
    let again = Regenerator::new(&repo, binary.clone())
        .regenerate()
        .unwrap();
    assert_eq!(files, again);

    let sql = check_sql(&binary, &repo).unwrap();
    assert!(sql.errors.is_empty(), "{:?}", sql.errors);
    let equivalence = check_equivalence(binary, &repo).unwrap();
    assert!(
        equivalence.differences.is_empty(),
        "{:?}",
        equivalence.differences
    );
    assert_eq!(equivalence.state_objects, equivalence.chain_objects);
}

/// Ancestor tracking rows carry over: argMax-latest state survives the
/// copy, so status agrees with what the ancestor tooling recorded, and a
/// second import refuses rather than double-counting. Requires a cached
/// ClickHouse binary (`zedb pin`); skips silently without one, leaving
/// tracking import unverified.
#[tokio::test]
async fn tracking_rows_import_and_preserve_state() {
    let Some(binary) = any_cached_binary() else {
        eprintln!("skipping: no cached clickhouse binary (run `zedb pin`)");
        return;
    };
    let server = EphemeralServer::start(&binary).unwrap();
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../zedb-core/tests/fixtures/demo-repo");
    let repo = MigrationRepo::open_root(&fixture).unwrap();
    let client = zedb_ch::ChClient::new(ChConfig {
        url: server.http_url.clone(),
        user: "default".into(),
        password: None,
        database: None,
        read_only: false,
        driver: Default::default(),
    });

    // An ancestor-shaped tracking table with history: demo_a upgraded
    // through 00100, demo_b upgraded then rolled back 00100.
    client
        .execute(
            "CREATE TABLE default.schema_migrations (\
             db String, migration UInt32, action LowCardinality(String), \
             status LowCardinality(String), error Nullable(String), \
             recorded_at DateTime64(3), duration_secs Decimal(9,2), run_id UUID) \
             ENGINE = MergeTree ORDER BY (db, migration)",
        )
        .await
        .unwrap();
    client
        .execute(
            "INSERT INTO default.schema_migrations VALUES \
             ('demo_a', 0, 'upgrade', 'success', NULL, '2026-01-01 10:00:00', 1.0, generateUUIDv4()), \
             ('demo_a', 100, 'upgrade', 'success', NULL, '2026-01-01 10:01:00', 1.0, generateUUIDv4()), \
             ('demo_b', 0, 'upgrade', 'success', NULL, '2026-01-01 10:00:00', 1.0, generateUUIDv4()), \
             ('demo_b', 100, 'upgrade', 'success', NULL, '2026-01-01 10:01:00', 1.0, generateUUIDv4()), \
             ('demo_b', 100, 'rollback', 'success', NULL, '2026-01-02 09:00:00', 1.0, generateUUIDv4())",
        )
        .await
        .unwrap();

    let runner = Runner::new(
        &repo,
        RunnerOptions {
            server: ChConfig {
                url: server.http_url.clone(),
                user: "default".into(),
                password: None,
                database: None,
                read_only: false,
                driver: Default::default(),
            },
            admin: None,
            cluster: None,
            no_cluster: true,
            write: true,
            dry_run: false,
            overrides: BTreeMap::new(),
        },
    );
    let imported = runner
        .import_tracking("default.schema_migrations")
        .await
        .unwrap();
    assert_eq!(imported, 5);

    let statuses = runner
        .status(&Targets::Databases(vec!["demo_a".into(), "demo_b".into()]))
        .await
        .unwrap();
    assert_eq!(statuses[0].head, Some(100), "demo_a is fully upgraded");
    assert!(statuses[0].pending.is_empty());
    assert_eq!(statuses[1].head, Some(0), "demo_b's rollback carried over");
    assert_eq!(statuses[1].pending, [100]);

    // A second import refuses rather than double-counting.
    let error = runner
        .import_tracking("default.schema_migrations")
        .await
        .expect_err("must refuse on existing rows")
        .to_string();
    assert!(error.contains("refusing"), "{error}");
}

/// The lifecycle check passes on the real imported repo: clustered
/// ephemeral server, restricted migrator with admin routing, rollback
/// walk, and a clean final schema diff. Requires both a local
/// analytics-clickhouse-ddl checkout and a cached ClickHouse binary
/// (`zedb pin`); skips silently otherwise, leaving the clustered
/// lifecycle path unverified.
#[tokio::test]
async fn ancestor_repo_passes_the_lifecycle_check() {
    let Some(ancestor) = ancestor() else {
        eprintln!("skipping: no analytics-clickhouse-ddl checkout");
        return;
    };
    let Some(binary) = any_cached_binary() else {
        eprintln!("skipping: no cached clickhouse binary (run `zedb pin`)");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let destination = dir.path().join("imported");
    import_repo(&ancestor, &destination).unwrap();
    let repo = MigrationRepo::open_root(&destination).unwrap();
    let files = Regenerator::new(&repo, binary.clone())
        .regenerate()
        .unwrap();
    write_tree(&repo, &files).unwrap();

    let report = zedb_ch::lifecycle::check_lifecycle(&repo, binary)
        .await
        .unwrap();
    assert!(report.differences.is_empty(), "{:?}", report.differences);
    assert_eq!(report.expected_objects, report.live_objects);
    assert!(
        report
            .steps
            .iter()
            .any(|step| step.contains("without admin credentials failed")),
        "the real chain must exercise admin routing: {:?}",
        report.steps
    );
}
