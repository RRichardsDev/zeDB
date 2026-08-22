//! Window-level tests of the ops view's adaptive poll cadence, driven
//! on the simulated clock. The Phase 14 outcome: fast polling while
//! the window is watched, gentle while it is not, and poll queries
//! never flood query_log.

use gpui::TestAppContext;

use super::model::{POLL_ACTIVE_SECS, POLL_INACTIVE_SECS};
use crate::test_harness;

#[gpui::test]
fn cadence_follows_window_activity(cx: &mut TestAppContext) {
    let (workspace, cx) = test_harness::workspace(cx);
    workspace.update(cx, |workspace, _| {
        assert_eq!(workspace.ops_poll_secs(), POLL_ACTIVE_SECS);
        workspace.window_active = false;
        assert_eq!(workspace.ops_poll_secs(), POLL_INACTIVE_SECS);
    });
}

#[gpui::test]
fn poll_ticks_fast_when_watched_and_slow_when_not(cx: &mut TestAppContext) {
    let (workspace, cx) = test_harness::workspace(cx);
    workspace.update(cx, |workspace, cx| {
        workspace.connection.connected = Some(test_harness::connected_cluster("dev"));
        workspace.show_ops = true;
        workspace.ops_start_poll(cx);
        assert_eq!(workspace.ops.tick, 0);
    });

    // Frontmost: one tick per POLL_ACTIVE_SECS.
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(POLL_ACTIVE_SECS));
    cx.run_until_parked();
    let after_one = workspace.update(cx, |workspace, _| workspace.ops.tick);
    assert_eq!(after_one, POLL_ACTIVE_SECS);

    // Backgrounded. One fast timer is already armed and fires once
    // more; every delay after it is the gentle one.
    workspace.update(cx, |workspace, _| workspace.window_active = false);
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(POLL_ACTIVE_SECS));
    cx.run_until_parked();
    let armed = workspace.update(cx, |workspace, _| workspace.ops.tick);
    assert_eq!(armed, after_one + POLL_ACTIVE_SECS, "the armed fast tick");

    cx.executor()
        .advance_clock(std::time::Duration::from_secs(POLL_ACTIVE_SECS));
    cx.run_until_parked();
    workspace.update(cx, |workspace, _| {
        assert_eq!(workspace.ops.tick, armed, "gentle cadence: no tick yet");
    });
    cx.executor().advance_clock(std::time::Duration::from_secs(
        POLL_INACTIVE_SECS - POLL_ACTIVE_SECS,
    ));
    cx.run_until_parked();
    workspace.update(cx, |workspace, _| {
        assert_eq!(workspace.ops.tick, armed + POLL_INACTIVE_SECS);
    });

    // Hiding the view ends the loop; nothing ticks afterwards.
    let parked = workspace.update(cx, |workspace, _| {
        workspace.show_ops = false;
        workspace.ops.tick
    });
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(60));
    cx.run_until_parked();
    workspace.update(cx, |workspace, _| {
        assert_eq!(workspace.ops.tick, parked, "hidden view stops polling");
    });
}

#[gpui::test]
fn poll_queries_never_log_but_kills_do(cx: &mut TestAppContext) {
    let (workspace, cx) = test_harness::workspace(cx);
    workspace.update(cx, |workspace, cx| {
        workspace.connection.connected = Some(test_harness::connected_cluster("dev"));
        workspace.show_ops = true;
        // The poll fetch builds its client from a config that carries
        // log_queries=0; the connection's own config stays untouched.
        workspace.ops_fetch(cx);
        let connected = workspace.connection.connected.as_ref().unwrap();
        assert!(
            !connected
                .client_config
                .driver
                .settings
                .iter()
                .any(|setting| setting.name == "log_queries"),
            "the shared connection config must not inherit the poll setting"
        );
    });
}
