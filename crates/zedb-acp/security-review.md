# `zedb-acp` security review

## Review metadata

| Field | Value |
| --- | --- |
| Status | Complete, nine confirmed findings remediated and verified |
| Baseline commit | `47feb08c8f6ef7b0ad1a9ac10533364ed66b46cf` |
| Branch | `security/zedb-acp-2026-aug` |
| Review host | Apple silicon, macOS 26.2.0 |
| Rust | `rustc 1.94.1 (e408947bf 2026-03-25)` |
| Cargo | `cargo 1.94.1 (29ea6fb6a 2026-03-24)` |
| Started | 2026-08-21 |
| Scope | `crates/zedb-acp`, direct app integration, and relevant configuration boundaries |

The review starts from the completed `zedb-ch` security branch. Its MCP
implementation is treated as previously reviewed; only the ACP handoff and
reachability assertions are in this scope.

## Evidence status

| Evidence | Status | Notes |
| --- | --- | --- |
| Formatting | Passed | `cargo fmt --all --check` |
| Clippy | Passed | `cargo clippy -p zedb-acp --all-targets -- -D warnings` |
| Crate tests | Passed | Six unit and five lifecycle tests |
| Reachable dependency tree | Passed | Small direct graph: Tokio, Serde, dirs, thiserror, and their transitives |
| Dependency advisory policy | Passed with warnings | Configured cargo-deny command passes; known unmaintained workspace GUI transitives remain warnings |
| Raw lockfile audit | Informational failure | Two vulnerable `quick-xml 0.30.0` entries are present but unreachable from workspace targets under the checked host graph |
| GitNexus PDG | Complete with limitations | Final refresh: 28,948 nodes, 64,717 edges, 187 clusters, 300 flows; no modeled findings in ACP or agent integration files |
| Manual threat model | Complete | Acquisition, credential handoff, permission, sync, protocol, bridge, and persistence boundaries traced |
| Adversarial tests | Passed locally | Oversized frames, dead child, cancellation, permission round-trip, credentialless MCP forwarding, and complete serial integration suites; no production account or database used |

## Initial authority model

### Assets

- The local user's process and filesystem authority.
- Migration repositories and any uncommitted work inside them.
- Database endpoints, identities, passwords, and reachable server authority.
- The structural review boundary around read-only and propose-only agent tools.
- Permission decisions and persistent grants.
- Conversation, tool-call, error, and screen-context data.
- Application availability and UI integrity.

### Actors and failure sources

- The user operating the agent pane and permission cards.
- An installed, cooperative ACP agent and its provider.
- A compromised or substituted ACP adapter package.
- Malicious or confused model output driving an otherwise valid agent.
- A user-configured custom executable.
- Another local process running as the same or a different user.
- Stale, concurrent, or restarted sessions sharing one agent process.
- Malformed or unexpectedly large protocol traffic.

### Principal boundaries

| Boundary | Input | Sensitive sink | Required control |
| --- | --- | --- | --- |
| Registry and PATH to adapter | Package name, executable path, cached bytes | `Command::new` | Reviewed version, explicit trust, stable executable identity |
| App to agent process | Environment, cwd, MCP configuration, prompts | Third-party process | Least disclosure, bounded messages, explicit capability handoff |
| Agent stdout to app | JSON-RPC frames and stderr | Heap, queues, transcript, UI | Framing limits, strict envelopes, queue bounds, safe rendering |
| Agent request to permission decision | Session, tool call, title, options | Persistent or one-time grant | Stable identity, session binding, offered-option validation, cancellation |
| MCP child to app bridge | Unix socket request | Live app state and propose-only UI actions | Caller authentication, session binding, restrictive mode, limits and deadline |
| Agent events to disk | Messages, tool details, errors | Transcript and debug files | Redaction, restrictive mode, bounded retention, atomic writes |

## Findings register

| ID | Severity | Confidence | Status | Title | CWE |
| --- | --- | --- | --- | --- | --- |
| ZACP-001 | High | High | Fixed | Built-in agents execute mutable npm adapter releases | CWE-494 |
| ZACP-002 | High | High | Fixed | Database credentials are disclosed to the agent process | CWE-200, CWE-668 |
| ZACP-003 | High | High | Fixed | Settings sync imports executable definitions and permission grants | CWE-15, CWE-829 |
| ZACP-004 | High | High | Fixed | Persistent permission grants use agent-controlled display identity | CWE-863 |
| ZACP-005 | Medium | High | Fixed | ACP framing, queues, and request waits are unbounded | CWE-400 |
| ZACP-006 | Medium | High | Fixed | Permissions are not bound to the live session or cancelled turn | CWE-863 |
| ZACP-007 | Medium | High | Fixed | App bridge is locally unauthenticated, permissive, and unbounded | CWE-306, CWE-400 |
| ZACP-008 | Medium | High | Fixed | Agent transcripts and diagnostic logs are permissive and unbounded | CWE-532, CWE-400 |
| ZACP-009 | Low | High | Fixed | CI omits `zedb-acp` Clippy and tests | CWE-693 |

### ZACP-001: built-in agents execute mutable npm adapter releases

**Affected code:** `discovery::discover_known` in `src/discovery.rs`.

Both built-in agents launch `npx -y` with a bare package name. The npm `latest`
tag therefore decides which adapter bytes execute with the local user's
authority each time the cache misses or npm refreshes resolution. Discovery
checks whether `npx` and the underlying agent appear installed, but establishes
no adapter version or content trust. The Claude adapter alone published many
releases during the month of review, confirming that the selected code is not a
stable part of a reviewed zeDB release.

**Required fix:** pin reviewed exact adapter versions, make upgrades explicit
source changes, and add tests proving discovery emits exact specifications.
Document that npm integrity and registry trust remain the acquisition anchor
unless adapters are bundled with independently reviewed hashes later.

**Resolution:** built-ins use exact reviewed adapter versions and discovery
tests assert the emitted package specifications. npm and its registry remain a
documented residual acquisition dependency; adapter upgrades now require a
source change and review.

### ZACP-002: database credentials are disclosed to the agent process

**Affected code:** app `Workspace::agent_mcp_server_config`,
`AgentConnection::new_session`, and app `run_mcp_serve`.

The app places `ZEDB_MCP_URL`, user, and password in `McpServerConfig.env`, then
serializes that structure as the `session/new` request written to the agent's
stdin. The agent runtime must read this value to spawn the MCP child. The
password is therefore disclosed to the third-party agent before the child
constructs a read-only `McpServer`. Environment privacy from unrelated users
does not protect a secret deliberately sent to the parent process.

An agent or compromised adapter can use the credential with its own network
tools, outside the MCP server's `read_only = true` construction and query caps.
If the database identity has write grants, the documented structural no-write
boundary does not hold.

**Required fix:** never place database credentials in ACP session data. Keep
the live database client in the app and expose only a bounded, authenticated,
read-only capability to the MCP child. Possession of that capability may permit
the documented read-only operations, but must not reveal the underlying
credential or reach a write client.

**Resolution:** ACP session configuration contains no ClickHouse URL, user, or
password. Connection-dependent tools cross an authenticated bridge and execute
inside the app with a freshly cloned configuration forced to `read_only`. A
regression proves `run_query` works through the bridge without a credential in
the MCP child.

### ZACP-003: settings sync imports executable definitions and grants

**Affected code:** `sync::sanitized_preferences`, `sync::apply_preferences`,
app `settings_sync_apply`, and agent registry startup.

The sync payload copies `Preferences` after stripping only repo and sync paths.
It retains `custom_agents`, `last_agent`, and `agent_always_allow`. Applying a
pulled payload replaces those local values. Opening the pane then resolves the
imported commands, and the error-bar convenience path can automatically start
the remembered agent. Imported permission keys can also enable automatic
approval.

This turns a settings repository and its writers into a source of local
executable configuration and authorization state. Those are machine-local
trust decisions, not portable presentation preferences.

**Required fix:** strip all agent executable, selection, and approval fields
from sync payloads and retain the local values when applying pulled settings.
Add regressions for both outbound redaction and inbound preservation.

**Resolution:** outbound settings clear custom commands, remembered selection,
and legacy permission grants. Applying pulled settings preserves all three
local values. Core regressions cover both directions.

### ZACP-004: persistent grants use agent-controlled display identity

**Affected code:** app `Workspace::agent_apply_event_for` and
`Workspace::agent_answer_permission`.

Persistent permission keys are `agent display name|tool title`. Custom agents
may duplicate a built-in display name, and ACP defines the title as
human-readable text supplied by the agent. The title is not a stable authority
identifier. A later request with the same title but different input or tool can
reuse the grant. Auto-approval then chooses the first option whose ID contains
`always`, otherwise any kind containing `allow`, otherwise the first option.
It does not require the exact `allow_always` kind originally selected.

**Required fix:** remove unsafe cross-request automatic approval unless a
stable, client-verifiable operation identity exists. At minimum, validate every
selected option against the current offered set and its exact ACP kind. Existing
title-based grants must not continue granting authority.

**Resolution:** title-based automatic approval and persistence were removed.
Every answer is checked against the exact option IDs offered by that live
request; unknown selections resolve as cancelled.

### ZACP-005: ACP resources and waits are unbounded

**Affected code:** `AgentConnection::spawn`, `request`, and `route_message`.

Stdout and stderr use `BufReadExt::lines`, which allocates until a newline.
Outgoing and event channels are unbounded, the pending request map has no cap,
and `request` has no deadline. A silent or flooding adapter can grow memory or
leave initialization, session creation, and prompts pending indefinitely.
Trimming transcript entry count occurs after already materializing and logging
agent data, and a single assistant entry can grow without a byte limit.

**Required fix:** bounded frame readers, channel capacity and backpressure,
pending-request ceilings, per-operation deadlines, output limits, and focused
silent-peer and oversized-frame regressions.

**Resolution:** ACP stdout and stderr frames, outgoing and event queues,
pending requests, and request waits are bounded. Oversized input is drained
without retaining the frame; oversized output is rejected before enqueue.
Lifecycle coverage includes dead children, cancellation, and oversized output.

### ZACP-006: permissions are not bound to the live session or cancelled turn

**Affected code:** `route_message`, app `Workspace::agent_apply_event_for`, and
`Workspace::agent_cancel`.

ACP permission requests carry a required `sessionId`, but events are routed to
whichever visible thread shares the process cache key. The received session ID
is not compared with the live thread. Reused agent processes host successive
sessions, so a stale request can be displayed or auto-approved in another
session. Cancelling a turn sends `session/cancel` but does not respond
`cancelled` to its outstanding permission requests, despite the ACP v1
requirement.

**Required fix:** bind permissions and session updates to an exact session,
reject or cancel mismatches, and drain the current session's outstanding
permission responders on turn cancellation and replacement.

**Resolution:** all session updates retain their required `sessionId` and the
app accepts them only for the matching live thread. Permission requests use the
same equality check. Turn cancellation drains every pending permission as an
ACP cancelled outcome; replacing a thread drops its responders, which the ACP
router also converts to cancelled.

### ZACP-007: app bridge is unauthenticated, permissive, and unbounded

**Affected code:** app `Workspace::agent_ensure_bridge`.

The bridge binds a predictable PID-named socket under a persistent `0755`
directory. Live and stale socket files observed on the review host are `0755`.
There is no application token or peer check. Any process able to connect can
read `repo_root` and `cloud_context`, navigate the app, highlight controls, or
place attacker-selected SQL into query and migration editors. The server uses
an unbounded `read_line`, unbounded request channel, and no request deadline.
Old socket files accumulate after process exit.

**Required fix:** use a private per-process directory or `0700` bridge
directory, force socket mode `0600`, authenticate requests with a random
capability, bound frames and queues, impose deadlines, and remove the socket on
shutdown or startup cleanup.

**Resolution:** the bridge directory and socket are forced to `0700` and
`0600`; each session registration rotates a 256-bit capability and queued work
is revalidated against the current token. Frames, active connections, queues,
reads, and app replies are bounded or timed. The live socket is removed when
the pane state drops.

### ZACP-008: transcript and diagnostic persistence is permissive and unbounded

**Affected code:** app `persist_transcript` and `agent_log`.

The app persists message text, tool titles, permission titles, errors, raw tool
updates, permission input, and stderr. Observed files are mode `0644`; no code
forces a private mode on files or their parent. The debug JSONL file only
appends and has no rotation or byte cap. Agent-controlled material can expose
queries, paths, tool parameters, errors, or secrets to other local users and
consume disk indefinitely.

**Required fix:** force `0600`, write the transcript atomically, cap persisted
entry and byte counts, rotate or truncate the debug log, and avoid raw sensitive
tool payloads where structured minimal logging is enough.

**Resolution:** transcript and diagnostic directories and files are created
private from their first write. Transcripts use bounded entries and atomic
replacement. Diagnostics rotate at 1 MiB, cap individual structured data, and
record sizes or identifiers instead of prompt text, stderr, errors, raw tool
payloads, or permission input.

### ZACP-009: CI omits `zedb-acp`

**Affected code:** `.github/workflows/ci.yaml`.

The Linux quality job runs strict Clippy and tests for core, ClickHouse, and CLI
crates but not ACP. The app job compiles ACP transitively on macOS, but does not
run its unit or lifecycle tests and does not establish strict lint coverage for
its targets.

**Required fix:** add `zedb-acp` to Linux strict Clippy and test commands.

**Resolution:** Linux CI now includes every ACP target in strict Clippy and its
unit and lifecycle tests in the test job.

## Review log

### 2026-08-21: baseline established

- Created `security/zedb-acp-2026-aug` at baseline commit `47feb08`.
- Read the product and ACP contracts before reviewing integration behavior.
- Passed formatting, strict crate Clippy, four unit tests, and four integration
  tests.
- The configured dependency advisory gate passed with documented unmaintained
  warnings. The raw lockfile audit reported two vulnerable old `quick-xml`
  entries, but `cargo tree` found no reachable workspace target using that
  version on the review host.
- Began a PDG-enabled GitNexus refresh and manual P0 boundary tracing.

### 2026-08-21: P0 findings confirmed

- Completed the PDG refresh: 28,648 nodes, 64,007 edges, 185 clusters, and 300
  flows. GitNexus reported no modeled ACP or agent-integration taint findings;
  closure, callback, property, implicit, and unmodeled Rust I/O flows remain
  important false-negative classes.
- Traced the credential handoff from the live connection through
  `agent_mcp_server_config`, ACP `session/new`, and `run_mcp_serve`.
- Compared permission behavior with the official ACP v1 schema, including the
  required session ID, exact option kinds, and cancellation response rule.
- Confirmed local modes on the review host: the app data and MCP directories
  and socket files are `0755`; transcript and debug-log files are `0644`.
- Confirmed nine findings: four High, four Medium, and one Low.

### 2026-08-21: remediation and verification complete

- Pinned both built-in adapters and isolated machine-local agent execution and
  authorization settings from sync.
- Removed title-based permission reuse, bound updates and permissions to the
  current session, and made cancellation resolve outstanding requests.
- Added bounded ACP framing, queues, pending work, deadlines, transcript and
  diagnostic persistence, and an authenticated private bridge with per-session
  capability rotation.
- Kept live ClickHouse credentials inside the app process. The child forwards
  the seven connection-dependent read tools to an app-owned read-only MCP
  server and never receives the underlying URL, user, or password.
- Passed formatting, strict Clippy across all affected crates, six ACP unit and
  five lifecycle tests, all core tests, 99 app unit and two app integration
  tests, the full serial ClickHouse suite, and the configured advisory gate.
- Refreshed the PDG index after remediation. Targeted transport, bridge, and
  event-handler scans reported no modeled taint findings, subject to the
  documented closure, property, and implicit-flow limitations.
