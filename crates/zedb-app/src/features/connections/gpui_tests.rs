//! Window-level tests of the connection form: focus cycling, typing,
//! validation, save, and cancel. `ZEDB_CONFIG_DIR` is sandboxed by the
//! harness, so saves land in a temp directory, and the empty-password
//! path keeps the Keychain out entirely.

use gpui::{Focusable as _, TestAppContext};

use crate::test_harness;

#[gpui::test]
fn form_tab_key_cycles_fields_and_typing_lands(cx: &mut TestAppContext) {
    let (workspace, cx) = test_harness::workspace(cx);
    let (name, first_node_name) = workspace.update(cx, |workspace, cx| {
        workspace.start_add(cx);
        let form = workspace.connection.form.as_ref().expect("form open");
        (form.name.clone(), form.nodes[0].name.clone())
    });

    // With nothing focused, the first tab lands on the name field.
    cx.simulate_keystrokes("tab");
    workspace.update_in(cx, |_, window, cx| {
        assert!(name.read(cx).focus_handle(cx).is_focused(window));
    });

    // Typing goes through real key dispatch into the focused input.
    cx.simulate_input("local1");
    workspace.update(cx, |_, cx| {
        assert_eq!(name.read(cx).text(), "local1");
    });

    // The next tab moves on to the first node's name field.
    cx.simulate_keystrokes("tab");
    workspace.update_in(cx, |_, window, cx| {
        assert!(first_node_name.read(cx).focus_handle(cx).is_focused(window));
    });

    // Shift-tab cycles back.
    cx.simulate_keystrokes("shift-tab");
    workspace.update_in(cx, |_, window, cx| {
        assert!(name.read(cx).focus_handle(cx).is_focused(window));
    });
}

#[gpui::test]
fn save_validates_before_persisting(cx: &mut TestAppContext) {
    let (workspace, cx) = test_harness::workspace(cx);
    workspace.update(cx, |workspace, cx| {
        workspace.start_add(cx);
        // start_add prefills node and user; only the name is missing.
        workspace.save_form(cx);
        assert!(
            workspace.connection.form.is_some(),
            "an invalid form must stay open"
        );
        assert!(
            workspace
                .notice
                .as_deref()
                .unwrap_or_default()
                .contains("required"),
            "notice: {:?}",
            workspace.notice
        );
        assert!(workspace.connection.connections.is_empty());
    });
}

#[gpui::test]
fn save_persists_into_the_sandboxed_config(cx: &mut TestAppContext) {
    let (workspace, cx) = test_harness::workspace(cx);
    workspace.update(cx, |workspace, cx| {
        workspace.start_add(cx);
        let name = workspace.connection.form.as_ref().unwrap().name.clone();
        name.update(cx, |input, cx| input.set_text("save-persists", cx));
        workspace.save_form(cx);
        assert!(
            workspace.connection.form.is_none(),
            "{:?}",
            workspace.notice
        );
        assert_eq!(workspace.connection.connections.len(), 1);
        assert_eq!(workspace.connection.connections[0].name, "save-persists");
        assert_eq!(
            workspace.notice.as_deref(),
            Some("Saved save-persists without testing")
        );
    });
    // The write went to the ZEDB_CONFIG_DIR sandbox and reads back.
    let loaded = zedb_core::load_connections().expect("load saved connections");
    assert!(loaded.iter().any(|config| config.name == "save-persists"));
}

#[gpui::test]
fn duplicate_names_are_rejected(cx: &mut TestAppContext) {
    let (workspace, cx) = test_harness::workspace(cx);
    workspace.update(cx, |workspace, cx| {
        workspace
            .connection
            .connections
            .push(test_harness::saved_connection(
                "dup-name",
                zedb_core::EnvTier::Dev,
                true,
            ));
        workspace.start_add(cx);
        let name = workspace.connection.form.as_ref().unwrap().name.clone();
        name.update(cx, |input, cx| input.set_text("dup-name", cx));
        workspace.save_form(cx);
        assert!(workspace.connection.form.is_some());
        assert!(
            workspace
                .notice
                .as_deref()
                .unwrap_or_default()
                .contains("already exists"),
            "notice: {:?}",
            workspace.notice
        );
        assert_eq!(workspace.connection.connections.len(), 1);
    });
}

#[gpui::test]
fn cancel_discards_the_form(cx: &mut TestAppContext) {
    let (workspace, cx) = test_harness::workspace(cx);
    workspace.update(cx, |workspace, cx| {
        workspace.start_add(cx);
        assert!(workspace.connection.form.is_some());
        workspace.cancel_form(cx);
        assert!(workspace.connection.form.is_none());
        assert!(workspace.connection.connections.is_empty());
    });
}
