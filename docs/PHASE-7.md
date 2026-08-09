# Phase 7: complex data in the grid

Status: SHIPPED in v0.1.14 (2026-08-09). The "unfinished thread"
slice of the phase-7 ideas list (now PHASE-7.1-IDEAS.md): make
composite and JSON values first-class in the results grid, plus the
statement-targeting bugs the work flushed out.

## What shipped

- **JSON column type support.** The driver parses the JSON type
  (parameters skipped unparsed) and asks the server for the string
  form in RowBinary (output_format_binary_write_json_as_string), so
  SELECT * against JSON columns works instead of failing loudly.
- **Composite rendering.** Arrays, maps, and tuples render inline as
  quoted ClickHouse literals when short, and as a compact face
  ("[...] 200 items") when long; cmd-c copies a SQL-pasteable
  literal; multi-line values collapse to one line in cells.
- **The cell inspector.** Clicking a composite, JSON, or long value
  opens a right-docked panel with the value expanded (JSON
  pretty-printed via serde_json, composites one element per line),
  tree-sitter colored with the editor's theme, with copy and
  escape-to-close.
- **Cached cell coloring.** Inline composite/JSON cells are colored
  in the grid itself through two long-lived highlighters and a
  per-cell run cache (query compilation is ~10ms, parsing is
  microseconds); validated jank-free against sat.complexBulk's 1M
  rows.
- **Types in color.** A shared type_highlight lexer colors type
  strings by role everywhere they appear (DESCRIBE and
  system.columns grids, the Columns tab, the inspector header):
  containers blue, leaves orange, Nullable muted, literals as
  literals, tuple field names plain.
- **The wrong-statement ghost, dead.** Three stacked causes found by
  instrumented builds and synthetic input: Run presses silently
  swallowed while a previous query streamed (now cancel-and-run);
  the schema hover card occluding and eating caret-placing clicks
  (now click-through); and the caret one-past-a-semicolon binding to
  the next statement's segment while visually on the finished line
  (statement_at_cursor now keeps it on its own line).
- Editor coloring for statements the SQL grammar cannot parse
  (DESCRIBE and friends): salvaged keywords and dot-qualified table
  names inside ERROR regions, with char-boundary-safe slicing.

## Test fixtures

Local docker keeps sat.complexTypes (four curated edge-case rows:
plain, empty, unicode/quotes, long), sat.complexBulk (1M generated
rows, same shape), and sat.arrayValues (the original 2026-08-08
three-column table, empty by design).

## Notes for later

- The vendored gpui-component patch count reached seven during this
  phase; docs/VENDOR-PATCHES.md catalogs them for the eventual
  rebase.
- Cell-level tree-sitter coloring holds up at million-row scale only
  because of the per-cell cache; keep that invariant if the grid's
  rendering changes.
