# Multi-cursor for the SQL editor (plan)

Status: PLANNING, on branch `multi-cursor`. Not for release until
solid; the editor is the most-used surface and cannot ship a
half-built version.

## What we're building (UX spec, from the user)

- **cmd-D** selects the word under the cursor. Pressing again selects
  the next occurrence of that word, and again, and again, each new
  occurrence becoming another selection/cursor.
- At the end of the buffer with occurrences remaining, **wrap** to the
  top and continue, stopping one before the starting occurrence.
- **Typing** replaces every selection with the typed text
  simultaneously.
- **Left/Right arrow** drops the highlight and collapses each
  selection to a bare cursor (left → to selection start, right → to
  selection end), leaving a multi-cursor.
- (Implied) **Escape** collapses back to a single cursor.

## Grounding (done 2026-08-10)

No ready-made drop-in exists:

- **gpui ships no text editor.** It's the UI framework; the editor
  with multi-cursor lives in Zed's separate `editor` crate. Our
  vendored gpui has no `Editor`.
- **gpui-component `InputState` is single-selection**, in our pin and
  on upstream `main`: one `selected_range: Selection`. Upgrading the
  vendor would not help.
- **No importable crate.** Helix's `helix-core` and Zed's `editor`
  are open source but welded to their own rope/buffer/transaction
  types.

Best references to copy the *design* from (not code):

- **Helix** (`helix-core`, Rust, same problem, clean): `Selection` is
  a set of ranges (anchor + head); every edit is a `Transaction`/
  changeset that mutates text AND maps all selection positions
  through the change. This is the pattern we port. See
  https://deepwiki.com/helix-editor/helix/2.3-selection-and-transaction-management
- **VS Code / monaco** (MIT, TS): reference for the specific cmd-D
  "add selection to next find match" + wrap-around algorithm.

## Our current model (what we're changing)

Vendored `gpui-component`, all paths relative to
`vendor/gpui-component/src/input/`:

- `cursor.rs`: `Selection { start: usize, end: usize }` — byte
  offsets, no per-selection direction. Direction is a single global
  `selection_reversed: bool` on the state.
- `state.rs`:
  - `InputState.selected_range: Selection` (~43 refs) — the single
    selection/cursor. `selection_reversed`, `last_selected_range`,
    `selected_word_range` support it.
  - Core edit path: `replace_text_in_range()` (~1963) replaces one
    range with text and sets one cursor after; called by IME
    (`EntityInputHandler`) and `insert()` / `replace_text()`. Every
    typed character flows through here.
  - Movement/selection: `move_to()`, `select_to()` (~1629),
    `move_left/right`, `select_left/right`, word variants — all read
    and write `selected_range`.
  - Mouse: `on_mouse_down()` (~1217) sets `selected_range`.
- `element.rs` (~126-171): the paint loop computes one `cursor_pos`
  plus `cursor_start`/`cursor_end` and paints a single caret and a
  single selection rectangle.

## Design: selection set + change mapping

Port Helix's two ideas, minimally:

1. **Selection set.** Keep `selected_range: Selection` as the primary
   and add `extra_selections: Vec<Selection>` for the others.
   (Refinement adopted in stage 1: this beats a `Vec<Selection>` +
   `primary: usize` because it leaves the ~63 existing
   single-selection call sites untouched, so stage 1 is provably
   behavior-identical with extras empty.) `selection_set()` returns
   extras + primary sorted; `is_multi_selection()` reports more than
   one. Invariant: extras stay sorted and non-overlapping with the
   primary.

2. **Change mapping.** A single helper that, given an edit as a set
   of `(range, replacement)` changes, (a) applies them to the rope
   left-to-right and (b) remaps every selection's offsets through the
   accumulated byte delta. All editing ops (insert, backspace,
   delete, paste) build their change list and call this one helper,
   instead of each doing ad-hoc offset arithmetic. This is the part
   that makes multi-edit correct rather than a bug farm.

Per-selection direction: cmd-D selections are all forward, so stage 1
can keep the single `selection_reversed` for the primary and treat
added selections as forward. Promote to per-selection anchor/head
only if a later stage needs it (block selection etc. — out of scope).

## Stages (each a real checkpoint; editor stays usable throughout)

**Stage 1 — Selection model, behavior-identical. DONE (9cd27c3).**
Added `extra_selections: Vec<Selection>` beside `selected_range`, the
tested `Selection::mapped_through_edit` primitive, and
`map_extra_selections` wired into `replace_text_in_range`. Extras
empty everywhere, so behavior is unchanged; verified by workspace
tests and a manual editing pass. `selection_set` /
`is_multi_selection` in place for stage 2.

**Stage 2 — Render N highlights. DONE.**
Added `layout_extra_selections` (reuses `layout_match_range`) and an
`extra_selection_paths` field on `PrepaintState`, painted with the
selection color next to the primary. Verified by temporarily seeding
two extra selections: two highlights painted at the right offsets;
seed then reverted. Refinement: **per-cursor caret rendering is
deferred to stage 5**, where collapsed bare cursors first exist.
During cmd-D the extras are non-empty words, so the highlight is what
you see; the caret/scroll math is avoided here to keep stage 2
low-risk.

**Stage 3 — Edit across N. DONE.**
`multi_replace_ranges` replaces every selection's range with the new
text in one logical edit: text mutated right-to-left (offsets stay
valid), carets computed analytically by the unit-tested
`multi_edit_carets`, primary = first caret, rest become extras. The
monolith `replace_text_in_range` guards for "targets the current
selection" (no explicit range / no IME) and fans out; backspace and
delete expand each cursor to its neighbor char then multi-replace.
Verified with a temporary word-occurrence seed: typing replaced all,
backspace deleted all, offsets correct; scaffolding reverted.
Not yet done: paste-across-N (insert uses an explicit range, stays
single) and single-undo grouping (currently one history entry per
sub-edit) — deferred to polish. IME stays single (guard excludes it).

**Stage 4 — cmd-D.**
New action + binding. First press: select word under primary cursor
(reuse existing word-range logic). Subsequent: find next occurrence
of the selected text after the last selection, wrapping to the top,
stopping one before the start; add it to the set and scroll it into
view. Verify: iterative select down a column of repeated identifiers,
wrap-around, stop-before-start.

**Stage 5 — Polish.**
Type-replaces-all already falls out of stage 3. Add: Left/Right
collapses each selection to a cursor (start/end) keeping multi;
Escape collapses to the single primary cursor. Verify the full UX
spec end to end.

## Risks and non-goals

- **Highest risk: the daily editor.** Stage 1 must be provably
  behavior-identical before anything else; if it isn't, stop.
- **IME + multi is undefined**; we suspend composition during multi.
- **Undo/redo** must record multi-edits as one history entry
  (grouping already exists via `push_history` / `end_grouping`).
- **Non-goals (for now):** alt-click to add a cursor, column/block
  selection, select-all-occurrences-at-once, find-and-replace UI.
  cmd-D and its described behavior only.
- This becomes a **tenth vendored gpui-component patch** (the largest
  by far); catalog it in docs/VENDOR-PATCHES.md when it lands.

## Where we are

Branch `multi-cursor` created off `main` at v0.1.17. This document is
the plan; stage 1 is the next action.
