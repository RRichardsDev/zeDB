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
