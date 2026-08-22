//! Window-level tests of the query-analytics surface: view
//! exclusivity, fetch failure landing honestly, grid events driving
//! sort/filter refetches and drill-in, and a full render with the
//! detail pane open.

use gpui::TestAppContext;
use zedb_ch::analytics::{AnalyticsWindow, FingerprintRun, QueryTestimony};

use crate::grid_spike::GridEvent;
use crate::test_harness;

#[gpui::test]
fn analytics_needs_a_connection_and_views_stay_exclusive(cx: &mut TestAppContext) {
    let (workspace, cx) = test_harness::workspace(cx);
    workspace.update(cx, |workspace, cx| {
        workspace.analytics_toggle(cx);
        assert!(!workspace.show_analytics);
        assert!(
            workspace
                .notice
                .as_deref()
                .unwrap_or_default()
                .contains("Connect"),
            "notice: {:?}",
            workspace.notice
        );

        workspace.connection.connected = Some(test_harness::connected_cluster("dev"));
        workspace.show_ops = true;
        workspace.analytics_toggle(cx);
        assert!(workspace.show_analytics);
        assert!(!workspace.show_ops, "analytics closes the ops view");

        workspace.ops_toggle(cx);
        assert!(workspace.show_ops);
        assert!(!workspace.show_analytics, "ops closes analytics back");
    });
}

#[gpui::test]
fn fetch_failure_lands_as_an_error_not_a_spinner(cx: &mut TestAppContext) {
    let (workspace, cx) = test_harness::workspace(cx);
    workspace.update(cx, |workspace, cx| {
        workspace.connection.connected = Some(test_harness::connected_cluster("dev"));
        workspace.analytics_toggle(cx);
        assert!(workspace.analytics.loading);
    });
    // The dead endpoint fails on the real tokio runtime; wait for it.
    test_harness::wait_for(cx, std::time::Duration::from_secs(30), |cx| {
        workspace
            .update(cx, |workspace, _| !workspace.analytics.loading)
            .then_some(())
    });
    workspace.update(cx, |workspace, _| {
        assert!(workspace.analytics.error.is_some(), "the failure is shown");
        assert!(workspace.analytics.rows_meta.is_empty());
        assert!(workspace.analytics.fetched_at.is_some());
    });
}

#[gpui::test]
fn grid_events_drive_sort_filter_and_drill_in(cx: &mut TestAppContext) {
    let (workspace, cx) = test_harness::workspace(cx);
    workspace.update(cx, |workspace, cx| {
        workspace.connection.connected = Some(test_harness::connected_cluster("dev"));
        workspace.show_analytics = true;
        workspace.analytics.rows_meta = vec![
            (
                "111".into(),
                "SELECT count() FROM events WHERE kind = ?".into(),
            ),
            ("222".into(), "INSERT INTO events SELECT ?".into()),
        ];

        // Sort request lands in state and starts a refetch.
        workspace.analytics_grid_event(
            &GridEvent::SortRequested {
                sort: vec![("p95_ms".into(), false)],
            },
            cx,
        );
        assert_eq!(workspace.analytics.sort, vec![("p95_ms".into(), false)]);
        assert!(workspace.analytics.loading);
        workspace.analytics.loading = false;

        // Filter request replaces per column; clearing removes it.
        workspace.analytics_grid_event(
            &GridEvent::FilterRequested {
                column: "shape".into(),
                predicate: Some("shape ILIKE '%tenant%'".into()),
            },
            cx,
        );
        assert_eq!(
            workspace.analytics.filters,
            vec![("shape".into(), "shape ILIKE '%tenant%'".into())]
        );
        workspace.analytics.loading = false;
        workspace.analytics_grid_event(
            &GridEvent::FilterRequested {
                column: "shape".into(),
                predicate: None,
            },
            cx,
        );
        assert!(workspace.analytics.filters.is_empty());
        workspace.analytics.loading = false;

        // Double-clicked row drills in via the row metadata.
        workspace.analytics_grid_event(&GridEvent::RowActivated { row: 1 }, cx);
        assert_eq!(workspace.analytics.selected.as_deref(), Some("222"));
        assert_eq!(
            workspace.analytics.selected_shape,
            "INSERT INTO events SELECT ?"
        );
        assert!(workspace.analytics.runs_loading);

        // An out-of-range row does nothing.
        workspace.analytics_grid_event(&GridEvent::RowActivated { row: 9 }, cx);
        assert_eq!(workspace.analytics.selected.as_deref(), Some("222"));
    });
}

#[gpui::test]
fn header_actions_route_to_the_visible_grid(cx: &mut TestAppContext) {
    let (workspace, cx) = test_harness::workspace(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.connection.connected = Some(test_harness::connected_cluster("dev"));

        // The router follows what is on screen.
        workspace.show_analytics = true;
        assert_eq!(
            workspace.visible_grid(),
            Some(workspace.analytics.grid.clone())
        );
        workspace.show_analytics = false;
        assert_eq!(
            workspace.visible_grid(),
            Some(workspace.query.tabs[0].result_grid.clone())
        );

        // The filter panel opens on the analytics grid, not a hidden
        // query tab (the "filter does nothing" regression).
        workspace.show_analytics = true;
        workspace.analytics.grid.update(cx, |grid, cx| {
            grid.begin_result(
                vec![zedb_core::ColumnMeta {
                    name: "shape".into(),
                    type_name: "String".into(),
                }],
                None,
                cx,
            );
            grid.finish_result(false, cx);
        });
        workspace.analytics_open_column_filter("shape".into(), window, cx);
        let opened = workspace
            .analytics
            .grid
            .update(cx, |grid, cx| grid.close_filter_panel(cx));
        assert!(opened, "the panel opened on the analytics grid");
    });
}

#[gpui::test]
fn scope_selection_dispatches_from_the_menu_overlay(cx: &mut TestAppContext) {
    use gpui::Focusable as _;

    let (workspace, cx) = test_harness::workspace(cx);
    let grid = workspace.update(cx, |workspace, cx| {
        workspace.connection.connected = Some(test_harness::connected_cluster("dev"));
        workspace.show_analytics = true;
        cx.notify();
        workspace.analytics.grid.clone()
    });
    // Menus dispatch actions from an overlay via the focused path;
    // focus the grid, as a user interacting with the view would have.
    cx.run_until_parked();
    workspace.update_in(cx, |_, window, cx| {
        window.focus(&grid.focus_handle(cx));
    });
    cx.dispatch_action(crate::analytics::SetAnalyticsScope {
        cluster: Some("zedb_cluster".into()),
    });
    workspace.update(cx, |workspace, _| {
        assert_eq!(
            workspace.analytics.scope.cluster(),
            Some("zedb_cluster"),
            "the scope selection reached the workspace"
        );
        assert!(
            workspace.analytics.loading || workspace.analytics.error.is_some(),
            "and a refetch started (dead endpoint may already have failed it)"
        );
    });
}

#[gpui::test]
fn window_change_clears_the_drill_in(cx: &mut TestAppContext) {
    let (workspace, cx) = test_harness::workspace(cx);
    workspace.update(cx, |workspace, cx| {
        workspace.connection.connected = Some(test_harness::connected_cluster("dev"));
        workspace.show_analytics = true;
        workspace.analytics.selected = Some("111".into());
        workspace.analytics.selected_shape = "SELECT 1".into();
        workspace.analytics.runs = vec![FingerprintRun {
            query_id: "q-1".into(),
            at: "2026-08-22 10:00:00".into(),
            duration_ms: 5,
            memory: 1024,
            read_rows: 10,
            exception: String::new(),
            host: String::new(),
        }];

        workspace.analytics_set_window(1, cx);
        assert_eq!(workspace.analytics.window, AnalyticsWindow::LastHour);
        assert!(workspace.analytics.selected.is_none());
        assert!(workspace.analytics.runs.is_empty());
        assert!(workspace.analytics.loading, "a fresh fetch started");

        // Same window again is a no-op, not a refetch storm.
        workspace.analytics.loading = false;
        workspace.analytics_set_window(1, cx);
        assert!(!workspace.analytics.loading);
    });
}

#[gpui::test]
fn panel_renders_with_grid_data_and_open_detail(cx: &mut TestAppContext) {
    let (workspace, cx) = test_harness::workspace(cx);
    workspace.update(cx, |workspace, cx| {
        workspace.connection.connected = Some(test_harness::connected_cluster("dev"));
        workspace.show_analytics = true;
        workspace.analytics.rows_meta = vec![("111".into(), "SELECT count()".into())];
        workspace.analytics.grid.update(cx, |grid, cx| {
            grid.begin_result(
                vec![
                    zedb_core::ColumnMeta {
                        name: "shape".into(),
                        type_name: "String".into(),
                    },
                    zedb_core::ColumnMeta {
                        name: "runs".into(),
                        type_name: "UInt64".into(),
                    },
                ],
                None,
                cx,
            );
            grid.append_rows(
                vec![vec![
                    zedb_core::Value::String("SELECT count()".into()),
                    zedb_core::Value::UInt(12),
                ]],
                cx,
            );
            grid.finish_result(false, cx);
        });
        workspace.analytics.selected = Some("111".into());
        workspace.analytics.selected_shape = "SELECT count()".into();
        workspace.analytics.testimony = Some((
            "q-1".into(),
            QueryTestimony {
                query: "SELECT count()".into(),
                duration_ms: 1500,
                memory: 4 * 1024 * 1024,
                read_rows: 100,
                read_bytes: 4096,
                result_rows: 1,
                exception: String::new(),
                events: vec![("SelectedRows".into(), 100)],
            },
        ));
        cx.notify();
    });
    // The render must survive a full frame with the grid populated and
    // the detail pane open; a panic here is the test failure.
    cx.run_until_parked();
    workspace.update(cx, |workspace, cx| {
        assert_eq!(workspace.analytics.grid.read(cx).row_count(), 1);
    });
}
