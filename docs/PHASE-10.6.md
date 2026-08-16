# Phase 10.6: agent pane drag, select, scroll

Status: PLANNED. From `docs/IRL-ISSUES.md`.

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
