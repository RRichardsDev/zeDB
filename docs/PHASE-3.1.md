# Phase 3.1 plan: the agent pane

Goal: a pane in zeDB where you start an AI thread with the coding
agents you already have, the way Zed's agent panel does. zeDB does not
ship a model, an API key store, or a login flow: it spawns the agent
CLI already installed on the machine (Claude Code, Codex, and anything
else that speaks the protocol) and talks to it over the Agent Client
Protocol, so the agent's own auth just works. What zeDB adds that a
generic editor cannot: the thread starts inside the open migration
repo with the fleet context (connection, schema, chain state) ready to
hand to the agent.

## Why ACP

The Agent Client Protocol is the open JSON-RPC-over-stdio protocol
Zed's external agents speak; Claude Code and Codex both have
maintained adapters. Speaking it buys every current and future adapter
for free, keeps zeDB out of the model/auth business entirely, and
keeps the trust boundary clean: the agent process runs as the user,
with the user's credentials, and zeDB is just the client rendering the
conversation and answering permission requests.

## Working rules

- Same as before: every milestone ends buildable, on main, demoable in
  30 seconds; riskiest unknowns first; devlog as we go; UI follows
  docs/UI-DESIGN.md and reuses existing primitives.
- zeDB never stores or proxies model credentials. If an agent is not
  authenticated, the fix is the agent's own login flow, surfaced, not
  wrapped.
- The agent acts through its own tools with its own permissions; the
  ACP permission-request flow renders in the pane and nothing is
  auto-approved by default. zeDB's own safety ladder is not reachable
  by the agent: an agent wanting to mutate a fleet gets to use the CLI
  under its own consent flags like any other process the user runs.
- Protocol client code lives in a new zedb-acp crate (headless,
  testable against a fake agent); the pane is a thin GPUI client of
  it, per the headless-core principle.

## Milestones

### M0. The protocol round trip

A headless ACP client: spawn an agent subprocess, initialize, open a
session, send a prompt, stream the response, cancel, and shut down
cleanly, proven by tests against a scripted fake agent and manually
against a real installed one. This is the riskiest unknown (protocol
fidelity, subprocess lifecycle, streaming), so it lands first and
alone.

Done when: a test drives a fake agent through the full lifecycle, and
a smoke binary can ask an installed Claude Code or Codex a question
and print the streamed answer.

### M1. The thread pane

The right-hand pane: start a thread, streamed markdown-ish rendering
(paragraphs, code blocks with the existing SQL highlighting where
tagged, tool-call lines shown compactly), a working input box, cancel,
and a new-thread picker listing the configured agents. One thread at a
time is fine; history within the session scrolls.

Done when: a conversation with a real agent reads comfortably in the
pane, long responses stream without jank, and cancel actually stops
the turn.

### M2. Agent discovery and settings

The picker finds what is installed: known adapters looked up on PATH
(and the standard install locations), plus user-configured entries in
preferences (name, command, args) behind an Add More Agents affordance.
Missing or unauthenticated agents show actionable states (not
installed, needs login via its own CLI) rather than errors.

Done when: a machine with Claude Code installed shows it ready with no
configuration, one without shows how to get it, and a custom
ACP-speaking command can be added by hand.

### M3. Fleet context via MCP

The zeDB difference, and the second protocol: ACP connects the pane to
the agent; MCP connects the agent to zeDB's data. A `zedb mcp` CLI
subcommand serves a stdio MCP server over the same zedb-core/zedb-ch
calls everything else uses, exposing READ-ONLY tools: fleet status,
chain and migration contents, schema of a database, drift findings,
rendered dry-runs. Useful on its own (register it with a terminal
agent today), and the pane registers it automatically: ACP sessions
carry an MCP server list, so every thread starts in the open repo
checkout with the zedb server pointed at the current connection. A
context chip for pasting a specific item stays as the manual fallback.

No write tools, deliberately: applies, rollbacks, and commits are not
reachable over MCP. An agent that wants to mutate uses the CLI with
its explicit consent flags like any other process the user runs, which
keeps the safety ladder meaningful.

The same server carries per-connection ClickHouse query tools: run a
query (read-only, enforced server-side with readonly=1 on every call
regardless of the connection's posture), list databases and tables,
describe an object. Built natively on zedb-ch's client rather than
spawning the official Python/uv MCP server: no Python prerequisite,
the connection credentials never leave zeDB's process, and the tools
version in lockstep with the app. The agent can therefore answer data
questions and iterate on SQL against the live connection by itself.

Read-only is not the same as harmless: an agent iterating on SQL can
scan a cluster to its knees. Agent queries carry server-side caps
(max_execution_time, max_result_rows, max_bytes_to_read) with
defaults tight enough to be safe on production, loosenable per
connection in its settings. And the thread always wears the
connection's environment tier badge (the dev/staging/production
colors), so there is never doubt about whose data the agent is
touching.

The pane is also ambiently context-aware: when a message is sent, the
app attaches a snapshot of what the user is looking at (which screen;
the selected database and its row status; drift findings already
fetched; an open action modal or authoring draft; the connection name
and tier) as a visible context block on the prompt, shown as a chip
and toggleable per thread, never invisible. "wait, what's wrong with
that db" then resolves against the screen, and the agent digs further
through the tools.

Done when: "why is zedb_kappa drifted?" is answerable by the agent
calling the drift and schema tools itself, asking "what's wrong with
this database?" while its row is selected in the matrix works without
naming it, the same server works under a terminal-run Claude Code
against the same repo, and nothing mutating is reachable over the
protocol.

### M4. The authoring and editor bridges

The agent fills the draft. Alongside the read-only fleet tools, pane
sessions get a draft surface hosted by the app itself (the CLI cannot
reach the editors): a propose_migration tool taking upgrade SQL,
rollback SQL, rollback class, and targeted flag. Calling it opens or
fills the authoring overlay, visibly, for the user to read, check, and
save exactly as if they had typed it. So "add column x to table y ON
CLUSTER, a String" in the thread lands as a ready draft in the
editors, with placeholders like ${db} and ${cluster} used correctly
because the fleet tools told the agent how this repo templates.

The query editor gets the same treatment: a propose_query tool drops
SQL into a query tab (new or current, visibly), and every SQL block in
the thread carries an insert-into-editor affordance, so a query the
agent wrote or already ran through its read-only tools is one click
from being yours. Write statements the agent cannot run itself arrive
exactly this way: drafted into the editor for the user to read and run
under their own connection posture and consent, never executed by the
agent.

This deliberately does not breach the no-write rule: a draft is
memory-only, and the existing gates (check against the pinned server,
explicit save, the ladder for any deploy) still stand between the
proposal and reality. The tool exists only while the pane session is
attached to the running app; a terminal agent using zedb mcp does not
get it.

The agent can also drive the view: a navigate tool (pane-only, like
the propose tools) switches between fleet, query editor, and
connections, selects a database row, or opens a migration in the
overlay. Navigation is workspace state the user can flip right back,
so it needs no consent machinery, but every use is narrated in the
thread (opened fleet view: zedb_kappa) so the UI never moves
unexplained. "Show me what's wrong with kappa" becomes: fetch drift,
navigate to the row, explain what is on screen.

Agents also edit repo files through their own tools, which today the
app only notices on manual refresh. While a pane session is attached,
the app watches the open repo: file changes refresh the chain, matrix
staleness, and git chip, and an authoring overlay showing a migration
that changed underneath says so instead of silently editing history
that moved.

Done when: the sentence above produces a correct two-sided draft in
the overlay in one round trip, an edited proposal can be re-proposed
without clobbering user edits silently (the overlay warns before
replacing a dirty draft), saving still requires the human check, a
query from the thread lands in a query tab in one click, a write
statement the agent could not run arrives as an editor draft rather
than an apology, and an agent editing a migration file on disk is
reflected in the matrix and git chip without a manual refresh.

### M5. Permissions and daily-driver polish

Also the focus primer: pane sessions get a lightweight AGENTS.md-style
context brief (what zeDB is, the open repo, the zedb mcp tools and
when to reach for them) so threads start oriented on this
application's world. Deliberately light-touch: the user knows whose
agent they are running and it must still do anything they ask; this
is orientation, not restriction.

The ACP permission flow rendered properly (what the agent wants to do,
approve or deny, per request), session restore for the pane across app
restarts (reopen the last thread's transcript read-only), keyboard
focus behavior that does not fight the query editor, and whatever the
first week of real threads surfaces, fixed or parked in IDEAS.md.

Done when: an agent editing migration files under the user's approval
feels safe and legible, and the pane earns a place in the daily layout
rather than being a demo.

## Order and dependencies

M0 → M1 → M2 and M3 in either order → M4 (needs M3's fleet tools so
proposals template correctly) → M5. Phase 3's M4/M5 (the live loop
walk and the soak) continue in parallel; nothing here blocks them.

## Explicitly not in Phase 3.1

- Shipping or bundling any model, key, or login flow.
- An agent-facing bridge into zeDB's write paths (applies, rollbacks).
  Agents mutate fleets the way any process does: through the CLI with
  explicit consent flags. Revisit only after the runners decision.
- Multi-thread management, thread sharing, or persistence beyond
  reopening the last transcript.
- Building our own agent. The pane is a client.

## Risks

- **Streaming markdown rendering.** Mostly defused: gpui-component
  (already a dependency) ships TextView with GFM Markdown rendering.
  What remains is integration cost: re-rendering efficiently as chunks
  stream in, and routing fenced SQL blocks through the existing
  highlighting. Budget for it in M1, not zero.
- **ACP is young.** The protocol and the adapters version fast;
  pinning the protocol version and testing against a scripted fake
  agent (M0) is the defense, and adapter breakage must degrade to an
  actionable message in the picker, not a dead pane.
- **Database content is untrusted model input.** Table comments,
  names, and data flow into the agent's context through the query and
  schema tools; a hostile comment is a prompt-injection path. The
  blast radius is already bounded by design (read-only tools,
  human-gated editors, no reachable write paths), and that boundary is
  the mitigation to preserve, not a filter to write.
- **Subprocess lifecycle.** Agent processes outliving the pane, dying
  mid-turn, or leaking on quit; M0's client owns spawn-to-shutdown and
  the tests must cover the ugly exits.

## Phase exit

Phase 3.1 is done when M5's done-condition holds in real use, at which
point the Phase 4 draft (embedded runners) is next in line for its M0
decision, informed by however much of the lifecycle the agent pane has
made people actually exercise.
