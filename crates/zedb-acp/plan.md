# `zedb-acp` security review plan

## Purpose

Review `zedb-acp` and its direct app integration as a hostile-process boundary.
The integration discovers and launches third-party agent programs, exchanges
ACP JSON-RPC over stdio, forwards MCP server configuration, renders agent
output, records transcripts, and answers permission requests.

The review must answer three questions:

1. Can an agent, adapter, local process, or crafted protocol message gain more
   authority than the user granted?
2. Can credentials or sensitive conversation data cross an unintended process
   or persistence boundary?
3. Can malformed, malicious, or unexpectedly large traffic compromise app
   availability or confuse sessions and permission decisions?

The run is a review first. Findings are established with code traces and
adversarial evidence before remediation. Each product-code fix receives a
GitNexus impact check and a focused regression test.

## Security contracts

- `docs/contracts/PRODUCT-PRINCIPLES.md`: agents remain an optional accelerator
  for a hands-on product. The user drives server and migration writes.
- `docs/contracts/ACP-STANDARDS.md`: agent-facing database tools are read-only
  or propose-only, app state is resolved live, and the agent cannot unlock or
  reach server-write controls through the integration.
- `AGENTS.md`: run GitNexus impact analysis before changing a symbol, warn on
  High or Critical blast radius, and run `detect_changes()` before commit.

Use OWASP ASVS 5.0 where applicable, CWE identifiers for findings, RustSec for
the locked dependency graph, and the ACP v1 schema as the protocol authority.
This is not a claim of standards compliance.

## Scope and priority

| Priority | Surface | Primary files | Main risks |
| --- | --- | --- | --- |
| P0 | Adapter discovery and process launch | `src/discovery.rs`, `src/lib.rs` | Package substitution, path confusion, inherited authority, unsafe environment, incomplete cleanup |
| P0 | MCP and credential handoff | app `controller.rs`, `controller/bridge.rs` | Credential disclosure to the agent, capability theft, cluster substitution, read-only bypass |
| P0 | Permission mediation | `src/lib.rs`, `src/protocol.rs`, app `controller/events.rs`, `controller/messages.rs` | Cross-session approval, forged identity, stale or replayed requests, unsafe persistent grants |
| P1 | ACP framing and routing | `src/lib.rs`, `src/protocol.rs` | Oversized frames, malformed envelopes, ID confusion, unbounded queues, missing deadlines |
| P1 | Local app bridge | app `controller/bridge.rs` | Unauthenticated local callers, socket permissions, oversized frames, UI manipulation, data disclosure |
| P1 | Transcript and diagnostic persistence | app `mod.rs`, view helpers | Secret leakage, permissive modes, unbounded growth, terminal or rendering abuse |
| P2 | Custom-agent configuration | `src/discovery.rs`, app `controller/registry.rs`, core `preferences.rs` | Ambiguous parsing, executable replacement, identity collision, unsafe defaults |

`zedb-ch` MCP internals are out of scope except for assertions at the handoff.
The completed `zedb-ch` review remains authoritative for its own implementation.

## Run sequence

### 0. Establish a reproducible baseline

- Record commit, branch, host, Rust and Cargo versions, and worktree state.
- Run formatting, strict `zedb-acp` Clippy, crate tests, the configured
  `cargo-deny` advisory gate, and a reachable dependency tree.
- Refresh GitNexus with `--pdg`; record modeled findings and known analysis
  gaps.
- Record missing CI coverage, skipped tests, unavailable agents, and any live
  checks that would require user credentials.

### 1. Build the threat model and authority map

- Inventory the app user, installed agent CLI, downloaded adapter, MCP child,
  other same-user processes, migration repository, and remote model provider.
- Trace agent discovery through spawn, initialize, session creation, MCP
  registration, prompts, updates, permissions, cancellation, and shutdown.
- Trace database and app capabilities from their source to every process that
  can observe or invoke them.
- Distinguish a cooperative agent, a compromised adapter, malicious model
  output, a malicious local process, and ordinary stale-session behavior.

### 2. Review acquisition, launch, and lifecycle safety

- Verify every built-in executable and adapter has an explicit version and
  trust decision. Review registry access, update behavior, redirects, cache
  use, and execution after failed acquisition.
- Review path search, symlinks, file ownership, custom commands, arguments,
  working directories, environment inheritance, stdio, process groups,
  cancellation, timeouts, and child reaping.
- Prove a dead or silent adapter cannot leave requests or permission prompts
  pending indefinitely.

### 3. Review protocol and availability safety

- Validate JSON-RPC version, ID types, response shape, method shape, session
  identifiers, option identifiers, and protocol-version negotiation.
- Bound stdout, stderr, individual frames, JSON nesting, outgoing messages,
  pending requests, event queues, permission queues, and transcript growth.
- Test malformed UTF-8, partial frames, oversized frames, duplicate IDs,
  unknown responses, notifications arriving outside a prompt, and floods.

### 4. Review permissions and cross-session isolation

- Bind each permission request to the correct process, session, tool-call ID,
  visible operation, and offered option set.
- Verify the selected option was actually offered and that cancellation drains
  every outstanding permission request as ACP requires.
- Review persistent grants for stable, non-agent-controlled identity and exact
  authority. Test duplicate display names, title reuse, option reordering,
  adapter restart, stale sessions, and concurrent sessions.

### 5. Review MCP, bridge, and persistence boundaries

- Determine which process receives database credentials and whether it can use
  them outside the read-only MCP server.
- Authenticate and permission the local app bridge, bind calls to a live
  session, and bound request, reply, queue, and wait time.
- Confirm bridge tools cannot unlock, apply, roll back, wake, stop, or otherwise
  reach a server-write sink.
- Review transcript and debug-log content, size, rotation, file mode, atomicity,
  symlink behavior, and error redaction.

### 6. Remediate and close

- Record severity, confidence, affected code, exploit narrative, recommended
  fix, regression evidence, and resolution for every finding.
- Run impact analysis before each symbol change and warn before High or
  Critical blast radius edits.
- Add a changelog entry for every user-visible hardening change; use the devlog
  for internal-only work.
- Pass formatting, strict Clippy, crate and relevant app tests, dependency
  policy, adversarial regressions, refreshed PDG analysis, and GitNexus
  `detect_changes()` before closing or committing.

## Outcome

The 2026-08-21 review confirmed and remediated four High, four Medium, and one
Low finding. Exact evidence, resolutions, residual dependencies, and
verification results are recorded in `security-review.md`. No production agent
account or database was used.
