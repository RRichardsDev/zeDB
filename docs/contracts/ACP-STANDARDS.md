# ACP standards: what the in-app agent may and may not do

The agent pane runs the user's own ACP agent (Claude Code, Codex,
anything speaking ACP) with the user's own credentials, and zeDB
registers its MCP server into that session. This page is the contract
for that integration. `docs/contracts/PRODUCT-PRINCIPLES.md` states the spine
this derives from; when the two seem to disagree, the principles win.

## The rule of thumb

**The agent observes, diagnoses, drafts, and points. The user
decides, applies, and owns every write.**

## What the agent CAN do

- **Observe and diagnose** (read-only, capped):
  `fleet_status`, `drift`, `check_chain`, `regen_preview`,
  `list_migrations`, `migration_sql`, `dry_run`, `list_databases`,
  `list_tables`, `describe`, `run_query` (execution-time, row, and
  byte caps enforced server-side), `schema_search`, `lint_sql`,
  `cloud_context` (the active connection's ClickHouse Cloud
  control-plane picture: warehouse services with state, 30-day cost
  with a high-burn verdict, answered from the app's live state with
  freshness stated; on Cloud, `run_query`'s byte cap doubles as a
  per-query billing ceiling, and the reply says so). There is
  deliberately no wake or stop tool: service state changes cost money
  and stay with the user on the connection page.
- **Draft into the UI, writing nothing**: `propose_migration` fills
  the authoring overlay; `propose_query` fills a query editor tab.
  Both leave review, checks, and saving to the user.
- **Steer attention, not the wheel**: `navigate` switches views;
  `highlight_control` flashes a purple border on one fleet control
  (lock, upgrade_all, rollback, new_migration, regen, check_chain,
  verify_all) for a few seconds.
- **Edit repo files with its own tools, on request.** The migration
  repo is ordinary files, and the agent runs with the user's own
  file access; editing `current-state/` or drafting by hand is
  legitimate once the user chooses that route. The canonical Regen
  write stays a button.

## Etiquette: diagnose, explain, then ask

A failure is a question, not a cue to point. The expected flow:

1. Find out why with the read-only tools (`check_chain`,
   `regen_preview`, `drift`).
2. Explain the cause in plain words.
3. Offer the choice: highlight the control so the user acts, or do
   what the agent's own tools can legitimately do (e.g. update
   `current-state/` files to match `regen_preview`).

`highlight_control` fires only when the user picks that option or
asks where something is; never as a reflex on seeing a failure.

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
  A signpost that nothing renders counts as invisible: `highlight_control` refuses (with a navigation hint) when the target control is not on screen, and brings the fleet view up, narrated, for its toolbar controls.

## Session mechanics (for maintainers)

- The MCP server is this same executable in `zedb-mcp-serve` mode.
  ACP session registrations pass non-secret config in the server's
  environment (`ZEDB_MCP_*`), never argv; direct and legacy invocations may
  instead pass a 0600 delete-on-read config file. Database credentials never
  enter ACP session data or the agent-spawned child's environment. Connection-dependent read tools execute
  inside the app through the authenticated bridge, using a client forced
  read-only for that call.
- State follows the app live: the migration repo is resolved per call
  over the app bridge socket, so a repo attached, switched, or grown
  mid-session is seen on the next tool call. Do not capture app state
  at session start.
- App-hosted tools forward over a private unix socket in the user's data dir.
  Every request carries a random capability minted per session registration;
  a small bounded set of recent capabilities stays valid so MCP children of a
  reused agent process keep working, and anything older expires. Frames,
  queues, and waits are bounded; the reply deadline follows the tool (drift
  gets minutes, everything else seconds). Replies carry `isError` so failures
  surface in the agent, not in the app.
- The primer (`AGENT_PRIMER` in `features/agent/mod.rs`) is
  orientation, not restriction: it must never be the only thing
  standing between the agent and a write. Safety lives in what the
  tools can reach.
- Permission choices are request-scoped. ACP tool titles are human-readable
  text supplied by the agent, not stable authority identifiers, so zeDB never
  reuses a past approval merely because a later request has the same title.
  Every selected option must be one the current request actually offered, and
  cancelling a turn cancels all of that session's outstanding requests.

## Git credentials (the broker)

Repos opened through the picker's git route use HTTPS remotes; when
zeDB itself runs git against github.com/gitlab.com over HTTPS, it
sets `GIT_ASKPASS` to the zeDB binary in a hidden answer mode that
reads a stored elevated token from the Keychain. This is a deliberate
widening of the older "elevated tokens are never stored" stance:
the token is Keychain-held per host, revocable at the provider, and
never touches argv, env values, or `.git/config`. SSH URLs the user
types stay on their own git and keys. Planned evolution
(docs/wip/IRL-ISSUES.md): multi-account sign-in with the account bound
per cluster connection.

## Adding a tool: the checklist

1. Is it read-only against servers, or does it only fill UI the user
   reviews? If neither, it does not ship.
2. Does it answer for THIS app's state (connection, repo, screen),
   resolved live rather than captured at session start?
3. Are its caps enforced server-side (not by inspecting SQL)?
4. If it touches the window, is it narrated in the thread?
5. Update the primer's tool list, this page, and the tool definition
   in `crates/zedb-ch/src/mcp.rs` together.
