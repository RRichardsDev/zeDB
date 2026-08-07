# Changelog

User-facing changes per release. Maintained as work happens: every
change worth a user's attention gets a line under Unreleased in the
same commit (or session) that makes it; cutting a release renames the
section to the version. Engineering internals live in docs/devlog.md,
not here. The release workflow publishes the version's section as the
GitHub release notes.

## Unreleased

- The update pill in the status bar wears the standard muted border,
  and the version number reads in plain white.
- Schema sidebar databases collapse again while a filter is applied;
  the chevron now always tells the truth. Editing the filter re-expands
  everything matching.
- Result grid columns are drag-resizable from the header dividers.
- Click a result column header to sort by it: the query's top-level
  ORDER BY is rewritten in the editor (on its own line) and just that
  statement re-runs on the server. Clicks cycle ascending, descending,
  and no sort; shift-click builds multi-column sorts with numbered
  arrows; the indicators always reflect the SQL that actually ran.
- Right-click a result header for an "Order by" submenu (Descending,
  Ascending, Clear); with shift held it becomes "Add to order by" for
  multi-column sorts. Header clicks now cycle descending first.
- Right-click a result header for "Filter...": any column with ten or
  fewer distinct values gets a checkbox list (a capped server probe
  checks, short-circuiting past ten; Enum variants come straight from
  the type), everything else a text field (plain text means
  contains, %patterns% and operators like > 10 pass through). Filters
  become managed conjuncts in the query's top-level WHERE, hand-written
  predicates survive (and simple ones light up the indicators and
  pre-fill the panel just like UI-made filters), filtered headers show a muted purple border, hovering any header
  summarises every active sort and filter, and the
  statement re-runs like header sorts do.
- Re-running keeps the previous results on screen (with a "running"
  hint in the header) until replacement rows stream in, and header
  tiles show a hand cursor. Rapid sort and filter changes coalesce
  into one run: the SQL and indicators update instantly, the query
  fires after a beat, and an in-flight run is cancelled and restarted
  rather than blocking the next change.
- Read/size progress in the status bar resets per statement instead of
  carrying a previous statement's totals; the vim mode chip reads
  "-- INSERT --" and lives in the bottom status bar (with the command
  line and recording indicator) instead of next to query output; NULL cells
  render muted and italic.

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
