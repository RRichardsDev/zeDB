//! Query-log analytics against a real ephemeral server: fingerprints
//! aggregate, drill-in runs return, and the testimony carries
//! ProfileEvents. Skips without a trusted cached binary, like every
//! replay-backed test.

use zedb_ch::analytics::AnalyticsWindow;
use zedb_ch::ephemeral::EphemeralServer;
use zedb_ch::test_support::{any_cached_binary, http_query};
use zedb_ch::{ChClient, ChConfig};

fn client_for(server: &EphemeralServer) -> ChClient {
    ChClient::new(ChConfig {
        url: server.http_url.clone(),
        user: "default".into(),
        password: None,
        database: None,
        read_only: false,
        driver: Default::default(),
        native_port: None,
    })
}

#[tokio::test]
async fn fingerprints_runs_and_testimony_round_trip() {
    let Some(binary) = any_cached_binary() else {
        eprintln!("skipping: no cached clickhouse binary (run `zedb pin`)");
        return;
    };
    let server = EphemeralServer::start(&binary).unwrap();

    // A recognizable workload: the same shape three times with
    // different literals (one fingerprint, three runs), plus a failure.
    for n in [1, 2, 3] {
        http_query(&server, &format!("SELECT count() FROM numbers({n}000)"));
    }
    // An error run: table does not exist. Sent raw; http_query panics
    // on non-200 and this one must fail.
    let _ = std::panic::catch_unwind(|| http_query(&server, "SELECT * FROM no_such_table_zedb"));
    http_query(&server, "SYSTEM FLUSH LOGS");

    let client = client_for(&server);
    let fingerprints = client
        .query_fingerprints(AnalyticsWindow::LastHour, None)
        .await
        .expect("fingerprints aggregate");
    assert!(!fingerprints.is_empty());

    let numbers = fingerprints
        .iter()
        .find(|fingerprint| fingerprint.sample.contains("numbers"))
        .expect("the numbers() shape is fingerprinted");
    assert_eq!(numbers.runs, 3, "three literals, one shape");
    assert!(numbers.sample.contains('?'), "literals are normalized");
    assert!(numbers.p50_ms <= numbers.p99_ms);

    let failed = fingerprints
        .iter()
        .find(|fingerprint| fingerprint.sample.contains("no_such_table_zedb"))
        .expect("the failing shape is fingerprinted");
    assert!(failed.errors >= 1, "the failure is counted as an error");

    let runs = client
        .fingerprint_runs(&numbers.hash, AnalyticsWindow::LastHour, None)
        .await
        .expect("runs drill in");
    assert_eq!(runs.len(), 3);
    assert!(runs.iter().all(|run| run.exception.is_empty()));

    let testimony = client
        .query_testimony(&runs[0].query_id, None)
        .await
        .expect("testimony fetch")
        .expect("query_log has the run");
    assert!(testimony.query.contains("numbers"));
    assert!(
        !testimony.events.is_empty(),
        "a finished query carries ProfileEvents"
    );
    assert!(
        testimony.events.windows(2).all(|w| w[0].1 >= w[1].1),
        "events arrive largest first"
    );

    // A bogus drill-in key never reaches SQL.
    assert!(client
        .fingerprint_runs("evil'; --", AnalyticsWindow::LastHour, None)
        .await
        .is_err());
}
