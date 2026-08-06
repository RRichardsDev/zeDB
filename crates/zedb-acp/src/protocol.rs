//! Agent Client Protocol wire types (docs/PHASE-3.1.md M0).
//!
//! Typed where load-bearing, `serde_json::Value` where the protocol is
//! young and adapters vary: an unknown field or update kind must never
//! break the conversation, only arrive as `raw`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The ACP revision this client speaks.
pub const PROTOCOL_VERSION: u16 = 1;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    pub protocol_version: u16,
    pub client_capabilities: ClientCapabilities,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientCapabilities {
    pub fs: FsCapabilities,
}

/// File-system capabilities offered to the agent. zeDB offers none in
/// M0: agents use their own file tools; the pane is a conversation.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FsCapabilities {
    pub read_text_file: bool,
    pub write_text_file: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub protocol_version: u16,
    #[serde(default)]
    pub agent_capabilities: Value,
    #[serde(default)]
    pub auth_methods: Vec<Value>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewSessionParams {
    pub cwd: String,
    pub mcp_servers: Vec<McpServerConfig>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerConfig {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<EnvVariable>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EnvVariable {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewSessionResult {
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptParams {
    pub session_id: String,
    pub prompt: Vec<ContentBlock>,
}

/// Content sent to the agent. Received content is handled leniently
/// from raw `Value`s instead (see `text_of`).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text { text: String },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptResult {
    pub stop_reason: String,
}

/// Extract readable text from a content block value, whatever its shape.
pub fn text_of(content: &Value) -> Option<&str> {
    content.get("text").and_then(Value::as_str)
}

/// One option the agent offers on a permission request.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionOption {
    pub option_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub kind: String,
}

/// What happened during a turn, decoded from `session/update`
/// notifications and agent-initiated requests.
#[derive(Debug)]
pub enum AgentEvent {
    /// A chunk of the agent's visible reply.
    MessageChunk { text: String },
    /// A chunk of the agent's reasoning, when it streams any.
    ThoughtChunk { text: String },
    /// A tool call started (or was announced).
    ToolCall {
        id: String,
        title: String,
        status: String,
        raw: Value,
    },
    /// A tool call changed state.
    ToolCallUpdate {
        id: String,
        status: String,
        raw: Value,
    },
    /// The agent published or revised a plan.
    Plan { raw: Value },
    /// An update kind this client does not know; carried, not dropped.
    Other { kind: String, raw: Value },
    /// The agent asks the user to approve something; exactly one call
    /// of `respond` answers it.
    PermissionRequest {
        session_id: String,
        tool_call: Value,
        options: Vec<PermissionOption>,
        responder: tokio::sync::oneshot::Sender<PermissionOutcome>,
    },
    /// A line the agent wrote to stderr (auth hints live here).
    Stderr { line: String },
    /// The agent process ended or the pipe closed.
    Closed { reason: String },
}

/// The user's answer to a permission request.
#[derive(Debug, Clone)]
pub enum PermissionOutcome {
    Selected { option_id: String },
    Cancelled,
}

impl PermissionOutcome {
    pub(crate) fn to_result_value(&self) -> Value {
        match self {
            Self::Selected { option_id } => serde_json::json!({
                "outcome": { "outcome": "selected", "optionId": option_id }
            }),
            Self::Cancelled => serde_json::json!({
                "outcome": { "outcome": "cancelled" }
            }),
        }
    }
}

/// Decode one `session/update` notification into events.
pub(crate) fn decode_session_update(params: &Value) -> Option<AgentEvent> {
    let update = params.get("update")?;
    let kind = update.get("sessionUpdate").and_then(Value::as_str)?;
    let event = match kind {
        "agent_message_chunk" => AgentEvent::MessageChunk {
            text: update
                .get("content")
                .and_then(text_of)
                .unwrap_or_default()
                .to_string(),
        },
        "agent_thought_chunk" => AgentEvent::ThoughtChunk {
            text: update
                .get("content")
                .and_then(text_of)
                .unwrap_or_default()
                .to_string(),
        },
        "tool_call" => AgentEvent::ToolCall {
            id: update
                .get("toolCallId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            title: update
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            status: update
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("pending")
                .to_string(),
            raw: update.clone(),
        },
        "tool_call_update" => AgentEvent::ToolCallUpdate {
            id: update
                .get("toolCallId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            status: update
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            raw: update.clone(),
        },
        "plan" => AgentEvent::Plan {
            raw: update.clone(),
        },
        other => AgentEvent::Other {
            kind: other.to_string(),
            raw: update.clone(),
        },
    };
    Some(event)
}
