//! Window-level tests of query tab management and the export dialog:
//! the per-frame visibility invariants, action-dispatch wiring, and the
//! export overlay's open/cancel lifecycle.

use gpui::{Focusable as _, TestAppContext};

use crate::test_harness;
use crate::CloseQueryTab;

#[gpui::test]
fn add_and_close_query_tabs(cx: &mut TestAppContext) {
    let (workspace, cx) = test_harness::workspace(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_query_editor(cx);
        workspace.add_query_tab(window, cx);
        assert_eq!(workspace.query.tabs.len(), 2);
        assert_eq!(workspace.query.active_tab, 1);
        assert_eq!(workspace.query.tabs[1].name, "Tab 2");

        let id = workspace.query.tabs[1].id;
        workspace.close_query_tab(id, cx);
        assert_eq!(workspace.query.tabs.len(), 1);
        assert_eq!(workspace.query.active_tab, 0);
    });
}

#[gpui::test]
fn closing_the_last_tab_leaves_the_query_view(cx: &mut TestAppContext) {
    let (workspace, cx) = test_harness::workspace(cx);
    workspace.update(cx, |workspace, cx| {
        workspace.open_query_editor(cx);
        let id = workspace.query.tabs[0].id;
        workspace.close_query_tab(id, cx);
        assert!(workspace.query.tabs.is_empty());
        assert!(
            !workspace.show_query_editor,
            "closing the last tab is a way out of the query view"
        );
    });
    // The render invariant must respect that exit: a redraw must not
    // conjure a scratch tab behind the cluster overview.
    cx.run_until_parked();
    workspace.update(cx, |workspace, _| {
        assert!(workspace.query.tabs.is_empty());
    });
}

#[gpui::test]
fn close_others_keeps_the_kept_tab(cx: &mut TestAppContext) {
    let (workspace, cx) = test_harness::workspace(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_query_editor(cx);
        workspace.add_query_tab(window, cx);
        workspace.add_query_tab(window, cx);
        assert_eq!(workspace.query.tabs.len(), 3);

        let keep = workspace.query.tabs[1].id;
        workspace.close_other_query_tabs(keep, cx);
        assert_eq!(workspace.query.tabs.len(), 1);
        assert_eq!(workspace.query.tabs[0].id, keep);
        assert_eq!(workspace.query.active_tab, 0);
    });
}

/// The action goes through the window's dispatch tree to the handlers
/// the workspace registers in render, proving that wiring end to end.
#[gpui::test]
fn close_tab_action_dispatches_to_the_workspace(cx: &mut TestAppContext) {
    let (workspace, cx) = test_harness::workspace(cx);
    let id = workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_query_editor(cx);
        workspace.add_query_tab(window, cx);
        // Actions dispatch from the focused node; land focus on the
        // active tab's editor, as running a query would.
        let editor = workspace.query.tabs[1].editor.clone();
        window.focus(&editor.read(cx).focus_handle(cx));
        workspace.query.tabs[1].id
    });
    cx.run_until_parked();
    cx.dispatch_action(CloseQueryTab { tab_id: id });
    workspace.update(cx, |workspace, _| {
        assert_eq!(workspace.query.tabs.len(), 1);
    });
}

#[gpui::test]
fn export_needs_a_displayed_result(cx: &mut TestAppContext) {
    let (workspace, cx) = test_harness::workspace(cx);
    workspace.update(cx, |workspace, cx| {
        workspace.export_open(cx);
        assert!(workspace.export.is_none());
        assert!(
            workspace
                .notice
                .as_deref()
                .unwrap_or_default()
                .contains("Run a query first"),
            "notice: {:?}",
            workspace.notice
        );
    });
}

#[gpui::test]
fn export_opens_with_a_csv_default_and_cancels_clean(cx: &mut TestAppContext) {
    let (workspace, cx) = test_harness::workspace(cx);
    workspace.update(cx, |workspace, cx| {
        workspace.query.tabs[0].displayed_statement = Some("select 1".to_string());
        workspace.export_open(cx);
        let export = workspace.export.as_ref().expect("export dialog open");
        assert_eq!(export.statement, "select 1");
        assert!(!export.running);
        let path = export.path_input.read(cx).text();
        assert!(path.ends_with(".csv"), "default path: {path}");

        workspace.export_cancel(cx);
        assert!(workspace.export.is_none());
    });
}
