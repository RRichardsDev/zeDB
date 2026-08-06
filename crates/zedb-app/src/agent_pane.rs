//! The agent pane (docs/PHASE-3.1.md M1): AI threads with installed
//! coding agents over ACP. zeDB renders the conversation and answers
//! permission requests; the agent brings its own auth and tools.
//!
//! M1 scope: one thread at a time, a built-in agent list (discovery is
//! M2), streamed markdown via gpui-component's TextView, compact tool
//! lines, inline permission cards, cancel. Sessions start in the open
//! migration repo's checkout when there is one.

use std::sync::Arc;

use gpui::{div, prelude::*, px, rgb, svg, Action, Context, Entity, Focusable, Window};
use gpui_component::button::Button;
use gpui_component::input::{Input, InputState};
use gpui_component::menu::{DropdownMenu, PopupMenu};
use gpui_component::text::TextView;
use tokio::sync::oneshot;
use zedb_acp::{AcpError, AgentConnection, AgentEvent, PermissionOption, PermissionOutcome};

use crate::author::RollbackChoice;
use crate::rt;
use crate::theme::{BG, BG_SIDEBAR, BG_STATUS, BORDER, DANGER, SUCCESS, TEXT, TEXT_DIM};
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
propose_migration (fills the migration authoring overlay with a draft), \
propose_query (puts SQL in the query editor), navigate (switch views, select \
a database).\n\
- Prefer them over any other configured ClickHouse MCP servers for anything \
about what is on screen here; other servers may point at unrelated clusters.\n\
- zeDB's write paths are consent-gated: you cannot apply migrations or run \
writes through the zedb tools. Propose drafts (propose_migration, \
propose_query) and the user reviews, checks, and applies through zeDB.\n\
- Migrations template with ${db} and ${cluster}; each lives in \
migrations/YYYY/MM/NNNNN as upgrade.sql plus rollback.sql whose first line is \
'-- rollback-class: clean|structural|irreversible'.\n\
This is orientation, not restriction: do whatever the user asks.";

/// The icon shipped for a discovered agent id.
fn icon_for(id: &str) -> &'static str {
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
        }
    }
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

impl Workspace {
    pub(crate) fn agent_toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.agent.open = !self.agent.open;
        if self.agent.open {
            self.agent_refresh_registry();
            self.agent_focus_composer(window, cx);
        }
        cx.notify();
    }

    /// Land keyboard focus in the composer when there is one.
    fn agent_focus_composer(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(thread) = self.agent.thread.as_ref() {
            let handle = thread.input.read(cx).focus_handle(cx);
            window.focus(&handle);
        }
    }

    /// Re-run discovery and re-resolve the user's custom agents.
    pub(crate) fn agent_refresh_registry(&mut self) {
        let mut agents = zedb_acp::discovery::discover_known();
        for custom in &self.preferences.custom_agents {
            agents.push(zedb_acp::discovery::resolve_custom(
                &custom.name,
                &custom.command,
                &custom.args,
            ));
        }
        self.agent.agents = agents;
    }

    /// Spawn an agent and open a thread with it. The spawn is cheap and
    /// synchronous; initialize and session setup stream in behind it.
    pub(crate) fn agent_start_thread(
        &mut self,
        agent_index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.agent.agents.is_empty() {
            self.agent_refresh_registry();
        }
        let Some(agent) = self.agent.agents.get(agent_index).cloned() else {
            return;
        };
        if let zedb_acp::discovery::Availability::Missing { hint } = &agent.availability {
            self.notice = Some(format!("{}: {hint}", agent.name));
            self.notice_warning = true;
            self.notice_flash_id += 1;
            cx.notify();
            return;
        }
        let name = agent.name.clone();
        let icon = icon_for(&agent.id).to_string();
        let program = agent.command.clone();
        let args = agent.args.clone();
        let cwd = self
            .fleet
            .repo
            .as_ref()
            .map(|repo| repo.root.clone())
            .or_else(dirs::home_dir);

        // Reuse the agent's live process when one exists; otherwise
        // spawn and pump. AgentConnection spawns tokio tasks and a
        // tokio child process; both need the runtime context or they
        // abort the app from inside an AppKit event handler.
        let cache_key = format!("{}|{program}", agent.id);
        let (connection, fresh_events) = match self.agent.connections.get(&cache_key) {
            Some(connection) => (connection.clone(), None),
            None => {
                let _runtime = rt::tokio().enter();
                let mut connection =
                    match AgentConnection::spawn(&program, &args, &[], cwd.as_deref()) {
                        Ok(connection) => connection,
                        Err(error) => {
                            self.notice = Some(format!("Could not start {name}: {error}"));
                            self.notice_warning = true;
                            self.notice_flash_id += 1;
                            cx.notify();
                            return;
                        }
                    };
                let events = connection.take_events();
                let connection = Arc::new(connection);
                self.agent
                    .connections
                    .insert(cache_key.clone(), connection.clone());
                (connection, Some(events))
            }
        };

        self.agent.next_generation += 1;
        let generation = self.agent.next_generation;
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .auto_grow(1, 6)
                .placeholder("Message the agent...")
        });
        // Enter sends; shift-enter (bound app-side to the secondary
        // Enter action) stays a newline and is ignored here.
        cx.subscribe_in(
            &input,
            window,
            |this, _, event: &gpui_component::input::InputEvent, window, cx| {
                if let gpui_component::input::InputEvent::PressEnter { secondary: false } = event {
                    this.agent_send(window, cx);
                }
            },
        )
        .detach();
        let reused = fresh_events.is_none();
        agent_log(
            "thread_start",
            serde_json::json!({ "agent": name, "reused_process": reused }),
        );
        self.agent.thread = Some(ThreadState {
            agent_name: name.to_string(),
            agent_icon: icon.clone(),
            include_context: true,
            cache_key: cache_key.clone(),
            break_assistant: false,
            primed: false,
            scroll: gpui::ScrollHandle::new(),
            stick_to_bottom: true,
            connection: connection.clone(),
            session_id: None,
            entries: Vec::new(),
            input,
            running: false,
            status: Some("starting...".into()),
            pending_permissions: std::collections::VecDeque::new(),
            generation,
        });
        self.agent.picker_open = false;
        self.agent_focus_composer(window, cx);
        cx.notify();

        // Watch the open repo while this thread lives: agents edit
        // files through their own tools and the chain, matrix, and git
        // chip must not go silently stale.
        self.agent_watch_repo(generation, cx);

        // Handshake: initialize, then open the session in the repo,
        // registering the zedb MCP server (this same executable in a
        // hidden serve mode) so the agent gets the fleet and query
        // tools. Credentials travel via a 0600 file the server deletes
        // on read, never argv or env.
        let bridge_socket = self.agent_ensure_bridge(cx);
        let mcp_servers = self.agent_mcp_server_config(bridge_socket);
        let handshake_connection = connection.clone();
        let handshake_cwd = cwd.clone().unwrap_or_else(|| "/".into());
        let handle = rt::tokio().spawn(async move {
            if !reused {
                handshake_connection.initialize().await?;
            }
            let session = handshake_connection
                .new_session(&handshake_cwd, mcp_servers)
                .await?;
            Ok::<_, AcpError>(session.session_id)
        });
        cx.spawn(async move |this, cx| {
            let result = handle.await;
            this.update(cx, |this, cx| {
                let Some(thread) = this.agent.thread.as_mut() else {
                    return;
                };
                if thread.generation != generation {
                    return;
                }
                let flattened = match result {
                    Ok(inner) => inner.map_err(|error| error.to_string()),
                    Err(error) => Err(error.to_string()),
                };
                match flattened {
                    Ok(session_id) => {
                        agent_log(
                            "session_started",
                            serde_json::json!({ "session_id": session_id }),
                        );
                        thread.session_id = Some(session_id);
                        thread.status = None;
                    }
                    Err(error) => {
                        agent_log("session_failed", serde_json::json!({ "error": error }));
                        thread.status = Some(format!(
                            "could not start a session: {error}; if this is an auth \
                             problem, log in with the agent's own CLI first"
                        ));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();

        // Event pump: one per process, surviving across threads. Events
        // land in whichever thread currently belongs to this process;
        // a Closed event also evicts the cached connection so the next
        // thread respawns cleanly.
        if let Some(mut events) = fresh_events {
            let pump_key = cache_key.clone();
            cx.spawn(async move |this, cx| {
                while let Some(event) = events.recv().await {
                    let closed = matches!(event, AgentEvent::Closed { .. });
                    let alive = this
                        .update(cx, |this, cx| {
                            if closed {
                                this.agent.connections.remove(&pump_key);
                            }
                            this.agent_apply_event_for(&pump_key, event, cx)
                        })
                        .unwrap_or(false);
                    if closed || !alive {
                        break;
                    }
                }
            })
            .detach();
        }
    }

    pub(crate) fn agent_open_add_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.agent.open = true;
        self.agent.add_form = Some(AddAgentForm {
            name: cx.new(|cx| InputState::new(window, cx).placeholder("Name")),
            command: cx.new(|cx| {
                InputState::new(window, cx).placeholder("command and args, e.g. my-agent --acp")
            }),
            error: None,
        });
        cx.notify();
    }

    fn agent_save_custom(&mut self, cx: &mut Context<Self>) {
        let Some(form) = &self.agent.add_form else {
            return;
        };
        let name = form.name.read(cx).value().trim().to_string();
        let command_line = form.command.read(cx).value().trim().to_string();
        let mut parts = command_line.split_whitespace().map(str::to_string);
        let command = parts.next().unwrap_or_default();
        if name.is_empty() || command.is_empty() {
            if let Some(form) = &mut self.agent.add_form {
                form.error = Some("both a name and a command are needed".into());
            }
            cx.notify();
            return;
        }
        self.preferences.custom_agents.push(zedb_core::CustomAgent {
            name,
            command,
            args: parts.collect(),
        });
        if let Err(error) = zedb_core::save_preferences(&self.preferences) {
            self.notice = Some(format!("Could not save preferences: {error}"));
            self.notice_warning = true;
            self.notice_flash_id += 1;
        }
        self.agent.add_form = None;
        self.agent_refresh_registry();
        cx.notify();
    }

    fn agent_remove_custom(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.preferences.custom_agents.len() {
            self.preferences.custom_agents.remove(index);
            if let Err(error) = zedb_core::save_preferences(&self.preferences) {
                self.notice = Some(format!("Could not save preferences: {error}"));
                self.notice_warning = true;
                self.notice_flash_id += 1;
            }
            self.agent_refresh_registry();
        }
        cx.notify();
    }

    /// Poll the open repo's file signature while `generation` is the
    /// live thread; on change, reopen the repo and refresh git state.
    fn agent_watch_repo(&mut self, generation: u64, cx: &mut Context<Self>) {
        fn signature(root: &std::path::Path) -> u64 {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            let stamp = |path: &std::path::Path, hasher: &mut _| {
                if let Ok(meta) = std::fs::metadata(path) {
                    path.hash(hasher);
                    meta.len().hash(hasher);
                    if let Ok(modified) = meta.modified() {
                        modified.hash(hasher);
                    }
                }
            };
            stamp(&root.join("zedb.toml"), &mut hasher);
            stamp(&root.join("exclusions.toml"), &mut hasher);
            let mut stack = vec![root.join("migrations")];
            while let Some(dir) = stack.pop() {
                let Ok(entries) = std::fs::read_dir(&dir) else {
                    continue;
                };
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        stack.push(path);
                    } else {
                        stamp(&path, &mut hasher);
                    }
                }
            }
            hasher.finish()
        }

        let Some(root) = self.fleet.repo.as_ref().map(|repo| repo.root.clone()) else {
            return;
        };
        let mut last = signature(&root);
        cx.spawn(async move |this, cx| loop {
            gpui::Timer::after(std::time::Duration::from_secs(2)).await;
            let stale = this
                .update(cx, |this, cx| {
                    let live = this
                        .agent
                        .thread
                        .as_ref()
                        .is_some_and(|thread| thread.generation == generation);
                    if !live {
                        return true;
                    }
                    let current = signature(&root);
                    if current != last {
                        last = current;
                        if let Ok(reopened) = zedb_core::repo::MigrationRepo::open_root(&root) {
                            this.fleet.repo = Some(Arc::new(reopened));
                        }
                        this.fleet.git = zedb_core::git::read_git_status(&root);
                        if let Some(thread) = this.agent.thread.as_mut() {
                            thread
                                .entries
                                .push(ThreadEntry::Info("repo files changed on disk".into()));
                        }
                        cx.notify();
                    }
                    false
                })
                .unwrap_or(true);
            if stale {
                break;
            }
        })
        .detach();
    }

    /// Bind the app-tool bridge socket once; returns its path.
    fn agent_ensure_bridge(&mut self, cx: &mut Context<Self>) -> Option<std::path::PathBuf> {
        if let Some(path) = &self.agent.bridge_socket {
            return Some(path.clone());
        }
        let dir = dirs::data_local_dir()?.join("zedb").join("mcp");
        std::fs::create_dir_all(&dir).ok()?;
        let path = dir.join(format!("app-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let _runtime = rt::tokio().enter();
        let listener = tokio::net::UnixListener::bind(&path).ok()?;
        let (requests_tx, mut requests_rx) = tokio::sync::mpsc::unbounded_channel();
        rt::tokio().spawn(async move {
            use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let requests_tx = requests_tx.clone();
                tokio::spawn(async move {
                    let (read_half, mut write_half) = stream.into_split();
                    let mut line = String::new();
                    if BufReader::new(read_half)
                        .read_line(&mut line)
                        .await
                        .is_err()
                    {
                        return;
                    }
                    let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
                        return;
                    };
                    let (respond_tx, respond_rx) = oneshot::channel();
                    let request = BridgeRequest {
                        tool: value
                            .get("tool")
                            .and_then(|tool| tool.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        arguments: value
                            .get("arguments")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                        respond: respond_tx,
                    };
                    if requests_tx.send(request).is_err() {
                        return;
                    }
                    let (text, is_error) =
                        respond_rx.await.unwrap_or(("app closed".to_string(), true));
                    let reply = serde_json::json!({ "text": text, "isError": is_error });
                    let _ = write_half.write_all(reply.to_string().as_bytes()).await;
                    let _ = write_half.write_all(b"\n").await;
                });
            }
        });
        cx.spawn(async move |this, cx| {
            while let Some(request) = requests_rx.recv().await {
                let live = this
                    .update(cx, |this, cx| {
                        let BridgeRequest {
                            tool,
                            arguments,
                            respond,
                        } = request;
                        let outcome = this.agent_handle_bridge_tool(&tool, &arguments, cx);
                        let _ = respond.send(outcome);
                    })
                    .is_ok();
                if !live {
                    break;
                }
            }
        })
        .detach();
        self.agent.bridge_socket = Some(path.clone());
        Some(path)
    }

    /// Execute one app tool; narrated in the thread so the UI never
    /// changes unexplained. Returns (reply text, is_error).
    fn agent_handle_bridge_tool(
        &mut self,
        tool: &str,
        arguments: &serde_json::Value,
        cx: &mut Context<Self>,
    ) -> (String, bool) {
        let narrate = |this: &mut Self, line: String, cx: &mut Context<Self>| {
            if let Some(thread) = this.agent.thread.as_mut() {
                thread.entries.push(ThreadEntry::Info(line));
            }
            cx.notify();
        };
        match tool {
            "navigate" => {
                let view = arguments
                    .get("view")
                    .and_then(|view| view.as_str())
                    .unwrap_or_default();
                let database = arguments
                    .get("database")
                    .and_then(|database| database.as_str())
                    .map(str::to_string);
                match view {
                    "fleet" => {
                        self.show_fleet = true;
                        self.show_query_editor = false;
                        if self.fleet.repo.is_none()
                            && !self.fleet.repo_path.read(cx).text().trim().is_empty()
                        {
                            self.fleet_open_repo(cx);
                        }
                        if let Some(database) = &database {
                            self.fleet.selected = Some(database.clone());
                        }
                        narrate(
                            self,
                            format!(
                                "agent: opened fleet view{}",
                                database
                                    .as_ref()
                                    .map(|database| format!(" ({database} selected)"))
                                    .unwrap_or_default()
                            ),
                            cx,
                        );
                        ("fleet view opened".into(), false)
                    }
                    "query" => {
                        self.agent.pending_effects.push(AgentEffect::OpenQueryView);
                        narrate(self, "agent: opened the query editor".into(), cx);
                        ("query editor opened".into(), false)
                    }
                    "connections" => {
                        self.show_fleet = false;
                        self.show_query_editor = false;
                        narrate(self, "agent: opened the connections view".into(), cx);
                        ("connections view opened".into(), false)
                    }
                    other => (format!("unknown view: {other}"), true),
                }
            }
            "propose_query" => {
                let Some(sql) = arguments.get("sql").and_then(|sql| sql.as_str()) else {
                    return ("sql argument required".into(), true);
                };
                self.agent.pending_effects.push(AgentEffect::ProposeQuery {
                    sql: sql.to_string(),
                });
                narrate(self, "agent: placed SQL in a query tab".into(), cx);
                (
                    "SQL placed in a new query editor tab for the user to review and run".into(),
                    false,
                )
            }
            "propose_migration" => {
                if self.fleet.repo.is_none() {
                    return (
                        "no migration repo is open in zeDB; ask the user to open one \
                         in the fleet view first"
                            .into(),
                        true,
                    );
                }
                if self.author.is_some() {
                    return (
                        "the authoring overlay is already open with a draft; ask the user \
                         to close or save it first, then propose again"
                            .into(),
                        true,
                    );
                }
                let Some(upgrade_sql) = arguments.get("upgrade_sql").and_then(|sql| sql.as_str())
                else {
                    return ("upgrade_sql argument required".into(), true);
                };
                self.agent
                    .pending_effects
                    .push(AgentEffect::ProposeMigration {
                        upgrade_sql: upgrade_sql.to_string(),
                        rollback_sql: arguments
                            .get("rollback_sql")
                            .and_then(|sql| sql.as_str())
                            .map(str::to_string),
                        rollback_class: arguments
                            .get("rollback_class")
                            .and_then(|class| class.as_str())
                            .unwrap_or("clean")
                            .to_string(),
                        targeted: arguments
                            .get("targeted")
                            .and_then(|targeted| targeted.as_bool())
                            .unwrap_or(false),
                    });
                narrate(
                    self,
                    "agent: proposed a migration draft (authoring overlay)".into(),
                    cx,
                );
                (
                    "draft placed in the authoring overlay; the user reviews, checks \
                     against the pinned server, and saves"
                        .into(),
                    false,
                )
            }
            other => (format!("unknown app tool: {other}"), true),
        }
    }

    /// Apply queued effects; called from render, where a Window exists.
    pub(crate) fn agent_drain_effects(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let effects = std::mem::take(&mut self.agent.pending_effects);
        for effect in effects {
            match effect {
                AgentEffect::OpenQueryView => {
                    self.open_query_editor_for_agent(window, cx);
                }
                AgentEffect::ProposeQuery { sql } => {
                    self.open_query_tab_with(&sql, window, cx);
                }
                AgentEffect::ProposeMigration {
                    upgrade_sql,
                    rollback_sql,
                    rollback_class,
                    targeted,
                } => {
                    // The overlay lives in the fleet view; surface it.
                    self.show_fleet = true;
                    self.show_query_editor = false;
                    self.author_open(window, cx);
                    let Some(author) = self.author.as_mut() else {
                        continue;
                    };
                    let choice = match rollback_class.as_str() {
                        "structural" => RollbackChoice::Structural,
                        "irreversible" => {
                            if rollback_sql.is_none() {
                                RollbackChoice::NoFile
                            } else {
                                RollbackChoice::Irreversible
                            }
                        }
                        _ => RollbackChoice::Clean,
                    };
                    author.targeted = targeted;
                    author.rollback_choice = choice;
                    author.upgrade.update(cx, |input, cx| {
                        input.set_value(upgrade_sql.clone(), window, cx);
                    });
                    if let (Some(rollback_sql), Some(marker)) =
                        (rollback_sql.as_deref(), choice.marker())
                    {
                        let text = if rollback_sql.trim_start().starts_with("-- rollback-class:") {
                            rollback_sql.to_string()
                        } else {
                            format!("{marker}\n{rollback_sql}")
                        };
                        author.rollback.update(cx, |input, cx| {
                            input.set_value(text, window, cx);
                        });
                    }
                    cx.notify();
                }
            }
        }
    }

    /// The zedb MCP server registration for a new session, when there
    /// is anything to serve (an open repo, a connection, or both).
    fn agent_mcp_server_config(
        &self,
        bridge_socket: Option<std::path::PathBuf>,
    ) -> Vec<zedb_acp::McpServerConfig> {
        let repo = self
            .fleet
            .repo
            .as_ref()
            .map(|repo| repo.root.display().to_string());
        let connection = self.connected.as_ref().map(|connected| {
            (
                connected.client_config.url.clone(),
                connected.client_config.user.clone(),
                connected.client_config.password.clone().unwrap_or_default(),
            )
        });
        let mut config = serde_json::Map::new();
        if let Some(repo) = repo {
            config.insert("repo".into(), repo.into());
        }
        if let Some((url, user, password)) = connection {
            config.insert("url".into(), url.into());
            config.insert("user".into(), user.into());
            config.insert("password".into(), password.into());
        }
        if let Some(socket) = bridge_socket {
            config.insert("app_socket".into(), socket.display().to_string().into());
        }
        let Some(dir) = dirs::data_local_dir().map(|dir| dir.join("zedb").join("mcp")) else {
            return Vec::new();
        };
        if std::fs::create_dir_all(&dir).is_err() {
            return Vec::new();
        }
        let path = dir.join(format!("session-{}.json", self.agent.next_generation));
        let value = serde_json::Value::Object(config);
        if std::fs::write(&path, value.to_string()).is_err() {
            return Vec::new();
        }
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        let Ok(exe) = std::env::current_exe() else {
            return Vec::new();
        };
        vec![zedb_acp::McpServerConfig {
            name: "zedb".into(),
            command: exe.display().to_string(),
            args: vec!["zedb-mcp-serve".into(), path.display().to_string()],
            env: Vec::new(),
        }]
    }

    /// Fold one agent event into the transcript. Returns whether this
    /// process still has a pane consumer (the pane may show another
    /// agent's thread; events for an idle cached process are dropped
    /// but the pump stays alive for its next thread).
    fn agent_apply_event_for(
        &mut self,
        cache_key: &str,
        event: AgentEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        match &event {
            AgentEvent::MessageChunk { text } => {
                agent_log("chunk", serde_json::json!({ "text": text }));
            }
            AgentEvent::ThoughtChunk { .. } => {}
            AgentEvent::ToolCall { raw, .. } | AgentEvent::ToolCallUpdate { raw, .. } => {
                agent_log("tool", raw.clone());
            }
            AgentEvent::Plan { raw } => agent_log("plan", raw.clone()),
            AgentEvent::Other { kind, raw } => {
                agent_log(
                    "other_update",
                    serde_json::json!({ "kind": kind, "raw": raw }),
                );
            }
            AgentEvent::PermissionRequest { tool_call, .. } => {
                agent_log("permission_request", tool_call.clone());
            }
            AgentEvent::Stderr { line } => {
                agent_log("stderr", serde_json::json!({ "line": line }));
            }
            AgentEvent::Closed { reason } => {
                agent_log("closed", serde_json::json!({ "reason": reason }));
            }
        }
        let Some(thread) = self.agent.thread.as_mut() else {
            return true;
        };
        if thread.cache_key != cache_key {
            return true;
        }
        match event {
            AgentEvent::MessageChunk { text } => {
                if thread.entries.len() > 600 {
                    thread.entries.drain(..100);
                    thread
                        .entries
                        .insert(0, ThreadEntry::Info("(older messages trimmed)".into()));
                }
                match thread.entries.last_mut() {
                    Some(ThreadEntry::Assistant(existing)) if !thread.break_assistant => {
                        existing.push_str(&text);
                    }
                    _ => thread.entries.push(ThreadEntry::Assistant(text)),
                }
                thread.break_assistant = false;
            }
            AgentEvent::ThoughtChunk { .. } => {}
            AgentEvent::ToolCall {
                id, title, status, ..
            } => {
                thread.entries.push(ThreadEntry::Tool { id, title, status });
                thread.break_assistant = true;
            }
            AgentEvent::ToolCallUpdate { id, status, .. } => {
                thread.break_assistant = true;
                for entry in thread.entries.iter_mut().rev() {
                    if let ThreadEntry::Tool {
                        id: existing,
                        status: existing_status,
                        ..
                    } = entry
                    {
                        if *existing == id {
                            *existing_status = status;
                            break;
                        }
                    }
                }
            }
            AgentEvent::Plan { .. } => {}
            AgentEvent::Other { .. } => {}
            AgentEvent::PermissionRequest {
                tool_call,
                options,
                responder,
                ..
            } => {
                let title = tool_call
                    .get("title")
                    .and_then(|title| title.as_str())
                    .unwrap_or("the agent asks for permission")
                    .to_string();
                // Always-allow memory: auto-approve tools the user has
                // permanently blessed for this agent.
                let key = format!("{}|{title}", thread.agent_name);
                if self.preferences.agent_always_allow.contains(&key) {
                    let choice = options
                        .iter()
                        .find(|option| option.option_id.contains("always"))
                        .or_else(|| options.iter().find(|option| option.kind.contains("allow")))
                        .or(options.first())
                        .map(|option| option.option_id.clone());
                    if let Some(option_id) = choice {
                        agent_log(
                            "permission_auto",
                            serde_json::json!({ "title": title, "option": option_id }),
                        );
                        let _ = responder.send(PermissionOutcome::Selected { option_id });
                        thread.entries.push(ThreadEntry::Info(format!(
                            "auto-approved: {title} (always allow)"
                        )));
                        cx.notify();
                        return true;
                    }
                }
                let input = tool_call.get("rawInput").and_then(|raw| {
                    if raw.is_null() || raw == &serde_json::json!({}) {
                        None
                    } else {
                        let mut text = raw.to_string();
                        if text.len() > 240 {
                            text.truncate(240);
                            text.push_str("...");
                        }
                        Some(text)
                    }
                });
                thread.entries.push(ThreadEntry::Permission {
                    title,
                    input,
                    options,
                    answered: None,
                });
                // Requests queue; answers pop in arrival order.
                thread.pending_permissions.push_back(responder);
            }
            AgentEvent::Stderr { line } => {
                if thread.session_id.is_none() {
                    let line = clean_log_line(&line);
                    if !line.is_empty() {
                        thread.status = Some(line);
                    }
                }
            }
            AgentEvent::Closed { reason } => {
                thread.running = false;
                thread.status = Some(reason);
            }
        }
        cx.notify();
        true
    }

    pub(crate) fn agent_send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(thread) = self.agent.thread.as_mut() else {
            return;
        };
        if thread.running {
            return;
        }
        let Some(session_id) = thread.session_id.clone() else {
            return;
        };
        let text = thread.input.read(cx).value().trim().to_string();
        if text.is_empty() {
            return;
        }
        thread
            .input
            .update(cx, |input, cx| input.set_value("", window, cx));
        let include_context = thread.include_context;
        thread.entries.push(ThreadEntry::User(text.clone()));
        if include_context {
            thread
                .entries
                .push(ThreadEntry::Info("screen context attached".into()));
        }
        thread.entries.push(ThreadEntry::Assistant(String::new()));
        thread.running = true;
        thread.status = None;
        let generation = thread.generation;
        let connection = thread.connection.clone();
        cx.notify();

        let primer = if let Some(thread) = self.agent.thread.as_mut() {
            if thread.primed {
                None
            } else {
                thread.primed = true;
                Some(AGENT_PRIMER)
            }
        } else {
            None
        };
        let mut full_text = String::new();
        if let Some(primer) = primer {
            full_text.push_str(primer);
            full_text.push_str("\n\n");
        }
        if include_context {
            full_text.push_str(&self.agent_ambient_context());
            full_text.push_str("\n\n");
        }
        full_text.push_str(&text);
        agent_log(
            "prompt",
            serde_json::json!({ "session_id": session_id, "text": full_text }),
        );
        let handle =
            rt::tokio().spawn(async move { connection.prompt(&session_id, &full_text).await });
        cx.spawn(async move |this, cx| {
            let result = handle.await;
            this.update(cx, |this, cx| {
                let Some(thread) = this.agent.thread.as_mut() else {
                    return;
                };
                if thread.generation != generation {
                    return;
                }
                thread.running = false;
                let flattened = match result {
                    Ok(inner) => inner.map_err(|error| error.to_string()),
                    Err(error) => Err(error.to_string()),
                };
                match flattened {
                    Ok(result) => {
                        agent_log(
                            "turn_done",
                            serde_json::json!({ "stop_reason": result.stop_reason }),
                        );
                        if result.stop_reason != "end_turn" {
                            thread
                                .entries
                                .push(ThreadEntry::Info(format!("[{}]", result.stop_reason)));
                        }
                    }
                    Err(error) => {
                        agent_log("turn_error", serde_json::json!({ "error": error }));
                        thread.status = Some(error);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// A snapshot of what the user is looking at; deictic questions
    /// resolve against it and the agent digs further through the zedb
    /// MCP tools. Attached visibly, never behind the user's back.
    fn agent_ambient_context(&self) -> String {
        let mut lines: Vec<String> = Vec::new();
        let screen = if self.show_fleet {
            "fleet view (databases x migrations matrix)"
        } else if self.show_query_editor {
            "query editor"
        } else {
            "connections and schema view"
        };
        lines.push(format!("screen: {screen}"));
        if let Some(connected) = &self.connected {
            lines.push(format!(
                "connection: {} ({} tier, {})",
                connected.name,
                match self.fleet_tier() {
                    zedb_core::EnvTier::Dev => "dev",
                    zedb_core::EnvTier::Staging => "staging",
                    zedb_core::EnvTier::Production => "production",
                },
                if connected.client_config.read_only {
                    "read-only"
                } else {
                    "write-capable"
                },
            ));
        } else {
            lines.push("connection: none".into());
        }
        if let Some(repo) = &self.fleet.repo {
            lines.push(format!(
                "migration repo: {} ({} migration(s){})",
                repo.root.display(),
                repo.migrations.len(),
                self.fleet
                    .git
                    .as_ref()
                    .map(|git| format!(", git {}", git.summary()))
                    .unwrap_or_default(),
            ));
        }
        if self.show_fleet {
            if let Some(selected) = &self.fleet.selected {
                if let Some(row) = self.fleet.rows.iter().find(|row| row.database == *selected) {
                    let head = row
                        .head
                        .map(|head| format!("{head:05}"))
                        .unwrap_or_else(|| "none".into());
                    lines.push(format!(
                        "selected database: {} (head {head}, {} pending, {} customised, {} failed{})",
                        row.database,
                        row.pending.len(),
                        row.customised.len(),
                        row.failed.len(),
                        row.excluded
                            .as_ref()
                            .map(|group| format!(", excluded by {group}"))
                            .unwrap_or_default(),
                    ));
                    if let Some(drift) = self.fleet.drift.get(selected) {
                        if drift.findings.is_empty() {
                            lines.push("drift: verified clean".into());
                        } else {
                            lines.push(format!("drift findings: {}", drift.findings.join("; ")));
                        }
                    }
                }
            }
            if self.fleet.pending_action.is_some() {
                lines.push("an apply confirmation modal is open".into());
            }
        }
        if let Some(author) = &self.author {
            lines.push(format!(
                "authoring overlay open on migration {:05}{}",
                author.number,
                if author.existing.is_some() {
                    " (existing)"
                } else {
                    " (new draft)"
                },
            ));
        }
        format!(
            "[zeDB screen context, attached by the app]\n{}",
            lines.join("\n")
        )
    }

    pub(crate) fn agent_cancel(&mut self, cx: &mut Context<Self>) {
        let Some(thread) = self.agent.thread.as_ref() else {
            return;
        };
        if let Some(session_id) = &thread.session_id {
            let _ = thread.connection.cancel(session_id);
        }
        cx.notify();
    }

    fn agent_answer_permission(&mut self, option_id: Option<String>, cx: &mut Context<Self>) {
        let Some(thread) = self.agent.thread.as_mut() else {
            return;
        };
        let Some(responder) = thread.pending_permissions.pop_front() else {
            return;
        };
        let outcome = match &option_id {
            Some(option_id) => PermissionOutcome::Selected {
                option_id: option_id.clone(),
            },
            None => PermissionOutcome::Cancelled,
        };
        agent_log(
            "permission_answer",
            serde_json::json!({ "option": option_id }),
        );
        let _ = responder.send(outcome);
        // Mark the OLDEST unanswered card (queue order), and remember
        // permanent grants across sessions.
        let agent_name = thread.agent_name.clone();
        let mut remember: Option<String> = None;
        for entry in thread.entries.iter_mut() {
            if let ThreadEntry::Permission {
                title, answered, ..
            } = entry
            {
                if answered.is_none() {
                    let chosen = option_id.clone().unwrap_or_else(|| "cancelled".into());
                    if chosen.contains("always") {
                        remember = Some(format!("{agent_name}|{title}"));
                    }
                    *answered = Some(chosen);
                    break;
                }
            }
        }
        if let Some(key) = remember {
            if !self.preferences.agent_always_allow.contains(&key) {
                self.preferences.agent_always_allow.push(key);
                let _ = zedb_core::save_preferences(&self.preferences);
            }
        }
        cx.notify();
    }

    pub(crate) fn agent_panel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mut panel = div()
            .w(px(self.agent.width))
            .flex_none()
            .h_full()
            .relative()
            .flex()
            .flex_col()
            .border_l_1()
            .border_color(rgb(BORDER))
            .bg(rgb(BG_SIDEBAR))
            .child(
                div()
                    .id("agent-pane-resize")
                    .absolute()
                    .left_0()
                    .top_0()
                    .bottom_0()
                    .w(px(6.))
                    .cursor_col_resize()
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(|this, _: &gpui::MouseDownEvent, _, cx| {
                            this.agent.resizing = true;
                            cx.notify();
                        }),
                    ),
            );

        // Header: agent name, new thread, close.
        let title = self
            .agent
            .thread
            .as_ref()
            .map(|thread| {
                if thread.running {
                    format!("{} Thread · working", thread.agent_name)
                } else {
                    format!("{} Thread", thread.agent_name)
                }
            })
            .unwrap_or_else(|| "New Thread".into());
        panel = panel.child(
            div()
                .flex_none()
                .px_3()
                .py_2()
                .flex()
                .items_center()
                .justify_between()
                .border_b_1()
                .border_color(rgb(BORDER))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .when_some(
                            self.agent
                                .thread
                                .as_ref()
                                .map(|thread| thread.agent_icon.clone()),
                            |header, icon| {
                                header
                                    .child(svg().path(icon).size(px(14.)).text_color(rgb(TEXT_DIM)))
                            },
                        )
                        .child(div().text_color(rgb(TEXT)).child(title)),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(
                            Button::new("agent-new-thread")
                                .label("+")
                                .compact()
                                .outline()
                                .dropdown_menu({
                                    // Snapshot the registry for the
                                    // closure: (name, icon, hint, missing).
                                    let rows: Vec<(String, String, Option<String>, bool)> = self
                                        .agent
                                        .agents
                                        .iter()
                                        .map(|agent| {
                                            use zedb_acp::discovery::Availability;
                                            let (hint, missing) = match &agent.availability {
                                                Availability::Ready => (None, false),
                                                Availability::NeedsLogin { hint } => {
                                                    (Some(hint.clone()), false)
                                                }
                                                Availability::Missing { hint } => {
                                                    (Some(hint.clone()), true)
                                                }
                                            };
                                            (
                                                agent.name.clone(),
                                                icon_for(&agent.id).to_string(),
                                                hint,
                                                missing,
                                            )
                                        })
                                        .collect();
                                    move |menu: PopupMenu, _, _| {
                                        // A Zed-style section header: small
                                        // and dim, unmistakably not an item.
                                        let mut menu = menu.menu_element_with_disabled(
                                            Box::new(StartAgentThread { index: usize::MAX }),
                                            true,
                                            |_, _| {
                                                div()
                                                    .text_xs()
                                                    .text_color(rgb(TEXT_DIM))
                                                    .child("External Agents")
                                            },
                                        );
                                        for (index, (name, icon_path, hint, missing)) in
                                            rows.clone().into_iter().enumerate()
                                        {
                                            menu = menu.menu_element_with_disabled(
                                                Box::new(StartAgentThread { index }),
                                                missing,
                                                move |_, _| {
                                                    div()
                                                        .w_full()
                                                        .py_0p5()
                                                        .flex()
                                                        .flex_col()
                                                        .gap_0p5()
                                                        .when(!missing, |row| row.cursor_pointer())
                                                        .child(
                                                            div()
                                                                .flex()
                                                                .items_center()
                                                                .gap_2()
                                                                .child(
                                                                    svg()
                                                                        .path(icon_path.clone())
                                                                        .size(px(19.))
                                                                        .text_color(rgb(TEXT_DIM)),
                                                                )
                                                                .child(name.clone()),
                                                        )
                                                        .when_some(hint.clone(), |row, hint| {
                                                            row.child(
                                                                div()
                                                                    .text_xs()
                                                                    .text_color(rgb(TEXT_DIM))
                                                                    .child(hint),
                                                            )
                                                        })
                                                },
                                            );
                                        }
                                        menu.separator()
                                            .menu("Add More Agents", Box::new(OpenAddAgent))
                                    }
                                }),
                        )
                        .child(
                            div()
                                .id("agent-close")
                                .px_2()
                                .py_1()
                                .rounded(px(3.))
                                .text_color(rgb(TEXT_DIM))
                                .child("x")
                                .hover(|button| button.bg(rgb(0x303640)).cursor_pointer())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.agent.open = false;
                                    cx.notify();
                                })),
                        ),
                ),
        );

        // The Add More Agents form.
        if let Some(form) = &self.agent.add_form {
            let mut card = div()
                .flex_none()
                .p_2()
                .border_b_1()
                .border_color(rgb(BORDER))
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(TEXT_DIM))
                        .child("Add an ACP-speaking agent (name + command line):"),
                )
                .child(
                    div()
                        .rounded(px(3.))
                        .border_1()
                        .border_color(rgb(BORDER))
                        .bg(rgb(BG))
                        .child(
                            Input::new(&form.name)
                                .appearance(false)
                                .bordered(false)
                                .focus_bordered(false)
                                .pl(px(4.)),
                        ),
                )
                .child(
                    div()
                        .rounded(px(3.))
                        .border_1()
                        .border_color(rgb(BORDER))
                        .bg(rgb(BG))
                        .child(
                            Input::new(&form.command)
                                .appearance(false)
                                .bordered(false)
                                .focus_bordered(false)
                                .pl(px(4.)),
                        ),
                );
            if let Some(error) = &form.error {
                card = card.child(div().text_xs().text_color(rgb(DANGER)).child(error.clone()));
            }
            // Existing custom agents, removable.
            for (index, custom) in self.preferences.custom_agents.iter().enumerate() {
                card = card.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .text_xs()
                        .text_color(rgb(TEXT_DIM))
                        .child(format!("{} ({})", custom.name, custom.command))
                        .child(
                            div()
                                .id(("agent-custom-remove", index))
                                .px_2()
                                .rounded(px(3.))
                                .child("remove")
                                .hover(|button| button.bg(rgb(0x303640)).cursor_pointer())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.agent_remove_custom(index, cx);
                                })),
                        ),
                );
            }
            card = card.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .id("agent-add-save")
                            .px_3()
                            .py_1()
                            .rounded(px(3.))
                            .border_1()
                            .border_color(rgb(BORDER))
                            .text_color(rgb(TEXT))
                            .child("Add")
                            .hover(|button| button.bg(rgb(0x303640)).cursor_pointer())
                            .on_click(cx.listener(|this, _, _, cx| this.agent_save_custom(cx))),
                    )
                    .child(
                        div()
                            .id("agent-add-cancel")
                            .px_3()
                            .py_1()
                            .rounded(px(3.))
                            .text_color(rgb(TEXT_DIM))
                            .child("Cancel")
                            .hover(|button| button.bg(rgb(0x303640)).cursor_pointer())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.agent.add_form = None;
                                cx.notify();
                            })),
                    ),
            );
            panel = panel.child(card);
        }

        // Transcript.
        let mut transcript = div()
            .id("agent-transcript")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p_3()
            .flex()
            .flex_col()
            .gap_2();
        if let Some(thread) = self.agent.thread.as_ref() {
            transcript = transcript.track_scroll(&thread.scroll);
            if thread.stick_to_bottom {
                thread.scroll.scroll_to_bottom();
            }
            let scroll = thread.scroll.clone();
            transcript = transcript.on_scroll_wheel(cx.listener(
                move |this, event: &gpui::ScrollWheelEvent, _, cx| {
                    let Some(thread) = this.agent.thread.as_mut() else {
                        return;
                    };
                    let upward = match event.delta {
                        gpui::ScrollDelta::Pixels(delta) => delta.y > gpui::px(0.),
                        gpui::ScrollDelta::Lines(delta) => delta.y > 0.,
                    };
                    if upward {
                        thread.stick_to_bottom = false;
                    } else {
                        // Re-stick when the wheel brings us near the end.
                        let max = scroll.max_offset().height;
                        let position = -scroll.offset().y;
                        if max - position < gpui::px(40.) {
                            thread.stick_to_bottom = true;
                        }
                    }
                    cx.notify();
                },
            ));
        }
        if let Some(thread) = self.agent.thread.as_ref() {
            for (index, entry) in thread.entries.iter().enumerate() {
                transcript = transcript.child(render_entry(index, entry, window, cx));
            }
            if thread.running {
                transcript = transcript.child(
                    div()
                        .text_color(rgb(TEXT_DIM))
                        .text_xs()
                        .child("working..."),
                );
            }
        } else {
            transcript = transcript.child(
                div()
                    .w_full()
                    .py_8()
                    .flex()
                    .justify_center()
                    .text_color(rgb(TEXT_DIM))
                    .child("Start a thread with the + menu above"),
            );
        }
        panel = panel.child(transcript);

        // Status line (auth hints, stop errors).
        if let Some(status) = self
            .agent
            .thread
            .as_ref()
            .and_then(|thread| thread.status.clone())
        {
            panel = panel.child(
                div()
                    .flex_none()
                    .px_3()
                    .py_1()
                    .bg(rgb(BG_STATUS))
                    .border_t_1()
                    .border_color(rgb(BORDER))
                    .text_xs()
                    .text_color(rgb(DANGER))
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .child(status),
            );
        }

        // Composer.
        if let Some(thread) = self.agent.thread.as_ref() {
            let running = thread.running;
            let ready = thread.session_id.is_some();
            panel = panel.child(
                div()
                    .flex_none()
                    .p_2()
                    .border_t_1()
                    .border_color(rgb(BORDER))
                    .flex()
                    .items_end()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .rounded(px(3.))
                            .border_1()
                            .border_color(rgb(BORDER))
                            .bg(rgb(BG))
                            .child(
                                Input::new(&thread.input)
                                    .appearance(false)
                                    .bordered(false)
                                    .focus_bordered(false)
                                    .pl(px(4.)),
                            ),
                    )
                    .map(|composer| {
                        if running {
                            composer.child(
                                div()
                                    .id("agent-cancel")
                                    .px_3()
                                    .py_1()
                                    .rounded(px(3.))
                                    .border_1()
                                    .border_color(rgb(DANGER))
                                    .text_color(rgb(DANGER))
                                    .child("Stop")
                                    .hover(|button| button.bg(rgb(0x303640)).cursor_pointer())
                                    .on_click(cx.listener(|this, _, _, cx| this.agent_cancel(cx))),
                            )
                        } else {
                            composer.child(
                                div()
                                    .id("agent-send")
                                    .group("agent-send")
                                    .size(px(28.))
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(3.))
                                    .border_1()
                                    .border_color(rgb(BORDER))
                                    .child(
                                        svg()
                                            .path("icons/send.svg")
                                            .size(px(14.))
                                            .text_color(rgb(if ready {
                                                TEXT_DIM
                                            } else {
                                                0x454b55
                                            }))
                                            .when(ready, |icon| {
                                                icon.group_hover("agent-send", |icon| {
                                                    icon.text_color(rgb(TEXT))
                                                })
                                            }),
                                    )
                                    .when(ready, |button| {
                                        button
                                            .hover(|button| {
                                                button.bg(rgb(0x303640)).cursor_pointer()
                                            })
                                            .tooltip(|window, cx| {
                                                gpui_component::tooltip::Tooltip::new(
                                                    "Send (enter)",
                                                )
                                                .build(window, cx)
                                            })
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.agent_send(window, cx)
                                            }))
                                    }),
                            )
                        }
                    }),
            );
        }

        panel
    }
}

/// Strip ANSI escape sequences and control characters, and clamp the
/// length: adapters log freely (codex-acp dumps whole model configs in
/// one colored line) and the status line is one line, not a firehose.
fn clean_log_line(line: &str) -> String {
    let mut out = String::new();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for n in chars.by_ref() {
                    if n.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        if !c.is_control() {
            out.push(c);
        }
        if out.len() >= 200 {
            out.push_str("...");
            break;
        }
    }
    out.trim().to_string()
}

/// Fenced code blocks in a markdown reply, for insert-into-editor;
/// untagged and sql/clickhouse-tagged fences count.
fn fenced_sql_blocks(text: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut in_fence = false;
    let mut takes_sql = false;
    let mut current = String::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("```") {
            if in_fence {
                if takes_sql && !current.trim().is_empty() {
                    blocks.push(current.trim_end().to_string());
                }
                current.clear();
                in_fence = false;
            } else {
                in_fence = true;
                let language = rest.trim().to_lowercase();
                takes_sql = language.is_empty() || language == "sql" || language == "clickhouse";
            }
        } else if in_fence {
            current.push_str(line);
            current.push('\n');
        }
    }
    blocks
}

fn render_entry(
    index: usize,
    entry: &ThreadEntry,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> gpui::AnyElement {
    match entry {
        ThreadEntry::User(text) => div()
            .p_2()
            .rounded(px(4.))
            .bg(rgb(0x2c3a4d))
            .text_color(rgb(TEXT))
            .child(
                TextView::markdown(("agent-user", index), text.clone(), window, cx)
                    .selectable(true),
            )
            .into_any_element(),
        ThreadEntry::Assistant(text) => {
            if text.is_empty() {
                div().into_any_element()
            } else if text.trim_start().starts_with("Automatic approval review") {
                // Adapter housekeeping (Codex's auto-approval notices),
                // visually separated from the agent's actual reply.
                div()
                    .p_2()
                    .rounded(px(4.))
                    .border_1()
                    .border_color(rgb(0xd7a65f))
                    .text_xs()
                    .text_color(rgb(0xd7a65f))
                    .child(
                        TextView::markdown(("agent-notice", index), text.clone(), window, cx)
                            .selectable(true),
                    )
                    .into_any_element()
            } else {
                let blocks = fenced_sql_blocks(text);
                let mut body = div().text_color(rgb(TEXT)).flex().flex_col().gap_1().child(
                    TextView::markdown(("agent-md", index), text.clone(), window, cx)
                        .selectable(true),
                );
                for (block_index, block) in blocks.into_iter().enumerate() {
                    let label = if block_index == 0 {
                        "insert into editor".to_string()
                    } else {
                        format!("insert block {} into editor", block_index + 1)
                    };
                    body = body.child(
                        div()
                            .id(("agent-insert-sql", index * 16 + block_index))
                            .px_2()
                            .py_0p5()
                            .rounded(px(3.))
                            .border_1()
                            .border_color(rgb(BORDER))
                            .text_xs()
                            .text_color(rgb(TEXT_DIM))
                            .child(label)
                            .hover(|button| button.bg(rgb(0x303640)).cursor_pointer())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_query_tab_with(&block, window, cx);
                            })),
                    );
                }
                body.into_any_element()
            }
        }
        ThreadEntry::Tool { title, status, .. } => div()
            .flex()
            .items_center()
            .gap_1()
            .text_xs()
            .text_color(rgb(TEXT_DIM))
            .child(
                svg()
                    .path("icons/check-chain.svg")
                    .size(px(11.))
                    .text_color(rgb(if status == "completed" {
                        SUCCESS
                    } else {
                        TEXT_DIM
                    })),
            )
            .child(format!("{title} ({status})"))
            .into_any_element(),
        ThreadEntry::Permission {
            title,
            input,
            options,
            answered,
        } => {
            let mut card = div()
                .p_2()
                .rounded(px(4.))
                .border_1()
                .border_color(rgb(0xd7a65f))
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .text_color(rgb(0xd7a65f))
                        .text_xs()
                        .child(format!("Permission: {title}")),
                )
                .when_some(input.clone(), |card, input| {
                    card.child(
                        div()
                            .text_xs()
                            .font_family("Menlo")
                            .text_color(rgb(TEXT_DIM))
                            .child(input),
                    )
                });
            match answered {
                Some(choice) => {
                    card = card.child(
                        div()
                            .text_xs()
                            .text_color(rgb(TEXT_DIM))
                            .child(format!("answered: {choice}")),
                    );
                }
                None => {
                    let mut row = div().flex().items_center().gap_2();
                    for (option_index, option) in options.iter().enumerate() {
                        let option_id = option.option_id.clone();
                        let label = if option.name.is_empty() {
                            option.option_id.clone()
                        } else {
                            option.name.clone()
                        };
                        row = row.child(
                            div()
                                .id(("agent-permission", index * 8 + option_index))
                                .px_2()
                                .py_1()
                                .rounded(px(3.))
                                .border_1()
                                .border_color(rgb(BORDER))
                                .text_color(rgb(TEXT))
                                .text_xs()
                                .child(label)
                                .hover(|button| button.bg(rgb(0x303640)).cursor_pointer())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.agent_answer_permission(Some(option_id.clone()), cx);
                                })),
                        );
                    }
                    card = card.child(row);
                }
            }
            card.into_any_element()
        }
        ThreadEntry::Info(text) => div()
            .text_xs()
            .text_color(rgb(TEXT_DIM))
            .child(text.clone())
            .into_any_element(),
    }
}
