//! The agent pane: AI threads with the user's installed coding agents
//! over ACP. zeDB renders the conversation and answers permission
//! requests; the agent brings its own auth and tools.
//!
//! One thread at a time, streamed markdown via gpui-component's
//! TextView, compact tool lines, inline permission cards, cancel.
//! Sessions start in the open migration repo's checkout when there is
//! one.

use std::sync::Arc;

use gpui::{div, prelude::*, px, svg, Action, Context, Entity, Focusable, Window};
use gpui_component::button::Button;
use gpui_component::input::{Input, InputState};
use gpui_component::menu::{DropdownMenu, PopupMenu};
use gpui_component::text::TextView;
use tokio::sync::oneshot;
use zedb_acp::{AcpError, AgentConnection, AgentEvent, PermissionOption, PermissionOutcome};

use crate::author::RollbackChoice;
use crate::rt;
use crate::theme;
use crate::Workspace;

#[derive(Clone, PartialEq, Action)]
#[action(no_json, no_register)]
pub struct StartAgentThread {
    pub index: usize,
}

#[derive(Clone, PartialEq, Action)]
#[action(no_json, no_register)]
pub struct OpenAddAgent;

/// Window-needing work requested by an agent over the bridge.
pub enum AgentEffect {
    /// A queued error-bar ask whose session just became ready.
    SendPendingAsk,
    ProposeMigration {
        upgrade_sql: String,
        rollback_sql: Option<String>,
        rollback_class: String,
        targeted: bool,
    },
    ProposeQuery {
        sql: String,
    },
    OpenQueryView,
}

/// One forwarded tool call awaiting the app's answer.
struct BridgeRequest {
    tool: String,
    arguments: serde_json::Value,
    respond: oneshot::Sender<(String, bool)>,
}

/// Sent once at the start of every thread: orientation on zeDB and
/// its tools. Deliberately light-touch: the user knows whose agent
/// they are running and it must still do anything they ask.
const AGENT_PRIMER: &str = "[zeDB agent primer]\n\
You are running inside zeDB, a ClickHouse explorer and fleet migration tool; \
this thread lives in its agent pane.\n\
- The zedb MCP tools (mcp__zedb__*) answer for THIS app's open migration repo \
and connection: fleet_status, list_migrations, migration_sql, dry_run, drift, \
list_databases, list_tables, describe, run_query (read-only, capped), \
schema_search and lint_sql (instant, served from zeDB's schema cache: use \
them for reconnaissance and to check drafted SQL, but confirm with describe \
before DDL since cached answers can lag), \
propose_migration (fills the migration authoring overlay with a draft), \
propose_query (puts SQL in the query editor), navigate (switch views, select \
a database).\n\
- HARD RULE: anything about this app's connection, screen, schema, or data \
is answered ONLY through the zedb tools. Other configured ClickHouse MCP \
servers point at unrelated clusters, no matter how similar their names look; \
never substitute one for the zedb connection. If the zedb tools are missing \
from this session, say exactly that and stop; answering from another server \
would silently describe the wrong cluster.\n\
- zeDB's write paths are consent-gated: you cannot apply migrations or run \
writes through the zedb tools. Propose drafts (propose_migration, \
propose_query) and the user reviews, checks, and applies through zeDB.\n\
- The fleet view's controls are: lock (unlock writes), upgrade_all, \
rollback (per database), new_migration, regen (rewrite current-state from \
the chain), check_chain, verify_all. check_chain and regen_preview exist \
as read-only tools; the server-write controls are the user's alone.\n\
- Diagnosis before direction: when something fails, find out WHY with the \
read-only tools (check_chain, regen_preview, drift) and explain it. Do \
not call highlight_control as a reflex. After explaining, offer the \
choice: highlight the relevant control so the user acts (e.g. Regen), or \
do what your own tools can legitimately do yourself (regen_preview gives \
the canonical current-state; with the user's go-ahead you may edit \
current-state/ files directly). Highlight only when the user picks that \
option or asks where something is. Server writes are never yours either \
way.\n\
- Delivering SQL: follow the screen context. With the query editor open, \
hand SQL over with propose_query, not propose_migration. If that SQL is DDL \
(CREATE/ALTER/DROP) and a migration repo is open, still deliver it with \
propose_query, and add one sentence offering to capture it as a migration \
instead; draft one with propose_migration only if the user says yes. Open \
the migration overlay unprompted only when the user is working in the fleet \
view or asked for a migration.\n\
- Migrations template with ${db} and ${cluster}; each lives in \
migrations/YYYY/MM/NNNNN as upgrade.sql plus rollback.sql whose first line is \
'-- rollback-class: clean|structural|irreversible'.\n\
This is orientation, not restriction: do whatever the user asks.";

/// The icon shipped for a discovered agent id.
pub(crate) fn icon_for(id: &str) -> &'static str {
    match id {
        "claude-code" => "icons/agent-claude.svg",
        "codex" => "icons/agent-codex.svg",
        _ => "icons/sparkle.svg",
    }
}

/// One rendered item in the transcript.
pub enum ThreadEntry {
    User(String),
    /// Accumulates the current turn's streamed reply; markdown.
    Assistant(String),
    Tool {
        id: String,
        title: String,
        status: String,
    },
    /// A permission request; `answered` records the chosen option.
    Permission {
        title: String,
        input: Option<String>,
        options: Vec<PermissionOption>,
        answered: Option<String>,
    },
    Info(String),
}

pub struct ThreadState {
    pub agent_name: String,
    pub agent_icon: String,
    /// Attach a snapshot of what the user is looking at to each send.
    pub include_context: bool,
    /// Which cached connection this thread belongs to.
    pub cache_key: String,
    /// Tool activity happened since the last message chunk, so the
    /// next chunk starts a fresh paragraph instead of gluing onto the
    /// previous one (approval notices and answers otherwise merge).
    pub break_assistant: bool,
    /// The session primer has been sent (first send of the thread).
    pub primed: bool,
    /// Transcript scroll position, for stick-to-bottom streaming.
    pub scroll: gpui::ScrollHandle,
    /// Follow new content while the user has not scrolled away.
    pub stick_to_bottom: bool,
    pub connection: Arc<AgentConnection>,
    pub session_id: Option<String>,
    pub entries: Vec<ThreadEntry>,
    pub input: Entity<InputState>,
    pub running: bool,
    pub status: Option<String>,
    pub pending_permissions: std::collections::VecDeque<oneshot::Sender<PermissionOutcome>>,
    pub generation: u64,
}

pub struct AgentPaneState {
    pub open: bool,
    pub width: f32,
    pub resizing: bool,
    pub thread: Option<ThreadState>,
    pub picker_open: bool,
    pub next_generation: u64,
    /// What discovery found, in menu order: built-ins then customs.
    pub agents: Vec<zedb_acp::discovery::DiscoveredAgent>,
    /// One live process per agent, shared across threads: ACP is
    /// multi-session, and respawning per thread made macOS re-ask
    /// keychain approvals for the agent's credentials on every thread.
    pub connections: std::collections::HashMap<String, Arc<AgentConnection>>,
    /// The unix socket where app-hosted tools (propose_*, navigate)
    /// arrive from the MCP serve subprocess; created lazily once.
    pub bridge_socket: Option<std::path::PathBuf>,
    /// Effects that need a Window (editor creation); queued by the
    /// bridge pump and drained at the top of render.
    pub pending_effects: Vec<AgentEffect>,
    /// The Add More Agents form, when open.
    pub add_form: Option<AddAgentForm>,
    /// A previous session's transcript, reopened read-only.
    pub restored: Option<(String, Vec<(String, String)>)>,
    /// An error-bar ask waiting for a session: (visible message,
    /// hidden context). Sent automatically once ready.
    pub pending_ask: Option<(String, String)>,
}

pub struct AddAgentForm {
    pub name: Entity<InputState>,
    pub command: Entity<InputState>,
    pub error: Option<String>,
}

impl AgentPaneState {
    pub fn new(width: f32) -> Self {
        Self {
            open: false,
            width,
            resizing: false,
            thread: None,
            picker_open: false,
            next_generation: 0,
            agents: Vec::new(),
            connections: std::collections::HashMap::new(),
            bridge_socket: None,
            pending_effects: Vec::new(),
            add_form: None,
            restored: None,
            pending_ask: None,
        }
    }
}

/// Where the last thread's transcript persists for read-only reopen.
fn transcript_path() -> Option<std::path::PathBuf> {
    dirs::data_local_dir().map(|dir| dir.join("zedb").join("last-thread.json"))
}

/// Serialize the live transcript; called after each completed turn.
fn persist_transcript(thread: &ThreadState) {
    let Some(path) = transcript_path() else {
        return;
    };
    let entries: Vec<serde_json::Value> = thread
        .entries
        .iter()
        .map(|entry| {
            let (kind, text) = match entry {
                ThreadEntry::User(text) => ("user", text.clone()),
                ThreadEntry::Assistant(text) => ("assistant", text.clone()),
                ThreadEntry::Tool { title, status, .. } => ("tool", format!("{title} ({status})")),
                ThreadEntry::Permission {
                    title, answered, ..
                } => (
                    "permission",
                    format!("{title} ({})", answered.as_deref().unwrap_or("unanswered")),
                ),
                ThreadEntry::Info(text) => ("info", text.clone()),
            };
            serde_json::json!({ "kind": kind, "text": text })
        })
        .collect();
    let value = serde_json::json!({ "agent": thread.agent_name, "entries": entries });
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, value.to_string());
}

/// Load the persisted transcript, if any.
fn load_transcript() -> Option<(String, Vec<(String, String)>)> {
    let raw = std::fs::read_to_string(transcript_path()?).ok()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let agent = value.get("agent")?.as_str()?.to_string();
    let entries = value
        .get("entries")?
        .as_array()?
        .iter()
        .filter_map(|entry| {
            Some((
                entry.get("kind")?.as_str()?.to_string(),
                entry.get("text")?.as_str()?.to_string(),
            ))
        })
        .collect();
    Some((agent, entries))
}

/// Append one line to the agent debug log
/// (~/Library/Application Support/zedb/agent-log.jsonl). Best-effort:
/// logging must never break the conversation.
fn agent_log(kind: &str, data: serde_json::Value) {
    let Some(dir) = dirs::data_local_dir().map(|dir| dir.join("zedb")) else {
        return;
    };
    let _ = std::fs::create_dir_all(&dir);
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or(0);
    let line = serde_json::json!({ "ts": millis, "kind": kind, "data": data });
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("agent-log.jsonl"))
    {
        use std::io::Write;
        let _ = writeln!(file, "{line}");
    }
}

mod controller;
#[path = "controller/bridge.rs"]
mod controller_bridge;
#[path = "controller/events.rs"]
mod controller_events;
#[path = "controller/messages.rs"]
mod controller_messages;
#[path = "controller/registry.rs"]
mod controller_registry;
mod view;
