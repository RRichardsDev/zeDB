# Phase 0 plan: the explorer

Goal: zeDB becomes your daily driver for ClickHouse exploration, replacing
clickhouse-client / DBeaver for "look at schema, run query, read results".
Nothing in this phase mutates a database.

## Working rules

- Every milestone ends buildable, on main, with something you can
  demo in 30 seconds. No milestone depends on an unfinished later one.
- Riskiest unknowns get spiked before features are built on them.
- Keep `docs/devlog.md` as you go: GPUI findings, gotchas, missing
  primitives. This is the raw material for upstream contributions and
  costs almost nothing in the moment.
- Local dev/test ClickHouse: a pinned `clickhouse` binary running
  `clickhouse server` (or ephemeral `clickhouse local`) seeded with a
  synthetic dataset. Never develop against a real environment.

## Milestones

### M0. Skeleton

Cargo workspace with `zedb-core`, `zedb-ch`, `zedb-app` (empty `zedb-cli`
and `zedb-driver` can wait; add crates when they get their first real
code). GPUI app opens a window with a placeholder layout: sidebar, main
pane, status bar. CI (GitHub Actions): fmt, clippy, test, build on macOS.

Done when: `cargo run -p zedb-app` opens the window; CI is green.

### M1. Headless query round-trip

In `zedb-ch`: connect over HTTP, execute a query, stream results as typed
rows (RowBinary with names/types). In `zedb-core`: a driver-shaped
interface in front of it (informal for now; the formal capability trait is
Phase 1). Integration tests run against an ephemeral local server.
No UI in this milestone.

Done when: a test connects, runs `SELECT` against a seeded table, and
asserts typed values (including Nullable, DateTime64, arrays, enums:
ClickHouse's type zoo is the actual work here).

### M2. Grid spike (de-risk, throwaway allowed)

Virtualized data grid in GPUI fed by synthetic data: 1M+ rows, 50+
columns, only visible cells rendered. Scroll, column widths, row hover,
cell selection + copy. Measure: smooth scroll (no dropped frames) and
bounded memory.

This is the single riskiest GPUI unknown in the project. It comes before
any real feature UI so that if the approach is wrong, nothing is built on
top of it yet. Spike code may be rewritten; findings go in the devlog.

Done when: 1M synthetic rows scroll smoothly with flat memory, and you
have written down how (uniform_list vs custom element, etc.).

### M3. Connections

Connection model in core: logical cluster name, one or more node or load
balancer endpoints, shared credentials, environment tier (dev / staging /
production), read-only flag (default on). Persisted config in the platform
config dir; secrets in the OS keychain, never in the config file. UI:
connection picker, add/edit/delete form, per-node connection test,
connect/disconnect,
tier shown as an unmistakable visual identity (color/badge) from day one.

Done when: you can add your staging cluster in the UI, test its nodes,
connect through a healthy endpoint, and see the tier badge; invalid
credentials are not presented as a successful tested save, and the config
file contains no secret material. Deleting a connection also removes its
saved credentials and leaves selection and active-connection state valid.

### M4. Schema tree

Introspection in core via system tables: databases, tables, views,
materialized views, dictionaries, with engine and row/size metadata.
Sidebar tree UI: lazy-loaded, filterable-by-typing, handles hundreds of
databases (your fleet is the test case). Selecting a table shows columns
and types.

Done when: pointed at staging, the tree loads fast, filter narrows
instantly, and expanding any node never blocks the UI.

### M5. Object inspector

Selecting an object shows its `SHOW CREATE` DDL (read-only, highlighted
once M8 lands) plus a summary tab: columns, engine, partition key, order
by, sizes. This is the read-only half of "first-class ClickHouse":
surfacing what generic tools bury.

Done when: clicking through tables/views/MVs on staging answers "what is
this object" without leaving the app.

### M6. Query editor v1

Plain-text multiline editor pane (GPUI text input primitives; highlighting
is M8, completion is not Phase 0). Cmd+Enter runs the buffer (or
selection) against the active connection. Server errors render readably
(ClickHouse error codes and messages, not a debug dump). Cancel button
that actually kills the HTTP request. Multiple editor tabs. The editor model
and actions must be command-driven so the default keymap and Vim mode share
one editing core rather than becoming separate editors.

Done when: type query, run, see error or results; cancel a
`SELECT sleep(3)` mid-flight.

### M7. Results for real

Wire M1 streaming into the M2 grid: results stream in and render
incrementally, grid stays responsive on multi-million-row results, status
bar shows rows read / bytes / elapsed (from ClickHouse progress headers).
Guardrails: default row cap with explicit "stream more", so a careless
`SELECT *` on a billion-row table degrades gracefully.

Done when: `SELECT * FROM <big staging table>` streams into the grid
without jank, and progress/timing is live in the status bar.

### M8. SQL highlighting

Tree-sitter SQL grammar wired into the editor and the DDL viewer. Pick the
best available grammar and note its ClickHouse gaps in the devlog (a
ClickHouse-specific grammar fork is a possible future contribution, not
Phase 0 work).

Done when: editor and inspector are highlighted and typing latency is
unchanged.

### M9. Vim mode and preferences

A Preferences surface with a persistent Vim-mode toggle. Vim support covers
normal, insert, visual, visual-line, and visual-block modes; counts; motions;
operators; text objects; registers; marks; search; repeat; macros; and the
command-line actions relevant to editing query buffers. Unsupported Vim
features must be documented explicitly rather than silently behaving
differently.

Done when: an experienced Vim user can edit query buffers for a full week
without switching Vim mode off to work around missing core editing behavior.

### M10. Daily-driver polish

The gap-closing milestone, driven by a week of real use. Expected
contents: keyboard-first navigation (palette or shortcuts for connection
switch, tree focus, editor focus), query history (session-level is
enough), result cell copy in useful shapes (value, row, TSV block),
window/layout state restored across launches, error and empty states that
do not look like placeholders.

Done when: you stop opening DBeaver / clickhouse-client for exploration
tasks for a full week, and anything still forcing you back is either
fixed or explicitly parked in IDEAS.md.

## Order and dependencies

M0 → M1 and M2 in either order (M1 is pure Rust, M2 is pure GPUI; good
parallel tracks) → M3 → M4 → M5/M6 in either order → M7 (needs M1+M2+M6)
→ M8 → M9 → M10.

## Explicitly not in Phase 0

Autocomplete/LSP-style editor intelligence, saved query library, charts,
export formats beyond clipboard, guest drivers, anything from Phase 1/2
(migrations, fleet), Linux packaging polish (build should work; polish
waits).

## Phase exit

Phase 0 is done when M10's done-condition holds. The next step after that
is Phase 1 (headless migration engine), which starts with finalizing the
new repo format: see SPEC.md open decisions.
