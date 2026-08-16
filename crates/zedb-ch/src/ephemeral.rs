//! Throwaway single-node ClickHouse server for tests and the lifecycle
//! check: the pinned binary, a temp data directory, random ports, killed
//! on drop. Nothing it does can touch a real environment.

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug, thiserror::Error)]
pub enum EphemeralError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("server did not answer /ping within {0:?}; log tail:\n{1}")]
    NotReady(Duration, String),
    #[error("{0}")]
    Startup(String),
}

const SERVER_CONFIG: &str = r#"<clickhouse>
    <tcp_port>{tcp}</tcp_port>
    <http_port>{http}</http_port>
    <listen_host>127.0.0.1</listen_host>
    <path>{path}/</path>
    <user_directories>
        <users_xml><path>users.xml</path></users_xml>
        <local_directory><path>{path}/access/</path></local_directory>
    </user_directories>
    <mark_cache_size>268435456</mark_cache_size>
    <logger><level>warning</level><console>1</console></logger>
    {cluster_config}
</clickhouse>
"#;

/// Single-node cluster: the server is its own only replica, with an
/// embedded Keeper for Replicated coordination and the distributed DDL
/// queue. This lets checks run SQL exactly as written (ON CLUSTER,
/// Replicated engines, zk paths) instead of declustering it.
const CLUSTER_CONFIG: &str = r#"<interserver_http_port>{interserver}</interserver_http_port>
    <keeper_server>
        <tcp_port>{keeper}</tcp_port>
        <server_id>1</server_id>
        <log_storage_path>{path}/keeper/logs</log_storage_path>
        <snapshot_storage_path>{path}/keeper/snapshots</snapshot_storage_path>
        <coordination_settings>
            <raft_logs_level>warning</raft_logs_level>
        </coordination_settings>
        <raft_configuration>
            <server><id>1</id><hostname>127.0.0.1</hostname><port>{raft}</port></server>
        </raft_configuration>
    </keeper_server>
    <zookeeper>
        <node><host>127.0.0.1</host><port>{keeper}</port></node>
    </zookeeper>
    <remote_servers>
        <{cluster}>
            <shard>
                <replica><host>127.0.0.1</host><port>{tcp}</port></replica>
            </shard>
        </{cluster}>
    </remote_servers>
    <macros>
        <shard>01</shard>
        <replica>replica1</replica>
    </macros>
    <distributed_ddl><path>/clickhouse/task_queue/ddl</path></distributed_ddl>"#;

const USERS_CONFIG: &str = r#"<clickhouse>
    <users>
        <default>
            <password></password>
            <networks><ip>127.0.0.1</ip></networks>
            <profile>default</profile>
            <quota>default</quota>
            <access_management>1</access_management>
        </default>
    </users>
    <profiles><default/></profiles>
    <quotas><default/></quotas>
</clickhouse>
"#;

pub struct EphemeralServer {
    process: Child,
    pub http_url: String,
    _dir: tempfile::TempDir,
}

/// Ports already handed out by this process. Probed ports are
/// released before the server binds them, so without this two
/// concurrent tests in one binary can draw the same port; the loser
/// then pings the winner's server and mistakes it for its own.
static TAKEN_PORTS: std::sync::Mutex<Option<std::collections::HashSet<u16>>> =
    std::sync::Mutex::new(None);

fn free_port() -> std::io::Result<u16> {
    loop {
        let port = TcpListener::bind("127.0.0.1:0")?.local_addr()?.port();
        let mut taken = TAKEN_PORTS.lock().expect("port registry");
        if taken.get_or_insert_with(Default::default).insert(port) {
            return Ok(port);
        }
    }
}

impl EphemeralServer {
    /// Start a server from `binary`; blocks until it answers `/ping`.
    /// Retries once on a port clash (free ports are probed, then released,
    /// so another process can race for them).
    pub fn start(binary: &Path) -> Result<Self, EphemeralError> {
        let mut last = Self::start_once(binary, None)?;
        for _ in 0..2 {
            match last {
                Ok(server) => return Ok(server),
                Err(_) => last = Self::start_once(binary, None)?,
            }
        }
        last
    }

    /// Start a single-node *clustered* server (embedded Keeper, real
    /// distributed DDL) whose cluster is named `cluster`.
    pub fn start_clustered(binary: &Path, cluster: &str) -> Result<Self, EphemeralError> {
        let mut last = Self::start_once(binary, Some(cluster))?;
        for _ in 0..2 {
            match last {
                Ok(server) => return Ok(server),
                Err(_) => last = Self::start_once(binary, Some(cluster))?,
            }
        }
        last
    }

    fn start_once(
        binary: &Path,
        cluster: Option<&str>,
    ) -> Result<Result<Self, EphemeralError>, EphemeralError> {
        let dir = tempfile::tempdir()?;
        let root = dir.path();
        let tcp = free_port()?;
        let http = free_port()?;
        let cluster_config = match cluster {
            Some(cluster) => CLUSTER_CONFIG
                .replace("{interserver}", &free_port()?.to_string())
                .replace("{keeper}", &free_port()?.to_string())
                .replace("{raft}", &free_port()?.to_string())
                .replace("{cluster}", cluster),
            None => String::new(),
        };
        let config = SERVER_CONFIG
            .replace("{cluster_config}", &cluster_config)
            .replace("{tcp}", &tcp.to_string())
            .replace("{http}", &http.to_string())
            .replace("{path}", &root.to_string_lossy());
        let config_path = root.join("config.xml");
        std::fs::write(&config_path, config)?;
        std::fs::write(root.join("users.xml"), USERS_CONFIG)?;
        let log = std::fs::File::create(root.join("server.log"))?;

        let process = Command::new(binary)
            .arg("server")
            .arg("--config-file")
            .arg(&config_path)
            // Without this ClickHouse forks a watchdog parent, and killing
            // the spawned pid would orphan the real server.
            .env("CLICKHOUSE_WATCHDOG_ENABLE", "0")
            .stdout(Stdio::null())
            .stderr(log)
            .spawn()?;

        let root_path = root.to_path_buf();
        let server = Self {
            process,
            http_url: format!("http://127.0.0.1:{http}"),
            _dir: dir,
        };
        Ok(match server.wait_ready(Duration::from_secs(30)) {
            // Answering /ping is not enough: across test binaries a
            // port clash can leave us pinging someone else's server.
            // The data path is unique per instance; confirm it is ours.
            Ok(()) => match server.confirm_identity(&root_path) {
                Ok(()) => Ok(server),
                Err(error) => Err(error),
            },
            Err(error) => Err(error),
        })
    }

    /// The server on our port must be the one we spawned: its default
    /// disk path lives inside our temp directory.
    fn confirm_identity(&self, root: &Path) -> Result<(), EphemeralError> {
        let query = "SELECT+path+FROM+system.disks+WHERE+name%3D%27default%27";
        let response =
            ureq_get(&format!("{}/?query={query}", self.http_url)).map_err(EphemeralError::Io)?;
        if response.contains(&root.to_string_lossy().to_string()) {
            Ok(())
        } else {
            Err(EphemeralError::Startup(
                "port clash: answered by a different server instance".into(),
            ))
        }
    }

    fn wait_ready(&self, timeout: Duration) -> Result<(), EphemeralError> {
        let deadline = Instant::now() + timeout;
        let ping = format!("{}/ping", self.http_url);
        while Instant::now() < deadline {
            if let Ok(response) = ureq_get(&ping) {
                // The body arrives chunked; the status line is the signal.
                if response.starts_with("HTTP/1.1 200") {
                    return Ok(());
                }
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        let log = std::fs::read_to_string(self._dir.path().join("server.log")).unwrap_or_default();
        let tail: String = log
            .lines()
            .rev()
            .take(5)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n");
        Err(EphemeralError::NotReady(timeout, tail))
    }

    pub fn data_dir(&self) -> PathBuf {
        self._dir.path().to_path_buf()
    }
}

/// Tiny blocking GET returning the raw response (status line included)
/// so readiness probing needs no async runtime.
fn ureq_get(url: &str) -> std::io::Result<String> {
    use std::io::{Read, Write};
    let address = url
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap_or_default();
    let path = url
        .trim_start_matches("http://")
        .split_once('/')
        .map(|(_, rest)| format!("/{rest}"))
        .unwrap_or_else(|| "/".into());
    let mut stream = std::net::TcpStream::connect(address)?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n"
    )?;
    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    Ok(response)
}

impl Drop for EphemeralServer {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}
