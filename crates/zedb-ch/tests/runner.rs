use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use zedb_ch::ephemeral::EphemeralServer;
use zedb_ch::runner::{Runner, RunnerOptions, Targets};
use zedb_ch::ChConfig;
use zedb_core::repo::MigrationRepo;

/// The three tests here each run a real server and interleave heavy
/// DDL. Run concurrently, filtered SELECTs (tracking reads,
/// system.tables) intermittently return empty despite the rows
/// provably existing: see devlog 2026-08-16. Serialize them until
/// that is understood; correctness first.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../zedb-core/tests/fixtures/demo-repo")
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

fn options(server: &EphemeralServer, write: bool) -> RunnerOptions {
    RunnerOptions {
        server: ChConfig {
            url: server.http_url.clone(),
            user: "default".into(),
            password: None,
            database: None,
            read_only: false,
            driver: Default::default(),
            native_port: None,
        },
        admin: None,
        cluster: None,
        no_cluster: true,
        write,
        dry_run: false,
        overrides: BTreeMap::new(),
    }
}

fn dbs(names: &[&str]) -> Targets {
    Targets::Databases(names.iter().map(ToString::to_string).collect())
}

/// Proves the whole runner surface against a real ephemeral server:
/// upgrade, status, peel-from-top and class enforcement, the --to walk,
/// stamp, targeted apply with allow-list, and read-only refusal.
/// Requires a cached ClickHouse binary (`zedb pin`); skips silently
/// without one, leaving every one of those behaviours unverified.
#[tokio::test]
async fn runner_walks_the_chain_on_a_real_server() {
    let _serial = SERIAL.lock().await;
    let Some(binary) = any_cached_binary() else {
        eprintln!("skipping: no cached clickhouse binary (run `zedb pin`)");
        return;
    };
    let server = EphemeralServer::start(&binary).unwrap();
    let repo = MigrationRepo::open_root(&fixture()).unwrap();

    // Without --write, every mutating entry point refuses.
    let readonly = Runner::new(&repo, options(&server, false));
    let refused = readonly.upgrade(&dbs(&["demo_a"]), None).await;
    let message = refused.expect_err("read-only must refuse").to_string();
    assert!(message.contains("--write"), "{message}");

    // Upgrade two databases through the whole fleet chain.
    let runner = Runner::new(&repo, options(&server, true));
    runner
        .upgrade(&dbs(&["demo_a", "demo_b"]), None)
        .await
        .unwrap();
    let statuses = runner.status(&dbs(&["demo_a", "demo_b"])).await.unwrap();
    for status in &statuses {
        assert_eq!(status.head, Some(100), "{}", status.database);
        assert!(status.pending.is_empty(), "{:?}", status.pending);
        assert!(status.failed.is_empty());
    }

    // The events table and view really exist on the server.
    let client = zedb_ch::ChClient::new(options(&server, true).server);
    let result = client
        .query("SELECT count() FROM system.tables WHERE database = 'demo_a'")
        .await
        .unwrap();
    assert_eq!(result.rows[0][0].to_string(), "2");

    // Peel-from-top: rolling back 00000 while 00100 is applied is refused.
    let error = runner
        .rollback_one(&dbs(&["demo_a"]), 0, false, false)
        .await
        .expect_err("00000 is not the head")
        .to_string();
    assert!(error.contains("peel from the top"), "{error}");
    runner
        .rollback_one(&dbs(&["demo_a"]), 100, false, false)
        .await
        .unwrap();
    let statuses = runner.status(&dbs(&["demo_a"])).await.unwrap();
    assert_eq!(statuses[0].head, Some(0));
    assert_eq!(statuses[0].pending, [100]);

    // The --to walk brings demo_b down to the baseline too.
    runner
        .rollback_to(&dbs(&["demo_b"]), 0, false)
        .await
        .unwrap();
    let statuses = runner.status(&dbs(&["demo_b"])).await.unwrap();
    assert_eq!(statuses[0].head, Some(0));

    // Upgrade back to the top; both are current again.
    runner
        .upgrade(&dbs(&["demo_a", "demo_b"]), None)
        .await
        .unwrap();

    // Stamp: a hand-provisioned database is adopted without executing.
    client
        .execute("CREATE DATABASE demo_adopted")
        .await
        .unwrap();
    runner.stamp(&dbs(&["demo_adopted"]), 100).await.unwrap();
    let statuses = runner.status(&dbs(&["demo_adopted"])).await.unwrap();
    assert_eq!(statuses[0].head, Some(100));
    assert!(statuses[0].pending.is_empty());

    // Targeted apply honors the allow list.
    let error = runner
        .apply_targeted(&dbs(&["demo_a"]), 200)
        .await
        .expect_err("demo_a is not in the allow list")
        .to_string();
    assert!(error.contains("demo_pilot"), "{error}");
    client.execute("CREATE DATABASE demo_pilot").await.unwrap();
    runner.upgrade(&dbs(&["demo_pilot"]), None).await.unwrap();
    runner
        .apply_targeted(&dbs(&["demo_pilot"]), 200)
        .await
        .unwrap();
    let statuses = runner.status(&dbs(&["demo_pilot"])).await.unwrap();
    assert_eq!(statuses[0].customised, [200]);
    let result = client
        .query(
            "SELECT count() FROM system.columns \
             WHERE database = 'demo_pilot' AND table = 'events' AND name = 'backfilled'",
        )
        .await
        .unwrap();
    assert_eq!(result.rows[0][0].to_string(), "1");

    // Removing the customisation requires --targeted friction.
    let error = runner
        .rollback_one(&dbs(&["demo_pilot"]), 200, false, false)
        .await
        .expect_err("targeted rollback needs consent")
        .to_string();
    assert!(error.contains("--targeted"), "{error}");
    runner
        .rollback_one(&dbs(&["demo_pilot"]), 200, false, true)
        .await
        .unwrap();

    // Tracking metadata is versioned.
    let meta = client
        .query("SELECT value FROM default.zedb_meta WHERE key = 'tracking_version'")
        .await
        .unwrap();
    assert_eq!(meta.rows[0][0].to_string(), "1");
}

/// Proves that --all discovers from the registry query, skips exclusion
/// groups out loud, and that --group / --db still target them
/// deliberately. Requires a cached ClickHouse binary (`zedb pin`); skips
/// silently without one, leaving fleet targeting unverified.
#[tokio::test]
async fn fleet_targeting_discovers_and_skips_exclusions() {
    let _serial = SERIAL.lock().await;
    let Some(binary) = any_cached_binary() else {
        eprintln!("skipping: no cached clickhouse binary (run `zedb pin`)");
        return;
    };
    let server = EphemeralServer::start(&binary).unwrap();
    let repo = MigrationRepo::open_root(&fixture()).unwrap();
    let client = zedb_ch::ChClient::new(options(&server, true).server);
    for database in ["demo_one", "demo_two", "demo_frozen"] {
        client
            .execute(&format!("CREATE DATABASE {database}"))
            .await
            .unwrap();
    }

    let runner = Runner::new(&repo, options(&server, true));
    let resolved = runner.resolve_targets(&Targets::All).await.unwrap();
    assert_eq!(resolved.databases, ["demo_one", "demo_two"]);
    assert_eq!(
        resolved.skipped,
        [("demo_frozen".to_string(), "frozen".to_string())]
    );

    // --all upgrades only the non-excluded databases.
    runner.upgrade(&Targets::All, None).await.unwrap();
    let statuses = runner
        .status(&dbs(&["demo_one", "demo_two", "demo_frozen"]))
        .await
        .unwrap();
    assert_eq!(statuses[0].head, Some(100));
    assert_eq!(statuses[1].head, Some(100));
    assert_eq!(
        statuses[2].head, None,
        "excluded database must be untouched"
    );

    // --group targets the excluded database deliberately.
    let group = runner
        .resolve_targets(&Targets::Group("frozen".into()))
        .await
        .unwrap();
    assert_eq!(group.databases, ["demo_frozen"]);
    runner
        .upgrade(&Targets::Group("frozen".into()), None)
        .await
        .unwrap();
    let statuses = runner.status(&dbs(&["demo_frozen"])).await.unwrap();
    assert_eq!(statuses[0].head, Some(100));

    // An unknown group is a readable error.
    let error = runner
        .resolve_targets(&Targets::Group("nope".into()))
        .await
        .map(|resolved| resolved.databases)
        .expect_err("unknown group")
        .to_string();
    assert!(error.contains("frozen"), "{error}");
}

/// Proves that a freshly upgraded database verifies clean, that manual
/// drift (added column, foreign table) is detected and named, and that a
/// database behind head verifies clean at its own chain position.
/// Requires a cached ClickHouse binary (`zedb pin`); skips silently
/// without one, leaving drift detection unverified.
#[tokio::test]
async fn verify_detects_drift_at_chain_position() {
    let _serial = SERIAL.lock().await;
    let Some(binary) = any_cached_binary() else {
        eprintln!("skipping: no cached clickhouse binary (run `zedb pin`)");
        return;
    };
    let server = EphemeralServer::start(&binary).unwrap();
    let repo = MigrationRepo::open_root(&fixture()).unwrap();
    let client = zedb_ch::ChClient::new(options(&server, true).server);

    let runner = Runner::new(&repo, options(&server, true));
    runner
        .upgrade(&dbs(&["demo_clean", "demo_drifted", "demo_behind"]), None)
        .await
        .unwrap();
    runner
        .rollback_to(&dbs(&["demo_behind"]), 0, false)
        .await
        .unwrap();

    let verifier = zedb_ch::verify::Verifier::new(&repo, &runner, binary.clone());

    let drifts = verifier
        .verify(&dbs(&["demo_clean", "demo_behind"]))
        .await
        .unwrap();
    for drift in &drifts {
        assert!(
            drift.findings.is_empty(),
            "{}: {:?}",
            drift.database,
            drift.findings
        );
    }
    assert_eq!(drifts[1].head, Some(0), "demo_behind sits at the baseline");

    // Manual drift: an added column and a foreign table.
    client
        .execute("ALTER TABLE demo_drifted.events ADD COLUMN sneaky UInt8")
        .await
        .unwrap();
    client
        .execute("CREATE TABLE demo_drifted.rogue (x UInt8) ENGINE = Memory")
        .await
        .unwrap();
    let drifts = verifier.verify(&dbs(&["demo_drifted"])).await.unwrap();
    let findings = &drifts[0].findings;
    assert_eq!(findings.len(), 2, "{findings:?}");
    assert!(
        findings
            .iter()
            .any(|finding| finding.contains("drifted: ${db}.events") && finding.contains("sneaky")),
        "{findings:?}"
    );
    assert!(
        findings
            .iter()
            .any(|finding| finding.contains("unexpected") && finding.contains("rogue")),
        "{findings:?}"
    );
}
