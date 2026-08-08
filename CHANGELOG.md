# Changelog

User-facing changes per release. Maintained as work happens: every
change worth a user's attention gets a line under Unreleased in the
same commit (or session) that makes it; cutting a release renames the
section to the version. Engineering internals live in docs/devlog.md,
not here. The release workflow publishes the version's section as the
GitHub release notes.

## Unreleased

- The fresh-install default query surveys the server (largest tables
  across all databases) instead of assuming a specific database.
- Optional GitHub sign-in from Preferences (device flow, `read:user`
  scope only): shows your avatar and name in Preferences and the
  toolbar, and lays the groundwork for settings sync. The one-time
  code is auto-copied to the clipboard and presented GitHub-style;
  the token lives in the macOS Keychain.
- Settings sync (Preferences): keep preferences, connections, and
  custom agents in a git repo you own. Pulls on launch and window
  refocus, pushes on change, and a fresh machine inherits the repo's
  settings on enable. Passwords never sync. Paste any git URL, or,
  signed in to GitHub, zeDB spots an existing zedb-settings repo via
  your own ssh key and prefills it, and can create a private one for
  you with a one-time approval that is never kept.
- Command palette on cmd-shift-P: type to filter, arrows and Enter to
  run; includes Open settings.json, Preferences, new query, fleet and
  agent panes, vim mode, sync now, and disconnect.
- The settings file is now `settings.json` (renamed from
  `preferences.json`; migrated automatically) and can be opened in
  your editor from the palette.
- cmd-N in the open agent pane starts a new thread with the
  last-used agent (remembered in settings; the picker opens if
  there's no history yet).

## v0.1.5 - 2026-08-08

Results-grid release: sort, filter, and shape query results from the
grid itself, with the SQL always telling the truth.

### Sorting from the grid

- Click a column header to sort (descending first, then ascending,
  then off). Shift-click builds multi-column sorts with numbered
  arrows; right-click offers an "Order by" submenu.
- The query's top-level ORDER BY is rewritten in the editor, on its
  own line, and only that statement re-runs. Indicators always
  reflect the SQL that actually executed, including hand-written
  sorts.

### Filtering from the grid

- Right-click a header for "Filter...". Columns with ten or fewer
  distinct values (checked by a capped server probe; Enum variants
  read from the type) get a checkbox list, including a "(null)" entry
  for nullable columns; everything else gets a text field where plain
  text means contains, and %patterns%, operators (> 10), and
  "is null" pass through.
- Filters land as managed conjuncts in the top-level WHERE.
  Hand-written predicates survive, light the indicators, and pre-fill
  the panel. Filtered headers wear a muted purple border; hovering a
  header summarises its sorts and filters.
- Enter applies, Escape or clicking anywhere else closes.

### Grid feel

- Columns drag-resize from the header dividers; widths are remembered
  per column set for the session.
- Re-running keeps the previous rows on screen (with a "running"
  hint) until the replacement streams in. Rapid sort/filter changes
  coalesce into one run, cancelling anything in flight.
- Timestamps render two-tone (muted-red date, muted time) and NULLs
  muted italic.

### Fixes and polish

- Splitters keep their tight 1px seam but carry a wide invisible grab
  band on both sides; column dividers grab from either side and never
  mis-fire a sort.
- Schema sidebar databases collapse correctly while a filter is
  applied.
- Refocusing the window quietly re-checks cluster health and looks
  for updates (debounced), so a dead connection is noticed on return.
- Status-bar read/size counters reset per statement; the vim mode
  chip reads "-- INSERT --" and lives in the bottom bar; the update
  pill matches the badge styling.

## v0.1.4 - 2026-08-07

- Agents gained two instant, cache-backed tools: `schema_search`
  (fleet-wide search over database, table, and column names) and
  `lint_sql` (checks drafted SQL identifiers with line numbers before
  anything runs). Answers carry a "cached as of" stamp; live tools
  remain the source of truth. Terminal agents opt in with
  `zedb mcp --cache-connection <name>`.
- Right-click a table in the query editor or the schema sidebar and
  jump straight to its DDL.
- Hover cards resolve database-qualified names (even when the bare
  table name exists in several databases), cover database names
  themselves, and include column counts.
- The completion popup supports arrow-key navigation, enter to accept,
  and escape to dismiss, including under vim mode, and widens to fit
  the longest suggestion.

## v0.1.3 - 2026-08-07

- SQL editor schema intelligence, phase one of polish: completions
  after `db.`, `table.`, and `alias.` qualifiers; unknown tables and
  columns get squiggles with precise token matching; typo detection no
  longer misfires across spaces (`e. from`).
- Column metadata warms automatically from sidebar expansion, object
  clicks, and databases referenced in the SQL you type; the popup
  reopens once metadata arrives.

## v0.1.2 - 2026-08-06

- The agent pane: run Claude Code, Codex, or any ACP agent inside
  zeDB (cmd-i). Threads stream markdown replies, show tool activity,
  and gate every risky action behind inline permission cards with
  remembered approvals. Agents see your screen context and reach zeDB
  through its own read-only MCP tools, including proposing migrations
  and queries into the app for your review.
- The schema cache: instant sidebar loads, per-connection snapshots
  persisted across launches, and background refreshes on the health
  poll and after your own DDL.
- Connection management parity: edit, duplicate, and delete from both
  the right-click menu and the footer buttons.

## v0.1.1 - 2026-08-06

- The managed migration lifecycle in-app: author migrations with live
  syntax checks, regen and chain checks, commit and push, per-step
  apply progress with timings, and view or edit migrations from the
  fleet matrix.
- Open a migration repo by pasting a git URL; empty repos
  auto-initialize pinned to the live server; pull for behind
  checkouts.
- Fleet defaults to every non-system database; tracking moved to a
  dedicated `zedb_config` database with a repo identity column.
- About panel, Check for Updates, and periodic update checks.
- Read-only/write posture pills on connections and right-click
  duplicate.

## v0.1.0 - 2026-08-06

- First release: ClickHouse cluster explorer (connections with
  keychain-held credentials, schema tree, virtualized results grid),
  streamed query execution with progress, SQL highlighting, vim mode,
  and the migration fleet view with drift verification.
