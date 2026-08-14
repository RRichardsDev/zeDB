//! The zedb MCP server: read-only fleet and ClickHouse tools for
//! agents, over the Model Context Protocol's stdio transport
//! (JSON-RPC 2.0, one object per line).
//!
//! Deliberately read-only end to end: the connection is forced
//! read-only (server-side readonly, not SQL inspection) and every
//! query carries execution-time, row, and byte caps. Nothing mutating
//! is reachable over this protocol; agents that want to change the
//! world go through the CLI's consent flags like any other process.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use zedb_core::repo::MigrationRepo;

use crate::runner::{Runner, RunnerOptions, Targets};
use crate::schema_cache::SchemaSnapshot;
use crate::schema_intelligence;
use crate::verify::Verifier;
use crate::{ChClient, ChConfig};

mod handlers;

/// Server-side caps applied to every agent query.
#[derive(Clone, Copy)]
pub struct QueryCaps {
    pub max_execution_time_secs: u32,
    pub max_result_rows: u64,
    pub max_bytes_to_read: u64,
}

impl Default for QueryCaps {
    fn default() -> Self {
        Self {
            max_execution_time_secs: 15,
            max_result_rows: 200,
            // 10 GiB scanned is generous for exploration and still a
            // ceiling a production cluster survives.
            max_bytes_to_read: 10 * 1024 * 1024 * 1024,
        }
    }
}

pub struct McpServer {
    repo: Option<MigrationRepo>,
    config: Option<ChConfig>,
    caps: QueryCaps,
    /// When set, the app-hosted tools (propose_migration, propose_query,
    /// navigate) are offered and forwarded over this unix socket to the
    /// running zeDB app, which owns the editors these tools fill.
    app_socket: Option<std::path::PathBuf>,
    /// When set, schema_search and lint_sql serve from this on-disk
    /// schema snapshot, re-read per call so background refreshes land.
    schema_cache_path: Option<std::path::PathBuf>,
}

impl McpServer {
    /// Connection is forced read-only regardless of what was passed.
    pub fn new(repo: Option<MigrationRepo>, config: Option<ChConfig>, caps: QueryCaps) -> Self {
        let config = config.map(|mut config| {
            config.read_only = true;
            config
        });
        Self {
            repo,
            config,
            caps,
            app_socket: None,
            schema_cache_path: None,
        }
    }

    /// Serve schema_search and lint_sql from this snapshot file.
    pub fn with_schema_cache(mut self, path: std::path::PathBuf) -> Self {
        self.schema_cache_path = Some(path);
        self
    }

    /// Enable the app-hosted tools, forwarded to `socket`.
    pub fn with_app_bridge(mut self, socket: std::path::PathBuf) -> Self {
        self.app_socket = Some(socket);
        self
    }

    /// Forward an app tool call over the bridge; one JSONL round trip.
    async fn forward_app_tool(&self, name: &str, arguments: &Value) -> Result<String, String> {
        let Some(socket) = &self.app_socket else {
            return Err("this tool needs the zeDB app; the pane provides it".into());
        };
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let stream = tokio::net::UnixStream::connect(socket)
            .await
            .map_err(|error| format!("app bridge unavailable: {error}"))?;
        let (read_half, mut write_half) = stream.into_split();
        let request = json!({ "tool": name, "arguments": arguments });
        write_half
            .write_all(request.to_string().as_bytes())
            .await
            .map_err(|error| error.to_string())?;
        write_half
            .write_all(b"\n")
            .await
            .map_err(|error| error.to_string())?;
        let mut line = String::new();
        BufReader::new(read_half)
            .read_line(&mut line)
            .await
            .map_err(|error| error.to_string())?;
        let reply: Value = serde_json::from_str(&line).map_err(|error| error.to_string())?;
        let text = reply
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or("(no reply)")
            .to_string();
        if reply
            .get("isError")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            Err(text)
        } else {
            Ok(text)
        }
    }

    fn client(&self) -> Result<ChClient, String> {
        self.config
            .clone()
            .map(ChClient::new)
            .ok_or_else(|| "no ClickHouse connection configured for this server".into())
    }

    /// The repo for this call, resolved fresh: the app's currently
    /// open repo when the bridge is up (repos attach, detach, and
    /// grow mid-session), else the one captured at startup, re-opened
    /// so migrations scaffolded since launch are seen.
    async fn effective_repo(&self) -> Result<MigrationRepo, String> {
        match self.bridge_repo_root().await {
            // The app answered: its word is truth, including "none".
            Some(root) if root.is_empty() => Err("no migration repo open for this server".into()),
            Some(root) => MigrationRepo::open_root(std::path::Path::new(&root))
                .map_err(|error| error.to_string()),
            // No bridge (CLI-style serving): startup config decides.
            None => match &self.repo {
                Some(repo) => {
                    MigrationRepo::open_root(&repo.root).map_err(|error| error.to_string())
                }
                None => Err("no migration repo open for this server".into()),
            },
        }
    }

    /// Ask the app which repo is open right now; None when no bridge
    /// is configured or it does not answer.
    async fn bridge_repo_root(&self) -> Option<String> {
        if self.app_socket.is_none() {
            return None;
        }
        self.forward_app_tool("repo_root", &Value::Null).await.ok()
    }

    fn runner_for<'a>(&self, repo: &'a MigrationRepo) -> Result<Runner<'a>, String> {
        let config = self
            .config
            .clone()
            .ok_or_else(|| "no ClickHouse connection configured for this server".to_string())?;
        Ok(Runner::new(
            repo,
            RunnerOptions {
                server: config,
                admin: None,
                cluster: None,
                no_cluster: true,
                write: false,
                dry_run: false,
                overrides: BTreeMap::new(),
            },
        ))
    }

    /// Handle one JSON-RPC message; None for notifications.
    pub async fn handle(&self, message: Value) -> Option<Value> {
        let id = message.get("id").cloned();
        let method = message.get("method").and_then(Value::as_str)?;
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        // Notifications (notifications/initialized etc.) need no reply.
        let id = id?;
        let result = match method {
            "initialize" => Ok(json!({
                "protocolVersion": params
                    .get("protocolVersion")
                    .and_then(Value::as_str)
                    .unwrap_or("2024-11-05"),
                "capabilities": { "tools": {} },
                "serverInfo": {
                    "name": "zedb",
                    "version": env!("CARGO_PKG_VERSION"),
                },
            })),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({
                "tools": tool_definitions(
                    self.app_socket.is_some(),
                    self.schema_cache_available(),
                )
            })),
            "tools/call" => self.call_tool(&params).await,
            other => Err(format!("method not supported: {other}")),
        };
        Some(match result {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err(message) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32603, "message": message },
            }),
        })
    }

    async fn call_tool(&self, params: &Value) -> Result<Value, String> {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or("tool call without a name")?;
        let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
        let outcome = match name {
            "fleet_status" => self.tool_fleet_status().await,
            "list_migrations" => self.tool_list_migrations().await,
            "migration_sql" => self.tool_migration_sql(&arguments).await,
            "dry_run" => self.tool_dry_run(&arguments).await,
            "drift" => self.tool_drift(&arguments).await,
            "list_databases" => self.tool_list_databases().await,
            "list_tables" => self.tool_list_tables(&arguments).await,
            "describe" => self.tool_describe(&arguments).await,
            "run_query" => self.tool_run_query(&arguments).await,
            "schema_search" => self.tool_schema_search(&arguments),
            "lint_sql" => self.tool_lint_sql(&arguments),
            "propose_migration" | "propose_query" | "navigate" => {
                self.forward_app_tool(name, &arguments).await
            }
            other => Err(format!("unknown tool: {other}")),
        };
        // Tool errors are results with isError, per MCP; protocol errors
        // stay JSON-RPC errors.
        Ok(match outcome {
            Ok(text) => json!({ "content": [{ "type": "text", "text": text }] }),
            Err(text) => json!({
                "content": [{ "type": "text", "text": text }],
                "isError": true,
            }),
        })
    }
}

fn join_numbers(numbers: &[u32]) -> String {
    numbers
        .iter()
        .map(|number| format!("{number:05}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn required_str<'a>(arguments: &'a Value, key: &str) -> Result<&'a str, String> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{key} argument required"))
}

fn sql_quote(text: &str) -> String {
    format!("'{}'", text.replace('\\', "\\\\").replace('\'', "\\'"))
}

fn tool_definitions(with_app_tools: bool, with_schema_cache: bool) -> Value {
    let string_arg = |name: &str, description: &str| {
        json!({
            "type": "object",
            "properties": { name: { "type": "string", "description": description } },
            "required": [name],
        })
    };
    let tools = json!([
        {
            "name": "fleet_status",
            "description": "Migration state of every database in the fleet: head, pending, customised, failed, excluded.",
            "inputSchema": { "type": "object", "properties": {} },
        },
        {
            "name": "list_migrations",
            "description": "The migration chain: numbers, rollback classes, targeted flags, headlines, and the repo's template parameters.",
            "inputSchema": { "type": "object", "properties": {} },
        },
        {
            "name": "migration_sql",
            "description": "Full upgrade and rollback SQL of one migration by number.",
            "inputSchema": {
                "type": "object",
                "properties": { "number": { "type": "integer" } },
                "required": ["number"],
            },
        },
        {
            "name": "dry_run",
            "description": "The SQL that upgrading one database would run, rendered with its parameters.",
            "inputSchema": string_arg("database", "target database name"),
        },
        {
            "name": "drift",
            "description": "Schema drift findings for one database versus the repo's expected state. Slow: replays the chain.",
            "inputSchema": string_arg("database", "database to verify"),
        },
        {
            "name": "list_databases",
            "description": "Databases on the connected ClickHouse.",
            "inputSchema": { "type": "object", "properties": {} },
        },
        {
            "name": "list_tables",
            "description": "Tables in one database with engine and row estimate.",
            "inputSchema": string_arg("database", "database name"),
        },
        {
            "name": "describe",
            "description": "Columns of one table: name, type, default, comment.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "database": { "type": "string" },
                    "table": { "type": "string" },
                },
                "required": ["database", "table"],
            },
        },
        {
            "name": "run_query",
            "description": "Run a read-only SQL query on the connected ClickHouse. Server enforces read-only plus execution-time, row, and byte caps.",
            "inputSchema": string_arg("sql", "the SQL to run"),
        },
    ]);
    let Value::Array(mut items) = tools else {
        unreachable!("tool list is an array")
    };
    if with_schema_cache {
        items.push(json!({
            "name": "schema_search",
            "description": "Instant fleet-wide search over zeDB's cached schema: database, table, and column names matching a substring. Great for reconnaissance; answers are cached, so confirm with describe before anything destructive.",
            "inputSchema": string_arg("query", "substring to search for"),
        }));
        items.push(json!({
            "name": "lint_sql",
            "description": "Check SQL identifiers against zeDB's cached schema without running anything: unknown tables and columns are reported with line numbers. Conservative: ambiguous or uncached names pass silently.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "sql": { "type": "string", "description": "the SQL to check" },
                    "database": { "type": "string", "description": "default database for unqualified names" },
                },
                "required": ["sql"],
            },
        }));
    }
    if with_app_tools {
        items.push(json!({
            "name": "propose_migration",
            "description": "Fill zeDB's migration authoring overlay with a draft: upgrade SQL, rollback SQL, rollback class, targeted flag. Writes NOTHING; the user reviews, checks against the pinned server, and saves. Use ${db} and ${cluster} placeholders as the repo's migrations do.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "upgrade_sql": { "type": "string" },
                    "rollback_sql": { "type": "string", "description": "omit for an irreversible migration" },
                    "rollback_class": { "type": "string", "enum": ["clean", "structural", "irreversible"] },
                    "targeted": { "type": "boolean" },
                },
                "required": ["upgrade_sql"],
            },
        }));
        items.push(json!({
            "name": "propose_query",
            "description": "Put SQL into a zeDB query editor tab for the user to run themselves. Use this for any statement you cannot or should not run (writes, DDL) and for queries the user asked to have in the editor.",
            "inputSchema": string_arg("sql", "the SQL to place in the editor"),
        }));
        items.push(json!({
            "name": "navigate",
            "description": "Switch what the zeDB window shows: view is one of fleet, query, connections; optionally select a database row in the fleet.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "view": { "type": "string", "enum": ["fleet", "query", "connections"] },
                    "database": { "type": "string" },
                },
                "required": ["view"],
            },
        }));
    }
    Value::Array(items)
}

/// Serve MCP over stdio until stdin closes.
pub async fn serve_stdio(server: McpServer) -> std::io::Result<()> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let mut lines = BufReader::new(stdin).lines();
    while let Some(line) = lines.next_line().await? {
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if let Some(response) = server.handle(message).await {
            stdout.write_all(response.to_string().as_bytes()).await?;
            stdout.write_all(b"\n").await?;
            stdout.flush().await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bare_server() -> McpServer {
        McpServer::new(None, None, QueryCaps::default())
    }

    #[tokio::test]
    async fn initialize_lists_tools_and_echoes_version() {
        let server = bare_server();
        let response = server
            .handle(serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": { "protocolVersion": "2025-06-18" },
            }))
            .await
            .expect("response");
        assert_eq!(
            response["result"]["protocolVersion"],
            serde_json::json!("2025-06-18")
        );
        let tools = server
            .handle(serde_json::json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }))
            .await
            .expect("response");
        let names: Vec<&str> = tools["result"]["tools"]
            .as_array()
            .expect("tools array")
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect();
        assert!(names.contains(&"fleet_status"));
        assert!(names.contains(&"run_query"));
        assert!(names.contains(&"drift"));
    }

    #[tokio::test]
    async fn schema_tools_serve_from_the_cache_file() {
        use crate::schema_cache::{CachedObjectKind, SchemaCache, TableRecord};
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("schema.json");
        let cache = SchemaCache::open(&path).unwrap();
        cache
            .publish_tables(vec![TableRecord {
                total_bytes: None,
                database: "analytics".into(),
                name: "events".into(),
                engine: "MergeTree".into(),
                kind: CachedObjectKind::Table,
                total_rows: Some(1),
                comment: String::new(),
            }])
            .unwrap();

        let server =
            McpServer::new(None, None, QueryCaps::default()).with_schema_cache(path.clone());
        let tools = server
            .handle(serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }))
            .await
            .expect("response");
        let names: Vec<&str> = tools["result"]["tools"]
            .as_array()
            .expect("tools array")
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect();
        assert!(names.contains(&"schema_search"));
        assert!(names.contains(&"lint_sql"));

        let search = server
            .handle(serde_json::json!({
                "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                "params": { "name": "schema_search", "arguments": { "query": "even" } },
            }))
            .await
            .expect("response");
        let text = search["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("analytics.events (MergeTree)"));
        assert!(text.contains("cached as of"));

        let lint = server
            .handle(serde_json::json!({
                "jsonrpc": "2.0", "id": 3, "method": "tools/call",
                "params": { "name": "lint_sql", "arguments": {
                    "sql": "SELECT * FROM analytics.eventzz",
                } },
            }))
            .await
            .expect("response");
        let text = lint["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("Unknown table or view `analytics.eventzz`"));
    }

    #[tokio::test]
    async fn notifications_are_silent_and_missing_deps_are_tool_errors() {
        let server = bare_server();
        assert!(server
            .handle(serde_json::json!({
                "jsonrpc": "2.0", "method": "notifications/initialized"
            }))
            .await
            .is_none());
        let response = server
            .handle(serde_json::json!({
                "jsonrpc": "2.0", "id": 3, "method": "tools/call",
                "params": { "name": "run_query", "arguments": { "sql": "SELECT 1" } },
            }))
            .await
            .expect("response");
        assert_eq!(response["result"]["isError"], serde_json::json!(true));
        let text = response["result"]["content"][0]["text"]
            .as_str()
            .expect("text");
        assert!(text.contains("no ClickHouse connection"));
    }
}
