# `zedb-ch` security review plan

## Purpose

Review `zedb-ch` as a hostile-input boundary, not only as a database driver. The
crate accepts connection details, credentials, SQL, migration repositories, MCP
requests, ClickHouse responses, and downloaded binaries. It can read and write
local files, start processes, and execute statements against ClickHouse.

The review must answer two questions:

1. Can an untrusted input cross a trust boundary with more authority than
   intended?
2. Can malformed, malicious, or unexpectedly large input compromise
   confidentiality, integrity, availability, or tenant isolation?

The run is a review first. Fixes are separate, individually scoped changes with
their own regression tests. A clean scanner result is never treated as proof of
safety.

## Security contracts

The following repository contracts are review requirements:

- `docs/contracts/ACP-STANDARDS.md`: MCP tools must remain read-only or
  propose-only, agent queries must have server-enforced caps, and the agent must
  never reach server writes or the write lock.
- `docs/contracts/FORMAT.md`: migration numbering, target restrictions,
  identifier validation, rollback classes, tracking records, and generated
  state must remain trustworthy under adversarial repository contents.
- `AGENTS.md`: run GitNexus impact analysis before changing any symbol, warn
  before HIGH or CRITICAL changes, and run `detect_changes()` before a commit.

Use OWASP ASVS 5.0 as a coverage checklist where its controls apply, CWE IDs to
classify root causes, and RustSec to check the locked Rust dependency graph.
This is not a claim of ASVS compliance.

## Scope and priority

| Priority | Surface | Primary files | Main risks |
| --- | --- | --- | --- |
| P0 | Binary acquisition and execution | `src/pin.rs`, `src/replay.rs`, `src/ephemeral.rs` | Supply-chain compromise, missing authenticity checks, unsafe redirects, archive traversal, symlink races, command execution, insecure permissions |
| P0 | Database transport and credentials | `src/client.rs`, `src/client/*`, `src/native.rs`, `src/native/*` | Plaintext credential exposure, weak TLS identity, SSRF, cross-connection pooling, secret leakage, missing time or size limits |
| P0 | Agent-facing MCP boundary | `src/mcp.rs`, `src/mcp/handlers.rs` | Read-only bypass, cap bypass, tool confusion, excessive data exposure, malformed JSON-RPC, write-capability reachability |
| P0 | SQL execution and privilege separation | `src/runner*`, `src/lifecycle.rs`, `src/checks.rs`, `src/verify.rs`, `src/schema/*`, `src/workload.rs` | SQL injection in constructed statements, identifier or literal confusion, multi-statement bypass, admin credential misuse, incorrect target selection |
| P1 | Untrusted response decoding | `src/rowbinary.rs`, `src/types.rs`, `src/native/codec.rs`, `src/explain.rs` | Panic, integer overflow, oversized allocation, decompression amplification, recursion or CPU exhaustion, silent type confusion |
| P1 | Migration repository and local persistence | `src/regen*`, `src/replay.rs`, `src/schema_cache.rs`, `src/schema_cache/*` | Path traversal, symlink attacks, non-atomic writes, unsafe permissions, malicious TOML or SQL, stale or cross-connection cache data |
| P2 | Query editing and schema intelligence | `src/schema_intelligence*` | Parser differentials, Unicode boundary errors, generated SQL corruption, denial of service |

Tests, `Cargo.toml`, `Cargo.lock`, workspace CI, and release scripts are in scope
when they provide evidence about these surfaces. Other crates are in scope only
at a boundary with `zedb-ch`; findings there are handed to that crate's review.

## Run sequence

### 0. Establish a reproducible baseline

- Record the commit, toolchain, target platform, enabled Cargo features, and
  dirty-worktree state.
- Refresh the GitNexus index with PDG enabled and record index freshness. Run
  its repository-wide taint report, while documenting known false-negative
  classes such as property flows, callbacks, and unmodelled Rust APIs.
- Run the existing quality gates without modifying code:
  `cargo fmt --all --check`,
  `cargo clippy -p zedb-ch --all-targets -- -D warnings`, and
  `cargo test -p zedb-ch`.
- Record skipped integration tests, unavailable ClickHouse binaries, network
  dependencies, and platform-specific paths. A skipped test is not a pass.
- Create the findings register described below before reviewing code.

Exit criterion: the exact review baseline and all unavailable evidence are
written down.

### 1. Build the threat model and data-flow map

- Inventory assets: ClickHouse data, admin and migration credentials, cloud
  endpoints, migration history, downloaded executables, local caches, and MCP
  responses.
- Inventory actors: local user, migration author, compromised migration repo,
  malicious or misconfigured ClickHouse server, network attacker, untrusted MCP
  caller, and compromised download origin.
- Draw trust boundaries for HTTP, native TCP/TLS, MCP stdio, filesystem reads
  and writes, subprocesses, Keychain-fed credentials, and the `zedb-core`
  configuration boundary.
- Trace every entry point to its security-sensitive sinks with GitNexus and
  manual call-chain review. Mark where validation, authorization, size limits,
  timeouts, escaping, and secret redaction occur.
- Write abuse cases before judging controls. Include cross-connection data
  leakage, agent-triggered mutation, credential forwarding to an attacker,
  tampered binary execution, malicious migration paths, and response-driven
  memory exhaustion.

Exit criterion: each P0 surface has assets, attacker capabilities, entry
points, sinks, controls, and at least one abuse case.

### 2. Review dependencies and the executable supply chain

- Run `cargo audit` against `Cargo.lock` and record every vulnerability,
  warning, exception, and dependency path.
- Inspect `cargo tree -p zedb-ch --edges normal,build` and duplicate security
  libraries. Confirm TLS and certificate features are intentional.
- Review lockfile and CI behavior for reproducible dependency resolution.
- Follow every `pin.rs` path from version discovery through URL selection,
  redirects, download, unpacking, permission changes, cache publication,
  version checking, and execution.
- Verify authenticity and integrity, archive entry confinement, HTTPS-only
  policy, redirect policy, file ownership and mode, symlink resistance,
  atomicity, concurrent download behavior, cleanup, and rollback after failure.
- Treat a downloaded executable that is checked only by asking it for its
  version as untrusted until independent authenticity is established.

Exit criterion: every executable byte has a documented trust decision from
origin to execution.

### 3. Review transport, authentication, and connection isolation

- Trace construction and validation of HTTP and native endpoints. Test URL
  scheme, authority, embedded credentials, IPv4 and IPv6, local and metadata
  addresses, redirects, and guessed native ports.
- Confirm certificates, hostnames, protocol versions, and HTTP/native server
  identity are validated consistently. Identify every path that permits
  plaintext credentials or plaintext database traffic.
- Confirm passwords and tokens cannot appear in debug output, errors, URLs,
  logs, query IDs, pool keys, persisted caches, environment snapshots, or test
  artifacts.
- Review timeout and limit coverage for connect, request, response body,
  streaming, decompression, cancellation, retry, and idle pooled connections.
- Prove pool keys and cache keys cannot reuse an authenticated connection or
  schema snapshot across users, databases, TLS modes, or distinct servers.
- Review fallback from native transport to HTTP for duplicate execution,
  inconsistent authorization, and confused server identity.

Exit criterion: credentials never cross an untrusted or plaintext boundary,
and connection reuse has a documented isolation key.

### 4. Review SQL, authorization, and the MCP contract

- Classify SQL as user-authored, migration-authored, or internally constructed.
  Do not report intentional raw SQL execution as injection without an authority
  boundary crossing.
- Inventory every interpolation of identifiers, literals, settings, database
  names, cluster names, repository IDs, and parameters. Verify ClickHouse-aware
  quoting and strict identifier validation at the final sink.
- Test comments, quoting, backticks, escapes, Unicode, semicolons, nested
  queries, `FORMAT`, `INTO OUTFILE`, table functions, external URLs, and
  ClickHouse settings that can alter resource or filesystem behavior.
- Verify server-side `readonly` and resource caps on every agent query path.
  Prove there is no MCP call chain to `execute`, apply, rollback, stamp,
  unlocking, service wake/stop, or another server mutation.
- Verify tool argument schemas, unknown fields, numeric bounds, output caps,
  error redaction, app-bridge identity, and per-call live state resolution.
- Trace admin and migration credentials separately through lifecycle and runner
  code. Confirm privilege escalation cannot occur because a statement is
  misclassified or routed through a fallback.
- Test fleet target discovery, exclusions, targeted allow lists, tracking
  database exclusion, cluster expansion, and failure recovery under adversarial
  names and responses.

Exit criterion: each write-capable sink has an explicit human-authorized path,
and the MCP surface has no route to one.

### 5. Review filesystem, process, parser, and availability safety

- Enumerate all paths derived from repo content, configuration, server output,
  environment, and network responses. Test absolute paths, `..`, separators,
  Unicode normalization, symlinks, hard links, and time-of-check/time-of-use
  races.
- Verify sensitive files use restrictive permissions and durable atomic writes.
  Confirm cleanup cannot remove paths outside an owned temporary directory.
- Review every process launch for executable selection, fixed arguments,
  environment inheritance, current directory, stdio handling, termination,
  timeout, and child cleanup. Confirm no shell parses attacker-controlled text.
- Audit RowBinary, native codec, type, JSON, and TOML parsing for checked
  arithmetic, allocation limits, nesting limits, EOF handling, panics, and
  error-message data disclosure.
- Add or run focused fuzz and property tests for parsers, statement splitting,
  quoting, endpoint parsing, archive handling, and MCP argument decoding.
- Exercise cancellation, partial responses, malformed lengths, very large
  declared values, compressed responses, slow peers, concurrent callers, and
  disk-full or permission-denied failures.

Exit criterion: untrusted input has explicit byte, row, nesting, time, and path
boundaries before expensive or privileged work.

### 6. Record and triage findings

Create `security-review.md` in this crate when the review begins. Each finding
must contain:

- stable ID, title, status, severity, confidence, CWE, and affected files or
  symbols;
- asset, attacker prerequisites, trust-boundary crossing, and impact;
- exact code path and supporting evidence;
- minimal safe reproduction using local or ephemeral resources only;
- existing mitigations and why they are sufficient or insufficient;
- recommended fix, regression test, residual risk, and owner decision.

Severity guide:

- **Critical**: plausible unauthenticated or low-privilege compromise of
  credentials, executable integrity, or arbitrary production data.
- **High**: cross-connection data exposure, MCP-to-write escape, SQL authority
  escalation, or reliable remote denial of service.
- **Medium**: meaningful exploit requiring strong prerequisites, limited data
  exposure, or recoverable local integrity loss.
- **Low**: defense-in-depth gap with small practical impact.
- **Informational**: verified hardening or maintainability improvement without a
  demonstrated security impact.

Severity is based on impact and realistic exploitability, not scanner wording.
Suspected issues stay marked unconfirmed until reproduced or proven by code.

### 7. Remediate in controlled slices

- Obtain the symbol's GitNexus upstream impact report before editing it. Stop
  and warn before HIGH or CRITICAL blast-radius changes.
- Fix one finding or one tightly coupled group at a time. Add the failing
  security regression test first where practical.
- Prefer removing authority, narrowing inputs, and enforcing limits at the
  privileged sink over blacklist-based detection.
- Preserve intentional raw-query and migration capabilities while preventing
  confused-deputy paths.
- Run targeted tests, the crate quality gates, and relevant ephemeral
  ClickHouse tests after each slice.
- Add a changelog entry for user-visible security behavior changes, or a
  `docs/devlog.md` entry for internal-only hardening.
- Run GitNexus `detect_changes()` before every commit and verify only expected
  symbols and execution flows changed.

Exit criterion: each accepted finding is fixed with a regression test, formally
accepted with rationale and expiry, or tracked as unresolved with an owner.

### 8. Close the review

- Re-run dependency audit, static checks, taint analysis, focused adversarial
  tests, crate tests, and integration tests from a clean baseline.
- Revisit every abuse case and every accepted-risk decision.
- Search for sibling implementations that could retain the same weakness.
- Summarize fixed, accepted, and open findings, test coverage added, evidence
  gaps, and the next review date.
- Extract reusable checks into CI only after their false-positive behavior and
  ownership are understood.

The crate passes this review only when no Critical or High finding remains open,
all P0 boundaries have direct evidence, and every unavailable test or analysis
is explicitly recorded as residual uncertainty.

## Safety rules for the run

- Never use production credentials, production migration repositories, or a
  production ClickHouse service for adversarial testing.
- Use loopback ephemeral servers and disposable temporary repositories. Keep
  network-reachable tests opt-in.
- Do not execute a downloaded artifact until its trust decision has been
  reviewed. Preserve suspicious artifacts only as hashes and metadata unless a
  controlled analysis environment is explicitly approved.
- Do not put credentials, bearer tokens, raw sensitive rows, or exploitable
  proof-of-concept payloads into logs, commits, screenshots, or public issues.
- Stop a test that escapes its temporary directory, contacts an unexpected
  host, or consumes uncontrolled memory, CPU, disk, or network bandwidth.
- Coordinate disclosure before publishing any finding that could endanger
  existing users.

## Reference baseline

- [OWASP ASVS 5.0](https://owasp.org/www-project-application-security-verification-standard/)
- [MITRE CWE](https://cwe.mitre.org/)
- [RustSec Advisory Database](https://rustsec.org/)
