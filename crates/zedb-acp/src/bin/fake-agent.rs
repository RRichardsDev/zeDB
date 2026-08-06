//! A scripted ACP agent for tests: speaks just enough of the protocol
//! to drive the client through its full lifecycle, with scenarios
//! selected by the FAKE_AGENT_SCENARIO environment variable.
//!
//! Scenarios:
//! - happy (default): initialize, session, three message chunks, a tool
//!   call with an update, end_turn.
//! - permission: asks for permission mid-turn and echoes the outcome.
//! - slow: streams chunks forever until session/cancel arrives, then
//!   ends the turn with a cancelled stop reason.
//! - die: exits abruptly right after initialize.

use std::io::{BufRead, Write};
use std::sync::mpsc;

use serde_json::{json, Value};

fn send(value: Value) {
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{value}").expect("write to stdout");
    stdout.flush().expect("flush stdout");
}

fn chunk(session_id: &str, text: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": {
            "sessionId": session_id,
            "update": {
                "sessionUpdate": "agent_message_chunk",
                "content": { "type": "text", "text": text },
            },
        },
    })
}

fn main() {
    let scenario = std::env::var("FAKE_AGENT_SCENARIO").unwrap_or_else(|_| "happy".into());
    let stdin = std::io::stdin();

    // Feed parsed messages through a channel so scenarios can wait for
    // cancellation while doing timed work.
    let (messages_tx, messages) = mpsc::channel::<Value>();
    std::thread::spawn(move || {
        for line in stdin.lock().lines() {
            let Ok(line) = line else { break };
            if let Ok(value) = serde_json::from_str::<Value>(&line) {
                if messages_tx.send(value).is_err() {
                    break;
                }
            }
        }
    });

    let mut permission_counter = 0u64;
    while let Ok(message) = messages.recv() {
        let id = message.get("id").cloned();
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let params = message.get("params").cloned().unwrap_or(Value::Null);

        match method.as_str() {
            "initialize" => {
                send(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "protocolVersion": 1,
                        "agentCapabilities": { "loadSession": false },
                        "authMethods": [],
                    },
                }));
                if scenario == "die" {
                    std::process::exit(1);
                }
            }
            "session/new" => {
                send(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": { "sessionId": "sess-1" },
                }));
            }
            "session/prompt" => {
                let session_id = params
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .unwrap_or("sess-1")
                    .to_string();
                match scenario.as_str() {
                    "permission" => {
                        permission_counter += 1;
                        let request_id = 10_000 + permission_counter;
                        send(json!({
                            "jsonrpc": "2.0",
                            "id": request_id,
                            "method": "session/request_permission",
                            "params": {
                                "sessionId": session_id,
                                "toolCall": { "title": "write file", "toolCallId": "tc-1" },
                                "options": [
                                    { "optionId": "allow", "name": "Allow", "kind": "allow_once" },
                                    { "optionId": "deny", "name": "Deny", "kind": "reject_once" },
                                ],
                            },
                        }));
                        // Wait for our permission response, then finish.
                        let outcome = loop {
                            let Ok(reply) = messages.recv() else {
                                std::process::exit(0);
                            };
                            if reply.get("id").and_then(Value::as_u64) == Some(request_id) {
                                break reply
                                    .get("result")
                                    .and_then(|r| r.get("outcome"))
                                    .and_then(|o| o.get("optionId"))
                                    .and_then(Value::as_str)
                                    .unwrap_or("cancelled")
                                    .to_string();
                            }
                        };
                        send(chunk(&session_id, &format!("outcome:{outcome}")));
                        send(json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": { "stopReason": "end_turn" },
                        }));
                    }
                    "slow" => loop {
                        send(chunk(&session_id, "tick "));
                        std::thread::sleep(std::time::Duration::from_millis(50));
                        match messages.try_recv() {
                            Ok(reply)
                                if reply.get("method").and_then(Value::as_str)
                                    == Some("session/cancel") =>
                            {
                                send(json!({
                                    "jsonrpc": "2.0",
                                    "id": id,
                                    "result": { "stopReason": "cancelled" },
                                }));
                                break;
                            }
                            Ok(_) => {}
                            Err(mpsc::TryRecvError::Empty) => {}
                            Err(mpsc::TryRecvError::Disconnected) => std::process::exit(0),
                        }
                    },
                    _ => {
                        send(chunk(&session_id, "Hello "));
                        send(chunk(&session_id, "from "));
                        send(chunk(&session_id, "fake-agent."));
                        send(json!({
                            "jsonrpc": "2.0",
                            "method": "session/update",
                            "params": {
                                "sessionId": session_id,
                                "update": {
                                    "sessionUpdate": "tool_call",
                                    "toolCallId": "tc-1",
                                    "title": "read zedb.toml",
                                    "status": "in_progress",
                                },
                            },
                        }));
                        send(json!({
                            "jsonrpc": "2.0",
                            "method": "session/update",
                            "params": {
                                "sessionId": session_id,
                                "update": {
                                    "sessionUpdate": "tool_call_update",
                                    "toolCallId": "tc-1",
                                    "status": "completed",
                                },
                            },
                        }));
                        send(json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": { "stopReason": "end_turn" },
                        }));
                    }
                }
            }
            _ => {
                if let Some(id) = id {
                    send(json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": { "code": -32601, "message": format!("unsupported: {method}") },
                    }));
                }
            }
        }
    }
}
