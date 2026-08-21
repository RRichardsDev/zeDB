//! Window-level tests of the agent bridge's UI-touching tools: every
//! action must be visible or honestly refused (the ACP "no invisible
//! UI changes" clause), and highlights clear on their timer.

use gpui::TestAppContext;

use crate::test_harness;

fn highlight(
    workspace: &mut crate::Workspace,
    control: &str,
    cx: &mut gpui::Context<crate::Workspace>,
) -> (String, bool) {
    workspace.agent_handle_bridge_tool(
        "highlight_control",
        &serde_json::json!({ "control": control }),
        cx,
    )
}

#[gpui::test]
fn highlighting_a_toolbar_control_brings_the_fleet_view_with_it(cx: &mut TestAppContext) {
    let (workspace, cx) = test_harness::workspace(cx);
    workspace.update(cx, |workspace, cx| {
        assert!(!workspace.show_fleet);
        let (reply, is_error) = highlight(workspace, "lock", cx);
        assert!(!is_error, "{reply}");
        assert!(
            workspace.show_fleet,
            "pointing at a fleet control opens the fleet view"
        );
        assert_eq!(workspace.control_highlight.as_deref(), Some("lock"));
    });

    // The flash clears on the simulated clock.
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(5));
    cx.run_until_parked();
    workspace.update(cx, |workspace, _| {
        assert!(workspace.control_highlight.is_none());
    });
}

#[gpui::test]
fn unrendered_rollback_control_is_refused_honestly(cx: &mut TestAppContext) {
    let (workspace, cx) = test_harness::workspace(cx);
    workspace.update(cx, |workspace, cx| {
        // No database selected: the detail panel (where rollback lives)
        // does not exist, so the bridge must refuse, not pretend.
        let (reply, is_error) = highlight(workspace, "rollback", cx);
        assert!(is_error, "an unrendered control must not claim success");
        assert!(reply.contains("no database is selected"), "{reply}");
        assert!(
            workspace.control_highlight.is_none(),
            "no highlight may be set on nothing"
        );
        assert!(
            !workspace.show_fleet,
            "a refused call must not change views either"
        );

        // With a database selected, the same call succeeds.
        workspace.fleet.selected = Some("tenant_01".into());
        let (reply, is_error) = highlight(workspace, "rollback", cx);
        assert!(!is_error, "{reply}");
        assert!(workspace.show_fleet);
        assert_eq!(workspace.control_highlight.as_deref(), Some("rollback"));
    });
}

#[gpui::test]
fn unknown_controls_are_still_rejected(cx: &mut TestAppContext) {
    let (workspace, cx) = test_harness::workspace(cx);
    workspace.update(cx, |workspace, cx| {
        let (reply, is_error) = highlight(workspace, "self_destruct", cx);
        assert!(is_error);
        assert!(reply.contains("unknown control"), "{reply}");
        assert!(workspace.control_highlight.is_none());
    });
}
