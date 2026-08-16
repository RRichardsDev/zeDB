# Phase 10.6: agent pane drag, select, scroll

Status: BUILT (2026-08-16). From `docs/IRL-ISSUES.md`. Landed as a
vendor selection-drag probe (docs/VENDOR-PATCHES.md patch 13) plus
transcript wiring: stick-to-bottom pauses while a selection drag is
live, and dragging near an edge autoscrolls.

Dragging in the AI chat fights text highlighting and scrolling.

## Scope

- One interaction model for the agent pane: drag selects text, the wheel
  and trackpad scroll, and dragging near the edges autoscrolls while
  selecting. No gesture does two things at once.
- Check `docs/UI-DESIGN.md` and the vendored gpui-component patches
  (`docs/VENDOR-PATCHES.md`) before touching low-level event handling;
  a vendor patch may be the right layer.

## Acceptance

- Selecting, scrolling, and drag-selecting past the viewport edge all
  behave like the query editor does.
