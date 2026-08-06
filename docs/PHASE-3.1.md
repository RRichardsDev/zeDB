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

Done when: "why is zedb_kappa drifted?" is answerable by the agent
calling the drift and schema tools itself, the same server works under
a terminal-run Claude Code against the same repo, and nothing mutating
is reachable over the protocol.

### M4. Permissions and daily-driver polish

The ACP permission flow rendered properly (what the agent wants to do,
approve or deny, per request), session restore for the pane across app
restarts (reopen the last thread's transcript read-only), keyboard
focus behavior that does not fight the query editor, and whatever the
first week of real threads surfaces, fixed or parked in IDEAS.md.

Done when: an agent editing migration files under the user's approval
feels safe and legible, and the pane earns a place in the daily layout
rather than being a demo.

## Order and dependencies

M0 → M1 → M2 and M3 in either order → M4. Phase 3's M4/M5 (the live
loop walk and the soak) continue in parallel; nothing here blocks
them.

## Explicitly not in Phase 3.1

- Shipping or bundling any model, key, or login flow.
- An agent-facing bridge into zeDB's write paths (applies, rollbacks).
  Agents mutate fleets the way any process does: through the CLI with
  explicit consent flags. Revisit only after the runners decision.
- Multi-thread management, thread sharing, or persistence beyond
  reopening the last transcript.
- Building our own agent. The pane is a client.

## Phase exit

Phase 3.1 is done when M4's done-condition holds in real use, at which
point the Phase 4 draft (embedded runners) is next in line for its M0
decision, informed by however much of the lifecycle the agent pane has
made people actually exercise.
