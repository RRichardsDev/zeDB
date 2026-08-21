//! zedb-acp: an Agent Client Protocol client.
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
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot};

const MAX_ACP_FRAME_BYTES: usize = 2 * 1024 * 1024;
const MAX_STDERR_LINE_BYTES: usize = 64 * 1024;
const MAX_PENDING_REQUESTS: usize = 64;
const OUTGOING_CAPACITY: usize = 64;
const EVENT_CAPACITY: usize = 256;
const HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
const PROMPT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

pub use protocol::{
    AgentEvent, ContentBlock, EnvVariable, InitializeResult, McpServerConfig, NewSessionResult,
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
    #[error("agent resource limit exceeded: {0}")]
    Limit(&'static str),
    #[error("agent request timed out: {0}")]
    Timeout(&'static str),
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, AcpError>>>>>;

/// A live connection to one agent process. Dropping it kills the
/// process; `events()` yields the conversation as it streams.
pub struct AgentConnection {
    child: Child,
    outgoing: mpsc::Sender<String>,
    pending: Pending,
    next_id: AtomicU64,
    events: Option<mpsc::Receiver<AgentEvent>>,
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
        let (event_tx, event_rx) = mpsc::channel(EVENT_CAPACITY);
        let (outgoing_tx, mut outgoing_rx) = mpsc::channel::<String>(OUTGOING_CAPACITY);

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
        // Awaiting the send gives real backpressure: a momentarily busy
        // consumer delays stderr instead of silently losing it.
        let stderr_events = event_tx.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr);
            loop {
                let line = match read_bounded_line(&mut reader, MAX_STDERR_LINE_BYTES).await {
                    Ok(BoundedLine::Line(line)) => String::from_utf8_lossy(&line).into_owned(),
                    Ok(BoundedLine::TooLarge) => {
                        "agent stderr line exceeded the 64 KiB safety limit".into()
                    }
                    Ok(BoundedLine::Eof) | Err(_) => break,
                };
                if stderr_events
                    .send(AgentEvent::Stderr { line })
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });

        // Reader pump: route responses to pending requests, decode
        // notifications into events, answer agent-initiated requests.
        let reader_pending = pending.clone();
        let reader_outgoing = outgoing_tx.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            let close_reason = loop {
                let line = match read_bounded_line(&mut reader, MAX_ACP_FRAME_BYTES).await {
                    Ok(BoundedLine::Line(line)) => line,
                    Ok(BoundedLine::TooLarge) => {
                        break "agent frame exceeded the 2 MiB safety limit";
                    }
                    Ok(BoundedLine::Eof) => break "agent process ended",
                    Err(_) => break "could not read agent output",
                };
                let Ok(message) = serde_json::from_slice::<Value>(&line) else {
                    continue;
                };
                if !route_message(message, &reader_pending, &reader_outgoing, &event_tx).await {
                    break "agent event consumer stopped";
                }
            };
            // Fail everything still in flight, then tell the consumer.
            {
                let mut pending = reader_pending.lock().expect("pending lock");
                for (_, responder) in pending.drain() {
                    let _ = responder.send(Err(AcpError::Closed));
                }
            }
            // Awaited so the terminal event survives a full queue; the
            // consumer relies on it to stop spinners and evict caches.
            let _ = event_tx
                .send(AgentEvent::Closed {
                    reason: close_reason.into(),
                })
                .await;
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
    pub fn take_events(&mut self) -> mpsc::Receiver<AgentEvent> {
        self.events.take().expect("events already taken")
    }

    async fn request(
        &self,
        method: &'static str,
        params: Value,
        deadline: std::time::Duration,
    ) -> Result<Value, AcpError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().expect("pending lock");
            if pending.len() >= MAX_PENDING_REQUESTS {
                return Err(AcpError::Limit("too many pending requests"));
            }
            pending.insert(id, tx);
        }
        let message = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let message = message.to_string();
        if message.len() > MAX_ACP_FRAME_BYTES {
            self.pending.lock().expect("pending lock").remove(&id);
            return Err(AcpError::Limit("outgoing frame exceeds 2 MiB"));
        }
        match tokio::time::timeout(deadline, self.outgoing.send(message)).await {
            Ok(Ok(())) => {}
            Ok(Err(_)) => {
                self.pending.lock().expect("pending lock").remove(&id);
                return Err(AcpError::Closed);
            }
            Err(_) => {
                self.pending.lock().expect("pending lock").remove(&id);
                return Err(AcpError::Timeout(method));
            }
        }
        match tokio::time::timeout(deadline, rx).await {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(_)) => Err(AcpError::Closed),
            Err(_) => {
                self.pending.lock().expect("pending lock").remove(&id);
                Err(AcpError::Timeout(method))
            }
        }
    }

    fn notify(&self, method: &str, params: Value) -> Result<(), AcpError> {
        let message = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        let message = message.to_string();
        if message.len() > MAX_ACP_FRAME_BYTES {
            return Err(AcpError::Limit("outgoing frame exceeds 2 MiB"));
        }
        self.outgoing
            .try_send(message)
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => AcpError::Limit("outgoing queue is full"),
                mpsc::error::TrySendError::Closed(_) => AcpError::Closed,
            })
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
                HANDSHAKE_TIMEOUT,
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
                HANDSHAKE_TIMEOUT,
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
                PROMPT_TIMEOUT,
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

/// Route one incoming message. Sends into the bounded event and outgoing
/// queues await free space (backpressure through the pipe) rather than
/// dropping; `false` means the consumer is gone and the reader should stop.
async fn route_message(
    message: Value,
    pending: &Pending,
    outgoing: &mpsc::Sender<String>,
    events: &mpsc::Sender<AgentEvent>,
) -> bool {
    if message.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return true;
    }
    let id = message.get("id");
    let method = message.get("method").and_then(Value::as_str);
    match (id, method) {
        // A response to one of our requests.
        (Some(id), None) => {
            let Some(id) = id.as_u64() else { return true };
            let Some(responder) = pending.lock().expect("pending lock").remove(&id) else {
                return true;
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
            true
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
                // cancelled; the agent must never hang on us. Awaited:
                // a momentarily full writer queue delays the answer, it
                // must never swallow it.
                let outcome = rx.await.unwrap_or(PermissionOutcome::Cancelled);
                let response = json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": outcome.to_result_value(),
                });
                let _ = respond_via.send(response.to_string()).await;
            });
            events
                .send(AgentEvent::PermissionRequest {
                    session_id,
                    tool_call,
                    options,
                    responder: tx,
                })
                .await
                .is_ok()
        }
        // Any other agent request: politely refuse rather than hang it.
        (Some(id), Some(method)) => {
            let response = json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": format!("method not supported: {method}") },
            });
            outgoing.send(response.to_string()).await.is_ok()
        }
        // A notification.
        (None, Some("session/update")) => {
            if let Some(params) = message.get("params") {
                if let Some(event) = protocol::decode_session_update(params) {
                    return events.send(event).await.is_ok();
                }
            }
            true
        }
        (None, _) => true,
    }
}

enum BoundedLine {
    Line(Vec<u8>),
    TooLarge,
    Eof,
}

/// Read and fully consume one newline-delimited frame without ever retaining
/// more than `limit` bytes. Oversized frames are drained so the next call
/// starts on a clean boundary.
async fn read_bounded_line<R>(reader: &mut R, limit: usize) -> std::io::Result<BoundedLine>
where
    R: AsyncBufRead + Unpin,
{
    let mut line = Vec::new();
    let mut too_large = false;
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return Ok(if too_large {
                BoundedLine::TooLarge
            } else if line.is_empty() {
                BoundedLine::Eof
            } else {
                BoundedLine::Line(line)
            });
        }
        let consumed = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |index| index + 1);
        let ended = available.get(consumed.saturating_sub(1)) == Some(&b'\n');
        if !too_large {
            if line.len().saturating_add(consumed) > limit {
                too_large = true;
                line.clear();
            } else {
                line.extend_from_slice(&available[..consumed]);
            }
        }
        reader.consume(consumed);
        if ended {
            return Ok(if too_large {
                BoundedLine::TooLarge
            } else {
                BoundedLine::Line(line)
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bounded_lines_drain_an_oversized_frame() {
        use tokio::io::AsyncWriteExt as _;

        let (reader, mut writer) = tokio::io::duplex(64);
        writer.write_all(b"0123456789\n{}\n").await.unwrap();
        drop(writer);
        let mut reader = BufReader::new(reader);
        assert!(matches!(
            read_bounded_line(&mut reader, 8).await.unwrap(),
            BoundedLine::TooLarge
        ));
        let BoundedLine::Line(line) = read_bounded_line(&mut reader, 8).await.unwrap() else {
            panic!("expected the next bounded frame");
        };
        assert_eq!(line, b"{}\n");
    }
}
