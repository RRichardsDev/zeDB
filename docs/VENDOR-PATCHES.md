# Vendored crate patches

Two crates are vendored with local patches, wired up via
`[patch.crates-io]` in the workspace Cargo.toml. Every patch site is
marked with a `zeDB patch` comment, so
`grep -rn "zeDB patch" vendor/` is always the authoritative list;
this file explains each one for the day a vendor gets rebased.

## vendor/gpui

gpui **0.2.2** exactly as published on crates.io, plus one backport:

### G1. Key-status deadlock fix (upstream zed#51035)

- `src/platform/mac/window.rs` (`window_did_change_key_status`)
- The spurious-becomeKey workaround sent `resignKeyWindow` while
  holding the window-state lock; macOS delivers `windowDidResignKey`
  synchronously, the function re-enters, and the main thread
  deadlocks (hard app freeze; hit 2026-08-09, diagnosed via
  `sample`). Backport of upstream commit `d7d8fcd` (merged
  2026-03-17): hoist the window pointer, drop the lock, then send.
- Drop this vendor entirely once a crates.io gpui release newer than
  0.2.2 ships with the fix (verify the function first).

## vendor/gpui-component

gpui-component **0.5.1** (upstream repo `longbridge/gpui-component`,
crate source `crates/ui`, commit
`0f0ab35233212f8f3277028995caf0c41e13ee6c`) with the patches below.

When rebasing: re-apply each patch (or verify upstream absorbed an
equivalent), then run the pinning tests listed below and click
through the affected UI.

## 1. Completion retrigger

- `src/input/lsp/completions.rs` (`retrigger_completion`)
- Re-runs the completion trigger at the current cursor. The app calls
  it when schema metadata arrives *after* the trigger character was
  typed, so the popup reopens with real suggestions instead of
  staying stale/empty.
- Referenced from `Cargo.toml`'s dependency comment; the app calls it
  from the schema-intelligence completion flow.

## 2. Completion popup width + menu-open check

- `src/input/popovers/completion_menu.rs` (top-of-file width
  constant) and `src/input/state.rs` (`is_completion_menu_open`-style
  hook near line 548)
- Widens the popup so the longest suggestion plus its detail column
  fits, and exposes whether the completion menu is showing so the
  app's key handling can defer to it.

## 3. Context-menu extension hook

- `src/input/state.rs` (~line 319) + `src/input/popovers/context_menu.rs`
- Host-app hook to append items to the editor's right-click menu.
  zeDB uses it for "View DDL" on recognized table names.

## 4. Completion highlight clamp

- `src/input/popovers/completion_menu.rs` (~line 95)
- A qualified filter ("table.x") can be longer than a suggestion's
  label; the highlight range now clamps to a char boundary within the
  label. Unpatched, StyledText panicked (app crash while typing
  `table.x`).

## 5. `SyntaxHighlighter::replace_all`

- `src/highlighter/highlighter.rs` (~line 356)
- Full-replacement update with a correct whole-document `InputEdit`,
  so one compiled highlighter (query compilation is ~10ms) can be
  reused across unrelated small texts. `update(None, ...)` describes
  the change as an insertion at offset 0, which mis-edits the
  previous tree. Used by the grid's per-cell highlight cache and the
  cell inspector.

## 6. SQL ERROR-region salvage

- `src/highlighter/highlighter.rs` (inside `styles()`, ~line 641)
- tree-sitter-sequel reduces statements it does not know (DESCRIBE,
  EXPLAIN, OPTIMIZE, KILL, TRUNCATE, ON CLUSTER, ...) to bare ERROR
  nodes with nothing capturable. Inside ERROR regions of SQL text
  the patch colors known statement keywords with the keyword style
  and identifiers after a dot (`db.table`) with the type style, so
  valid-but-unparsed ClickHouse statements still read as SQL.
  Slicing clamps to text length and char boundaries; unclamped it
  panicked on multibyte characters (app crash while typing accents
  or emoji in an unparsed statement).
- Pinned by `crates/zedb-app/tests/describe_probe.rs` (keyword color,
  dot-name color, multibyte no-panic sweep).

## 7. Hover card click-through

- `src/input/popovers/hover_popover.rs` (~line 197)
- The editor hover card had `.occlude()`, so a click aimed at text
  under the card was swallowed: the caret never moved and the next
  Run re-ran the previous statement (one third of the wrong-statement
  bug, 2026-08-09). The card is informational: clicks pass through to
  the editor; wheel scrolling stays contained via a scroll-wheel
  propagation stop.

## 8. Hover card padding

- `src/input/popovers/hover_popover.rs` (content div, same block as
  patch 7)
- The stock `.p_1()` made schema hover cards (db.table.column + type)
  read as cramped; widened to `.px_2p5().py_1p5()`.

## 9. Manual completion trigger

- `src/input/lsp/completions.rs` (`show_completion_menu`,
  `handle_completion_trigger` gains a `force` param) and its two
  callers in `completions.rs` / `state.rs`
- `show_completion_menu` opens the completion menu at the cursor
  regardless of the character before it, so a manual trigger (zeDB
  binds cmd-.) surfaces schema suggestions before any table/column
  letter is typed. `force` skips the `is_completion_trigger` gate.

## 10. Multi-cursor (cmd-D)

The largest patch by far: it adds VS Code / Zed-style multi-cursor to
the single-selection `InputState`. Upstream is single-selection in our
pin and on `main`, so there is nothing to absorb; on a rebase this is
re-applied wholesale. Design and staging rationale live in
`docs/MULTI-CURSOR-PLAN.md` (ports Helix's selection-set + change-
mapping idea, monaco's cmd-D next-occurrence algorithm). Sites, all
under `src/input/`:

- `cursor.rs`: `Selection::mapped_through_edit` (remap an offset
  through one edit), free fns `multi_edit_carets` (analytic caret
  offsets after right-to-left edits) and `word_range_at` (identifier
  under a cursor). Unit-tested in-file.
- `state.rs`: `extra_selections: Vec<Selection>` beside the primary
  `selected_range`, plus `multi_anchor`. `selection_set`,
  `is_multi_selection`, `map_extra_selections`, `multi_replace_ranges`
  (one logical edit across every selection: it applies the N text
  changes with only the cheap incremental line rewrap per edit, then
  runs the expensive document-wide work (reparse via `rehighlight_all`,
  lsp, search, one whole-doc history entry, one Change event) ONCE, so
  a big multi-edit is O(document) per keystroke, not O(cursors x
  document); also gives a single undo across all cursors), the
  `SelectNextOccurrence`
  action + `cmd-d`/`ctrl-d` binding and its `select_next_occurrence`
  handler, backspace/delete fan-out, and the Escape / plain-click
  collapse back to one cursor. `replace_text_in_range` guards for
  "targets the current selection, no IME" and fans out.
- `movement.rs`: Left/Right first collapse each selection to a bare
  cursor (`collapse_multi_to_edge`), then once collapsed move every
  cursor together one grapheme at a time (`move_multi_cursors`,
  `is_multi_collapsed`), deduping collisions.
- `mode.rs`: `rehighlight_all` re-highlights the whole document in one
  pass reusing the compiled query (patch #5's `replace_all`), for the
  batched multi-edit above.
- `element.rs`: a caret at the head of every extra cursor, computed in
  `layout_cursor` with the same line-walk / scroll / vertical centering
  as the primary, carried on `PrepaintState.extra_cursor_bounds` and
  painted in the primary's blink block; `layout_extra_selections`
  paints the N selection highlights.
- `input.rs`: `on_action` wiring for `select_next_occurrence`.

Invariants: extras stay sorted and non-overlapping with the primary;
IME stays single-selection (the fan-out guard excludes it). When a
crates.io gpui-component ships real multi-selection, reassess whether
this whole patch can be dropped.
