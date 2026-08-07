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
