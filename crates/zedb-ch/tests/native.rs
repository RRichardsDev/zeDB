//! Integration test: the native (TCP) transport against an ephemeral
//! ClickHouse server (real binary, temp data dir, random ports).
//!
//! Exercises port discovery (`getServerPort`), the same-server identity
//! check, value-mapping parity with the HTTP RowBinary path, and
//! read-only session enforcement. Skips with a warning when no
//! `clickhouse` binary is found.

use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use zedb_ch::native::NativeClient;
use zedb_ch::{ChClient, ChConfig, ChError};
use zedb_core::Value;

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
    let home = PathBuf::from(std::env::var("HOME").expect("HOME not set"));
    let home_local = home.join(".local/bin/clickhouse");
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

    fn config(&self, read_only: bool) -> ChConfig {
        ChConfig {
            url: format!("http://127.0.0.1:{}", self.http_port),
            user: "default".into(),
            password: None,
            database: None,
            read_only,
            driver: Default::default(),
        }
    }

    async fn wait_ready(&self) {
        let client = ChClient::new(self.config(false));
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

/// The native transport discovers the server's (random) tcp port from the
/// server itself, passes the same-server identity check, and decodes a
/// broad type surface identically to the HTTP RowBinary path. Bool is the
/// one documented divergence: the native protocol reports it as UInt8.
#[tokio::test]
async fn native_query_matches_http_decoding() {
    let Some(binary) = find_clickhouse() else {
        eprintln!("SKIP: no clickhouse binary found (set ZEDB_CLICKHOUSE_BIN or install one)");
        return;
    };
    let server = EphemeralServer::start(&binary);
    server.wait_ready().await;
    let http = ChClient::new(server.config(false));

    http.execute(
        "CREATE TABLE zoo (
            id UInt64,
            name String,
            score Float64,
            small Int8,
            maybe Nullable(String),
            maybe_missing Nullable(UInt32),
            born Date,
            seen DateTime('UTC'),
            precise DateTime64(3, 'UTC'),
            tags Array(String),
            kind Enum8('alpha' = 1, 'beta' = 2),
            lc LowCardinality(String),
            amount Decimal(18, 4),
            id2 UUID,
            addr IPv4,
            addr6 IPv6,
            pair Tuple(String, UInt8),
            attrs Map(String, UInt64)
        ) ENGINE = MergeTree ORDER BY id",
    )
    .await
    .expect("create table");
    http.execute(
        "INSERT INTO zoo VALUES (
            42, 'hello', 2.5, -7,
            'present', NULL,
            '2024-03-01', '2024-03-01 12:30:00', '2024-03-01 12:30:00.125',
            ['a', 'b'],
            'beta', 'low',
            12.3456,
            '550e8400-e29b-41d4-a716-446655440000',
            '192.168.0.1', '2001:db8::1',
            ('x', 9), {'k1': 10}
        )",
    )
    .await
    .expect("insert");

    let cfg = server.config(false);
    let native = NativeClient::connect(&cfg).await.expect("native connect");
    let sql = "SELECT * FROM zoo ORDER BY id";
    let over_native = native.query(sql).await.expect("native query");
    let over_http = http.query_http(sql).await.expect("http query");

    assert_eq!(over_native.rows, over_http.rows);
    assert_eq!(
        over_native
            .columns
            .iter()
            .map(|column| column.name.clone())
            .collect::<Vec<_>>(),
        over_http
            .columns
            .iter()
            .map(|column| column.name.clone())
            .collect::<Vec<_>>(),
    );
    // Spot-check the shapes that need real mapping work.
    let row = &over_native.rows[0];
    assert_eq!(row[0], Value::UInt(42));
    assert_eq!(row[4], Value::String("present".into()));
    assert_eq!(row[5], Value::Null);
    assert_eq!(row[8].to_string(), "2024-03-01 12:30:00.125");
    assert_eq!(row[10], Value::Enum("beta".into()));
    assert_eq!(
        row[12],
        Value::Decimal {
            value: 123456,
            scale: 4
        }
    );
}

/// A read-only config's native session really is read-only: the posture
/// is enforced by the server, exactly like the HTTP path's `readonly=2`.
#[tokio::test]
async fn native_readonly_session_rejects_writes() {
    let Some(binary) = find_clickhouse() else {
        eprintln!("SKIP: no clickhouse binary found (set ZEDB_CLICKHOUSE_BIN or install one)");
        return;
    };
    let server = EphemeralServer::start(&binary);
    server.wait_ready().await;

    let native = NativeClient::connect(&server.config(true))
        .await
        .expect("native connect");
    let refused = native
        .execute("CREATE TABLE nope (id UInt8) ENGINE = Memory")
        .await;
    assert!(
        matches!(refused, Err(ChError::Server { .. })),
        "read-only native session accepted DDL: {refused:?}"
    );
    // Reads still work.
    let result = native.query("SELECT 1").await.expect("read query");
    assert_eq!(result.rows[0][0], Value::UInt(1));
}

/// Opt-in integration coverage for ClickHouse 26.6 `STREAM CURSOR` against
/// the local compose stack. Run with `ZEDB_STREAM_TEST_URL` so ordinary test
/// runs do not require a particular server version.
#[tokio::test]
async fn native_stream_cursor_resumes_and_disconnect_cancels() {
    let Ok(url) = std::env::var("ZEDB_STREAM_TEST_URL") else {
        eprintln!("SKIP: set ZEDB_STREAM_TEST_URL to a ClickHouse 26.6+ HTTP endpoint");
        return;
    };
    let user = std::env::var("ZEDB_STREAM_TEST_USER").unwrap_or_else(|_| "zedb".into());
    let password = std::env::var("ZEDB_STREAM_TEST_PASSWORD").unwrap_or_else(|_| "zedb".into());
    let cfg = ChConfig {
        url,
        user,
        password: Some(password),
        database: None,
        read_only: false,
        driver: Default::default(),
    };
    let http = ChClient::new(cfg.clone());
    let table = format!("default.zedb_stream_test_{}", std::process::id());
    http.execute(&format!("DROP TABLE IF EXISTS {table}"))
        .await
        .expect("drop stale STREAM fixture");
    http.execute(&format!(
        "CREATE TABLE {table} (id UInt64, message String) \
         ENGINE = MergeTree ORDER BY id"
    ))
    .await
    .expect("create STREAM fixture");
    http.execute(&format!(
        "INSERT INTO {table} VALUES (1, 'one'), (2, 'two')"
    ))
    .await
    .expect("seed STREAM fixture");

    let first = NativeClient::connect(&cfg).await.expect("native connect");
    let mut first_rows = Vec::new();
    let mut cursor = None;
    tokio::time::timeout(
        Duration::from_secs(5),
        first.stream_blocks(
            &format!(
                "SELECT _block_number, _block_offset, id, message \
                 FROM {table} STREAM CURSOR {{}} \
                 SETTINGS enable_streaming_queries = 1"
            ),
            |_, rows| {
                for row in rows {
                    cursor = row_cursor(&row);
                    if let Some(Value::UInt(id)) = row.get(2) {
                        first_rows.push(*id);
                    }
                }
                false
            },
        ),
    )
    .await
    .expect("initial STREAM timed out")
    .expect("initial STREAM failed");
    assert_eq!(first_rows, vec![1, 2]);
    let (block_number, block_offset) = cursor.expect("STREAM returned no cursor");
    drop(first);
    wait_for_stream_exit(&http, &table).await;

    http.execute(&format!("INSERT INTO {table} VALUES (3, 'three')"))
        .await
        .expect("insert resumed row");
    let resumed = NativeClient::connect(&cfg).await.expect("native reconnect");
    let mut resumed_rows = Vec::new();
    tokio::time::timeout(
        Duration::from_secs(5),
        resumed.stream_blocks(
            &format!(
                "SELECT _block_number, _block_offset, id, message \
                 FROM {table} STREAM CURSOR {{'all': {{'block_number': {block_number}, \
                 'block_offset': {block_offset}}}}} \
                 SETTINGS enable_streaming_queries = 1"
            ),
            |_, rows| {
                resumed_rows.extend(rows.iter().filter_map(|row| match row.get(2) {
                    Some(Value::UInt(id)) => Some(*id),
                    _ => None,
                }));
                false
            },
        ),
    )
    .await
    .expect("resumed STREAM timed out")
    .expect("resumed STREAM failed");
    assert_eq!(resumed_rows, vec![3]);
    drop(resumed);
    wait_for_stream_exit(&http, &table).await;
    http.execute(&format!("DROP TABLE {table}"))
        .await
        .expect("drop STREAM fixture");
}

fn row_cursor(row: &[Value]) -> Option<(u64, u64)> {
    let unsigned = |value: &Value| match value {
        Value::UInt(value) => Some(*value),
        Value::Int(value) => u64::try_from(*value).ok(),
        _ => None,
    };
    Some((unsigned(row.first()?)?, unsigned(row.get(1)?)?))
}

async fn wait_for_stream_exit(http: &ChClient, table: &str) {
    let escaped = table.replace('\'', "''");
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let result = http
            .query_http(&format!(
                "SELECT count() FROM system.processes \
                 WHERE startsWith(query, 'SELECT _block_number') \
                   AND query LIKE '%FROM {escaped} STREAM CURSOR%'"
            ))
            .await
            .expect("query system.processes");
        if result.rows[0][0].to_string() == "0" {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("STREAM query for {table} remained in system.processes after disconnect");
}
