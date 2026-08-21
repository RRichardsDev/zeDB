//! The full connection lifecycle against the scripted fake agent,
//! including the ugly exits.

use zedb_acp::{AcpError, AgentConnection, AgentEvent, PermissionOutcome};

fn spawn_fake(scenario: &str) -> AgentConnection {
    AgentConnection::spawn(
        env!("CARGO_BIN_EXE_fake-agent"),
        &[],
        &[("FAKE_AGENT_SCENARIO".into(), scenario.into())],
        None,
    )
    .expect("spawn fake agent")
}

#[tokio::test]
async fn happy_lifecycle_streams_and_ends() {
    let mut agent = spawn_fake("happy");
    let mut events = agent.take_events();

    let init = agent.initialize().await.expect("initialize");
    assert_eq!(init.protocol_version, 1);

    let session = agent
        .new_session(std::path::Path::new("/tmp"), Vec::new())
        .await
        .expect("new session");
    assert_eq!(session.session_id, "sess-1");

    let result = agent
        .prompt(&session.session_id, "hello?")
        .await
        .expect("prompt");
    assert_eq!(result.stop_reason, "end_turn");

    let mut text = String::new();
    let mut tool_calls = 0;
    let mut tool_updates = 0;
    while let Ok(event) =
        tokio::time::timeout(std::time::Duration::from_secs(5), events.recv()).await
    {
        match event.expect("event stream open") {
            AgentEvent::MessageChunk { text: chunk, .. } => text.push_str(&chunk),
            AgentEvent::ToolCall { id, status, .. } => {
                assert_eq!(id, "tc-1");
                assert_eq!(status, "in_progress");
                tool_calls += 1;
            }
            AgentEvent::ToolCallUpdate { id, status, .. } => {
                assert_eq!(id, "tc-1");
                assert_eq!(status, "completed");
                tool_updates += 1;
                break; // the turn's last update in this scenario
            }
            _ => {}
        }
    }
    assert_eq!(text, "Hello from fake-agent.");
    assert_eq!((tool_calls, tool_updates), (1, 1));
    agent.shutdown().await;
}

#[tokio::test]
async fn permission_round_trip() {
    let mut agent = spawn_fake("permission");
    let mut events = agent.take_events();
    agent.initialize().await.expect("initialize");
    let session = agent
        .new_session(std::path::Path::new("/tmp"), Vec::new())
        .await
        .expect("new session");

    let agent = std::sync::Arc::new(agent);
    let session_id = session.session_id.clone();
    let prompt = tokio::spawn({
        let agent = agent.clone();
        let session_id = session_id.clone();
        async move { agent.prompt(&session_id, "do a thing").await }
    });

    let mut answered = false;
    let mut echoed = String::new();
    while let Ok(Some(event)) =
        tokio::time::timeout(std::time::Duration::from_secs(5), events.recv()).await
    {
        match event {
            AgentEvent::PermissionRequest {
                options, responder, ..
            } => {
                assert!(options.iter().any(|option| option.option_id == "allow"));
                responder
                    .send(PermissionOutcome::Selected {
                        option_id: "allow".into(),
                    })
                    .expect("respond");
                answered = true;
            }
            AgentEvent::MessageChunk { text, .. } => {
                echoed.push_str(&text);
                break;
            }
            _ => {}
        }
    }
    let result = prompt.await.expect("join").expect("prompt");
    assert!(answered, "permission request must arrive");
    assert_eq!(echoed, "outcome:allow");
    assert_eq!(result.stop_reason, "end_turn");
    drop(agent); // kill_on_drop reaps the process
}

#[tokio::test]
async fn cancel_stops_a_streaming_turn() {
    let mut agent = spawn_fake("slow");
    let mut events = agent.take_events();
    agent.initialize().await.expect("initialize");
    let session = agent
        .new_session(std::path::Path::new("/tmp"), Vec::new())
        .await
        .expect("new session");

    let agent = std::sync::Arc::new(agent);
    let session_id = session.session_id.clone();
    let prompt = tokio::spawn({
        let agent = agent.clone();
        let session_id = session_id.clone();
        async move { agent.prompt(&session_id, "stream forever").await }
    });

    // Let a couple of chunks arrive, then cancel.
    let mut chunks = 0;
    while let Ok(Some(event)) =
        tokio::time::timeout(std::time::Duration::from_secs(5), events.recv()).await
    {
        if matches!(event, AgentEvent::MessageChunk { .. }) {
            chunks += 1;
            if chunks == 2 {
                agent.cancel(&session_id).expect("cancel");
                break;
            }
        }
    }
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), prompt)
        .await
        .expect("cancel must end the turn")
        .expect("join")
        .expect("prompt");
    assert_eq!(result.stop_reason, "cancelled");
    drop(agent); // kill_on_drop reaps the process
}

#[tokio::test]
async fn agent_death_fails_pending_and_closes() {
    let mut agent = spawn_fake("die");
    let mut events = agent.take_events();
    agent.initialize().await.expect("initialize succeeds first");

    // The process exits right after initialize; the next request must
    // fail rather than hang, and Closed must arrive.
    let error = agent
        .new_session(std::path::Path::new("/tmp"), Vec::new())
        .await
        .expect_err("dead agent cannot open sessions");
    assert!(matches!(
        error,
        zedb_acp::AcpError::Closed | zedb_acp::AcpError::Spawn(_)
    ));

    let mut closed = false;
    while let Ok(Some(event)) =
        tokio::time::timeout(std::time::Duration::from_secs(5), events.recv()).await
    {
        if matches!(event, AgentEvent::Closed { .. }) {
            closed = true;
            break;
        }
    }
    assert!(closed, "Closed event must arrive when the agent dies");
}

#[tokio::test]
async fn oversized_outgoing_prompt_is_rejected_before_send() {
    let agent = spawn_fake("happy");
    agent.initialize().await.expect("initialize");
    let session = agent
        .new_session(std::path::Path::new("/tmp"), Vec::new())
        .await
        .expect("new session");

    let oversized = "x".repeat(2 * 1024 * 1024);
    let error = agent
        .prompt(&session.session_id, &oversized)
        .await
        .expect_err("oversized frame must not enter the writer queue");
    assert!(matches!(error, AcpError::Limit(_)));
    agent.shutdown().await;
}
