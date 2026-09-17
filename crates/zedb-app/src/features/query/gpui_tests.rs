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

/// Occurrence highlighting runs inside the editor's paint (the vendor
/// patch calls the provider per frame), so the proof is a full frame
/// with the caret parked on an alias in real SQL: a provider panic or
/// a bad range would fail the render.
#[gpui::test]
fn occurrence_provider_survives_frames_with_the_caret_on_an_alias(cx: &mut TestAppContext) {
    let (workspace, cx) = test_harness::workspace(cx);
    let editor = workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_query_editor(cx);
        let editor = workspace.query.tabs[0].editor.clone();
        // The scratch tab opens with sample SQL; an empty buffer keeps
        // the trigger at the end of the text, as typing a fresh query.
        editor.update(cx, |editor, cx| editor.set_value("", window, cx));
        window.focus(&editor.read(cx).focus_handle(cx));
        editor
    });
    cx.run_until_parked();
    cx.simulate_input("select af.id from events af where af.id > 1");
    cx.run_until_parked();
    // Walk the caret left into the final "af.id" so it sits on an
    // identifier with multiple occurrences; every step renders a frame
    // with the provider active.
    for _ in 0..8 {
        cx.simulate_keystrokes("left");
    }
    cx.run_until_parked();
    workspace.update(cx, |workspace, cx| {
        assert!(workspace.query.tabs[0]
            .editor
            .read(cx)
            .value()
            .contains("events af"));
    });
    // And the pure computation agrees about what would light up.
    let sql = "select af.id from events af where af.id > 1";
    let hits = zedb_ch::schema_intelligence::occurrences_at(sql, sql.rfind("af").unwrap());
    assert_eq!(hits.len(), 3);
    let _ = editor;
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

/// The executing-scope honesty loop: with a cluster selected, the
/// context-menu fix inserts ON CLUSTER into the buffer at the right
/// spot, visibly, and warns instead of guessing when no cluster is
/// picked.
#[gpui::test]
fn add_on_cluster_edits_the_buffer_explicitly(cx: &mut TestAppContext) {
    let (workspace, cx) = test_harness::workspace(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.connection.connected = Some(test_harness::connected_cluster("dev"));
        let editor = workspace.query.tabs[0].editor.clone();
        editor.update(cx, |editor, cx| {
            editor.set_value(
                "SELECT 1;\nCREATE TABLE t (x UInt64) ENGINE = MergeTree ORDER BY x;",
                window,
                cx,
            );
        });

        // No cluster picked: the handler says so and touches nothing.
        workspace.add_on_cluster_at(15, window, cx);
        assert!(
            workspace
                .notice
                .as_deref()
                .unwrap_or_default()
                .contains("Executing on"),
            "notice: {:?}",
            workspace.notice
        );
        assert!(!editor.read(cx).value().contains("ON CLUSTER"));

        // Cluster picked: the clause lands after the table name.
        workspace.set_apply_cluster(Some("zedb_cluster".into()), cx);
        workspace.add_on_cluster_at(15, window, cx);
        let sql = editor.read(cx).value().to_string();
        assert!(
            sql.contains("CREATE TABLE t ON CLUSTER `zedb_cluster` (x UInt64)"),
            "sql: {sql}"
        );
        // The untouched statement stays untouched.
        assert!(sql.starts_with("SELECT 1;"));
    });
}

/// Format SQL says what it needs (a connection, some SQL) instead of
/// silently doing nothing, and a failed round trip leaves the buffer
/// untouched and names the failure.
#[gpui::test]
fn format_sql_names_its_blockers_and_leaves_the_buffer_alone_on_failure(cx: &mut TestAppContext) {
    let (workspace, cx) = test_harness::workspace(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.format_sql(window, cx);
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
        let editor = workspace.query.tabs[0].editor.clone();
        editor.update(cx, |editor, cx| editor.set_value("  \n", window, cx));
        workspace.format_sql(window, cx);
        assert_eq!(workspace.notice.as_deref(), Some("Nothing to format"));

        editor.update(cx, |editor, cx| {
            editor.set_value("select 1 /* keep */", window, cx);
        });
        workspace.format_sql(window, cx);
        assert!(
            workspace
                .notice
                .as_deref()
                .unwrap_or_default()
                .contains("comment inside"),
            "notice: {:?}",
            workspace.notice
        );

        editor.update(cx, |editor, cx| {
            editor.set_value("select a,b from t", window, cx);
        });
        workspace.notice = None;
        workspace.format_sql(window, cx);
    });

    // The dead endpoint fails on the real tokio runtime; wait for it.
    let notice = test_harness::wait_for(cx, std::time::Duration::from_secs(10), |cx| {
        workspace.update(cx, |workspace, _| workspace.notice.clone())
    });
    assert!(notice.contains("Format failed"), "notice: {notice}");
    workspace.update(cx, |workspace, cx| {
        assert_eq!(
            workspace.query.tabs[0].editor.read(cx).value().as_ref(),
            "select a,b from t"
        );
    });
}

/// End to end: the server's formatQuery re-lays the buffer, leading
/// comments survive, a commented statement is left as written, and
/// the status line says both. Opt-in like the fleet e2e test.
#[gpui::test]
fn format_sql_formats_through_a_real_server(cx: &mut TestAppContext) {
    use zedb_ch::test_support::e2e_binary;

    if std::env::var_os("ZEDB_E2E").is_none() && std::env::var_os("ZEDB_E2E_DOWNLOAD").is_none() {
        eprintln!("skipping: end-to-end tier is opt-in (ZEDB_E2E=1, or ZEDB_E2E_DOWNLOAD=1 to allow a verified download)");
        return;
    }
    let Some(binary) = e2e_binary() else {
        eprintln!("skipping: no trusted cached ClickHouse binary");
        return;
    };
    let server = zedb_ch::ephemeral::EphemeralServer::start(&binary).expect("ephemeral server");

    let (workspace, cx) = test_harness::workspace(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        let mut connected = test_harness::connected_cluster("local-e2e");
        connected.active_endpoint = server.http_url.clone();
        connected.client_config.url = server.http_url.clone();
        workspace.connection.connected = Some(connected);
        let editor = workspace.query.tabs[0].editor.clone();
        editor.update(cx, |editor, cx| {
            editor.set_value(
                "-- head\nselect a,b from t where x=1;\nselect 1 /* c */ from t",
                window,
                cx,
            );
        });
        workspace.format_sql(window, cx);
    });

    let notice = test_harness::wait_for(cx, std::time::Duration::from_secs(30), |cx| {
        workspace.update(cx, |workspace, _| workspace.notice.clone())
    });
    assert_eq!(
        notice,
        "Formatted 1 statement; kept 1 statement with a comment inside as written"
    );
    workspace.update(cx, |workspace, cx| {
        assert_eq!(
            workspace.query.tabs[0].editor.read(cx).value().as_ref(),
            "-- head\nSELECT\n    a,\n    b\nFROM t\nWHERE x = 1;\nselect 1 /* c */ from t"
        );
    });

    // Formatting the result again is a no-op that says so: the
    // multi-line text round-trips (it goes up as a literal, since a
    // String query parameter cannot carry a newline).
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.notice = None;
        workspace.format_sql(window, cx);
    });
    let notice = test_harness::wait_for(cx, std::time::Duration::from_secs(30), |cx| {
        workspace.update(cx, |workspace, _| workspace.notice.clone())
    });
    assert_eq!(notice, "Already formatted; nothing to change");
}

/// End to end: running SYSTEM REFRESH VIEW keeps the tab running until
/// the view has actually rebuilt. The statement itself returns in
/// milliseconds, so without the client-side wait the run would be over
/// while the view was still being rebuilt. Opt-in like the other e2e
/// tests.
#[gpui::test]
fn a_view_refresh_runs_until_the_view_has_rebuilt(cx: &mut TestAppContext) {
    use zedb_ch::test_support::{e2e_binary, http_query};

    if std::env::var_os("ZEDB_E2E").is_none() && std::env::var_os("ZEDB_E2E_DOWNLOAD").is_none() {
        eprintln!("skipping: end-to-end tier is opt-in (ZEDB_E2E=1, or ZEDB_E2E_DOWNLOAD=1 to allow a verified download)");
        return;
    }
    let Some(binary) = e2e_binary() else {
        eprintln!("skipping: no trusted cached ClickHouse binary");
        return;
    };
    let server = zedb_ch::ephemeral::EphemeralServer::start(&binary).expect("ephemeral server");
    // A view whose rebuild takes two seconds, refreshed only on demand.
    http_query(&server, "CREATE DATABASE rv");
    http_query(
        &server,
        "CREATE MATERIALIZED VIEW rv.slow REFRESH EVERY 1 YEAR \
         ENGINE = MergeTree ORDER BY tuple() \
         AS SELECT number, sleepEachRow(0.2) AS slept FROM numbers(10) \
         SETTINGS allow_experimental_refreshable_materialized_view = 1",
    );
    // Let the refresh that creation kicks off finish first.
    std::thread::sleep(std::time::Duration::from_secs(3));

    let (workspace, cx) = test_harness::workspace(cx);
    workspace.update_in(cx, |workspace, window, cx| {
        let mut connected = test_harness::connected_cluster("local-e2e");
        connected.active_endpoint = server.http_url.clone();
        connected.client_config.url = server.http_url.clone();
        workspace.connection.connected = Some(connected);
        let editor = workspace.query.tabs[0].editor.clone();
        editor.update(cx, |editor, cx| {
            editor.set_value("SYSTEM REFRESH VIEW rv.slow", window, cx)
        });
        workspace.run_query(window, cx);
    });

    // A second in: the statement is long since answered, but the run is
    // still going because the view is still rebuilding.
    std::thread::sleep(std::time::Duration::from_secs(1));
    cx.run_until_parked();
    workspace.update(cx, |workspace, _| {
        assert!(
            matches!(
                workspace.query.tabs[0].outcome,
                crate::QueryOutcome::Running
            ),
            "the run ended while the view was still rebuilding"
        );
    });

    let outcome = test_harness::wait_for(cx, std::time::Duration::from_secs(60), |cx| {
        workspace.update(cx, |workspace, _| match &workspace.query.tabs[0].outcome {
            crate::QueryOutcome::Running => None,
            crate::QueryOutcome::Error(message) => Some(Err(message.clone())),
            crate::QueryOutcome::StatementError { message, .. } => Some(Err(message.clone())),
            _ => Some(Ok(())),
        })
    });
    if let Err(message) = outcome {
        panic!("the refresh should have finished cleanly: {message}");
    }
}

/// cmd-n opens a new query tab, from whichever view is on screen, and
/// lands on it.
#[gpui::test]
fn cmd_n_opens_a_new_query_tab(cx: &mut TestAppContext) {
    let (workspace, cx) = test_harness::workspace(cx);
    // Start away from the editor: the chord is global.
    workspace.update(cx, |workspace, cx| workspace.toggle_fleet(cx));
    cx.run_until_parked();
    cx.simulate_keystrokes("cmd-n");
    workspace.update(cx, |workspace, _| {
        assert_eq!(workspace.query.tabs.len(), 2, "cmd-n adds a tab");
        assert_eq!(
            workspace.query.active_tab, 1,
            "the new tab is the active one"
        );
        assert!(workspace.show_query_editor, "cmd-n shows the query editor");
        assert!(!workspace.show_fleet);
    });
}

/// The completion popup is as wide as its widest suggestion, whatever
/// the order. It used to measure its rows at zero width and fall back
/// to its minimum, clipping every name and engine it showed.
#[gpui::test]
fn completion_popup_fits_its_widest_suggestion(cx: &mut TestAppContext) {
    const LONG: &str = "conversion_facts_by_activity_daily";
    let short_only = completion_popup_width(cx, &["acts"]);
    let long_only = completion_popup_width(cx, &[LONG]);
    let long_behind_short = completion_popup_width(cx, &["acts", LONG]);
    assert!(
        long_only > short_only,
        "the popup ignores how wide its suggestions are: {long_only:?} for \
         a long name, {short_only:?} for a short one"
    );
    assert!(
        long_behind_short >= long_only,
        "the popup shrank to its first suggestion: {long_behind_short:?} \
         with a short name first, {long_only:?} with the long name alone"
    );
}

/// Type `analytics.` into a fresh query tab whose schema holds exactly
/// `tables`, and report how wide the completion popup paints.
fn completion_popup_width(cx: &mut TestAppContext, tables: &[&str]) -> gpui::Pixels {
    use zedb_ch::schema_cache::{CachedObjectKind, SchemaCache, TableRecord};

    let dir = tempfile::tempdir().expect("temp dir");
    let cache = SchemaCache::open(dir.path().join("schema.json")).expect("open schema cache");
    cache
        .publish_tables(
            tables
                .iter()
                .map(|name| TableRecord {
                    database: "analytics".to_string(),
                    name: (*name).to_string(),
                    engine: "ReplicatedMergeTree".to_string(),
                    kind: CachedObjectKind::Table,
                    total_rows: None,
                    total_bytes: None,
                    comment: String::new(),
                })
                .collect(),
        )
        .expect("publish tables");

    let (workspace, cx) = test_harness::workspace(cx);
    let editor = workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_query_editor(cx);
        workspace
            .schema
            .provider
            .set_context(Some(cache), Some("analytics".to_string()));
        let editor = workspace.query.tabs[0].editor.clone();
        // The scratch tab opens with sample SQL; an empty buffer keeps
        // the trigger at the end of the text, as typing a fresh query.
        editor.update(cx, |editor, cx| editor.set_value("", window, cx));
        window.focus(&editor.read(cx).focus_handle(cx));
        editor
    });
    cx.run_until_parked();
    cx.simulate_input("select count() from analytics.");
    cx.refresh().expect("schedule redraw");
    cx.run_until_parked();

    workspace.update(cx, |_, cx| {
        editor
            .read(cx)
            .completion_menu_bounds(cx)
            .expect("completion popup showing")
            .size
            .width
    })
}

/// The blue matched-prefix highlight covers exactly what the user
/// typed. The popup's query runs from wherever it opened (a whole
/// clause, here), so the highlight is matched against the label rather
/// than taken from that query's length, which used to lag the typing
/// and then swallow the whole row.
#[gpui::test]
fn completion_highlight_matches_the_typed_word(cx: &mut TestAppContext) {
    use gpui_component::input::completion_matched_prefix_len;
    use zedb_ch::schema_cache::{CachedObjectKind, SchemaCache, TableRecord};

    let dir = tempfile::tempdir().expect("temp dir");
    let cache = SchemaCache::open(dir.path().join("schema.json")).expect("open schema cache");
    cache
        .publish_tables(vec![TableRecord {
            database: "RefreshableViews".to_string(),
            name: "AFAS_ActivityFacts".to_string(),
            engine: "ReplacingMergeTree".to_string(),
            kind: CachedObjectKind::Table,
            total_rows: None,
            total_bytes: None,
            comment: String::new(),
        }])
        .expect("publish tables");

    let (workspace, cx) = test_harness::workspace(cx);
    let editor = workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_query_editor(cx);
        workspace
            .schema
            .provider
            .set_context(Some(cache), Some("RefreshableViews".to_string()));
        let editor = workspace.query.tabs[0].editor.clone();
        editor.update(cx, |editor, cx| editor.set_value("", window, cx));
        window.focus(&editor.read(cx).focus_handle(cx));
        editor
    });
    cx.run_until_parked();
    cx.simulate_input("select * from Refreshable");
    cx.run_until_parked();

    let query = workspace
        .update(cx, |_, cx| editor.read(cx).completion_menu_query(cx))
        .expect("completion popup showing");
    assert!(
        query.len() > "Refreshable".len(),
        "this test is only meaningful while the query outruns the typed \
         word, and it is {query:?}"
    );
    assert_eq!(
        completion_matched_prefix_len("RefreshableViews.AFAS_ActivityFacts", &query),
        "Refreshable".len(),
        "the highlight must cover the typed word, no more and no less"
    );
}

/// The matched-prefix rule itself, over the shapes the schema provider
/// actually produces.
#[test]
fn completion_highlight_rules() {
    use gpui_component::input::completion_matched_prefix_len;

    // A qualified suggestion against a partly typed database name.
    assert_eq!(
        completion_matched_prefix_len("RefreshableViews.AFAS_Facts", "select * from Refresh"),
        "Refresh".len()
    );
    // Case folds: typing lowercase still marks the match.
    assert_eq!(
        completion_matched_prefix_len("RefreshableViews.AFAS_Facts", "from refresh"),
        "Refresh".len()
    );
    // A bare column suggested after "table.": the segment after the
    // last dot is what was typed of it.
    assert_eq!(
        completion_matched_prefix_len("event_time", "t.even"),
        "even".len()
    );
    // A qualified label typed through its dot.
    assert_eq!(
        completion_matched_prefix_len("RefreshableViews.AFAS_Facts", "RefreshableViews.AFAS"),
        "RefreshableViews.AFAS".len()
    );
    // Nothing shared: no highlight rather than a stale one.
    assert_eq!(
        completion_matched_prefix_len("AFAS_Facts", "select * from "),
        0
    );
    // A query longer than the label cannot overrun it.
    assert_eq!(completion_matched_prefix_len("ab", "abcdef"), 2);
}
