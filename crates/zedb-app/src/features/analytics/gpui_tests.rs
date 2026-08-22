//! Window-level tests of the query-analytics surface: view
//! exclusivity, fetch failure landing honestly, drill-in state
//! clearing on window changes, and a full render with injected data.

use gpui::TestAppContext;
use zedb_ch::analytics::{AnalyticsWindow, FingerprintRun, QueryFingerprint, QueryTestimony};

use crate::test_harness;

fn fingerprint(hash: &str) -> QueryFingerprint {
    QueryFingerprint {
        hash: hash.into(),
        sample: "SELECT count() FROM events WHERE kind = ?".into(),
        runs: 12,
        errors: 1,
        p50_ms: 4.0,
        p95_ms: 60.0,
        p99_ms: 180.0,
        total_ms: 900,
        max_memory: 64 * 1024 * 1024,
        read_rows: 5_000_000,
        read_bytes: 995 * 1024 * 1024,
        users: 2,
        last_seen: "2026-08-22 10:00:00".into(),
    }
}

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
        assert!(workspace.analytics.fingerprints.is_empty());
        assert!(workspace.analytics.fetched_at.is_some());
    });
}

#[gpui::test]
fn window_change_clears_the_drill_in(cx: &mut TestAppContext) {
    let (workspace, cx) = test_harness::workspace(cx);
    workspace.update(cx, |workspace, cx| {
        workspace.connection.connected = Some(test_harness::connected_cluster("dev"));
        workspace.show_analytics = true;
        workspace.analytics.fingerprints = vec![fingerprint("111"), fingerprint("222")];
        workspace.analytics.selected = Some("111".into());
        workspace.analytics.runs = vec![FingerprintRun {
            query_id: "q-1".into(),
            at: "2026-08-22 10:00:00".into(),
            duration_ms: 5,
            memory: 1024,
            read_rows: 10,
            exception: String::new(),
            host: String::new(),
        }];
        workspace.analytics.testimony = Some((
            "q-1".into(),
            QueryTestimony {
                query: "SELECT 1".into(),
                duration_ms: 5,
                memory: 1024,
                read_rows: 10,
                read_bytes: 100,
                result_rows: 1,
                exception: String::new(),
                events: vec![("SelectedRows".into(), 10)],
            },
        ));

        workspace.analytics_set_window(1, cx);
        assert_eq!(workspace.analytics.window, AnalyticsWindow::LastHour);
        assert!(workspace.analytics.selected.is_none());
        assert!(workspace.analytics.runs.is_empty());
        assert!(workspace.analytics.testimony.is_none());
        assert!(workspace.analytics.loading, "a fresh fetch started");

        // Same window again is a no-op, not a refetch storm.
        workspace.analytics.loading = false;
        workspace.analytics_set_window(1, cx);
        assert!(!workspace.analytics.loading);
    });
}

#[gpui::test]
fn panel_renders_with_data_and_open_detail(cx: &mut TestAppContext) {
    let (workspace, cx) = test_harness::workspace(cx);
    workspace.update(cx, |workspace, cx| {
        workspace.connection.connected = Some(test_harness::connected_cluster("dev"));
        workspace.show_analytics = true;
        workspace.analytics.fingerprints = vec![fingerprint("111"), fingerprint("222")];
        workspace.analytics.selected = Some("111".into());
        workspace.analytics.runs = vec![FingerprintRun {
            query_id: "q-1".into(),
            at: "2026-08-22 10:00:00".into(),
            duration_ms: 1500,
            memory: 4 * 1024 * 1024,
            read_rows: 100,
            exception: "boom".into(),
            host: "node-1".into(),
        }];
        workspace.analytics.testimony = Some((
            "q-1".into(),
            QueryTestimony {
                query: "SELECT count() FROM events".into(),
                duration_ms: 1500,
                memory: 4 * 1024 * 1024,
                read_rows: 100,
                read_bytes: 4096,
                result_rows: 1,
                exception: "boom".into(),
                events: vec![
                    ("SelectedRows".into(), 100),
                    ("RealTimeMicroseconds".into(), 42),
                ],
            },
        ));
        cx.notify();
    });
    // The render must survive a full frame with the detail pane open;
    // a panic here is the test failure.
    cx.run_until_parked();
    workspace.update(cx, |workspace, _| {
        assert_eq!(workspace.analytics.fingerprints.len(), 2);
    });
}
