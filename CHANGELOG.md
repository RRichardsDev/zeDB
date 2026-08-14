# Changelog

User-facing changes per release. Maintained as work happens: every
change worth a user's attention gets a line under Unreleased in the
same commit (or session) that makes it; cutting a release renames the
section to the version. Engineering internals live in docs/devlog.md,
not here. The release workflow publishes the version's section as the
GitHub release notes.

## Unreleased

- New "Estimate query cost" command in the palette: a pre-flight
  `EXPLAIN ESTIMATE` for the statement Run would target, shown as a
  strip above the results with estimated rows, parts, and marks, a
  miniature primary-key pruning bar, and a plain-language warning when
  the scan is large or the WHERE is not covered by the primary key.
  It never blocks running the query.
- Fixed `PRIMARY KEY` not being split onto its own line in the schema
  panel's engine definition, so a table with an explicit primary key now
  reads as cleanly as one without.
- Reworded two query-advisor messages so they no longer use an em-dash.

## v0.1.29 - 2026-08-14

No functional changes. Internal code maintainability improvements.

## v0.1.28 - 2026-08-13

This release makes working query tabs easier to preserve and reuse, adds
editor-local query variables, and tidies connection status at a glance.

### Query workspace

- Press `cmd-s` to save the active query tab in the History drawer's Tabs
  section. Each tab is saved individually on this machine with its SQL and row
  limit, supports duplicate names through stable identity, and reopens as an
  ordinary query tab.
- Query editors support buffer-local `@set name=value` declarations and
  `${name}` substitution without sending the declarations to ClickHouse.

### Connection polish

- The connected indicator sits beside the compact environment and write marks
  at rest, then moves beside their expanded badges on hover.

## v0.1.27 - 2026-08-13

Native ClickHouse connections make reads and live tails faster, with an
opt-in preview of ClickHouse 26.6 streaming queries. This release also rolls
up a set of everyday tab, grid, connection-list, and error-bar refinements.

### Instant tails

- Read queries automatically use a persistent native TCP connection when the
  server exposes one, while retaining safe HTTP fallback.
- "Get instant updates" now works: compatible servers use Live View `WATCH`
  over native TCP, then fall back to fast native polling when `WATCH` is not
  supported.
- ClickHouse 26.6 `STREAM CURSOR` delivery is available as an experimental,
  disabled-by-default preference for compatible single-table tails. The flask
  beside "Get instant updates" opens the setting; unsupported queries and
  servers continue through the normal fallback ladder.
- Pausing, stopping, editing, or closing an instant tail releases its dedicated
  native query and resumes from the saved cursor where available.

### Everyday polish

- Preference descriptions wrap before their fixed-width controls instead of
  running underneath them in narrower windows.
- Query tabs can be reordered by dragging; the drop target shows an accent
  edge and a ghost of the tab follows the cursor.
- Default query tabs are named "Tab 1", "Tab 2" (was "Query N").
- Result grid columns auto-fit to their content (clamped to a sensible
  min/max) when a result doesn't fill the width, instead of a flat default;
  a remembered column resize still wins.
- Connection list shows the node count inline as a muted "(N)" next to the
  name, expanding to the full "N nodes" on hover; the connected dot stays on
  the name line.
- In the error bar, Copy is now a copy icon and the "Ask" action is just the
  remembered agent's logo.

## v0.1.26 - 2026-08-13

Phase 10: live tail, a `tail -f` for a ClickHouse table.

### Live tail

- Right-click a MergeTree table in the schema sidebar and pick a
  retained-row cap (20 / 50 / 100 / 500 / 1000 / Unlimited) to open a live
  view in its own "Tail N" tab (steel-blue border). It polls over HTTP on
  the query's leading ORDER BY key (`WHERE key > :last`, off the main
  thread, every ~1.5s), so cost stays flat however long it runs; the
  initial load is a light 20 rows.
- The tab editor shows the runnable query the tail is based on. Whatever you
  write there is what gets tailed: edit the columns, WHERE, joins, GROUP BY,
  ORDER BY key, or LIMIT and press Update Tail to re-base the live view. It
  validates before switching and repaints instantly; an invalid query (or
  one that drops the ORDER BY key) is reported and the running tail is kept.
- Newest rows land at the top and the buffer trims the oldest past the cap.
  The view follows the top while you are at it, but if you scroll down to
  read older rows it holds your place instead of yanking you back up.
  Pause / Resume / Stop are coloured controls.
- When a native ClickHouse port is reachable, a "Get instant updates" button
  appears (discovery runs off the main thread). True server-push over the
  native protocol is not built yet; the button says so.

## v0.1.25 - 2026-08-12

A round of real-world fixes and polish from daily use.

### Editor

- Multi-cursor: shift-cmd-left / shift-cmd-right (select to line start / end)
  now extends the selection at every cursor, not just the primary one.
- Query tabs have a right-click menu: Close tab, Close others, Close to the
  right. Running or errored tabs are protected, and one tab always remains.

### Schema & connections

- Switching cluster or node re-runs the open editors' diagnostics, so a
  database flagged "unknown" on one cluster drops its squiggly as soon as
  the new cluster reports it, instead of lingering until the next edit.
- The schema inspector's DDL tab drops its Copy bar; Copy is now a hover
  icon at the top-right of the DDL editor, leaving more room for the DDL.
- The dev environment colour is now blue (was green), consistent across the
  badges, the connection triangle glyph, and the fleet view; staging stays
  gold, production red.
- The connection list pluralizes the node count ("1 node" / "2 nodes").

## v0.1.24 - 2026-08-12

### Query advisor

- Two new findings finish Part A. "Query scans every partition" fires when
  a selective query filters on something other than the partition key, so
  ClickHouse read every partition; it names the partition key to add a
  predicate on. "Aggregation re-scans the whole table" fires when a GROUP
  BY collapses a big scan into a handful of groups, and offers copyable
  projection DDL rebuilt from the query itself (a two-step ADD PROJECTION /
  MATERIALIZE PROJECTION, deterministic, no model), or a materialized view.
- Findings without generated DDL (the partition advice) now have a
  copy-suggestion button, and the fix text wraps instead of being clipped
  when the panel is narrow.

### Saved & History

- Saved queries show when they were saved ("just now", "2h ago"), aligned
  to the right of the row. History moves its relative time to the right as
  well, keeping rows and duration on the left.

## v0.1.23 - 2026-08-12

- Relicensed to the PolyForm Noncommercial License 1.0.0: zeDB is
  source-available and free to use, modify, and share for any
  noncommercial purpose, but not for commercial use. Vendored components
  under `vendor/` remain under their own (Apache-2.0) licenses.

## v0.1.22 - 2026-08-12

Phase 9 makes ClickHouse more legible: a query advisor that turns a
query's plan into a fix, and a schema inspector that shows the MergeTree
lifecycle and materialized-view lineage. Plus a batch of editor, grid,
and agent-panel fixes.

### Query advisor

- On the Saved tab, Advise runs a saved query and, from its EXPLAIN plan
  and run stats, flags when the primary key isn't filtering it (scanned a
  lot to return a little). The fix is copyable DDL naming the real WHERE
  column, with the skip-index type chosen from the column's type and a
  cardinality probe (minmax for ranges, set(0) for low-cardinality
  equality, bloom_filter otherwise, tighter for very high cardinality).
  Copy / open-in-editor actions, plus an optional silent hand-off to a
  remembered agent; a clean query shows a short "looks fine" note.

### Schema inspector

- Parts tab: active parts grouped by partition (count, rows, sizes,
  ratio, merge level) with a "too many parts" warning, and, live, the
  merges in progress with a progress bar refreshing every couple of
  seconds; mutations are tagged.
- Dependencies tab: the materialized-view lineage as source -> view ->
  target chains, walkable both ways by clicking nodes, with broken
  pipelines (a referenced table that no longer exists) flagged.
- Projections tab: each projection's definition and size, with a one-line
  explanation of what a projection is.
- Moving between tables keeps the tab you were on.

### SQL editor and grid

- Fixed a crash where any non-ASCII character (accent, em-dash, arrow,
  emoji) typed or pasted into the editor aborted the app.
- Fixed cmd-c not copying from the editor (the results grid was stealing
  it); clicking a grid cell now focuses it so grid copy still works.
- Query tabs scroll (shift-wheel) instead of pushing the toolbar off
  screen when there are too many to fit.
- Bookmarking a query no longer jumps the drawer to the Saved tab.

### Agent panel

- Text selection can span several messages (one drag, one copy).
- Our messages render as a bordered box matching the composer.
- Opening the agent panel closes the history/saved drawer.

### Connections

- Connection rows are quieter at rest: a small triangle (environment) and
  square (read/write), with the full pills on hover.

## v0.1.21 - 2026-08-11

Phase 8: column storage intelligence. The schema inspector learns to
show how each column is stored, advise on savings, and apply the change.

### Per-column storage

- The Columns tab shows per-column storage: compressed and uncompressed
  size, the compression ratio, and the codec (colored like types). The
  table header gains an overall compression ratio. Per-column sizes
  exist only for Wide parts, so a table stored entirely in Compact parts
  shows the table ratio plus a note explaining the per-column columns
  are blank.

### Storage advisor

- An opt-in "Analyse" scans the table once for each column's
  distinct-value count (with a confirmation first on a writable
  connection, since it may create a temporary table), then an Advice
  lane flags each column: a green tick where storage is already fine
  (hover explains why), or an action icon where a codec or type change
  would help. The suggestions are rule-based (e.g. low-cardinality
  string to LowCardinality, timestamps to Delta coding), not AI, and the
  scan result is cached for the session.
- On a writable connection each suggestion is measured against a sample
  and shows how many times smaller the change would make the column
  (e.g. "22x").

### Applying suggestions

- Left-click applies a suggestion in place on a staging/dev connection
  (with a confirmation first when the table is large, since it rewrites
  data); on production it never applies in place but opens the query
  editor instead. Right-click always opens the editor with the full
  script. Codec changes include the `OPTIMIZE ... FINAL` needed to
  recompress existing data, and when the node selector is set to a
  cluster scope the statements run `ON CLUSTER` so they reach every
  node. Applying updates the changed column in place, with a spinner if
  it runs long.

### Fixes

- Fixed a schema-explorer bug where filtering showed matching databases
  with an expanded arrow but no objects, forcing a second click to load
  them; matches now populate from the warmed cache immediately.

## v0.1.20 - 2026-08-10

- Copying cells from the results grid now pastes cleanly into Excel,
  Google Sheets, and Numbers: the default copy (cmd-C) is tab-separated,
  which spreadsheets split into columns on a plain paste (comma-separated
  text did not). Right-click a cell for a menu with Copy and Copy as CSV
  when you specifically want the comma format; right-clicking a cell
  outside the current selection selects it first.

## v0.1.19 - 2026-08-10

- Multi-cursor edits with many cursors no longer stall the editor: a
  keystroke now does its document-wide work (re-highlight, etc.) once
  for the whole edit instead of once per cursor. A multi-cursor edit
  is also a single undo now, reverting every cursor together.

## v0.1.18 - 2026-08-10

Multi-cursor comes to the SQL editor.

- Multi-cursor in the SQL editor. cmd-D selects the word under the
  cursor; press again to add the next occurrence, and again, wrapping
  from the bottom of the editor back to the top and stopping one before
  where you started. Typing replaces every selection at once. Left or
  Right drops the highlights and leaves a cursor at each spot, then
  keeps moving them all together; Escape or a click returns to a single
  cursor.

## v0.1.17 - 2026-08-10

First round of real-use fixes.

- Backtick-quoted names (`db`.`table`.`column`) are now highlighted,
  completed, and hovered like bare ones; the tokenizer had been
  treating them as opaque strings.
- Column autocomplete for bare (unqualified) names: typing a column
  offers the columns of the tables in the current statement, deduped
  by name, and it works while you are still typing the SELECT list
  before the FROM. Suggestions, hover, and go-to are scoped to the
  statement under the cursor, so other queries in the editor no
  longer leak their tables in.
- cmd-. opens the schema completion menu on demand, even before any
  table or column letter is typed.
- Hovering a column resolves it even without a table qualifier,
  showing db.table.column (column in italic) and its type in a
  roomier card.
- The results grid supports multi-cell selection: cmd-A selects all,
  click-drag and shift-click select a rectangle, and cmd-C copies a
  region as CSV with a header row (single cells still copy their bare
  value).

## v0.1.16 - 2026-08-10

- Export current query results, from the command palette: a two-step
  dialog picks the scope (the tab's max-rows cap, or all rows) then
  the format (CSV, Parquet, JSONEachRow), defaulting the location
  quietly to Downloads (click the path to edit it, or the folder
  button for a native save panel). The download streams the server's
  own output format straight to disk, bypassing decode and the grid,
  with a live byte count and transfer rate; Cancel aborts it and
  removes the partial file.
- The status bar drops the stale "M8" milestone tag, showing just the
  version.

## v0.1.15 - 2026-08-09

The query editor learns to remember and to explain itself: history
and saved queries, EXPLAIN visualized, and errors that summon your
agent to fix the statement where it stands.

### Query history and saved queries

- Query history and saved queries: a resizable drawer beside the
  query editor (toolbar clock icon or the command palette) with
  History and Saved tabs and a search box filtering both. Every run
  is recorded automatically with its connection, time, duration, and
  row count (failed single statements record their error, in red);
  consecutive re-runs collapse into one entry, the newest 1000 are
  kept locally, and Clear history sits in a gutter behind an
  are-you-sure. Hovering any row shows the full statement,
  syntax-colored; clicking inserts it at the editor cursor as its
  own paragraph, and a renamed saved query inserts with a
  "-- Saved: name" comment above it.
- Bookmarking a history entry saves it instantly, named from the
  query's first line; saved queries show as full-width cards with
  star/rename/delete actions beneath (favorites pin to the top and
  keep their star lit). Saved queries live in settings.json and sync
  to your other machines; history stays local.

### EXPLAIN, visualized

- "Explain query" in the command palette draws
  the plan for the statement under the cursor as a colored tree in
  the results pane, and every MergeTree read shows its index pruning
  (selected vs initial parts and granules per index stage) with a
  utilization bar: green when the index prunes hard, red on a full
  scan. The pane scrolls both ways for deep plans. Works on servers
  back to at least 25.8.

### Errors grew hands

- The error bar offers Copy and Ask (your
  last-used agent, shown by its logo). Ask opens the agent pane,
  sends the error automatically once the session is ready, and
  attaches the failing tab and SQL invisibly; when the agent
  proposes a corrected query, it replaces the failed statement in
  the original tab instead of opening a new one.

### Polish and fixes

- Fixed a rare hard freeze of the whole app on macOS when a popup
  (tooltip, hover card) was open during a window activation change:
  an upstream gpui deadlock (zed#51035), fix backported.
- The connection list's posture and tier pills shrank to tag size,
  giving the connection names the room.
- The Run button carries the whole lifecycle: it reads "Running..."
  while a query streams and turns into Cancel on hover, replacing
  the dedicated Cancel button. "Run all" became Execute with a
  script glyph, and both buttons moved their keyboard shortcuts into
  tooltips.

## v0.1.14 - 2026-08-09

Complex data grows up in the results grid, and the long-standing
"it ran the wrong statement" ghost is dead.

### Composite values and JSON

- The JSON column type is supported: queries against tables with
  JSON columns no longer fail with "unsupported ClickHouse type".
- Arrays, maps, and tuples render honestly in the grid: short values
  inline as proper quoted literals, long ones as a compact face like
  "[...] 200 items", and cmd-c copies a SQL-pasteable literal.
  Multi-line values (DESCRIBE's named tuple types) no longer spill
  over neighboring rows.
- A cell inspector: clicking a composite, JSON, or long value opens
  a panel docked to the grid's right edge with the full value
  expanded (JSON pretty-printed, composites one element per line),
  syntax-colored to match the editor theme, with copy and
  escape-to-close.
- Inline composite and JSON cells are syntax-colored in the grid
  itself, computed once per cell and cached, so million-row scrolls
  stay smooth.

### Running the statement you meant

- Pressing Run while a previous query is still streaming cancels it
  and runs the statement you asked for; it was silently ignored.
- A caret at the end of a statement's line (just past the semicolon)
  runs that statement, not the neighbor below it.
- The schema hover card no longer swallows clicks: clicking into a
  statement whose text sat under the card now places the caret.

### Types in color

- Column types render in color wherever they appear (DESCRIBE and
  system.columns results, the schema inspector's Columns tab, the
  cell inspector's header): container types like Array, Map, and
  Tuple in blue, leaf types in the editor's orange, Nullable muted,
  Enum labels and numbers as literals, and named-tuple field names
  plain, so Array(Nullable(String)) reads as structure, annotation,
  payload.
- Statements the SQL grammar doesn't know (DESCRIBE, EXPLAIN,
  OPTIMIZE, KILL, TRUNCATE, and friends) get their keywords colored
  in the editor, and table names after a dot color like parsed
  object references (describe sat.foo).
- Fixed a crash when highlighting unparsed statements containing
  multibyte characters (accents, emoji).

## v0.1.13 - 2026-08-09

The ops view (Phase 6): a live cockpit for the cluster, new in the
toolbar while connected. One glance answers "what is this cluster
doing right now", and a runaway query dies in two clicks.

### Queries now

- Every query running on the cluster, refreshed every 2 seconds
  while the view is visible: elapsed, user, memory, read progress,
  and the query text. Each row carries the client's identity (tool,
  address, OS user, and the initial user when it differs), and the
  header counts open connections by protocol.
- KILL QUERY sits one click away on write connections and is
  honestly disabled on read-only ones. Kills from the ops view
  report "Query killed from the ops view" in the editor instead of
  a cryptic transport error, and server-side cancellations (code
  394) read as cancellations.

### Background, replication, and storage

- The view splits into tabs: Queries, Background, Replication,
  Storage, with the header and connection counters fixed above.
- Background: merges with progress bars, parts, and size, plus
  unfinished mutations with failing ones surfaced first alongside
  their fail reason.
- Replication (10s cadence): a green all-healthy line, or the
  problem replicas with readonly/session/delay/queue flags plus
  replication-queue exceptions.
- Storage (10s cadence): disk usage bars that go amber at 75% and
  red at 90%, and Largest Tables with a Top dropdown (10, 25, 50,
  100, or All), sizes and row counts right-aligned, and names
  colored like the SQL editor.

### Cluster-wide scope

- On connections with a known topology, a scope dropdown in the
  header switches every tab from the connected node to the whole
  cluster: queries, merges, mutations, replication problems, and
  disks fan out to every replica with a NODE column; Largest Tables
  sums one replica per shard for true cluster-wide table sizes;
  connection counters total across nodes; and Kill reaches queries
  on any node via ON CLUSTER.
- The view resets and refetches instantly when the connection or
  node changes (scope returns to the single node), and clusters
  whose server config carries no credentials for distributed
  queries get a one-line explanation instead of a raw error wall.

## v0.1.12 - 2026-08-08

Sharding awareness (docs/PHASE-5.md, complete). zeDB reads a
cluster's topology from what each node reports about itself and
stops implying equivalence it cannot verify; Distributed tables
remain the query path and nothing becomes configurable.

### Know your shards

- On connect, each node reports its shard/replica memberships
  (system.clusters, is_local), so topology works regardless of how
  endpoints are reached (port-mapped docker, DNS aliases).
- The node picker labels nodes with their shard when a cluster
  splits them, and switching to a different shard says so once:
  local tables show that shard's slice; Distributed tables are
  unaffected. Replicas, load balancers, and unknown topologies
  behave exactly as before.
- A read-only Topology section on the Cluster connection screen: one
  card per named cluster showing its shards and which nodes hold
  them (replicas in one cluster, shards in another over the same
  nodes both render truthfully). Clicking the already-selected
  connection in the sidebar returns to this screen, previously
  unreachable while connected.

### Honest Distributed tables

- A DT glyph in the schema sidebar (alongside T/V/MV/D) and the
  sharding key in the object overview, parsed from the engine
  definition.
- A real size and row count: the local table summed across shards
  (one replica per shard via the cluster() function), shown in
  parentheses with a "virtual" tooltip since the number is derived,
  not stored. Plain views stay sizeless; a materialized view's data
  belongs to its target table.

## v0.1.11 - 2026-08-08

- Replacing or closing a huge result set no longer freezes the app:
  the old rows are freed on a background thread. (A 193M-row result
  previously beachballed the UI for minutes when the next query
  landed.)

## v0.1.10 - 2026-08-08

Themes and a per-cluster driver.

### Theming

- Theme switching: Dark (unchanged), a first-draft Light, or System
  (follows macOS appearance). Switch from Preferences or the command
  palette; the choice lives in settings and syncs across machines.
  Built on Zed-style JSON theme configs, so custom themes become
  possible later.
- Moving over a theme command in the palette previews it live; Enter
  keeps it, dismissing reverts to the saved theme.
- First light-mode polish: badge tints, primary buttons, Connect
  hover. Expect further tuning.

### Driver and queries

- Per-cluster driver configuration in the connection form: a settings
  list sent with every query on that cluster, pre-seeded with
  removable max_execution_time and connect_timeout rows
  (connect_timeout configures the driver itself). Blank means what
  zeDB always did; synced with the connection through settings sync.
- Query results transfer compressed (zstd/gzip negotiated over HTTP),
  typically several times less data on large result sets. Add an
  enable_http_compression=0 driver setting to opt a cluster out.

### Fixes

- The completion popup no longer crashes on qualified names (table.x)
  whose typed prefix is longer than the completion label.
- Modal panels (new migration, commit, delete confirm, about, regen
  checks) block clicks to the view behind them; migration headers can
  no longer be clicked through the authoring window.
- system.* and INFORMATION_SCHEMA references no longer squiggle as
  unknown.
- The connection form scrolls when it outgrows the window instead of
  clipping the bottom.
- Schema sidebar tables show their on-disk size (B/KB/MB), small and
  right-aligned.

## v0.1.9 - 2026-08-08

- DDL and other resultless statements complete cleanly ("OK:
  statement executed") instead of failing with a RowBinary decode
  error; ClickHouse answers them with an empty body by design.

## v0.1.8 - 2026-08-08

Soak-mode fixes from a day of real use.

- Grid sorts and filters target the statement that actually ran: with
  identical queries in the editor, rewrites land on the executed one
  (tracked by its position, with a nearest-occurrence fallback), not
  the first lookalike.
- The connection form's read-only control is the same switch as the
  Vim mode toggle instead of an ON/OFF button.
- Disconnect is a red broken-plug icon (the connect plug with a line
  through it) instead of a stop square.

## v0.1.7 - 2026-08-08

GitLab joins GitHub as an identity provider, with the same one-click
settings-sync journey; plus connection-form polish.

### GitLab

- Sign in with GitLab from Preferences: the same device flow, code
  presentation, and Keychain handling as GitHub, with `read_user`
  scope only.
- The zedb-settings probe, URL prefill, and one-time elevated
  "Create on GitLab" all follow the signed-in provider.
- Switching providers unlinks a synced repo into the URL field (old
  URL prefilled for easy relinking) and, when the new account has its
  own zedb-settings, offers a one-click "Switch now". Signing out
  never unlinks; sync is plain git and works without any identity.

### Connection form

- Tab and shift-Tab move between the fields.
- Double-clicking a field selects its whole value, and only the
  focused field ever shows a selection.

## v0.1.6 - 2026-08-08

Identity and settings release: optional GitHub sign-in, settings that
follow you through a git repo you own, and a command palette.

### GitHub sign-in

- Optional sign-in from Preferences via the OAuth device flow
  (`read:user` scope only; no client secret ships in the app). The
  one-time code is auto-copied to the clipboard and presented
  GitHub-style; the token lives in the macOS Keychain.
- Your avatar and name appear in Preferences and the title bar; the
  title-bar avatar is a shortcut back to Preferences.

### Settings sync

- Keep preferences, connections, and custom agents in a git repo you
  own. zeDB pulls on launch and window refocus, pushes on change,
  and a fresh machine inherits the repo's settings when it enables
  sync. Passwords never sync; they stay in this Mac's Keychain.
- Paste any git URL, or, signed in to GitHub, zeDB spots an existing
  zedb-settings repo using your own ssh key and prefills its URL,
  and can create a private one for you with a one-time elevated
  approval that is used once and never kept.

### Command palette and shortcuts

- cmd-shift-P opens a command palette: type to filter, arrows and
  Enter to run. Also in the new View menu.
- cmd-I toggles the agent pane from anywhere. cmd-N in the open pane
  starts a new thread with the last-used agent, which the empty pane
  also offers as a button with the agent's logo.

### Housekeeping

- The settings file is now `settings.json` (renamed from
  `preferences.json`, migrated automatically) and opens in your
  editor from the palette or the View menu.
- The fresh-install default query surveys the server's largest
  tables across all databases instead of assuming a specific
  database.

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
