# `zedb-ch` security review

## Review metadata

| Field | Value |
| --- | --- |
| Status | Review complete, all findings remediated |
| Baseline commit | `171cb8551c7f68fbecb8b90a476ec0362909730e` |
| Branch | `security/zedb-ch-2026-aug` |
| Review host | Apple silicon, macOS 26.2.0 |
| Rust | `rustc 1.94.1 (e408947bf 2026-03-25)` |
| Cargo | `cargo 1.94.1 (29ea6fb6a 2026-03-24)` |
| Started | 2026-08-20 |
| Scope | `crates/zedb-ch`, plus directly relevant workspace boundaries |

The worktree also contains modifications to `AGENTS.md` and `CLAUDE.md` that
are outside this review and have not been touched. Review documentation added
during this run is not treated as a product-code change.

## Executive conclusion

`zedb-ch` passes the code-level security gate defined in `plan.md`. The review
confirmed four High findings, six Medium findings, and one Low workspace
handoff. All eleven findings are fixed. No Critical finding was established.
Remediation was completed on the dedicated security branch.

The highest-risk themes are trust before process execution, credentials sent
before transport trust is established, SQL text controlling admin credential
routing, and incomplete MCP response limits. These issues cross executable,
credential, authorization, and availability boundaries respectively, so they
should be fixed before treating the crate as production-secure.

Positive controls observed include normal Rust TLS certificate and hostname
validation on the secure native path, server-side `readonly = 2` on MCP
connections, no modeled or manually identified MCP route to `execute`, fixed
MCP tool dispatch, no shell interpretation of migration SQL, and no Rust
`unsafe` code in the crate.

## Evidence status

| Evidence | Status | Notes |
| --- | --- | --- |
| GitNexus index | Complete | Final PDG refresh: 28,449 nodes, 63,471 edges, 181 clusters, 300 flows |
| GitNexus taint findings | Complete with limitations | Zero modeled findings in `pin.rs`; callbacks, field flows, implicit flows, and eight incomplete CDGs remain gaps |
| Formatting | Passed | `cargo fmt --all --check` |
| Clippy | Passed | `cargo clippy -p zedb-ch --all-targets -- -D warnings` |
| Crate tests | Passed | 112 unit tests passed, one normally ignored live metadata test passed separately, and all 24 integration tests passed |
| Dependency advisory scan | Passed | No vulnerability, unsoundness, or yanked-package failure; configured unmaintained warnings remain visible |
| Manual threat model | Complete | P0 and P1 boundary reviews completed |
| Adversarial tests | Complete with limitations | Bounded local, loopback, filesystem, subprocess, and official metadata evidence; no production resources used |

## Threat model

### Assets and authority

- ClickHouse data and metadata, including production databases and migration
  tracking state.
- User, migration, and admin credentials.
- The local user's code-execution authority through cached ClickHouse
  executables and subprocesses.
- Migration repository integrity and the human approval boundary around apply,
  rollback, and lifecycle actions.
- Application availability when parsing server responses or serving MCP calls.

### Adversaries and failure sources

- A network observer or active attacker on a plaintext database connection.
- A malicious, compromised, redirected, or incorrectly published binary asset.
- A malicious migration repository that a user opens and later authorizes.
- A malicious or compromised ClickHouse endpoint returning crafted protocol
  data or very large results.
- An untrusted MCP caller operating within the documented read-only tool set.
- Ordinary misconfiguration that accidentally points credentials at the wrong
  endpoint.

### Principal trust boundaries

| Boundary | Input | Sensitive sink | Required control |
| --- | --- | --- | --- |
| Release network to local cache | Version and release bytes | Executable file and `Command::new` | Independent authenticity, size and path limits, atomic publication |
| Config to HTTP/native transport | URL, ports, username, password | Credential-bearing requests and handshakes | TLS-only credential policy and server identity before authentication |
| Migration repo to runner | SQL and parameters | Migration or admin `execute` | Explicit statement authorization and unambiguous credential routing |
| MCP caller to ClickHouse | Tool JSON and SQL | Read-only database query | Server read-only mode plus result, read, time, and output caps |
| Server response to decoder | Lengths, types, rows, errors | Heap allocation, recursion, logs | Checked arithmetic and byte, collection, nesting, and error limits |
| Local state to filesystem | Paths, audit data, cache data | User files and secrets at rest | Confined paths, restrictive modes, redaction, atomic writes |

The MCP write boundary is structurally intact in the reviewed code. The server
forces `read_only = true`, constructs a runner without admin or write
capability, and exposes fixed tool names. GitNexus found no modeled call path
from MCP `call_tool` to `ChClient::execute`. This is positive evidence, not a
proof against unmodeled flows.

## Findings register

| ID | Severity | Confidence | Status | Title | CWE |
| --- | --- | --- | --- | --- | --- |
| ZCH-001 | High | High | Fixed | Downloaded ClickHouse executable lacks independent authenticity verification | CWE-494 |
| ZCH-002 | High | High | Fixed | Native fallback and HTTP configuration can transmit database credentials in plaintext | CWE-319 |
| ZCH-003 | High | High | Fixed | Comment text can route arbitrary migration SQL through the admin client | CWE-863 |
| ZCH-004 | High | High | Fixed | MCP result limits do not bound generated response bytes | CWE-770 |
| ZCH-005 | Medium | High | Fixed | RowBinary and type decoding accept attacker-controlled allocation and recursion parameters | CWE-400 |
| ZCH-006 | Low | High | Fixed | Product graph enables vulnerable `h2` through feature unification | CWE-400 |
| ZCH-007 | Medium | Medium | Fixed | Unvalidated engine version controls executable cache paths | CWE-22 |
| ZCH-008 | Medium | High | Fixed | Binary downloads and archive extraction lack resource and path confinement | CWE-409, CWE-22 |
| ZCH-009 | Medium | High | Fixed | Tracking and audit persistence can retain non-password secrets | CWE-532 |
| ZCH-010 | Medium | High | Fixed | Regeneration paths can escape `current-state` and follow directory symlinks | CWE-22, CWE-59 |
| ZCH-011 | Medium | High | Fixed | Network and replay operations lack end-to-end deadlines | CWE-400 |

### ZCH-001: downloaded executable lacks independent authenticity verification

**Affected code:** `pin::download`, `pin::ensure_exact_binary`,
`pin::binary_reports_version` in `src/pin.rs`.

The crate downloads release bytes from a computed GitHub URL, writes or
extracts them, marks the result executable, publishes it to the cache, and then
executes it. The only content check asks the downloaded program to print its
version. That check is controlled by the program being checked and therefore
cannot establish authenticity or integrity.

An attacker able to replace the release response, compromise release
publication, or exploit redirect/origin trust obtains code execution as the
local user when the cache is verified. HTTPS narrows the attacker set but does
not provide artifact-level verification. The cached file is also executed
before a failed version check can quarantine it.

**Recommended fix:** distribute a trusted release manifest containing platform
artifact hashes, authenticate that manifest with a pinned project signing key
or equivalent release provenance, verify bytes before extraction or execution,
and publish the cache entry only after all checks pass. Quarantine and remove a
mismatched staging file without executing it a second time.

**Regression evidence required:** a substituted payload, modified archive, and
wrong hash must all fail before any process launch or final cache rename.

**Resolution:** fixed on `security/zedb-ch-2026-aug`. zeDB now embeds a
source-controlled trust manifest for all four supported 26.3.12.3 LTS assets.
The platform asset, version, channel, exact size, and SHA-256 digest must match
that manifest before a network request can supply executable bytes. GitHub's
release metadata is checked for agreement but is no longer the trust anchor.
Cached binaries are rehashed against the manifest before any process launch,
and fallback selection is limited to reviewed manifest versions. Unknown
versions fail closed until a reviewed manifest update is shipped. Regressions
prove substituted metadata and an executable cached payload fail before
execution; the live official release metadata also matched the checked-in
values during the final gate.

### ZCH-002: database credentials can cross plaintext transports

**Affected code:** `NativeClient::connect`, `connect_plain` in `src/native.rs`,
and `ChClient::ping` and `request` in `src/client.rs`.

Native connection discovery always includes plaintext candidates after TLS
candidates. An explicitly configured native port is tried first with TLS and
then as plaintext. KlickHouse sends the configured password in its client hello
during connection setup; zeDB verifies `serverUUID()` only after that handshake.
Consequently, the identity check occurs after a plaintext endpoint has already
received the credential. HTTP endpoints also accept `http://` and attach
`X-ClickHouse-Key` to both queries and `/ping`.

Reqwest follows redirects by default. Its cross-origin sensitive-header list
does not include the ClickHouse-specific credential headers, so a redirect can
forward `X-ClickHouse-Key` and `X-ClickHouse-User` to another authority.

**Recommended fix:** make encrypted transport mandatory whenever a credential
is present, unless the destination is an explicitly accepted loopback-only
development profile. Never try plaintext as fallback from TLS. Disable
redirects for credential-bearing requests or implement a policy that removes
all ClickHouse credential headers before any redirect. Do not authenticate
`/ping`.

**Regression evidence required:** a loopback plaintext listener must observe no
password after a failed TLS attempt, and cross-origin redirects must receive no
ClickHouse credential headers.

**Resolution:** fixed on `security/zedb-ch-2026-aug`. Remote plaintext HTTP is
rejected before request construction. Reqwest redirects are disabled for the
client, `/ping` sends no ClickHouse identity or password headers, and plaintext
native candidates are generated only for an explicit loopback HTTP endpoint.
Five focused regression tests cover endpoint classification, refusal before
connect, redirect refusal, unauthenticated ping, and native candidate policy.
All 89 library tests and strict Clippy pass after the change.

**Amendment (2026-08-21, owner-accepted risk):** the blanket refusal of remote
plain HTTP broke real deployments; many ClickHouse clusters, including the
owner's, expose no TLS endpoint at all. The owner accepted the residual
plaintext-transport risk for explicitly configured `http://` URLs: typing the
scheme is the deliberate opt-in, and the app offers no silent downgrade from
`https://`. Retained protections: URLs embedding credentials are rejected,
redirects stay disabled, `/ping` stays unauthenticated, and the native
transport still tries TLS first, offering plaintext candidates only when the
HTTP endpoint is itself explicit plain HTTP (never for an `https://` config).
A network attacker on the path of an `http://` connection can still read and
modify traffic including credentials; that is the accepted residual risk.
Regression tests were updated to the amended policy.

### ZCH-003: comments can select the admin executor

**Affected code:** `needs_admin` in `src/runner/status.rs` and
`Runner::apply_sql` in `src/runner/execution.rs`.

`needs_admin` matches the word `DEFINER` anywhere in lines that are not `--`
comments. `apply_sql` treats a match as authorization to select the admin
client. Block comments and string literals are not excluded. For example, an
otherwise ordinary statement containing `/* DEFINER */` is routed through the
admin connection when one is configured.

This makes migration text a confused-deputy control over credential selection.
A malicious repository still requires the user to authorize the migration, but
the approved migration authority and the admin authority are intentionally
separate. The classifier collapses that separation and can run unrelated SQL
with broader grants.

**Recommended fix:** do not grant authority based on lexical hints in untrusted
SQL. Use a strict allowlisted operation model or require explicit, separately
approved metadata for elevated statements. Parse and validate the exact
supported statement forms at the final routing point.

**Minimal local reproduction:** assert that
`needs_admin("SELECT 1 /* DEFINER */")` returns true, then verify with mock
executors that the admin client is selected. No live database is necessary.

**Resolution:** fixed on `security/zedb-ch-2026-aug`. Generic `DEFINER`
matching has been removed from credential selection. Admin routing now requires
one of the existing allowlisted statement forms at the start of the SQL body.
Focused regressions prove comments, string literals, view `DEFINER` clauses,
and block-comment text cannot select the admin executor.

### ZCH-004: MCP response bytes are not bounded

**Affected code:** `ChClient::query_guarded` and `request` in `src/client.rs`,
and MCP query dispatch in `src/mcp.rs`.

The agent path sets `max_result_rows`, `max_bytes_to_read`, and an execution
time. It does not set ClickHouse `max_result_bytes`, and the HTTP path calls
`Response::bytes()` before decoding. A query can generate a very large value in
one row without reading a comparable number of source bytes. The row cap and
read-byte cap therefore do not bound either the server response or the local
allocation.

An MCP caller can use the intended read-only query tool to exhaust application
memory. This violates the ACP contract that agent queries have server-enforced
row and byte caps even though no database mutation is possible.

The same boundary has no per-session rate, concurrency, or cumulative scan
budget. Its default read cap is 10 GiB per call, so repeated or parallel calls
can create substantial cluster load and cloud cost. Non-query tools such as
`migration_sql` and `dry_run` also return repository-derived strings without an
output limit, and the JSONL request reader has no maximum line size.

**Recommended fix:** set `max_result_bytes` and a strict overflow mode at the
server, enforce a client-side streamed byte ceiling before materialization,
bound request lines and error bodies, cap serialized MCP output independently,
and add per-session concurrency plus cumulative resource budgets.

**Regression evidence required:** a one-row generated string exceeding the
limit must terminate predictably without buffering the full response.

**Resolution:** fixed on `security/zedb-ch-2026-aug`. Guarded queries now set
ClickHouse `max_result_bytes` and stream response chunks through the same
client-side ceiling before RowBinary decoding. All tool text is written through
a bounded formatter, and final JSON serialization is capped independently with
worst-case escaping accounted for. Stdio requests are limited to 1 MiB and MCP
responses to 4 MiB; the app bridge uses the same bounded framing. Oversized
JSONL messages are drained without retaining the remainder, so the next request
can still be processed.

### ZCH-005: response decoding lacks allocation and nesting bounds

**Affected code:** `Reader`, `StreamingDecoder`, and `read_value` in
`src/rowbinary.rs`, plus recursive type parsing in `src/types.rs`.

Server-controlled varuint values are converted to `usize` and passed to
`Vec::with_capacity` for column, array, and map counts. Strings and incomplete
streaming rows can retain arbitrary response bytes. Nested arrays, maps,
nullable values, low-cardinality values, and tuples have no explicit depth
limit. `Reader::take` calculates `self.pos + n` without checked addition.
`DateTime64` accepts a precision above 9 and computes `9 - precision`, which
can panic in checked builds.

**Recommended fix:** introduce decoder-wide byte, element, column, row, and
nesting budgets; use checked conversions and addition; validate semantic type
parameters while parsing; and make all malformed inputs return structured
errors rather than panic or allocate from declared lengths.

**Resolution:** fixed on `security/zedb-ch-2026-aug`. Materialized responses,
retained streaming data, columns, header strings, individual values,
collections, cumulative decoded values, tuple width, and recursive nesting now
have explicit limits. Varuint conversion and reader offsets use checked
arithmetic, collection capacities are not preallocated directly from large
wire counts, and zero-column trailing data cannot cause a non-progress loop.
The type parser rejects excessive input and nesting plus unsafe `FixedString`,
`DateTime64`, and Decimal parameters before value decoding. Seven adversarial
regressions cover declared lengths, overflow, non-progress, numeric semantics,
and recursive type inputs. All 102 library tests, `tests/checks.rs`, formatting,
and strict Clippy pass.

### ZCH-006: product graph enables vulnerable `h2`

`cargo audit` reports RUSTSEC-2026-0258 for `h2 0.4.15`. A standalone
`cargo tree -p zedb-ch` does not contain `h2`, so this is not a vulnerability in
the crate's isolated feature graph. The workspace-wide graph does contain the
path `zedb-ch -> reqwest -> hyper -> h2` because product dependencies unify
Reqwest's HTTP/2 features. A peer can exploit the affected HTTP/2 behavior with
unbounded empty DATA frames. RustSec identifies `h2 0.4.16` as patched.

This is recorded as a product/workspace handoff rather than an open standalone
crate finding. The distinction matters for ownership, but not for the shipped
application's dependency exposure.

**Recommended fix:** update the locked dependency graph to `h2 >= 0.4.16`, run
the full workspace tests, and confirm no compatibility constraint keeps the
vulnerable version selected.

**Resolution:** fixed on `security/zedb-ch-2026-aug`. The workspace lockfile now
selects `h2 0.4.16`, the first patched release identified by RustSec. The
feature-unified dependency path was rechecked after the update.

### ZCH-007: engine version controls executable cache paths

`binary_path(version)` joins an unvalidated configuration or server-derived
version directly under the cache directory. Absolute components and traversal
components can escape the intended version directory. `cached_binary` then
executes a file at the resulting path if it exists. Exploitation requires a
matching local file or an additional file-placement capability, which lowers
confidence and severity, but the path and process boundary is unsafe.

**Recommended fix:** parse versions into a strict ClickHouse version type,
reject separators and traversal components, and verify the resolved parent is
the owned cache directory before any filesystem or process operation.

**Resolution:** fixed on `security/zedb-ch-2026-aug`. Every cache lookup and
automatic acquisition entry point now accepts exactly four nonempty decimal
version components before constructing or executing a cache path. Regression
tests reject separators, parent traversal, missing components, and extra
components.

### ZCH-008: binary acquisition lacks size and archive confinement

The downloader trusts `Content-Length` for initial allocation and then buffers
the entire response without a maximum. Linux invokes the system `tar` over the
whole archive into a staging directory without first validating entry paths,
link targets, entry count, or expanded size. A malicious or corrupted asset can
cause memory exhaustion, disk exhaustion, or archive path escape depending on
the platform tar implementation and archive contents.

**Recommended fix:** stream to a size-limited temporary file, validate the
compressed and expanded sizes, inspect each archive entry, reject absolute and
parent paths plus links, extract only the expected regular file, and use an
owned random temporary directory.

**Resolution:** fixed on `security/zedb-ch-2026-aug`. Download bodies are
streamed to random temporary files with an exact declared-size check and a 1
GiB ceiling. Rust archive handling rejects non-normal paths and links at the
expected binary path, caps entry count and expanded bytes, and copies only the
single expected regular file. Focused tests cover exact-file extraction and
link rejection.

### ZCH-009: tracking and audit persistence can retain secrets

`Runner::record` stores every resolved migration parameter in the ClickHouse
tracking table except parameters whose names contain `password`. API keys,
tokens, private keys, and arbitrary secrets with other names are persisted in
plaintext. Error redaction uses the same name heuristic before the error is
stored remotely and appended to the local audit log.

The local audit logger also creates `audit.jsonl` without setting an explicit
restrictive file mode. On the review host the file mode is `0644`, although a
`0700` parent currently protects it. The process-global native pool separately
keeps the full password in long-lived string keys, including failed connection
entries, increasing in-memory retention.

**Recommended fix:** do not persist parameter values by default. Use an
explicit allowlist of non-sensitive audit fields, provide a separately reviewed
opt-in for values that are operationally necessary, create secret-bearing
local state with mode `0600`, and replace plaintext password pool keys with a
nonreversible keyed digest or structured identity that can be purged.

**Resolution:** fixed on `security/zedb-ch-2026-aug`. Tracking rows no longer
persist any resolved template values. Durable errors redact every custom
parameter value, while retaining only the built-in database and cluster
identifiers. Audit entries reduce configured endpoints to scheme, host, and
port, and audit files are forced to mode `0600` on Unix even when an older file
already exists. Native pool keys replace passwords and driver setting values
with a process-salted SHA-256 session digest. Four focused regressions cover
redaction, endpoint minimization, file permissions, and pool-key retention.
All 109 library tests, `tests/checks.rs`, formatting, and strict Clippy pass.

### ZCH-010: regeneration is not confined to `current-state`

**Affected code:** `RepoConfig::load` in `zedb-core/src/repo/config.rs`,
`Regenerator::new` and `place` in `src/regen/tracking.rs`, and `write_tree` and
`collect_sql` in `src/regen/tree.rs`.

Scope names are deserialized as unrestricted map keys. Regeneration embeds the
selected scope name into each relative output path, then joins it to the
repository's `current-state` directory. Absolute paths and parent components
are not rejected, and the joined path is not checked for containment before
directory creation or file writing. A malicious repository can therefore make
an authorized regeneration write migration-derived SQL outside
`current-state`, potentially outside the repository.

The stale-file collector recursively follows paths for which `is_dir()` is
true. This follows directory symlinks. Any `.sql` file reached outside
`current-state` is treated as stale unless its relative string happens to be a
generated key, and `remove_file` can delete it.

**Recommended fix:** validate scope names at configuration load as one safe
path component, reject absolute paths, separators, `.` and `..`, and verify
every output path remains beneath an opened repository-owned directory. Walk
with symlink metadata and never recurse through symlinks. Use atomic,
no-follow file creation for regeneration output where supported.

**Regression evidence required:** absolute and parent-containing scope names
must be rejected, and a symlink beneath `current-state` must neither be
traversed nor alter its target.

**Resolution:** fixed on `security/zedb-ch-2026-aug`. Repo loading now requires
scope names to match `[a-z0-9_]+`. Regeneration verifies every parent remains
beneath `current-state`, rejects symlinked roots, directories, and files, and
publishes changed files through a temporary file in the verified directory.
Stale-file collection uses symlink metadata and fails closed without recursing.
Regression tests cover traversal names, normal atomic replacement, directory
symlink refusal, and preservation of the external target.

### ZCH-011: operations lack end-to-end deadlines

`ChClient::new` configures a connection timeout but no whole-request timeout.
`request`, streaming export, `/ping`, binary download, and native connection
setup can therefore remain pending when a peer accepts a connection and then
stalls. The MCP execution-time setting is enforced by a cooperating ClickHouse
server and does not bound a slow or malicious transport. `LocalReplay` also
waits for the child process without a deadline, so agent-accessible chain checks
can remain stuck on a wedged executable.

**Recommended fix:** define separate connect, first-byte, idle, total request,
download, and subprocess deadlines; propagate cancellation; terminate and reap
children on expiry; and ensure partial files are removed or clearly marked.

**Regression evidence required:** bounded loopback slow-peer tests and a
nonterminating disposable child must all return within their configured
deadlines without leaking a task, socket, process, or partial output.

**Resolution:** fixed on `security/zedb-ch-2026-aug`. The shared ClickHouse
HTTP client has a five-minute whole-request deadline. Native connection setup
has a one-minute total deadline, materialized query and execute operations have
five-minute deadlines, and long-lived native streams close their socket after
one idle minute. Release metadata calls are limited to 30 seconds, downloads
have a 20-minute total deadline plus a 30-second idle deadline, and cached
binary verification, smoke replay, SQL replay, and formatting run through a
shared bounded process helper. On timeout the helper kills and reaps the child
after concurrently draining its pipes. Slow-peer and nonterminating-child
regressions pass, as do all 105 library tests, `tests/checks.rs`, formatting,
strict Clippy, and the cargo-deny advisory gate.

## Evidence gaps and residual uncertainty

- The initial GitNexus index had no PDG taint layer and was three commits stale.
- The refreshed PDG skipped control-dependence graphs for
  `mcp.rs::tool_definitions` and `regen/tracking.rs` around line 308 because an
  exit node was not reverse reachable. CFG and reaching-definition layers were
  still built.
- GitNexus reported no modeled taint paths. Its current analysis does not fully
  model closures, callbacks, property or field propagation, and implicit flows.
- The two integration tests that stalled at baseline passed during the final
  complete crate run, along with every other integration test.
- Platform-specific behavior has initially been observed only on Apple silicon
  macOS.
- No adversarial test executed a newly downloaded binary. Pre-execution hash
  rejection was verified with a disposable executable payload, and existing
  trusted binary-backed tests passed.

## Prioritized remediation order

1. ZCH-002 is closed: credential-bearing plaintext and unsafe redirect paths
   are now refused, with loopback-only development compatibility.
2. Close ZCH-001 and ZCH-008 together with authenticated release metadata,
   pre-execution verification, bounded streaming, and confined extraction.
3. Close ZCH-003 by replacing SQL-text privilege inference with explicit,
   separately authorized admin operations.
4. Close ZCH-004 with server result-byte limits, client byte ceilings, bounded
   JSONL and tool output, and session-level concurrency and cost controls.
5. Close ZCH-010 before running regeneration on repositories that are not fully
   trusted by validating scope components and refusing symlink traversal.
6. ZCH-005 is closed with decoder budgets, checked arithmetic, and semantic
   type validation. Add transport and subprocess deadlines for ZCH-011 next.
7. Minimize secret persistence for ZCH-009 and update the product lockfile for
   the ZCH-006 workspace handoff.

After each slice, run targeted security regressions, the crate quality gates,
and GitNexus impact and change checks required by `AGENTS.md`. The gate should
be rerun only after all High findings are closed or explicitly accepted by the
owner with a documented expiry.

## Review log

### 2026-08-20: baseline opened

- Recorded the exact source commit and local Rust toolchain.
- Confirmed that no pre-existing product-code modifications were present in the
  worktree.
- Started a GitNexus refresh with program-dependence analysis enabled.

### 2026-08-20: baseline and dependency evidence

- Refreshed GitNexus at the baseline with PDG enabled.
- Passed formatting, Clippy with warnings denied, 84 unit tests, the checks
  integration test, and one binary-backed import test.
- Interrupted two binary-backed integration tests after prolonged silence and
  retained them as evidence gaps.
- Audited 847 locked packages with `cargo-audit 0.22.2`. The standalone crate
  graph contains none of the three reported vulnerable packages. The shipped,
  feature-unified product graph enables RUSTSEC-2026-0258 through Reqwest and
  Hyper. Two `quick-xml 0.30` advisories are elsewhere in the workspace and not
  in either examined `zedb-ch` target graph.
- Recorded unmaintained `paste 1.0.15` through `klickhouse 0.15.3` as
  informational supply-chain debt, with no demonstrated exploit.

### 2026-08-20: P0 boundary review

- Confirmed independent artifact authentication is absent before ClickHouse
  binary execution.
- Confirmed native TLS failure can fall back to an authenticated plaintext
  handshake, and HTTP credentials have no TLS-only or safe-redirect policy.
- Confirmed comment or string content containing `DEFINER` controls selection
  of the admin executor.
- Confirmed the MCP runner itself is read-only and has no modeled call path to
  `ChClient::execute`, but its generated result bytes are not bounded.
- Confirmed regeneration accepts scope names as path prefixes without
  confinement and follows directory symlinks during stale-file collection.

### 2026-08-20: ZCH-002 remediated

- Created branch `security/zedb-ch-2026-aug` with the review documents and
  existing unrelated worktree edits preserved.
- Disabled redirects for ClickHouse HTTP requests and removed credentials from
  `/ping`.
- Refused non-TLS HTTP except for literal loopback endpoints and removed
  plaintext native candidates outside the same loopback development posture.
- Added five focused security regressions. All 89 library tests and strict
  Clippy pass.

### 2026-08-20: binary acquisition hardened

- Added strict four-component version validation before cache path use or
  process execution, closing ZCH-007.
- Replaced whole-body buffering and system `tar` extraction with bounded
  streaming, SHA-256 verification, random staging files, and confined Rust
  archive extraction, closing ZCH-008.
- Required GitHub's exact release asset metadata and digest before publication,
  but retained ZCH-001 as mitigated because GitHub marks the upstream release
  mutable and its digest is not an independent trust anchor.
- Updated the workspace lockfile from vulnerable `h2 0.4.15` to patched
  `h2 0.4.16`, closing ZCH-006.
- Passed 93 library tests apart from the intentionally ignored live metadata
  check, plus the four loopback transport regressions with socket permission,
  and strict Clippy.

### 2026-08-20: ZCH-003 remediated

- Removed generic `DEFINER` keyword matching from migration credential
  selection.
- Kept admin routing only for the narrow statement families already identified
  as requiring elevated grants.
- Added regressions for comments, literals, view clauses, and legitimate
  allowlisted statements. Focused tests and strict Clippy pass.

### 2026-08-20: ZCH-004 remediated

- Added server and client result-byte ceilings to every guarded agent query.
- Bounded MCP input frames, tool text construction, app-bridge replies, and
  final serialized JSON responses.
- Added adversarial tests for oversized HTTP bodies, oversized JSONL recovery,
  UTF-8-safe truncation, and JSON escape expansion.
- Passed 97 library tests with one intentionally ignored live-network test,
  strict Clippy, and the cargo-deny advisory gate.

### 2026-08-20: ZCH-005 remediated

- Added explicit byte, column, collection, cumulative value, tuple-width, and
  nesting limits to materialized and streaming RowBinary decoding.
- Replaced unchecked wire-length conversions and reader offset addition with
  checked failures, and rejected zero-column trailing data without looping.
- Added type-string size and recursion limits plus semantic validation for
  fixed strings, high-precision timestamps, and decimals.
- Added seven adversarial regressions. All 102 library tests, the checks
  integration test, formatting, and strict Clippy pass.

### 2026-08-20: ZCH-011 remediated

- Added a five-minute whole-request deadline to the shared ClickHouse HTTP
  client, covering ping, materialized queries, streaming queries, and exports.
- Added a one-minute total native connection deadline, five-minute native
  query and execute deadlines, and a one-minute idle deadline that closes a
  stalled long-lived native stream.
- Added a shared bounded subprocess runner for replay, formatting, binary
  verification, and smoke checks. It drains pipes concurrently, then kills and
  reaps a child that exceeds its deadline.
- Added bounded silent-peer and nonterminating-child regressions. All 105
  library tests, the checks integration test, formatting, strict Clippy, and
  cargo-deny pass.

### 2026-08-20: ZCH-009 remediated

- Removed resolved template values from tracking rows and broadened durable
  error redaction to every custom parameter.
- Reduced audit endpoints to scheme, host, and port, and forced audit files to
  mode `0600` on Unix, including pre-existing files.
- Replaced plaintext native pool passwords and setting values with a
  process-salted SHA-256 session digest.
- Added four focused regressions. All 109 library tests, the checks integration
  test, formatting, and strict Clippy pass.

### 2026-08-20: ZCH-001 remediated

- Added a source-controlled trust manifest for the four supported platform
  assets of ClickHouse 26.3.12.3 LTS.
- Required manifest membership before download or cached execution, required
  live release metadata to agree with the manifest, and limited fallback
  selection to reviewed versions.
- Added regressions proving substituted metadata and executable cache bytes are
  rejected before process launch. The live official metadata check passed.

### 2026-08-20: final gate passed

- Passed 112 unit tests and all 24 integration tests, including the import and
  lifecycle cases that stalled at baseline. The normally ignored official
  metadata test also passed when run explicitly.
- Passed formatting, strict Clippy, and cargo-deny. Unmaintained transitive
  dependencies remain warnings under the documented CI policy.
- Refreshed GitNexus with PDG, found no modeled taint finding in `pin.rs`, and
  confirmed the accumulated security branch affects the expected critical
  acquisition, transport, runner, MCP, decoder, and regeneration flows.

### 2026-08-20: dependency policy added

- Added a cargo-deny CI job over all features and the supported macOS and Linux
  targets. Reachable vulnerabilities, unsoundness, and yanked crates fail CI;
  unmaintained transitive crates remain visible as warnings.
- Confirmed the old vulnerable `quick-xml` lockfile entry is not reachable from
  any workspace target and the patched `h2 0.4.16` product path is selected.

### 2026-08-21: post-review adjustments (owner decisions)

- Amended ZCH-002: explicit `http://` endpoints are accepted again as an
  owner-accepted risk (many clusters, including the owner's, have no TLS).
  URL-embedded credentials, redirects, authenticated ping, and TLS-to-plain
  native downgrade for `https://` configs all remain refused. Native plaintext
  candidates now key off the explicit plaintext scheme, not loopback.
- Restored the HTTP fallback for native read failures in `ChClient::query`.
  Only allowlisted read statements route natively, so a replay is harmless;
  server errors still do not fall back, and mutating statements still never
  route natively.
- Gave exports their own 24-hour total deadline plus a 60-second idle stall
  detector, replacing the general five-minute whole-request deadline that
  aborted large exports. Added a stalled-export regression test.

### 2026-08-20: ZCH-010 remediated

- Restricted scope names to one lowercase ASCII path component at repo load.
- Added path-containment and symlink checks to regeneration writes, diffs, and
  stale-file collection, with temporary-file publication for changed SQL.
- Added regressions proving traversal is rejected and symlink targets are not
  modified.

## External references

- [OWASP Application Security Verification Standard 5.0](https://owasp.org/www-project-application-security-verification-standard/)
  supplied the applicable transport, input, file, and resource-control
  checklist.
- [MITRE CWE](https://cwe.mitre.org/) supplied the weakness classifications.
- [RustSec](https://rustsec.org/) and the
  [upstream `h2` advisory](https://github.com/hyperium/hyper/security/advisories/GHSA-q83h-524g-xf6h)
  supplied dependency advisory evidence.
- [ClickHouse client guidance](https://clickhouse.com/integrations/clickhouse_client)
  documents secure native connections on port 9440.
- [ClickHouse HTTP interface documentation](https://github.com/ClickHouse/clickhouse-docs/blob/main/docs/integrations/interfaces/http.md)
  documents `max_result_bytes`, which is absent from the guarded MCP query.
