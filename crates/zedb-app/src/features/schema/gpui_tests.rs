//! Window-level tests of the in-place apply ladder for schema
//! suggestions: read-only connections divert to a query tab, large
//! tables demand a confirmation whose context must still match, and
//! cancel clears cleanly. The pure rules are unit-tested in model.rs;
//! this proves the wiring.

use gpui::TestAppContext;

use crate::test_harness;

const SQL: &str = "ALTER TABLE analytics.events MODIFY COLUMN id CODEC(ZSTD(3))";

#[gpui::test]
fn read_only_connection_diverts_to_a_query_tab(cx: &mut TestAppContext) {
    let (workspace, cx) = test_harness::workspace(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace
            .connection
            .connections
            .push(test_harness::saved_connection(
                "dev",
                zedb_core::EnvTier::Dev,
                true,
            ));
        let mut connected = test_harness::connected_cluster("dev");
        connected.client_config.read_only = true;
        workspace.connection.connected = Some(connected);

        let tabs_before = workspace.query.tabs.len();
        workspace.request_apply(0, vec![SQL.to_string()], SQL.to_string(), window, cx);
        assert!(
            workspace.schema.pending_apply.is_none(),
            "read-only must never queue an in-place apply"
        );
        assert_eq!(
            workspace.query.tabs.len(),
            tabs_before + 1,
            "the suggestion opens as an editable query tab instead"
        );
    });
}

#[gpui::test]
fn large_table_apply_waits_for_confirmation(cx: &mut TestAppContext) {
    let (workspace, cx) = test_harness::workspace(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace
            .connection
            .connections
            .push(test_harness::saved_connection(
                "dev",
                zedb_core::EnvTier::Dev,
                false,
            ));
        workspace.connection.connected = Some(test_harness::connected_cluster("dev"));
        workspace.schema.selected_object = Some(test_harness::selected_schema_object(
            "analytics",
            "events",
            2_000_000_000,
            window,
            cx,
        ));

        workspace.request_apply(0, vec![SQL.to_string()], SQL.to_string(), window, cx);
        let pending = workspace
            .schema
            .pending_apply
            .as_ref()
            .expect("pending apply");
        assert_eq!(pending.connection, "dev");
        assert_eq!(pending.database, "analytics");
        assert_eq!(pending.object, "events");

        workspace.cancel_apply(cx);
        assert!(workspace.schema.pending_apply.is_none());
    });
}

#[gpui::test]
fn confirmation_dies_when_the_selected_object_changes(cx: &mut TestAppContext) {
    let (workspace, cx) = test_harness::workspace(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace
            .connection
            .connections
            .push(test_harness::saved_connection(
                "dev",
                zedb_core::EnvTier::Dev,
                false,
            ));
        workspace.connection.connected = Some(test_harness::connected_cluster("dev"));
        workspace.schema.selected_object = Some(test_harness::selected_schema_object(
            "analytics",
            "events",
            2_000_000_000,
            window,
            cx,
        ));
        workspace.request_apply(0, vec![SQL.to_string()], SQL.to_string(), window, cx);
        assert!(workspace.schema.pending_apply.is_some());

        // The inspector moved to a different table before the user
        // confirmed; the stale confirmation must not run against it.
        workspace.schema.selected_object = Some(test_harness::selected_schema_object(
            "analytics",
            "other_table",
            2_000_000_000,
            window,
            cx,
        ));
        let tabs_before = workspace.query.tabs.len();
        workspace.confirm_apply(window, cx);
        assert!(workspace.schema.pending_apply.is_none());
        assert_eq!(workspace.query.tabs.len(), tabs_before, "nothing ran");
        assert!(
            workspace
                .notice
                .as_deref()
                .unwrap_or_default()
                .contains("changed"),
            "notice: {:?}",
            workspace.notice
        );
    });
}
