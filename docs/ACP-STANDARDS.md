# ACP standards: what the in-app agent may and may not do

The agent pane runs the user's own ACP agent (Claude Code, Codex,
anything speaking ACP) with the user's own credentials, and zeDB
registers its MCP server into that session. This page is the contract
for that integration. `docs/PRODUCT-PRINCIPLES.md` states the spine
this derives from; when the two seem to disagree, the principles win.

## The rule of thumb

**The agent observes, diagnoses, drafts, and points. The user
decides, applies, and owns every write.**

## What the agent CAN do

- **Observe and diagnose** (read-only, capped):
  `fleet_status`, `drift`, `check_chain`, `regen_preview`,
  `list_migrations`, `migration_sql`, `dry_run`, `list_databases`,
  `list_tables`, `describe`, `run_query` (execution-time, row, and
  byte caps enforced server-side), `schema_search`, `lint_sql`.
- **Draft into the UI, writing nothing**: `propose_migration` fills
  the authoring overlay; `propose_query` fills a query editor tab.
  Both leave review, checks, and saving to the user.
- **Steer attention, not the wheel**: `navigate` switches views;
  `highlight_control` flashes a purple border on one fleet control
  (lock, upgrade_all, rollback, new_migration, regen, check_chain,
  verify_all) for a few seconds. Pointing at a button the agent
  cannot press is the intended pattern, e.g. a failed `check_chain`
  caused by stale current-state highlights `regen`.

## What the agent CANNOT do, by construction

- **No server writes, ever.** The MCP connection is forced read-only
  regardless of the app connection's posture; there is no tool that
  applies a migration, rolls back, stamps tracking, or runs DDL/DML.
  This is not a policy the agent follows; it is a path that does not
  exist.
- **No unlocking.** The write lock is per-session human consent; the
  agent can highlight it, never toggle it.
- **No pressing its own suggestions.** Drafts land in editors and
  overlays; the Save/Run/Apply click is the user's.
- **No cluster substitution.** Anything about the app's connection is
  answered only through the zedb tools; other configured ClickHouse
  MCP servers point at unrelated clusters and are never a stand-in.
  If the zedb tools are missing, the agent says so and stops.
- **No invisible UI changes.** Every bridge action that touches the
  window (navigate, propose_*, highlight_control) is narrated in the
  thread transcript.

## Session mechanics (for maintainers)

- The MCP server is this same executable in `zedb-mcp-serve` mode.
  Config travels in the server's environment (`ZEDB_MCP_*`), never
  argv (world-readable) and never a file (agent runtimes respawn MCP
  servers; a delete-on-read file killed respawns and silently cost
  sessions their tools).
- State follows the app live: the migration repo is resolved per call
  over the app bridge socket, so a repo attached, switched, or grown
  mid-session is seen on the next tool call. Do not capture app state
  at session start.
- App-hosted tools forward over a unix socket in the user's data dir;
  replies carry `isError` so failures surface in the agent, not in
  the app.
- The primer (`AGENT_PRIMER` in `features/agent/mod.rs`) is
  orientation, not restriction: it must never be the only thing
  standing between the agent and a write. Safety lives in what the
  tools can reach.

## Adding a tool: the checklist

1. Is it read-only against servers, or does it only fill UI the user
   reviews? If neither, it does not ship.
2. Does it answer for THIS app's state (connection, repo, screen),
   resolved live rather than captured at session start?
3. Are its caps enforced server-side (not by inspecting SQL)?
4. If it touches the window, is it narrated in the thread?
5. Update the primer's tool list, this page, and the tool definition
   in `crates/zedb-ch/src/mcp.rs` together.
