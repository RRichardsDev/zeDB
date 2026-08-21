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
        if let Some(session_id) = event.update_session_id() {
            let Some(thread) = self.agent.thread.as_ref() else {
                return true;
            };
            if thread.cache_key != cache_key {
                return true;
            }
            if thread.session_id.as_deref() != Some(session_id) {
                agent_log(
                    "update_wrong_session",
                    serde_json::json!({ "session_id_bytes": session_id.len() }),
                );
                return true;
            }
        }
        match &event {
            AgentEvent::MessageChunk { text, .. } => {
                agent_log("chunk", serde_json::json!({ "bytes": text.len() }));
            }
            AgentEvent::ThoughtChunk { .. } => {}
            AgentEvent::ToolCall {
                id, title, status, ..
            } => {
                agent_log(
                    "tool",
                    serde_json::json!({
                        "id": id,
                        "title_bytes": title.len(),
                        "status": status,
                    }),
                );
            }
            AgentEvent::ToolCallUpdate { id, status, .. } => {
                agent_log(
                    "tool_update",
                    serde_json::json!({ "id": id, "status": status }),
                );
            }
            AgentEvent::Plan { .. } => agent_log("plan", serde_json::json!({})),
            AgentEvent::Other { kind, .. } => {
                agent_log("other_update", serde_json::json!({ "kind": kind }));
            }
            AgentEvent::PermissionRequest { tool_call, .. } => {
                let title_bytes = tool_call
                    .get("title")
                    .and_then(|title| title.as_str())
                    .map(str::len)
                    .unwrap_or(0);
                agent_log(
                    "permission_request",
                    serde_json::json!({
                        "tool_call_id": tool_call.get("toolCallId"),
                        "title_bytes": title_bytes,
                    }),
                );
            }
            AgentEvent::Stderr { line } => {
                agent_log("stderr", serde_json::json!({ "bytes": line.len() }));
            }
            AgentEvent::Closed { reason } => {
                agent_log(
                    "closed",
                    serde_json::json!({ "reason_bytes": reason.len() }),
                );
            }
        }
        let Some(thread) = self.agent.thread.as_mut() else {
            return true;
        };
        if thread.cache_key != cache_key {
            return true;
        }
        if thread.entries.len() > MAX_LIVE_TRANSCRIPT_ENTRIES {
            thread.entries.drain(..100);
            thread
                .entries
                .insert(0, ThreadEntry::Info("(older messages trimmed)".into()));
        }
        match event {
            AgentEvent::MessageChunk { text, .. } => {
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
                session_id,
                tool_call,
                options,
                responder,
                ..
            } => {
                if thread.pending_permissions.len() >= MAX_PENDING_PERMISSIONS {
                    agent_log("permission_queue_full", serde_json::json!({}));
                    let _ = responder.send(PermissionOutcome::Cancelled);
                    return true;
                }
                if thread.session_id.as_deref() != Some(session_id.as_str()) {
                    agent_log(
                        "permission_wrong_session",
                        serde_json::json!({ "session_id_bytes": session_id.len() }),
                    );
                    let _ = responder.send(PermissionOutcome::Cancelled);
                    return true;
                }
                let title = tool_call
                    .get("title")
                    .and_then(|title| title.as_str())
                    .unwrap_or("the agent asks for permission")
                    .to_string();
                let input = tool_call.get("rawInput").and_then(|raw| {
                    if raw.is_null() || raw == &serde_json::json!({}) {
                        None
                    } else {
                        let mut text = raw.to_string();
                        if text.len() > 240 {
                            // Byte-indexed truncate panics off a UTF-8
                            // boundary and the JSON is agent-supplied.
                            let mut end = 240;
                            while !text.is_char_boundary(end) {
                                end -= 1;
                            }
                            text.truncate(end);
                            text.push_str("...");
                        }
                        Some(text)
                    }
                });
                let option_ids = options
                    .iter()
                    .map(|option| option.option_id.clone())
                    .collect();
                // Cards and queued responders pair by id: answers land on
                // the exact card clicked, never "the oldest one".
                let request_id = thread.next_permission_id;
                thread.next_permission_id += 1;
                thread.entries.push(ThreadEntry::Permission {
                    request_id,
                    title,
                    input,
                    options,
                    answered: None,
                });
                thread.pending_permissions.push_back(PendingPermission {
                    request_id,
                    responder,
                    option_ids,
                });
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
                self.agent.connections.remove(cache_key);
            }
        }
        cx.notify();
        true
    }
}
