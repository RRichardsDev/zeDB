//! The ops view: what is this cluster doing right now. Small capped
//! SELECTs against system tables, polled only while the view is
//! visible; read-only by construction except KILL QUERY, which
//! follows the connection's write posture.

use gpui::Action;
use zedb_core::Value;

/// Poll cadence: fast while zeDB is frontmost (the user is watching),
/// gentle while it is in the background (nobody is). The slow lane
/// keeps its own wall-clock period either way.
pub(crate) const POLL_ACTIVE_SECS: u64 = 1;
pub(crate) const POLL_INACTIVE_SECS: u64 = 5;
pub(crate) const SLOW_POLL_SECS: u64 = 10;

/// Which node(s) the panels ask about. Cluster scope fans every
/// query out via clusterAllReplicas()/cluster() and exists only for
/// topologies the nodes reported at connect time.
#[derive(Clone, PartialEq, Eq, Default)]
pub enum OpsScope {
    #[default]
    Node,
    Cluster(String),
}

impl OpsScope {
    pub(crate) fn cluster(&self) -> Option<&str> {
        match self {
            OpsScope::Node => None,
            OpsScope::Cluster(name) => Some(name),
        }
    }
}

#[derive(Clone, Debug)]
pub struct OpsProcess {
    pub query_id: String,
    pub user: String,
    pub elapsed_secs: f64,
    pub read_rows: u64,
    pub read_bytes: u64,
    pub total_rows: u64,
    pub memory_bytes: u64,
    pub query: String,
    /// Where the client connects from, port stripped.
    pub address: String,
    /// client_name (native) or http_user_agent (HTTP), whichever is set.
    pub client: String,
    pub os_user: String,
    pub initial_user: String,
    /// hostName() of the node running it; empty in node scope.
    pub node: String,
}

#[derive(Clone, Debug)]
pub struct OpsMerge {
    pub database: String,
    pub table: String,
    pub elapsed_secs: f64,
    /// 0.0..=1.0
    pub progress: f64,
    pub num_parts: u64,
    pub total_size_bytes: u64,
    pub is_mutation: bool,
    pub node: String,
}

#[derive(Clone, Debug)]
pub struct OpsMutation {
    pub database: String,
    pub table: String,
    pub command: String,
    pub parts_to_do: u64,
    pub latest_fail_reason: String,
    pub node: String,
}

#[derive(Clone, Debug)]
pub struct OpsReplicaProblem {
    pub database: String,
    pub table: String,
    pub is_readonly: bool,
    pub session_expired: bool,
    pub delay_secs: u64,
    pub queue_size: u64,
    pub node: String,
}

#[derive(Clone, Debug)]
pub struct OpsQueueIssue {
    pub database: String,
    pub table: String,
    pub depth: u64,
    pub oldest_secs: u64,
    pub exception: String,
    pub node: String,
}

/// One Kafka consumer, from system.kafka_consumers (absent without
/// Kafka engine tables).
#[derive(Clone, Debug)]
pub struct OpsKafkaConsumer {
    pub database: String,
    pub table: String,
    pub stale_secs: u64,
    pub messages: u64,
    pub exception: String,
    pub node: String,
}

/// A materialized view that failed during insert, aggregated from
/// system.query_views_log (needs query_views_log enabled).
#[derive(Clone, Debug)]
pub struct OpsViewFailure {
    pub view: String,
    pub target: String,
    pub failures: u64,
    pub exception: String,
    pub node: String,
}

/// Pending async-insert batches, from system.asynchronous_inserts.
#[derive(Clone, Debug)]
pub struct OpsAsyncInsert {
    pub database: String,
    pub table: String,
    pub bytes: u64,
    pub entries: u64,
    pub oldest_secs: u64,
    pub node: String,
}

/// One node's Keeper/ZooKeeper session, from
/// system.zookeeper_connection (absent on keeper-less servers).
#[derive(Clone, Debug)]
pub struct OpsKeeper {
    pub name: String,
    pub host: String,
    pub uptime_secs: u64,
    pub expired: bool,
    pub node: String,
}

#[derive(Clone, Debug)]
pub struct OpsDisk {
    pub name: String,
    pub free: u64,
    pub total: u64,
    /// system.disks `type`: object-storage kinds make a percent-full
    /// bar meaningless, so the view labels them instead.
    pub kind: String,
    pub node: String,
}

impl OpsDisk {
    pub fn is_object_storage(&self) -> bool {
        matches!(
            self.kind.as_str(),
            "ObjectStorage" | "S3" | "s3" | "s3_plain" | "azure_blob_storage" | "gcs" | "web"
        )
    }
}

#[derive(Clone, Debug)]
pub struct OpsTopTable {
    pub database: String,
    pub table: String,
    pub bytes: u64,
    pub rows: u64,
}

/// How many rows the largest-tables list asks for.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum OpsTopLimit {
    #[default]
    Ten,
    TwentyFive,
    Fifty,
    Hundred,
    All,
}

impl OpsTopLimit {
    pub(crate) fn label(self) -> &'static str {
        match self {
            OpsTopLimit::Ten => "10",
            OpsTopLimit::TwentyFive => "25",
            OpsTopLimit::Fifty => "50",
            OpsTopLimit::Hundred => "100",
            OpsTopLimit::All => "All",
        }
    }

    /// The LIMIT clause, empty for All (the grouped result is one row
    /// per table, so unbounded stays small).
    pub(crate) fn clause(self) -> &'static str {
        match self {
            OpsTopLimit::Ten => " LIMIT 10",
            OpsTopLimit::TwentyFive => " LIMIT 25",
            OpsTopLimit::Fifty => " LIMIT 50",
            OpsTopLimit::Hundred => " LIMIT 100",
            OpsTopLimit::All => "",
        }
    }
}

#[derive(Clone, PartialEq, Action)]
#[action(no_json, no_register)]
pub struct SetOpsTopLimit {
    pub limit: OpsTopLimit,
}

#[derive(Clone, PartialEq, Action)]
#[action(no_json, no_register)]
pub struct SetOpsScope {
    /// None selects the connected node; Some(name) a known cluster.
    pub cluster: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum OpsTab {
    #[default]
    Queries,
    Background,
    Replication,
    Ingestion,
    Storage,
}

impl OpsTab {
    pub(crate) const ALL: [OpsTab; 5] = [
        OpsTab::Queries,
        OpsTab::Background,
        OpsTab::Replication,
        OpsTab::Ingestion,
        OpsTab::Storage,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            OpsTab::Queries => "Queries",
            OpsTab::Background => "Background",
            OpsTab::Replication => "Replication",
            OpsTab::Ingestion => "Ingestion",
            OpsTab::Storage => "Storage",
        }
    }
}

#[derive(Default)]
pub struct OpsState {
    pub processes: Vec<OpsProcess>,
    /// Wall-clock stamp of the last successful fetch.
    pub as_of: Option<chrono::DateTime<chrono::Local>>,
    pub error: Option<String>,
    pub poll_generation: u64,
    pub fetch_in_flight: bool,
    /// query_id currently being killed (disables its button).
    pub killing: Option<String>,
    /// Open-connection counters from system.metrics, label + count.
    pub connections: Vec<(String, u64)>,
    pub merges: Vec<OpsMerge>,
    pub mutations: Vec<OpsMutation>,
    /// Polled seconds accumulated since the last slow fetch; the slow
    /// queries run each time it crosses SLOW_POLL_SECS.
    pub tick: u64,
    pub replica_total: u64,
    pub replica_problems: Vec<OpsReplicaProblem>,
    pub queue_issues: Vec<OpsQueueIssue>,
    pub keeper: Vec<OpsKeeper>,
    pub kafka_consumers: Vec<OpsKafkaConsumer>,
    pub view_failures: Vec<OpsViewFailure>,
    pub async_inserts: Vec<OpsAsyncInsert>,
    pub disks: Vec<OpsDisk>,
    /// The service's tables are Shared*MergeTree (ClickHouse Cloud):
    /// ZooKeeper-era replication signals do not apply.
    pub smt: bool,
    pub top_tables: Vec<OpsTopTable>,
    pub slow_fetch_in_flight: bool,
    pub tab: OpsTab,
    pub top_limit: OpsTopLimit,
    pub scope: OpsScope,
}

pub(crate) fn number(value: Option<&Value>) -> u64 {
    match value {
        Some(Value::UInt(number)) => *number,
        Some(Value::Int(number)) => (*number).max(0) as u64,
        Some(Value::Float(number)) => number.max(0.0) as u64,
        _ => 0,
    }
}

pub(crate) fn float(value: Option<&Value>) -> f64 {
    match value {
        Some(Value::Float(number)) => *number,
        Some(Value::UInt(number)) => *number as f64,
        Some(Value::Int(number)) => *number as f64,
        _ => 0.0,
    }
}

pub(crate) fn text(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.clone(),
        _ => String::new(),
    }
}

/// A human address from system.processes: port stripped, the
/// IPv6-mapped-IPv4 prefix unwrapped, brackets removed.
pub(crate) fn display_address(address: &str) -> String {
    let mut host = address.trim();
    // [host]:port or host:port with a numeric tail.
    if let Some(stripped) = host.strip_prefix('[') {
        host = stripped.split(']').next().unwrap_or(stripped);
    } else if let Some((head, tail)) = host.rsplit_once(':') {
        if !tail.is_empty() && tail.chars().all(|ch| ch.is_ascii_digit()) {
            host = head;
        }
    }
    // ::ffff:172.18.0.1 is an IPv4 in IPv6 clothing.
    host.strip_prefix("::ffff:").unwrap_or(host).to_string()
}

/// A single-quoted ClickHouse string literal (cluster names in
/// table-function calls).
pub(crate) fn quoted(name: &str) -> String {
    format!("'{}'", name.replace('\\', "\\\\").replace('\'', "\\'"))
}

pub(crate) fn format_elapsed(seconds: f64) -> String {
    if seconds < 60.0 {
        format!("{seconds:.1}s")
    } else if seconds < 3600.0 {
        format!(
            "{}m {:02}s",
            (seconds / 60.0) as u64,
            (seconds % 60.0) as u64
        )
    } else {
        format!(
            "{}h {:02}m",
            (seconds / 3600.0) as u64,
            ((seconds % 3600.0) / 60.0) as u64
        )
    }
}
