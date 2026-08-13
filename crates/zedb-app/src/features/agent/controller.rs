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
}
