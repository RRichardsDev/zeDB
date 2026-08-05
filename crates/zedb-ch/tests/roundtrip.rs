//! Integration test: query round-trip against an ephemeral ClickHouse
//! server (real binary, temp data dir, random ports).
//!
//! Skips with a warning when no `clickhouse` binary is found; CI installs
//! one so the skip only ever happens on unprovisioned dev machines.

use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use chrono::{DateTime, NaiveDate, Utc};
use zedb_ch::{ChClient, ChConfig, ChError, QueryStreamEvent};
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
    let home_local = dirs_home().join(".local/bin/clickhouse");
    home_local.exists().then_some(home_local)
}

fn dirs_home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").expect("HOME not set"))
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
    pub http_port: u16,
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

    fn client(&self) -> ChClient {
        self.client_with(false)
    }

    fn client_with(&self, read_only: bool) -> ChClient {
        ChClient::new(ChConfig {
            url: format!("http://127.0.0.1:{}", self.http_port),
            user: "default".into(),
            password: None,
            database: None,
            read_only,
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

#[tokio::test]
async fn query_roundtrip_type_zoo() {
    let Some(binary) = find_clickhouse() else {
        eprintln!("SKIP: no clickhouse binary found (set ZEDB_CLICKHOUSE_BIN or install one)");
        return;
    };
    let server = EphemeralServer::start(&binary);
    server.wait_ready().await;
    let client = server.client();

    client
        .execute(
            "CREATE TABLE zoo (
                id UInt64,
                name String,
                score Float64,
                flag Bool,
                small Int8,
                maybe Nullable(String),
                maybe_missing Nullable(UInt32),
                born Date,
                seen DateTime('UTC'),
                precise DateTime64(3, 'UTC'),
                tags Array(String),
                nested Array(Nullable(UInt8)),
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

    client
        .execute(
            "INSERT INTO zoo VALUES (
                42, 'hello', 2.5, true, -7,
                'present', NULL,
                '2024-03-01', '2024-03-01 12:30:00', '2024-03-01 12:30:00.125',
                ['a', 'b'], [1, NULL, 3],
                'beta', 'low',
                12.3456,
                '550e8400-e29b-41d4-a716-446655440000',
                '192.168.0.1', '2001:db8::1',
                ('x', 9), {'k1': 10, 'k2': 20}
            )",
        )
        .await
        .expect("insert row");

    let databases = client.list_databases().await.expect("list databases");
    assert!(databases.iter().any(|database| database.name == "default"));

    let objects = client
        .list_schema_objects("default")
        .await
        .expect("list schema objects");
    let zoo = objects
        .iter()
        .find(|object| object.name == "zoo")
        .expect("zoo table appears in schema discovery");
    assert_eq!(zoo.engine, "MergeTree");
    assert_eq!(zoo.kind.label(), "table");

    let columns = client
        .list_columns("default", "zoo")
        .await
        .expect("list columns");
    assert_eq!(columns.len(), 20);
    assert_eq!(columns[0].name, "id");
    assert_eq!(columns[0].type_name, "UInt64");
    assert_eq!(columns[9].type_name, "DateTime64(3, 'UTC')");

    let result = client.query("SELECT * FROM zoo").await.expect("select");

    assert_eq!(result.rows.len(), 1);
    assert_eq!(result.columns.len(), 20);
    assert_eq!(result.columns[0].name, "id");
    assert_eq!(result.columns[9].type_name, "DateTime64(3, 'UTC')");

    let row = &result.rows[0];
    assert_eq!(row[0], Value::UInt(42));
    assert_eq!(row[1], Value::String("hello".into()));
    assert_eq!(row[2], Value::Float(2.5));
    assert_eq!(row[3], Value::Bool(true));
    assert_eq!(row[4], Value::Int(-7));
    assert_eq!(row[5], Value::String("present".into()));
    assert_eq!(row[6], Value::Null);
    assert_eq!(
        row[7],
        Value::Date(NaiveDate::from_ymd_opt(2024, 3, 1).unwrap())
    );
    let expect_dt =
        |s: &str| Value::DateTime(DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc));
    assert_eq!(row[8], expect_dt("2024-03-01T12:30:00Z"));
    assert_eq!(row[9], expect_dt("2024-03-01T12:30:00.125Z"));
    assert_eq!(
        row[10],
        Value::Array(vec![Value::String("a".into()), Value::String("b".into())])
    );
    assert_eq!(
        row[11],
        Value::Array(vec![Value::UInt(1), Value::Null, Value::UInt(3)])
    );
    assert_eq!(row[12], Value::Enum("beta".into()));
    assert_eq!(row[13], Value::String("low".into()));
    assert_eq!(
        row[14],
        Value::Decimal {
            value: 123456,
            scale: 4
        }
    );
    assert_eq!(row[15].to_string(), "550e8400-e29b-41d4-a716-446655440000");
    assert_eq!(row[16], Value::Ipv4("192.168.0.1".parse().unwrap()));
    assert_eq!(row[17], Value::Ipv6("2001:db8::1".parse().unwrap()));
    assert_eq!(
        row[18],
        Value::Tuple(vec![Value::String("x".into()), Value::UInt(9)])
    );
    assert_eq!(
        row[19],
        Value::Map(vec![
            (Value::String("k1".into()), Value::UInt(10)),
            (Value::String("k2".into()), Value::UInt(20)),
        ])
    );

    // Server errors surface with code and message, not as decode garbage.
    let err = client.query("SELECT nope FROM zoo").await.unwrap_err();
    match err {
        ChError::Server { code, message } => {
            assert_eq!(code, Some(47)); // UNKNOWN_IDENTIFIER
            assert!(message.contains("nope"), "message was: {message}");
        }
        other => panic!("expected server error, got: {other:?}"),
    }

    // Empty result sets still carry column metadata.
    let empty = client
        .query("SELECT * FROM zoo WHERE id = 0")
        .await
        .unwrap();
    assert_eq!(empty.rows.len(), 0);
    assert_eq!(empty.columns.len(), 20);

    // A read-only client can select but the server rejects writes.
    let ro = server.client_with(true);
    ro.query("SELECT id FROM zoo")
        .await
        .expect("select allowed");
    let err = ro
        .execute("INSERT INTO zoo (id) VALUES (99)")
        .await
        .unwrap_err();
    match err {
        ChError::Server { code, .. } => assert_eq!(code, Some(164)), // READONLY
        other => panic!("expected readonly rejection, got: {other:?}"),
    }

    // Connection testing must reject credentials the server does not accept.
    let invalid = ChClient::new(ChConfig {
        url: format!("http://127.0.0.1:{}", server.http_port),
        user: "default".into(),
        password: Some("definitely-wrong".into()),
        database: None,
        read_only: true,
    });
    assert!(
        invalid.test_connection().await.is_err(),
        "invalid credentials must fail the authenticated connection test"
    );
}

#[tokio::test]
async fn streaming_query_reports_progress_and_honors_cap() {
    let Some(binary) = find_clickhouse() else {
        eprintln!("SKIP: no clickhouse binary found (set ZEDB_CLICKHOUSE_BIN or install one)");
        return;
    };
    let server = EphemeralServer::start(&binary);
    server.wait_ready().await;
    let client = server.client();

    let mut columns = 0;
    let mut rows = 0;
    let mut batches = 0;
    let mut received_bytes = 0;
    let mut saw_server_progress = false;
    let summary = client
        .query_stream(
            "SELECT number, sleepEachRow(0.0001) FROM numbers(10000)",
            1_000,
            |event| match event {
                QueryStreamEvent::Columns(items) => columns = items.len(),
                QueryStreamEvent::Rows(items) => {
                    rows += items.len();
                    batches += 1;
                }
                QueryStreamEvent::Progress(progress) => {
                    received_bytes = received_bytes.max(progress.received_bytes);
                    saw_server_progress |=
                        progress.read_rows.is_some() && progress.read_bytes.is_some();
                }
            },
        )
        .await
        .expect("stream query");

    assert_eq!(columns, 2);
    assert_eq!(rows, 1_000);
    assert!(batches > 0);
    assert!(received_bytes > 0);
    assert!(saw_server_progress);
    assert_eq!(summary.rows, 1_000);
    assert!(summary.capped);
}
