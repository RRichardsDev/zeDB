use super::*;

impl Workspace {
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
    ///
    /// Config travels in the server's environment, not argv (argv is
    /// world-readable; env is same-user-only on macOS) and not a file:
    /// agent runtimes respawn MCP servers at will, and the old
    /// delete-on-read credentials file killed every respawn with "No
    /// such file or directory", silently costing the session its zedb
    /// tools.
    pub(crate) fn agent_mcp_server_config(
        &self,
        bridge_socket: Option<std::path::PathBuf>,
    ) -> Vec<zedb_acp::McpServerConfig> {
        let variable = |name: &str, value: String| zedb_acp::EnvVariable {
            name: name.to_string(),
            value,
        };
        let mut env = Vec::new();
        if let Some(repo) = &self.fleet.repo {
            env.push(variable("ZEDB_MCP_REPO", repo.root.display().to_string()));
        }
        if let Some(connected) = &self.connection.connected {
            env.push(variable(
                "ZEDB_MCP_URL",
                connected.client_config.url.clone(),
            ));
            env.push(variable(
                "ZEDB_MCP_USER",
                connected.client_config.user.clone(),
            ));
            env.push(variable(
                "ZEDB_MCP_PASSWORD",
                connected.client_config.password.clone().unwrap_or_default(),
            ));
        }
        if let Some(socket) = bridge_socket {
            env.push(variable(
                "ZEDB_MCP_APP_SOCKET",
                socket.display().to_string(),
            ));
        }
        if let Some(cache) = &self.schema.cache {
            env.push(variable(
                "ZEDB_MCP_SCHEMA_CACHE",
                cache.snapshot_path().display().to_string(),
            ));
        }
        let Ok(exe) = std::env::current_exe() else {
            return Vec::new();
        };
        vec![zedb_acp::McpServerConfig {
            name: "zedb".into(),
            command: exe.display().to_string(),
            args: vec!["zedb-mcp-serve".into()],
            env,
        }]
    }
}
