use super::view::clean_log_line;
use super::*;

impl Workspace {
    pub(crate) fn agent_toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.agent.open = !self.agent.open;
        if self.agent.open {
            // The agent pane and the history/saved drawer both dock right;
            // opening one closes the other so they never fight for space.
            self.history.open = false;
            self.agent_refresh_registry();
            self.agent_focus_composer(window, cx);
        }
        cx.notify();
    }

    /// Hand an error to the last-used agent: open the pane, reuse the
    /// live thread or start the remembered agent, and send the visible
    /// message automatically once a session is ready; `hidden` context
    /// (the failing tab and SQL) rides the prompt invisibly.
    pub(crate) fn agent_ask_about(
        &mut self,
        visible: String,
        hidden: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.agent.open {
            self.agent_toggle(window, cx);
        }
        let ready = self
            .agent
            .thread
            .as_ref()
            .is_some_and(|thread| thread.session_id.is_some() && !thread.running);
        if ready {
            self.agent_send_message(visible, Some(hidden), window, cx);
        } else {
            self.agent.pending_ask = Some((visible, hidden));
            if self.agent.thread.is_none() {
                self.agent_start_last_thread(window, cx);
            }
        }
        cx.notify();
    }

    /// Land keyboard focus in the composer when there is one.
    pub(crate) fn agent_focus_composer(&self, window: &mut Window, cx: &mut Context<Self>) {
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
    /// cmd-n in the open agent pane: a new thread with the last-used
    /// agent, falling back to the picker when there is no history.
    pub(crate) fn agent_start_last_thread(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.agent.agents.is_empty() {
            self.agent_refresh_registry();
        }
        let remembered = self
            .preferences
            .last_agent
            .as_deref()
            .and_then(|name| {
                self.agent
                    .agents
                    .iter()
                    .position(|agent| agent.name == name)
            })
            .or_else(|| (self.agent.agents.len() == 1).then_some(0));
        match remembered {
            Some(index) => self.agent_start_thread(index, window, cx),
            None => {
                self.agent.picker_open = true;
                cx.notify();
            }
        }
    }

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
        self.preferences.last_agent = Some(agent.name.clone());
        let _ = zedb_core::save_preferences(&self.preferences);
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
        self.agent.restored = None;
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
                let mut session_ready = false;
                match flattened {
                    Ok(session_id) => {
                        agent_log(
                            "session_started",
                            serde_json::json!({ "session_id": session_id }),
                        );
                        thread.session_id = Some(session_id);
                        thread.status = None;
                        session_ready = true;
                    }
                    Err(error) => {
                        agent_log("session_failed", serde_json::json!({ "error": error }));
                        thread.status = Some(format!(
                            "could not start a session: {error}; if this is an auth \
                             problem, log in with the agent's own CLI first"
                        ));
                    }
                }
                if session_ready && this.agent.pending_ask.is_some() {
                    // The queued error-bar ask sends on the next
                    // render, where a Window is in hand.
                    this.agent.pending_effects.push(AgentEffect::SendPendingAsk);
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

    pub(crate) fn agent_save_custom(&mut self, cx: &mut Context<Self>) {
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

    pub(crate) fn agent_remove_custom(&mut self, index: usize, cx: &mut Context<Self>) {
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
    pub(crate) fn agent_watch_repo(&mut self, generation: u64, cx: &mut Context<Self>) {
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
    pub(crate) fn agent_ensure_bridge(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<std::path::PathBuf> {
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
    pub(crate) fn agent_handle_bridge_tool(
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
                AgentEffect::SendPendingAsk => {
                    if let Some((visible, hidden)) = self.agent.pending_ask.take() {
                        self.agent_send_message(visible, Some(hidden), window, cx);
                    }
                }
                AgentEffect::OpenQueryView => {
                    self.open_query_editor_for_agent(window, cx);
                }
                AgentEffect::ProposeQuery { sql } => {
                    // An error-bar ask remembers where the failure
                    // lives; the fix replaces it in place. Otherwise a
                    // proposed query opens its own tab as before.
                    if !self.agent_apply_fix(&sql, window, cx) {
                        self.open_query_tab_with(&sql, window, cx);
                    }
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
    pub(crate) fn agent_mcp_server_config(
        &self,
        bridge_socket: Option<std::path::PathBuf>,
    ) -> Vec<zedb_acp::McpServerConfig> {
        let repo = self
            .fleet
            .repo
            .as_ref()
            .map(|repo| repo.root.display().to_string());
        let connection = self.connection.connected.as_ref().map(|connected| {
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
        if let Some(cache) = &self.schema.cache {
            config.insert(
                "schema_cache".into(),
                cache.snapshot_path().display().to_string().into(),
            );
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
    pub(crate) fn agent_apply_event_for(
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
        let Some(thread) = self.agent.thread.as_ref() else {
            return;
        };
        let text = thread.input.read(cx).value().trim().to_string();
        if text.is_empty() {
            return;
        }
        let input = thread.input.clone();
        if self.agent_send_message(text, None, window, cx) {
            input.update(cx, |input, cx| input.set_value("", window, cx));
        }
    }

    /// Send a turn: `visible` is what the transcript shows; `hidden`
    /// rides the prompt invisibly (like the primer and ambient
    /// context). Returns false when no session is ready.
    pub(crate) fn agent_send_message(
        &mut self,
        visible: String,
        hidden: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let _ = &window;
        let Some(thread) = self.agent.thread.as_mut() else {
            return false;
        };
        if thread.running {
            return false;
        }
        let Some(session_id) = thread.session_id.clone() else {
            return false;
        };
        let text = visible;
        let include_context = thread.include_context;
        thread.entries.push(ThreadEntry::User(text.clone()));
        if include_context {
            thread
                .entries
                .push(ThreadEntry::Info("screen context attached".into()));
        }
        if hidden.is_some() {
            thread
                .entries
                .push(ThreadEntry::Info("failed query attached".into()));
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
        if let Some(hidden) = &hidden {
            full_text.push_str(hidden);
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
                persist_transcript(thread);
                cx.notify();
            })
            .ok();
        })
        .detach();
        true
    }

    /// Replace the failed statement an error-bar ask came from with
    /// the agent's proposed fix, in its own tab. False when the
    /// target is gone (tab closed, text edited away): the caller
    /// falls back to a fresh tab.
    pub(crate) fn agent_apply_fix(
        &mut self,
        sql: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some((tab_id, failed_sql)) = self.agent_fix_target.take() else {
            return false;
        };
        let Some(index) = self.query.tabs.iter().position(|tab| tab.id == tab_id) else {
            return false;
        };
        let editor = self.query.tabs[index].editor.clone();
        let replaced = editor.update(cx, |editor, cx| {
            let text = editor.value().to_string();
            let Some(start) = text.find(&failed_sql) else {
                return false;
            };
            let fix = sql.trim().trim_end_matches(';');
            let updated = format!(
                "{}{}{}",
                &text[..start],
                fix,
                &text[start + failed_sql.len()..]
            );
            editor.set_value(updated, window, cx);
            true
        });
        if !replaced {
            return false;
        }
        self.query.active_tab = index;
        self.show_query_editor = true;
        self.show_fleet = false;
        self.show_ops = false;
        self.notice = Some("Agent fix applied to the failed statement".into());
        self.notice_warning = false;
        self.notice_flash_id += 1;
        cx.notify();
        true
    }

    /// A snapshot of what the user is looking at; deictic questions
    /// resolve against it and the agent digs further through the zedb
    /// MCP tools. Attached visibly, never behind the user's back.
    pub(crate) fn agent_ambient_context(&self) -> String {
        let mut lines: Vec<String> = Vec::new();
        let screen = if self.show_fleet {
            "fleet view (databases x migrations matrix)"
        } else if self.show_query_editor {
            "query editor"
        } else {
            "connections and schema view"
        };
        lines.push(format!("screen: {screen}"));
        if let Some(connected) = &self.connection.connected {
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

    pub(crate) fn agent_answer_permission(
        &mut self,
        option_id: Option<String>,
        cx: &mut Context<Self>,
    ) {
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
}
