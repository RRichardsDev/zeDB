//! Integration test: waiting out a refreshable materialized view against
//! a real ephemeral ClickHouse server.
//!
//! What it proves is a timing claim, so it has to be a real server:
//! `SYSTEM REFRESH VIEW` returns while the view is still rebuilding, and
//! `ChClient::wait_for_refresh` holds until it has finished and reports
//! the refresh's own failure.
//!
//! Skips with a warning when no `clickhouse` binary is found; CI installs
//! one so the skip only ever happens on unprovisioned dev machines.

use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use zedb_ch::refresh::RefreshTarget;
use zedb_ch::{ChClient, ChConfig};

/// The refresh rebuilds for this long: long enough that a statement
/// returning before it finishes is unambiguous, short enough to keep the
/// suite quick.
const REFRESH_ROWS: u64 = 10;
const REFRESH_ROW_SECONDS: f64 = 0.2;

fn refresh_duration() -> Duration {
    Duration::from_secs_f64(REFRESH_ROWS as f64 * REFRESH_ROW_SECONDS)
}

fn find_clickhouse() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("ZEDB_CLICKHOUSE_BIN") {
        return Some(PathBuf::from(path));
    }
    if let Ok(output) = Command::new("which").arg("clickhouse").output() {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                return Some(PathBuf::from(path));
            }
        }
    }
    let home_local = PathBuf::from(std::env::var("HOME").ok()?).join(".local/bin/clickhouse");
    home_local.exists().then_some(home_local)
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

struct EphemeralServer {
    child: Child,
    _dir: tempfile::TempDir,
    http_port: u16,
}

impl EphemeralServer {
    fn start(binary: &PathBuf) -> Self {
        let dir = tempfile::tempdir().expect("create temp dir");
        let http_port = free_port();
        let tcp_port = free_port();
        let path = dir.path();

        let users_xml = path.join("users.xml");
        std::fs::write(
            &users_xml,
            r#"<clickhouse>
    <profiles><default/></profiles>
    <users>
        <default>
            <password></password>
            <networks><ip>127.0.0.1</ip><ip>::1</ip></networks>
            <profile>default</profile>
            <quota>default</quota>
            <access_management>1</access_management>
        </default>
    </users>
    <quotas><default/></quotas>
</clickhouse>
"#,
        )
        .unwrap();

        let config_xml = path.join("config.xml");
        std::fs::write(
            &config_xml,
            format!(
                r#"<clickhouse>
    <logger><level>warning</level><console>1</console></logger>
    <listen_host>127.0.0.1</listen_host>
    <http_port>{http_port}</http_port>
    <tcp_port>{tcp_port}</tcp_port>
    <path>{data}/</path>
    <tmp_path>{data}/tmp/</tmp_path>
    <user_files_path>{data}/user_files/</user_files_path>
    <user_directories>
        <users_xml><path>{users}</path></users_xml>
    </user_directories>
</clickhouse>
"#,
                data = path.join("data").display(),
                users = users_xml.display(),
            ),
        )
        .unwrap();

        let child = Command::new(binary)
            .arg("server")
            .arg("--config-file")
            .arg(&config_xml)
            // Without this ClickHouse forks a watchdog parent; killing
            // the spawned pid on Drop would orphan the real server.
            .env("CLICKHOUSE_WATCHDOG_ENABLE", "0")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn clickhouse server");

        Self {
            child,
            _dir: dir,
            http_port,
        }
    }

    fn client(&self) -> ChClient {
        ChClient::new(ChConfig {
            url: format!("http://127.0.0.1:{}", self.http_port),
            user: "default".into(),
            password: None,
            database: None,
            read_only: false,
            driver: Default::default(),
            native_port: None,
        })
    }

    async fn wait_ready(&self) {
        let client = self.client();
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if client.ping().await {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("ephemeral clickhouse server did not become ready in 30s");
    }
}

impl Drop for EphemeralServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn target(view: &str) -> RefreshTarget {
    RefreshTarget {
        database: Some("rv".to_string()),
        view: view.to_string(),
    }
}

/// A refreshable MV over `select`, refreshed only on demand (a year
/// apart), and settled after the refresh its creation kicks off.
async fn create_view(client: &ChClient, view: &str, select: &str) {
    client
        .execute(&format!(
            "CREATE MATERIALIZED VIEW rv.{view} \
             REFRESH EVERY 1 YEAR \
             ENGINE = MergeTree ORDER BY tuple() \
             AS {select} \
             SETTINGS allow_experimental_refreshable_materialized_view = 1"
        ))
        .await
        .expect("create refreshable view");
    // The creation refresh runs straight away; let it finish so the test
    // is only ever looking at the refresh it asks for itself.
    let _ = client.wait_for_refresh(&target(view), None, |_| {}).await;
}

#[tokio::test]
async fn refresh_wait_holds_until_the_view_has_rebuilt() {
    let Some(binary) = find_clickhouse() else {
        eprintln!("SKIP: no clickhouse binary found (set ZEDB_CLICKHOUSE_BIN or install one)");
        return;
    };
    let server = EphemeralServer::start(&binary);
    server.wait_ready().await;
    let client = server.client();
    client
        .execute("CREATE DATABASE rv")
        .await
        .expect("create db");
    create_view(
        &client,
        "slow",
        &format!(
            "SELECT number, sleepEachRow({REFRESH_ROW_SECONDS}) AS slept \
             FROM numbers({REFRESH_ROWS})"
        ),
    )
    .await;

    let before = client
        .view_refresh_state(&target("slow"))
        .await
        .expect("read refresh state");
    let started = Instant::now();
    client
        .execute("SYSTEM REFRESH VIEW rv.slow")
        .await
        .expect("refresh view");
    let statement = started.elapsed();
    client
        .wait_for_refresh(&target("slow"), before.as_ref(), |_| {})
        .await
        .expect("wait for refresh");
    let waited = started.elapsed();

    // The bug, pinned: the statement itself comes back while the view is
    // still rebuilding.
    assert!(
        statement < refresh_duration() / 2,
        "SYSTEM REFRESH VIEW returned in {statement:?}, so it waited for \
         the rebuild after all and this test no longer proves anything"
    );
    // The fix: the wait holds for the whole rebuild.
    assert!(
        waited >= refresh_duration(),
        "the wait returned after {waited:?}, before the {:?} rebuild could \
         have finished",
        refresh_duration()
    );
    let after = client
        .view_refresh_state(&target("slow"))
        .await
        .expect("read refresh state")
        .expect("the view has a refresh row");
    assert!(!after.running(), "the view is idle once the wait returns");
    assert!(
        after.last_refresh > before.and_then(|state| state.last_refresh),
        "the refresh the wait returned on is the one it asked for"
    );
}

#[tokio::test]
async fn a_failed_refresh_is_reported_instead_of_lost() {
    let Some(binary) = find_clickhouse() else {
        eprintln!("SKIP: no clickhouse binary found (set ZEDB_CLICKHOUSE_BIN or install one)");
        return;
    };
    let server = EphemeralServer::start(&binary);
    server.wait_ready().await;
    let client = server.client();
    client
        .execute("CREATE DATABASE rv")
        .await
        .expect("create db");
    // Refreshes fine on creation, then fails on demand: the view reads a
    // table that is dropped out from under it.
    client
        .execute("CREATE TABLE rv.source (id UInt64) ENGINE = MergeTree ORDER BY id")
        .await
        .expect("create source");
    create_view(&client, "broken", "SELECT id FROM rv.source").await;
    client
        .execute("DROP TABLE rv.source")
        .await
        .expect("drop source");

    let before = client
        .view_refresh_state(&target("broken"))
        .await
        .expect("read refresh state");
    // The statement is accepted: the failure only exists in the view's
    // refresh state afterwards.
    client
        .execute("SYSTEM REFRESH VIEW rv.broken")
        .await
        .expect("refresh view");
    let error = client
        .wait_for_refresh(&target("broken"), before.as_ref(), |_| {})
        .await
        .expect_err("the failed refresh must surface");
    let message = error.to_string();
    assert!(
        message.contains("rv.broken"),
        "the error names the view: {message}"
    );
    assert!(
        message.to_ascii_lowercase().contains("source"),
        "the error carries the server's own reason: {message}"
    );
}
