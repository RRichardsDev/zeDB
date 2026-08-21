//! Window-level tests of shell chrome behavior: the status-bar notice
//! tones and their self-clearing timers, driven on the simulated test
//! clock (no wall-clock sleeps).

use gpui::TestAppContext;

use crate::test_harness;

#[gpui::test]
fn success_flash_is_green_and_clears_itself(cx: &mut TestAppContext) {
    let (workspace, cx) = test_harness::workspace(cx);

    workspace.update(cx, |workspace, cx| {
        workspace.flash_success("No newer release found; you are on v0.0.0", cx);
        assert!(workspace.notice.is_some());
        assert!(workspace.notice_success);
        assert!(!workspace.notice_warning);
    });

    // The flash outlives a redraw, then clears on its timer.
    cx.run_until_parked();
    workspace.update(cx, |workspace, _| assert!(workspace.notice_success));
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(4));
    cx.run_until_parked();
    workspace.update(cx, |workspace, _| {
        assert!(workspace.notice.is_none(), "success flash clears itself");
        assert!(!workspace.notice_success);
    });
}

#[gpui::test]
fn later_notices_replace_the_success_tone(cx: &mut TestAppContext) {
    let (workspace, cx) = test_harness::workspace(cx);

    workspace.update(cx, |workspace, cx| {
        workspace.flash_success("all good", cx);
        workspace.flash_warning("something else", cx);
        assert!(workspace.notice_warning);
        assert!(
            !workspace.notice_success,
            "a warning must never render green"
        );
    });

    // The superseded success timer must not clear the newer notice.
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(4));
    cx.run_until_parked();
    workspace.update(cx, |workspace, _| {
        assert!(
            workspace.notice.is_some(),
            "the stale success timer must not clear a newer notice"
        );
    });
}
