//! The agent pane (docs/PHASE-3.1.md M1): AI threads with installed
//! coding agents over ACP. zeDB renders the conversation and answers
//! permission requests; the agent brings its own auth and tools.
//!
//! M1 scope: one thread at a time, a built-in agent list (discovery is
//! M2), streamed markdown via gpui-component's TextView, compact tool
//! lines, inline permission cards, cancel. Sessions start in the open
//! migration repo's checkout when there is one.

use std::sync::Arc;

use gpui::{div, prelude::*, px, rgb, svg, Action, Context, Entity, Window};
use gpui_component::button::Button;
use gpui_component::input::{Input, InputState};
use gpui_component::menu::{DropdownMenu, PopupMenu};
use gpui_component::text::TextView;
use tokio::sync::oneshot;
use zedb_acp::{AcpError, AgentConnection, AgentEvent, PermissionOption, PermissionOutcome};

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
    pub connection: Arc<AgentConnection>,
    pub session_id: Option<String>,
    pub entries: Vec<ThreadEntry>,
    pub input: Entity<InputState>,
    pub running: bool,
    pub status: Option<String>,
    pub pending_permission: Option<oneshot::Sender<PermissionOutcome>>,
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
    /// The Add More Agents form, when open.
    pub add_form: Option<AddAgentForm>,
}

pub struct AddAgentForm {
    pub name: Entity<InputState>,
    pub command: Entity<InputState>,
    pub error: Option<String>,
}

impl AgentPaneState {
    pub fn new() -> Self {
        Self {
            open: false,
            width: 420.0,
            resizing: false,
            thread: None,
            picker_open: false,
            next_generation: 0,
            agents: Vec::new(),
            connections: std::collections::HashMap::new(),
            add_form: None,
        }
    }
}

impl Workspace {
    pub(crate) fn agent_toggle(&mut self, cx: &mut Context<Self>) {
        self.agent.open = !self.agent.open;
        if self.agent.open {
            self.agent_refresh_registry();
        }
        cx.notify();
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
        self.agent.thread = Some(ThreadState {
            agent_name: name.to_string(),
            agent_icon: icon.clone(),
            include_context: true,
            cache_key: cache_key.clone(),
            connection: connection.clone(),
            session_id: None,
            entries: Vec::new(),
            input,
            running: false,
            status: Some("starting...".into()),
            pending_permission: None,
            generation,
        });
        self.agent.picker_open = false;
        cx.notify();

        // Handshake: initialize, then open the session in the repo,
        // registering the zedb MCP server (this same executable in a
        // hidden serve mode) so the agent gets the fleet and query
        // tools. Credentials travel via a 0600 file the server deletes
        // on read, never argv or env.
        let mcp_servers = self.agent_mcp_server_config();
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
                        thread.session_id = Some(session_id);
                        thread.status = None;
                    }
                    Err(error) => {
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

    /// The zedb MCP server registration for a new session, when there
    /// is anything to serve (an open repo, a connection, or both).
    fn agent_mcp_server_config(&self) -> Vec<zedb_acp::McpServerConfig> {
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
        if repo.is_none() && connection.is_none() {
            return Vec::new();
        }
        let mut config = serde_json::Map::new();
        if let Some(repo) = repo {
            config.insert("repo".into(), repo.into());
        }
        if let Some((url, user, password)) = connection {
            config.insert("url".into(), url.into());
            config.insert("user".into(), user.into());
            config.insert("password".into(), password.into());
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
        let Some(thread) = self.agent.thread.as_mut() else {
            return true;
        };
        if thread.cache_key != cache_key {
            return true;
        }
        match event {
            AgentEvent::MessageChunk { text } => {
                if let Some(ThreadEntry::Assistant(existing)) = thread.entries.last_mut() {
                    existing.push_str(&text);
                } else {
                    thread.entries.push(ThreadEntry::Assistant(text));
                }
            }
            AgentEvent::ThoughtChunk { .. } => {}
            AgentEvent::ToolCall {
                id, title, status, ..
            } => {
                thread.entries.push(ThreadEntry::Tool { id, title, status });
            }
            AgentEvent::ToolCallUpdate { id, status, .. } => {
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
                // One at a time: a newer request supersedes an
                // unanswered one, which resolves as cancelled.
                let title = tool_call
                    .get("title")
                    .and_then(|title| title.as_str())
                    .unwrap_or("the agent asks for permission")
                    .to_string();
                thread.entries.push(ThreadEntry::Permission {
                    title,
                    options,
                    answered: None,
                });
                thread.pending_permission = Some(responder);
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

        let full_text = if include_context {
            format!("{}\n\n{text}", self.agent_ambient_context())
        } else {
            text
        };
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
                        if result.stop_reason != "end_turn" {
                            thread
                                .entries
                                .push(ThreadEntry::Info(format!("[{}]", result.stop_reason)));
                        }
                    }
                    Err(error) => {
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
            "[zeDB screen context, attached by the app; the zedb MCP tools go deeper]\n{}",
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
        let Some(responder) = thread.pending_permission.take() else {
            return;
        };
        let outcome = match &option_id {
            Some(option_id) => PermissionOutcome::Selected {
                option_id: option_id.clone(),
            },
            None => PermissionOutcome::Cancelled,
        };
        let _ = responder.send(outcome);
        for entry in thread.entries.iter_mut().rev() {
            if let ThreadEntry::Permission { answered, .. } = entry {
                if answered.is_none() {
                    *answered = Some(option_id.unwrap_or_else(|| "cancelled".into()));
                }
                break;
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
            .map(|thread| format!("{} Thread", thread.agent_name))
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
            .child(text.clone())
            .into_any_element(),
        ThreadEntry::Assistant(text) => {
            if text.is_empty() {
                div().into_any_element()
            } else {
                div()
                    .text_color(rgb(TEXT))
                    .child(TextView::markdown(
                        ("agent-md", index),
                        text.clone(),
                        window,
                        cx,
                    ))
                    .into_any_element()
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
                );
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
