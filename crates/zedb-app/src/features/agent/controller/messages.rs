use super::*;

impl Workspace {
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
            serde_json::json!({
                "session_id": session_id,
                "bytes": full_text.len(),
                "included_context": include_context,
            }),
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
                        agent_log(
                            "turn_error",
                            serde_json::json!({ "error_bytes": error.len() }),
                        );
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
        let Some(thread) = self.agent.thread.as_mut() else {
            return;
        };
        if let Some(session_id) = &thread.session_id {
            let _ = thread.connection.cancel(session_id);
        }
        for pending in thread.pending_permissions.drain(..) {
            let _ = pending.responder.send(PermissionOutcome::Cancelled);
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
        let Some(pending) = thread.pending_permissions.pop_front() else {
            return;
        };
        let outcome = match &option_id {
            Some(option_id) if pending.option_ids.contains(option_id) => {
                PermissionOutcome::Selected {
                    option_id: option_id.clone(),
                }
            }
            Some(_) | None => PermissionOutcome::Cancelled,
        };
        agent_log(
            "permission_answer",
            serde_json::json!({ "selected": option_id.is_some() }),
        );
        let _ = pending.responder.send(outcome);
        // Mark the oldest unanswered card. ACP tool titles are display text,
        // not authority identities, so approvals are never reused by title.
        for entry in thread.entries.iter_mut() {
            if let ThreadEntry::Permission {
                title: _, answered, ..
            } = entry
            {
                if answered.is_none() {
                    let chosen = option_id.clone().unwrap_or_else(|| "cancelled".into());
                    *answered = Some(chosen);
                    break;
                }
            }
        }
        cx.notify();
    }
}
