//! Window-level tests of the fleet safety ladder, on a real headless
//! gpui window: the same Workspace entity graph, render tree, and
//! notify flow the app runs. The pure ladder rules are unit-tested in
//! view.rs; these tests prove the wiring around them.

use gpui::{Focusable as _, TestAppContext};

use super::view::{FleetAction, FleetRow};
use crate::test_harness;

/// The execute path was actually entered: still running, or already
/// finished with a result. Actions against the harness's dead endpoint
/// fail in microseconds on a real tokio thread, so `action_running`
/// alone races the completion callback.
fn action_started(workspace: &crate::Workspace) -> bool {
    workspace.fleet.action_running || workspace.fleet.action_result.is_some()
}

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
            action_started(workspace),
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
        assert!(action_started(workspace));
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
        assert!(action_started(workspace));
    });
}

/// Mouse-driven, through rendered bounds: Cancel clicks away a
/// pending action; the Confirm button is inert while the ladder is
/// unsatisfied (its click handler is only attached when confirmable)
/// and a real click runs the action once the phrase matches.
#[gpui::test]
fn modal_buttons_click_through_their_rendered_bounds(cx: &mut TestAppContext) {
    let (workspace, cx) = test_harness::workspace(cx);
    let (_repo_dir, repo) = test_harness::migration_repo();

    let request = |workspace: &mut crate::Workspace, cx: &mut gpui::Context<crate::Workspace>| {
        workspace.fleet_request_action(FleetAction::UpgradeDatabase("analytics".into()), cx);
    };
    workspace.update(cx, |workspace, cx| {
        // No saved connection: production tier, phrase required.
        workspace.connection.connected = Some(test_harness::connected_cluster("prod"));
        workspace.fleet.repo = Some(repo);
        workspace.fleet.write_unlocked = true;
        workspace.show_fleet = true;
        request(workspace, cx);
    });

    // Cancel through its rendered bounds clears the pending action.
    test_harness::click(cx, "fleet-modal-cancel");
    workspace.update(cx, |workspace, cx| {
        assert!(workspace.fleet.pending_action.is_none());
        request(workspace, cx);
    });

    // Unsatisfied ladder: the click lands on Confirm and does nothing.
    test_harness::click(cx, "fleet-modal-confirm");
    workspace.update(cx, |workspace, cx| {
        assert!(!action_started(workspace));
        assert!(workspace.fleet.pending_action.is_some());
        workspace
            .fleet
            .confirm_input
            .clone()
            .update(cx, |input, cx| input.set_text("analytics", cx));
    });

    // With the phrase typed, the same click starts the action.
    test_harness::click(cx, "fleet-modal-confirm");
    workspace.update(cx, |workspace, _| {
        assert!(action_started(workspace));
    });
}

/// End to end against a real ClickHouse: zedb-ch's EphemeralServer (the
/// lifecycle-check fixture) running a trust-verified binary from the
/// pin cache. The confirmed upgrade must actually apply the fixture
/// migration. Skips when no binary is cached, so nothing downloads
/// during a normal test run.
#[gpui::test]
fn confirmed_upgrade_applies_migrations_to_a_real_server(cx: &mut TestAppContext) {
    use zedb_ch::test_support::{e2e_binary, http_query};

    let Some(binary) = e2e_binary() else {
        eprintln!(
            "skipping: no trusted cached ClickHouse binary \
             (run a fleet verify, or set ZEDB_E2E_DOWNLOAD=1)"
        );
        return;
    };
    let server = zedb_ch::ephemeral::EphemeralServer::start(&binary).expect("ephemeral server");
    http_query(&server, "CREATE DATABASE e2e_fleet");

    let (workspace, cx) = test_harness::workspace(cx);
    let (_repo_dir, repo) = test_harness::migration_repo_with(&[(0, Some("clean"))]);
    workspace.update(cx, |workspace, cx| {
        workspace
            .connection
            .connections
            .push(test_harness::saved_connection(
                "local-e2e",
                zedb_core::EnvTier::Staging,
                false,
            ));
        let mut connected = test_harness::connected_cluster("local-e2e");
        connected.active_endpoint = server.http_url.clone();
        connected.client_config.url = server.http_url.clone();
        workspace.connection.connected = Some(connected);
        workspace.fleet.repo = Some(repo);
        // The upgrade walks the pending list from the loaded matrix, as
        // the app has it after a refresh; seed the row it would show.
        workspace.fleet.rows = vec![FleetRow {
            database: "e2e_fleet".into(),
            head: None,
            pending: vec![0],
            customised: Vec::new(),
            failed: Vec::new(),
            excluded: None,
        }];
        workspace.fleet.write_unlocked = true;
        workspace.fleet_request_action(FleetAction::UpgradeDatabase("e2e_fleet".into()), cx);
        workspace.fleet_execute_action(cx);
        assert!(workspace.fleet.action_running);
    });

    // The runner works on the real tokio runtime; wait wall-clock.
    let result = test_harness::wait_for(cx, std::time::Duration::from_secs(60), |cx| {
        workspace.update(cx, |workspace, _| workspace.fleet.action_result.clone())
    });
    assert!(result.is_ok(), "upgrade failed: {result:?}");
    assert_eq!(
        http_query(&server, "EXISTS TABLE e2e_fleet.fixture_00000").trim(),
        "1",
        "the fixture migration's table must exist on the server"
    );
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
