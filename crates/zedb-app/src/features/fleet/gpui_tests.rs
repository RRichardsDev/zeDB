//! Window-level tests of the fleet safety ladder, on a real headless
//! gpui window: the same Workspace entity graph, render tree, and
//! notify flow the app runs. The pure ladder rules are unit-tested in
//! view.rs; these tests prove the wiring around them.

use gpui::{Focusable as _, TestAppContext};

use super::view::FleetAction;
use crate::test_harness;

#[gpui::test]
fn workspace_builds_and_renders_headlessly(cx: &mut TestAppContext) {
    let (workspace, cx) = test_harness::workspace(cx);
    workspace.update(cx, |workspace, _| {
        assert_eq!(workspace.query.tabs.len(), 1);
        assert!(workspace.fleet.pending_action.is_none());
        assert!(!workspace.fleet.write_unlocked);
    });
}

#[gpui::test]
fn fleet_action_requires_write_unlock(cx: &mut TestAppContext) {
    let (workspace, cx) = test_harness::workspace(cx);
    workspace.update(cx, |workspace, cx| {
        workspace.fleet_request_action(FleetAction::UpgradeAll, cx);
        assert!(workspace.fleet.pending_action.is_none());
        assert!(
            workspace
                .notice
                .as_deref()
                .unwrap_or_default()
                .contains("Unlock writes"),
            "notice: {:?}",
            workspace.notice
        );
    });
}

/// The full production ladder, driven through the window: request the
/// action, type the wrong phrase into the rendered modal's input, see
/// execution refused; type the right phrase, see it start.
#[gpui::test]
fn production_confirmation_gates_on_typed_phrase(cx: &mut TestAppContext) {
    let (workspace, cx) = test_harness::workspace(cx);
    let (_repo_dir, repo) = test_harness::migration_repo();

    let confirm_input = workspace.update(cx, |workspace, cx| {
        // A connected name absent from saved connections resolves to the
        // Production tier (fail closed), so the phrase is required.
        workspace.connection.connected = Some(test_harness::connected_cluster("prod"));
        workspace.fleet.repo = Some(repo);
        workspace.fleet.write_unlocked = true;
        workspace.show_fleet = true;
        workspace.fleet_request_action(FleetAction::UpgradeDatabase("analytics".into()), cx);
        assert!(workspace.fleet.pending_action.is_some());
        workspace.fleet.confirm_input.clone()
    });

    // Focus the modal's confirmation input and type through the window.
    workspace.update_in(cx, |_, window, cx| {
        window.focus(&confirm_input.focus_handle(cx));
    });
    cx.simulate_input("analytcis");

    workspace.update(cx, |workspace, cx| {
        assert_eq!(workspace.fleet.confirm_input.read(cx).text(), "analytcis");
        workspace.fleet_execute_action(cx);
        assert!(!workspace.fleet.action_running);
        assert!(
            workspace.fleet.pending_action.is_some(),
            "an incomplete confirmation must not clear the pending action"
        );
        assert!(
            workspace
                .notice
                .as_deref()
                .unwrap_or_default()
                .contains("Complete the confirmation"),
            "notice: {:?}",
            workspace.notice
        );
    });

    // A fresh request resets the input; retype the phrase correctly.
    let confirm_input = workspace.update(cx, |workspace, cx| {
        workspace.fleet_request_action(FleetAction::UpgradeDatabase("analytics".into()), cx);
        workspace.fleet.confirm_input.clone()
    });
    workspace.update_in(cx, |_, window, cx| {
        window.focus(&confirm_input.focus_handle(cx));
    });
    cx.simulate_input("analytics");

    workspace.update(cx, |workspace, cx| {
        workspace.fleet_execute_action(cx);
        assert!(
            workspace.fleet.action_running,
            "the matching phrase lets the action start (notice: {:?})",
            workspace.notice
        );
    });
}

/// A structural rollback on a non-production tier needs no phrase, but
/// it does need the explicit structural acknowledgement.
#[gpui::test]
fn structural_rollback_requires_acknowledgement(cx: &mut TestAppContext) {
    let (workspace, cx) = test_harness::workspace(cx);
    let (_repo_dir, repo) = test_harness::migration_repo_with(&[(0, Some("structural"))]);
    workspace.update(cx, |workspace, cx| {
        workspace
            .connection
            .connections
            .push(test_harness::saved_connection(
                "stag",
                zedb_core::EnvTier::Staging,
                false,
            ));
        workspace.connection.connected = Some(test_harness::connected_cluster("stag"));
        workspace.fleet.repo = Some(repo);
        workspace.fleet.write_unlocked = true;
        workspace.fleet_request_action(
            FleetAction::Rollback {
                database: "analytics".into(),
                number: 0,
            },
            cx,
        );

        workspace.fleet_execute_action(cx);
        assert!(
            !workspace.fleet.action_running,
            "unacknowledged structural work must not start"
        );
        assert!(workspace.fleet.pending_action.is_some());

        workspace.fleet.ack_structural = true;
        workspace.fleet_execute_action(cx);
        assert!(workspace.fleet.action_running);
    });
}

/// A migration with no rollback.sql is irreversible at run time: the
/// "irreversible" phrase is demanded even off production.
#[gpui::test]
fn missing_rollback_demands_the_irreversible_phrase(cx: &mut TestAppContext) {
    let (workspace, cx) = test_harness::workspace(cx);
    let (_repo_dir, repo) = test_harness::migration_repo_with(&[(0, None)]);
    workspace.update(cx, |workspace, cx| {
        workspace
            .connection
            .connections
            .push(test_harness::saved_connection(
                "stag",
                zedb_core::EnvTier::Staging,
                false,
            ));
        workspace.connection.connected = Some(test_harness::connected_cluster("stag"));
        workspace.fleet.repo = Some(repo);
        workspace.fleet.write_unlocked = true;
        workspace.fleet_request_action(
            FleetAction::Rollback {
                database: "analytics".into(),
                number: 0,
            },
            cx,
        );

        workspace.fleet_execute_action(cx);
        assert!(!workspace.fleet.action_running);

        workspace
            .fleet
            .confirm_input
            .clone()
            .update(cx, |input, cx| input.set_text("irreversible", cx));
        workspace.fleet_execute_action(cx);
        assert!(workspace.fleet.action_running);
    });
}

#[gpui::test]
fn confirmation_dies_when_the_connection_changes(cx: &mut TestAppContext) {
    let (workspace, cx) = test_harness::workspace(cx);
    let (_repo_dir, repo) = test_harness::migration_repo();

    workspace.update(cx, |workspace, cx| {
        workspace.connection.connected = Some(test_harness::connected_cluster("prod"));
        workspace.fleet.repo = Some(repo);
        workspace.fleet.write_unlocked = true;
        workspace.fleet_request_action(FleetAction::UpgradeDatabase("analytics".into()), cx);
        assert!(workspace.fleet.pending_action.is_some());

        // The cluster the user reviewed is gone; the confirmation must
        // die with it, whatever was typed.
        workspace.connection.connected = Some(test_harness::connected_cluster("staging"));
        workspace.fleet_execute_action(cx);
        assert!(workspace.fleet.pending_action.is_none());
        assert!(!workspace.fleet.action_running);
        assert!(
            workspace
                .notice
                .as_deref()
                .unwrap_or_default()
                .contains("connection or repository changed"),
            "notice: {:?}",
            workspace.notice
        );
    });
}
