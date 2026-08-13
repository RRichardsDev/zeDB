use super::view::clean_log_line;
use super::*;

impl Workspace {
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
}
