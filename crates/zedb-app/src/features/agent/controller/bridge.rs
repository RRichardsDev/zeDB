use super::*;

const MAX_BRIDGE_FRAME_BYTES: usize = 2 * 1024 * 1024;
const BRIDGE_QUEUE_CAPACITY: usize = 64;
const BRIDGE_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const BRIDGE_APP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

fn bridge_capability() -> Option<String> {
    use std::fmt::Write as _;
    use std::io::Read as _;
    let mut bytes = [0u8; 32];
    std::fs::File::open("/dev/urandom")
        .ok()?
        .read_exact(&mut bytes)
        .ok()?;
    let mut token = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut token, "{byte:02x}").ok()?;
    }
    Some(token)
}

async fn read_bridge_line<R>(reader: &mut R) -> std::io::Result<Option<Vec<u8>>>
where
    R: tokio::io::AsyncBufRead + Unpin,
{
    use tokio::io::AsyncBufReadExt as _;
    let mut line = Vec::new();
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return Ok((!line.is_empty()).then_some(line));
        }
        let consumed = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |index| index + 1);
        if line.len().saturating_add(consumed) > MAX_BRIDGE_FRAME_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "app bridge frame exceeds 2 MiB",
            ));
        }
        let ended = available.get(consumed.saturating_sub(1)) == Some(&b'\n');
        line.extend_from_slice(&available[..consumed]);
        reader.consume(consumed);
        if ended {
            return Ok(Some(line));
        }
    }
}

impl Workspace {
    /// Bind the app-tool bridge socket once; returns its path.
    pub(crate) fn agent_ensure_bridge(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<std::path::PathBuf> {
        if let Some(path) = self.agent.bridge_socket.clone() {
            let token = bridge_capability()?;
            let token_state = self.agent.bridge_token_state.as_ref()?;
            *token_state.lock().ok()? = token.clone();
            self.agent.bridge_token = Some(token);
            return Some(path);
        }
        let dir = dirs::data_local_dir()?.join("zedb").join("mcp");
        std::fs::create_dir_all(&dir).ok()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).ok()?;
        }
        let path = dir.join(format!("app-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let token = bridge_capability()?;
        let _runtime = rt::tokio().enter();
        let listener = tokio::net::UnixListener::bind(&path).ok()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).ok()?;
        }
        let (requests_tx, mut requests_rx) = tokio::sync::mpsc::channel(BRIDGE_QUEUE_CAPACITY);
        let active_connections =
            std::sync::Arc::new(tokio::sync::Semaphore::new(BRIDGE_QUEUE_CAPACITY));
        let token_state = std::sync::Arc::new(std::sync::Mutex::new(token.clone()));
        let listener_token_state = token_state.clone();
        rt::tokio().spawn(async move {
            use tokio::io::{AsyncWriteExt, BufReader};
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let Ok(active_connection) = active_connections.clone().try_acquire_owned() else {
                    continue;
                };
                let requests_tx = requests_tx.clone();
                let expected_token_state = listener_token_state.clone();
                tokio::spawn(async move {
                    let _active_connection = active_connection;
                    let (read_half, mut write_half) = stream.into_split();
                    let Ok(Ok(Some(line))) = tokio::time::timeout(
                        BRIDGE_READ_TIMEOUT,
                        read_bridge_line(&mut BufReader::new(read_half)),
                    )
                    .await
                    else {
                        return;
                    };
                    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&line) else {
                        return;
                    };
                    let valid_token = expected_token_state.lock().is_ok_and(|expected| {
                        value.get("token").and_then(|value| value.as_str())
                            == Some(expected.as_str())
                    });
                    if !valid_token {
                        return;
                    }
                    let (respond_tx, respond_rx) = oneshot::channel();
                    let request = BridgeRequest {
                        token: value
                            .get("token")
                            .and_then(|token| token.as_str())
                            .unwrap_or_default()
                            .to_string(),
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
                    if requests_tx.try_send(request).is_err() {
                        return;
                    }
                    let (text, is_error) =
                        match tokio::time::timeout(BRIDGE_APP_TIMEOUT, respond_rx).await {
                            Ok(Ok(reply)) => reply,
                            Ok(Err(_)) => ("app closed".to_string(), true),
                            Err(_) => ("app bridge reply deadline exceeded".to_string(), true),
                        };
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
                            token,
                            tool,
                            arguments,
                            respond,
                        } = request;
                        if this.agent.bridge_token.as_deref() != Some(token.as_str()) {
                            let _ = respond.send(("stale app bridge capability".into(), true));
                            return;
                        }
                        if tool == "mcp_call" {
                            this.agent_handle_mcp_call(arguments, respond);
                        } else {
                            let outcome = this.agent_handle_bridge_tool(&tool, &arguments, cx);
                            let _ = respond.send(outcome);
                        }
                    })
                    .is_ok();
                if !live {
                    break;
                }
            }
        })
        .detach();
        self.agent.bridge_socket = Some(path.clone());
        self.agent.bridge_token = Some(token);
        self.agent.bridge_token_state = Some(token_state);
        Some(path)
    }

    /// Execute connection-dependent MCP tools inside the app process. The
    /// agent-spawned MCP child receives only this bounded read capability, not
    /// the underlying ClickHouse credential.
    fn agent_handle_mcp_call(
        &self,
        arguments: serde_json::Value,
        respond: oneshot::Sender<(String, bool)>,
    ) {
        let Some(name) = arguments
            .get("name")
            .and_then(|name| name.as_str())
            .map(str::to_string)
        else {
            let _ = respond.send(("MCP tool name required".into(), true));
            return;
        };
        const CONNECTION_TOOLS: [&str; 7] = [
            "fleet_status",
            "dry_run",
            "drift",
            "list_databases",
            "list_tables",
            "describe",
            "run_query",
        ];
        if !CONNECTION_TOOLS.contains(&name.as_str()) {
            let _ = respond.send((format!("tool is not bridgeable: {name}"), true));
            return;
        }
        let tool_arguments = arguments
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        let repo_root = self.fleet.repo.as_ref().map(|repo| repo.root.clone());
        let config = self.connection.connected.as_ref().map(|connected| {
            let mut config = connected.client_config.clone();
            config.read_only = true;
            config
        });
        rt::tokio().spawn(async move {
            let repo =
                repo_root.and_then(|root| zedb_core::repo::MigrationRepo::open_root(&root).ok());
            let server = zedb_ch::mcp::McpServer::new(repo, config, Default::default());
            let request = serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": { "name": name, "arguments": tool_arguments },
            });
            let Some(reply) = server.handle(request).await else {
                let _ = respond.send(("MCP tool returned no reply".into(), true));
                return;
            };
            if let Some(error) = reply
                .get("error")
                .and_then(|error| error.get("message"))
                .and_then(|message| message.as_str())
            {
                let _ = respond.send((error.to_string(), true));
                return;
            }
            let result = reply.get("result").cloned().unwrap_or_default();
            let text = result
                .get("content")
                .and_then(|content| content.as_array())
                .and_then(|content| content.first())
                .and_then(|content| content.get("text"))
                .and_then(|text| text.as_str())
                .unwrap_or("(no reply)")
                .to_string();
            let is_error = result
                .get("isError")
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
            let _ = respond.send((text, is_error));
        });
    }

    /// The `cloud_context` reply: the active connection's Cloud
    /// control-plane picture from the app's live state, freshness
    /// stated, nothing fetched inline (the bridge is synchronous; the
    /// caller kicks the background refreshes).
    pub(crate) fn agent_cloud_context_report(&self) -> String {
        let Some(connected) = self.connection.connected.as_ref() else {
            return "no active connection; the user connects in zeDB first".to_string();
        };
        let name = connected.name.clone();
        let Some(cloud) = self
            .connection
            .connections
            .iter()
            .find(|connection| connection.name == name)
            .and_then(|connection| connection.cloud.clone())
        else {
            return format!(
                "the active connection ({name}) is not linked to a ClickHouse Cloud \
                 service; no control-plane context applies"
            );
        };
        let mut lines = vec![format!(
            "Active connection: {name} (ClickHouse Cloud, organization {})",
            cloud.org_id
        )];
        // Warehouse members with the watch map overlaid: the same
        // truth the sidebar and dashboard show.
        let services = &self.connection.cloud.services;
        let warehouse = services
            .iter()
            .find(|(_, service)| service.id == cloud.service_id)
            .and_then(|(_, service)| service.warehouse_id.clone());
        let members: Vec<&crate::clickhouse_cloud::CloudService> = services
            .iter()
            .filter(|(_, service)| match &warehouse {
                Some(warehouse) => service.warehouse_id.as_deref() == Some(warehouse.as_str()),
                None => service.id == cloud.service_id,
            })
            .map(|(_, service)| service)
            .collect();
        if members.is_empty() {
            lines.push(
                "Warehouse services: unknown (the app has not loaded the org's service \
                 list yet; it refreshes on window focus)"
                    .to_string(),
            );
        } else {
            lines.push(
                "Warehouse services (as of the app's last Cloud refresh; refreshes on \
                 focus and Cloud actions):"
                    .to_string(),
            );
            for service in members {
                let state = self
                    .connection
                    .cloud
                    .states
                    .get(&service.id)
                    .cloned()
                    .unwrap_or_else(|| service.state.clone());
                let mut facts = vec![state];
                if service.is_primary {
                    facts.push("primary".to_string());
                }
                if let Some(tier) = &service.tier {
                    facts.push(tier.clone());
                }
                if let (Some(replicas), Some(min), Some(max)) = (
                    service.num_replicas,
                    service.min_total_memory_gb,
                    service.max_total_memory_gb,
                ) {
                    facts.push(format!("{replicas} replicas, {min}-{max} GiB"));
                }
                if let Some(idle) = service.idle_timeout_minutes {
                    facts.push(format!("idles after {idle} min"));
                }
                lines.push(format!("- {}: {}", service.name, facts.join("; ")));
            }
        }
        let cost = &self.connection.cost_status;
        match cost.fetched_at {
            Some(at) if cost.connection.as_deref() == Some(name.as_str()) => {
                lines.push(format!(
                    "Cost (this warehouse, fetched {} min ago): today so far {} CHC; \
                     yesterday {} CHC; median of the last {} complete days {} CHC/day; \
                     high burn: {}",
                    at.elapsed().as_secs() / 60,
                    crate::format_chc(cost.today),
                    crate::format_chc(cost.yesterday),
                    cost.days,
                    crate::format_chc(cost.median),
                    if cost.high_burn() { "YES" } else { "no" },
                ));
            }
            _ => lines.push(
                "Cost: not loaded yet (fetched on connect and hourly; ask again shortly)"
                    .to_string(),
            ),
        }
        lines.push(
            "Billing note: run_query's bytes-to-read cap (10 GiB by default) is enforced \
             server-side; on Cloud, scanned bytes are paid compute, so treat that cap as a \
             per-query billing ceiling."
                .to_string(),
        );
        lines.push(
            "Service state changes (wake/stop) are deliberately not available here; the \
             user drives them from the connection page."
                .to_string(),
        );
        lines.join("\n")
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
            // Internal plumbing for the MCP server: the currently open
            // repo root, empty when none. Not narrated; nothing the
            // user sees changes.
            "repo_root" => (
                self.fleet
                    .repo
                    .as_ref()
                    .map(|repo| repo.root.display().to_string())
                    .unwrap_or_default(),
                false,
            ),
            // Read-only Cloud control-plane context (Phase 13 slice 3):
            // answered from the app's live state per call, with the
            // data's freshness stated, and refreshes kicked so a
            // follow-up call sees newer figures. Nothing the user sees
            // changes, so it is not narrated (like repo_root).
            "cloud_context" => {
                let report = self.agent_cloud_context_report();
                self.cost_status_refresh(false, cx);
                (report, false)
            }
            "highlight_control" => {
                const CONTROLS: [&str; 7] = [
                    "lock",
                    "upgrade_all",
                    "rollback",
                    "new_migration",
                    "regen",
                    "check_chain",
                    "verify_all",
                ];
                let control = arguments
                    .get("control")
                    .and_then(|control| control.as_str())
                    .unwrap_or_default()
                    .to_string();
                if !CONTROLS.contains(&control.as_str()) {
                    return (format!("unknown control: {control}"), true);
                }
                self.control_highlight = Some(control.clone());
                self.control_highlight_generation += 1;
                let generation = self.control_highlight_generation;
                cx.spawn(async move |this, cx| {
                    gpui::Timer::after(std::time::Duration::from_secs(4)).await;
                    this.update(cx, |this, cx| {
                        if this.control_highlight_generation == generation {
                            this.control_highlight = None;
                            cx.notify();
                        }
                    })
                    .ok();
                })
                .detach();
                narrate(self, format!("agent: pointed at the {control} control"), cx);
                (format!("{control} is highlighted for a few seconds"), false)
            }
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
    /// Non-secret config and the private bridge capability travel in
    /// the server's environment, not argv. Agent runtimes may respawn
    /// MCP servers, so this registration must remain reusable. Live
    /// database credentials never enter the ACP session configuration.
    pub(crate) fn agent_mcp_server_config(
        &self,
        bridge_socket: Option<std::path::PathBuf>,
        bridge_token: Option<String>,
    ) -> Vec<zedb_acp::McpServerConfig> {
        let variable = |name: &str, value: String| zedb_acp::EnvVariable {
            name: name.to_string(),
            value,
        };
        let mut env = Vec::new();
        if let Some(repo) = &self.fleet.repo {
            env.push(variable("ZEDB_MCP_REPO", repo.root.display().to_string()));
        }
        if let Some(socket) = bridge_socket {
            env.push(variable(
                "ZEDB_MCP_APP_SOCKET",
                socket.display().to_string(),
            ));
        }
        if let Some(token) = bridge_token {
            env.push(variable("ZEDB_MCP_APP_TOKEN", token));
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
