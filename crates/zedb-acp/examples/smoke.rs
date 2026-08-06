//! Manual smoke test against a real installed agent:
//!
//!   cargo run -p zedb-acp --example smoke -- "your prompt" <command> [args...]
//!
//! e.g. cargo run -p zedb-acp --example smoke -- "say hi" npx @zed-industries/claude-code-acp

use zedb_acp::{AgentConnection, AgentEvent};

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let prompt = args
        .next()
        .expect("usage: smoke <prompt> <command> [args...]");
    let command = args
        .next()
        .expect("usage: smoke <prompt> <command> [args...]");
    let rest: Vec<String> = args.collect();

    let cwd = std::env::current_dir().expect("cwd");
    let mut agent = AgentConnection::spawn(&command, &rest, &[], Some(&cwd)).expect("spawn agent");
    let mut events = agent.take_events();

    let pump = tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            match event {
                AgentEvent::MessageChunk { text } => print!("{text}"),
                AgentEvent::ThoughtChunk { .. } => {}
                AgentEvent::ToolCall { title, status, .. } => {
                    eprintln!("\n[tool] {title} ({status})")
                }
                AgentEvent::ToolCallUpdate { id, status, .. } => {
                    eprintln!("[tool] {id} -> {status}")
                }
                AgentEvent::PermissionRequest {
                    options, responder, ..
                } => {
                    // Smoke test: always pick the first option offered.
                    let first = options
                        .first()
                        .map(|option| option.option_id.clone())
                        .unwrap_or_default();
                    eprintln!("[permission] auto-selecting {first}");
                    let _ =
                        responder.send(zedb_acp::PermissionOutcome::Selected { option_id: first });
                }
                AgentEvent::Stderr { line } => eprintln!("[agent stderr] {line}"),
                AgentEvent::Plan { .. } => eprintln!("[plan updated]"),
                AgentEvent::Other { kind, .. } => eprintln!("[update: {kind}]"),
                AgentEvent::Closed { reason } => {
                    eprintln!("[closed] {reason}");
                    break;
                }
            }
            use std::io::Write;
            let _ = std::io::stdout().flush();
        }
    });

    let init = agent.initialize().await.expect("initialize");
    eprintln!("[initialized: protocol v{}]", init.protocol_version);
    let session = agent
        .new_session(&cwd, Vec::new())
        .await
        .expect("new session");
    eprintln!("[session {}]", session.session_id);
    let result = agent
        .prompt(&session.session_id, &prompt)
        .await
        .expect("prompt");
    println!("\n[stop: {}]", result.stop_reason);
    agent.shutdown().await;
    pump.abort();
}
