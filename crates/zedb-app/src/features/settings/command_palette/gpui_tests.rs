//! Window-level tests of the command palette, driven entirely through
//! the keyboard: the cmd-shift-p chord, filtering by typing, arrow
//! selection, enter to run, and escape to close. All of it flows
//! through the real keystroke interceptor.

use gpui::TestAppContext;

use crate::test_harness;

#[gpui::test]
fn chord_opens_typing_filters_enter_runs(cx: &mut TestAppContext) {
    let (workspace, cx) = test_harness::workspace(cx);

    cx.simulate_keystrokes("cmd-shift-p");
    workspace.update(cx, |workspace, _| {
        assert!(workspace.palette.open);
        assert!(!workspace.show_preferences);
    });

    // Typing lands in the palette input (focused by the toggle) and
    // narrows the command list down to Preferences.
    cx.simulate_input("preferences");
    workspace.update(cx, |workspace, cx| {
        assert_eq!(workspace.palette.input.read(cx).text(), "preferences");
        assert_eq!(workspace.palette_filtered(cx).len(), 1);
    });

    cx.simulate_keystrokes("enter");
    workspace.update(cx, |workspace, _| {
        assert!(
            !workspace.palette.open,
            "running a command closes the palette"
        );
        assert!(workspace.show_preferences, "Preferences ran");
    });
}

#[gpui::test]
fn chord_toggles_and_escape_closes(cx: &mut TestAppContext) {
    let (workspace, cx) = test_harness::workspace(cx);

    cx.simulate_keystrokes("cmd-shift-p");
    workspace.update(cx, |workspace, _| assert!(workspace.palette.open));

    // The same chord closes it again.
    cx.simulate_keystrokes("cmd-shift-p");
    workspace.update(cx, |workspace, _| assert!(!workspace.palette.open));

    cx.simulate_keystrokes("cmd-shift-p");
    workspace.update(cx, |workspace, _| assert!(workspace.palette.open));
    cx.simulate_keystrokes("escape");
    workspace.update(cx, |workspace, _| assert!(!workspace.palette.open));
}

#[gpui::test]
fn arrows_move_the_selection_and_wrap(cx: &mut TestAppContext) {
    let (workspace, cx) = test_harness::workspace(cx);

    cx.simulate_keystrokes("cmd-shift-p");
    let count = workspace.update(cx, |workspace, cx| {
        assert_eq!(workspace.palette.selected, 0);
        workspace.palette_filtered(cx).len()
    });
    assert!(count > 1);

    cx.simulate_keystrokes("down");
    workspace.update(cx, |workspace, _| {
        assert_eq!(workspace.palette.selected, 1);
    });

    // Up twice from index 1 wraps to the end of the list.
    cx.simulate_keystrokes("up up");
    workspace.update(cx, |workspace, _| {
        assert_eq!(workspace.palette.selected, count - 1);
    });
}
