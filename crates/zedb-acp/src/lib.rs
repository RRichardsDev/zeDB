//! zedb-acp: an Agent Client Protocol client (docs/PHASE-3.1.md M0).
//!
//! Spawns an installed agent CLI (Claude Code, Codex, anything
//! ACP-speaking) and converses with it over JSON-RPC 2.0, one JSON
//! object per line on stdio. zeDB is the client: it renders the
//! conversation and answers permission requests; the agent brings its
//! own auth and its own tools. Headless and GUI-free by design; the
//! pane is a thin consumer of `AgentEvent`s.

pub mod discovery;
pub mod protocol;

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot};

pub use protocol::{
    AgentEvent, ContentBlock, InitializeResult, McpServerConfig, NewSessionResult,
    PermissionOption, PermissionOutcome, PromptResult, PROTOCOL_VERSION,
};

#[derive(Debug, thiserror::Error)]
pub enum AcpError {
    #[error("could not spawn agent: {0}")]
    Spawn(std::io::Error),
    #[error("agent connection closed")]
    Closed,
    #[error("agent error {code}: {message}")]
    Rpc { code: i64, message: String },
    #[error("malformed agent response: {0}")]
    Protocol(String),
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, AcpError>>>>>;

/// A live connection to one agent process. Dropping it kills the
/// process; `events()` yields the conversation as it streams.
pub struct AgentConnection {
    child: Child,
    outgoing: mpsc::UnboundedSender<String>,
    pending: Pending,
    next_id: AtomicU64,
    events: Option<mpsc::UnboundedReceiver<AgentEvent>>,
}

impl AgentConnection {
    /// Spawn `program args...` in `cwd` and wire up the protocol pumps.
    /// No protocol traffic happens yet; call `initialize` next.
    pub fn spawn(
        program: &str,
        args: &[String],
        env: &[(String, String)],
        cwd: Option<&std::path::Path>,
    ) -> Result<Self, AcpError> {
        let mut command = Command::new(program);
        command
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for (name, value) in env {
            command.env(name, value);
        }
        if let Some(cwd) = cwd {
            command.current_dir(cwd);
        }
        let mut child = command.spawn().map_err(AcpError::Spawn)?;

        let stdin = child.stdin.take().expect("stdin was piped");
        let stdout = child.stdout.take().expect("stdout was piped");
        let stderr = child.stderr.take().expect("stderr was piped");

        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (outgoing_tx, mut outgoing_rx) = mpsc::unbounded_channel::<String>();

        // Writer pump: everything the client sends goes through one
        // task so requests and responses never interleave mid-line.
        tokio::spawn(async move {
            let mut stdin = stdin;
            while let Some(line) = outgoing_rx.recv().await {
                if stdin.write_all(line.as_bytes()).await.is_err() {
                    break;
                }
                if stdin.write_all(b"\n").await.is_err() {
                    break;
                }
                let _ = stdin.flush().await;
            }
        });

        // Stderr pump: agents talk auth problems here; surface them.
        let stderr_events = event_tx.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if stderr_events.send(AgentEvent::Stderr { line }).is_err() {
                    break;
                }
            }
        });

        // Reader pump: route responses to pending requests, decode
        // notifications into events, answer agent-initiated requests.
        let reader_pending = pending.clone();
        let reader_outgoing = outgoing_tx.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let Ok(message) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                route_message(message, &reader_pending, &reader_outgoing, &event_tx);
            }
            // Fail everything still in flight, then tell the consumer.
            let mut pending = reader_pending.lock().expect("pending lock");
            for (_, responder) in pending.drain() {
                let _ = responder.send(Err(AcpError::Closed));
            }
            let _ = event_tx.send(AgentEvent::Closed {
                reason: "agent process ended".into(),
            });
        });

        Ok(Self {
            child,
            outgoing: outgoing_tx,
            pending,
            next_id: AtomicU64::new(1),
            events: Some(event_rx),
        })
    }

    /// Take the event stream; exactly one consumer may hold it.
    pub fn take_events(&mut self) -> mpsc::UnboundedReceiver<AgentEvent> {
        self.events.take().expect("events already taken")
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value, AcpError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().expect("pending lock").insert(id, tx);
        let message = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        self.outgoing
            .send(message.to_string())
            .map_err(|_| AcpError::Closed)?;
        rx.await.map_err(|_| AcpError::Closed)?
    }

    fn notify(&self, method: &str, params: Value) -> Result<(), AcpError> {
        let message = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        self.outgoing
            .send(message.to_string())
            .map_err(|_| AcpError::Closed)
    }

    pub async fn initialize(&self) -> Result<InitializeResult, AcpError> {
        let params = protocol::InitializeParams {
            protocol_version: PROTOCOL_VERSION,
            client_capabilities: Default::default(),
        };
        let result = self
            .request(
                "initialize",
                serde_json::to_value(params).expect("serialize"),
            )
            .await?;
        serde_json::from_value(result).map_err(|error| AcpError::Protocol(error.to_string()))
    }

    pub async fn new_session(
        &self,
        cwd: &std::path::Path,
        mcp_servers: Vec<McpServerConfig>,
    ) -> Result<NewSessionResult, AcpError> {
        let params = protocol::NewSessionParams {
            cwd: cwd.display().to_string(),
            mcp_servers,
        };
        let result = self
            .request(
                "session/new",
                serde_json::to_value(params).expect("serialize"),
            )
            .await?;
        serde_json::from_value(result).map_err(|error| AcpError::Protocol(error.to_string()))
    }

    /// Send one user turn; resolves with the stop reason after the
    /// turn's updates have streamed through the event channel.
    pub async fn prompt(&self, session_id: &str, text: &str) -> Result<PromptResult, AcpError> {
        let params = protocol::PromptParams {
            session_id: session_id.to_string(),
            prompt: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
        };
        let result = self
            .request(
                "session/prompt",
                serde_json::to_value(params).expect("serialize"),
            )
            .await?;
        serde_json::from_value(result).map_err(|error| AcpError::Protocol(error.to_string()))
    }

    /// Ask the agent to stop the current turn; the in-flight prompt
    /// resolves with a cancelled stop reason.
    pub fn cancel(&self, session_id: &str) -> Result<(), AcpError> {
        self.notify("session/cancel", json!({ "sessionId": session_id }))
    }

    /// Kill the agent process. Dropping the connection does this too.
    pub async fn shutdown(mut self) {
        let _ = self.child.kill().await;
    }
}

fn route_message(
    message: Value,
    pending: &Pending,
    outgoing: &mpsc::UnboundedSender<String>,
    events: &mpsc::UnboundedSender<AgentEvent>,
) {
    let id = message.get("id");
    let method = message.get("method").and_then(Value::as_str);
    match (id, method) {
        // A response to one of our requests.
        (Some(id), None) => {
            let Some(id) = id.as_u64() else { return };
            let Some(responder) = pending.lock().expect("pending lock").remove(&id) else {
                return;
            };
            let outcome = if let Some(error) = message.get("error") {
                Err(AcpError::Rpc {
                    code: error.get("code").and_then(Value::as_i64).unwrap_or(0),
                    message: error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown error")
                        .to_string(),
                })
            } else {
                Ok(message.get("result").cloned().unwrap_or(Value::Null))
            };
            let _ = responder.send(outcome);
        }
        // An agent-initiated request we must answer.
        (Some(id), Some("session/request_permission")) => {
            let id = id.clone();
            let params = message.get("params").cloned().unwrap_or(Value::Null);
            let session_id = params
                .get("sessionId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let tool_call = params.get("toolCall").cloned().unwrap_or(Value::Null);
            let options = params
                .get("options")
                .and_then(Value::as_array)
                .map(|options| {
                    options
                        .iter()
                        .filter_map(|option| serde_json::from_value(option.clone()).ok())
                        .collect()
                })
                .unwrap_or_default();
            let (tx, rx) = oneshot::channel::<PermissionOutcome>();
            let respond_via = outgoing.clone();
            tokio::spawn(async move {
                // No answer (consumer dropped the responder) counts as
                // cancelled; the agent must never hang on us.
                let outcome = rx.await.unwrap_or(PermissionOutcome::Cancelled);
                let response = json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": outcome.to_result_value(),
                });
                let _ = respond_via.send(response.to_string());
            });
            let _ = events.send(AgentEvent::PermissionRequest {
                session_id,
                tool_call,
                options,
                responder: tx,
            });
        }
        // Any other agent request: politely refuse rather than hang it.
        (Some(id), Some(method)) => {
            let response = json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": format!("method not supported: {method}") },
            });
            let _ = outgoing.send(response.to_string());
        }
        // A notification.
        (None, Some("session/update")) => {
            if let Some(params) = message.get("params") {
                if let Some(event) = protocol::decode_session_update(params) {
                    let _ = events.send(event);
                }
            }
        }
        (None, _) => {}
    }
}
