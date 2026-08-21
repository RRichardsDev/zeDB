# `zedb-app` security review

## Review metadata

| Field | Value |
| --- | --- |
| Status | Complete; eight confirmed findings remediated and one accepted |
| Baseline commit | `0ff2654ae2489a0370fcac311591c16f50e6b5f3` |
| Branch | `security/zedb-cli-2026-aug` |
| Review host | Apple silicon, macOS 26.2 |
| Started | 2026-08-21 |
| Scope | `crates/zedb-app` and direct dependency calls whose security contract is chosen by the app |

The `zedb-ch`, `zedb-acp`, `zedb-cli`, and `zedb-core` security reviews are
treated as completed foundations. This pass concentrates on the desktop
application's orchestration, user-consent, network, filesystem, and update
boundaries.

## Evidence status

| Evidence | Status | Notes |
| --- | --- | --- |
| Formatting | Passed | `cargo fmt --all --check` |
| Clippy | Passed | `cargo clippy -p zedb-app --all-targets -- -D warnings` |
| App tests | Passed | 110 unit tests and 2 highlighting integration tests; focused policy regressions cover fleet consent, schema apply context, Cloud password rotation, export cleanup, managed checkout identity, staging paths, and updater limits |
| Direct client tests | Passed | Strict `zedb-ch` Clippy plus 150 passing tests; one live release test remains intentionally ignored |
| Signed updater test | Passed | The genuine team-signed fixture passes the exact requirement and swaps successfully |
| Manual exploit reproduction | Passed | An ad hoc bundle using the team text as its identifier passes ordinary strict verification but fails the new designated requirement |
| Dependency policy | Pre-existing failure | `cargo deny check` reports the repository's existing empty license allowlist and unmaintained transitive GPUI dependencies; this change adds no new package version |
| Change-scope analysis | Reviewed | Post-change PDG index: 95 changed symbols and 20 expected processes rooted only in schema apply, fleet confirmation, Cloud provisioning, and codec measurement; no modeled app taint finding |

## Threat model

### Assets and authority

- The installed zeDB application and its signing identity.
- Production ClickHouse data and migration tracking state.
- Cloud service passwords, organization API keys, and forge OAuth tokens.
- Migration and settings repositories selected by the user.
- Local files selected for export and application-owned state.

### Actors and failure sources

- A compromised or malformed release feed that does not possess the zeDB
  signing certificate.
- A remote Git repository or two unrelated remotes with the same basename.
- Connection, repo, or form state changing while a confirmation or async
  operation is outstanding.
- Existing database objects that collide with app-generated temporary names.
- Oversized or malformed provider responses and release archives.
- Ordinary user interaction, including cancelling or changing an in-progress
  operation.

An unrelated process with the same user's already-equivalent filesystem and
command execution authority is outside scope unless zeDB materially widens
that authority.

### Principal boundaries

| Boundary | Input | Sensitive sink | Required control |
| --- | --- | --- | --- |
| Release feed to updater | Version, asset URL, ZIP content, app bundle | Installed executable | Bounded download and extraction, exact Apple signing requirement, safe swap |
| UI state to server mutation | Connection, tier, repo, action, typed phrase | DDL, rollback, password rotation | Context binding and sink-side confirmation checks |
| Remote URL to managed checkout | Full clone URL | Repo opened, pulled, executed, or synced | Collision-resistant destination identity tied to the full remote |
| Schema analysis to trial DDL | Generated temporary table name | CREATE, INSERT, DROP | Unpredictable name and no pre-emptive drop of an existing object |
| Export dialog to filesystem | Editable path and cancellation | File creation and deletion | Bind cleanup to the path actually opened |
| Provider response to memory/disk | JSON, avatars, archives | Heap and local storage | Response and artifact size limits |

## Findings register

| ID | Severity | Confidence | Status | Title | CWE |
| --- | --- | --- | --- | --- | --- |
| ZAPP-001 | Critical | High | Fixed | Ad hoc bundle impersonates the updater's team check | CWE-347 |
| ZAPP-002 | Critical | High | Fixed | Managed checkout basename collision substitutes a different remote | CWE-706, CWE-829 |
| ZAPP-003 | High | High | Fixed | Mutation confirmations are stale or enforced only by the rendered button | CWE-602, CWE-863 |
| ZAPP-004 | High | High | Fixed | Codec measurement can drop an existing user table | CWE-706, CWE-459 |
| ZAPP-005 | High | High | Fixed | Cloud password rotation can complete into a different connection form | CWE-367, CWE-664 |
| ZAPP-006 | Medium | High | Fixed | Updater buffers and extracts an unbounded archive before trust is established | CWE-400, CWE-409 |
| ZAPP-007 | Medium | High | Fixed | Repo-owned prefix check stages unrelated paths | CWE-22, CWE-200 |
| ZAPP-008 | Low | High | Fixed | Export cancellation deletes the currently edited path, not the active export | CWE-367, CWE-73 |
| ZAPP-009 | Low | Medium | Accepted | Elevated forge token remains a silent-read Keychain credential | CWE-522 |

## Confirmed findings

### ZAPP-001: ad hoc bundle impersonates the updater's team check

`install_from_archive` first accepts any bundle that passes `codesign
--verify --strict`, then searches all `codesign -dvv` output for the substring
`TeamIdentifier=M8Y82YQ4GF`. An ad hoc signed bundle can set its ordinary code
identifier to that exact text. The review reproduced this locally: verification
passed, the output contained `Identifier=TeamIdentifier=M8Y82YQ4GF`, and the
real team field said `TeamIdentifier=not set`. The current substring test would
accept and install it.

Remediation: `codesign` now evaluates an explicit Apple requirement that binds
bundle identifier `dev.zedb.app`, an Apple certificate chain, and certificate
subject OU to the expected team. Display text is no longer an authority input.

### ZAPP-002: managed checkout basename collision substitutes a remote

Fleet clones and settings sync derive their managed directory solely from the
last URL segment. Two unrelated URLs ending in the same repository name select
the same directory. If that directory already contains `.git`, the app reuses
and pulls it without checking that its origin is the requested remote. Fleet
can therefore open migration content from one remote while displaying and
remembering another; settings sync can push local metadata to the wrong repo.

Remediation: both managed checkout paths now include a SHA-256-derived suffix
from the complete trimmed remote. A regression test covers unrelated remotes
with the same basename.

### ZAPP-003: mutation confirmations are stale or UI-only

Fleet action execution does not re-check the write lock, connected cluster,
repo, typed production phrase, or rollback acknowledgement. Those checks only
decide whether the rendered button receives a click handler. Connection reset
clears the lock but a stale action can otherwise remain reachable through
state transitions. The tier helper also reads the selected saved connection,
not necessarily the connected cluster.

The large-table schema apply confirmation stores only statements. Confirming
after a connection change runs them against whichever connection is current,
and `apply_suggestion` itself does not enforce the no-production or writable
policy.

Remediation: fleet confirmations carry their connected cluster and repository
root, use the connected cluster's tier, and repeat the write lock, context,
typed phrase, rollback acknowledgement, and run-state checks at execution.
Schema confirmations carry the connection and table identity; both confirmation
and the apply sink reject read-only, production, or changed context.

### ZAPP-004: codec measurement can drop an existing user table

Trial tables are named `_zedb_codec_trial_0`, `_zedb_codec_trial_1`, and so on,
with the sequence resetting on every app launch. `measure_codec_savings` begins
with `DROP TABLE IF EXISTS` for that name. If the selected database already
contains the predictable name, clicking Analyse destroys it before creating
the trial.

Remediation: every trial uses a machine-local unique identity. The client no
longer pre-drops a colliding name; cleanup is reachable only after CREATE has
succeeded and established ownership.

### ZAPP-005: Cloud password rotation can complete into another form

Password provisioning rotates the real service password asynchronously. While
it is working, the ordinary Cancel, Save, and Save and Connect controls remain
active. Completion updates whatever connection form happens to be open, not
the form and Cloud service that initiated the rotation. Cancelling A, opening
B, and receiving A's response can place A's new password into B while the old
password for A has already stopped working.

Remediation: provisioning requires the live Confirm stage, prevents Cancel and
both save paths while Working, and accepts completion only for the initiating
Cloud provenance and password input entity.

### ZAPP-006: updater archive resources are unbounded

The updater buffers the entire HTTP body in memory and passes the archive to
`ditto` before establishing signing trust. A release publisher compromise that
cannot sign code can still supply a very large response or compressed archive
and exhaust memory or disk when the user installs it.

Remediation: the archive streams to a unique private temporary directory with
a 512 MiB transfer ceiling. `zipinfo` must validate it and report no more than
2 GiB expanded before extraction. The archive-controlled filename is no longer
used locally.

### ZAPP-007: repo-owned prefix check stages unrelated paths

`repo_owned` accepts every path beginning with `migrations` or
`current-state`, including peers such as `migrations-secret.txt` and
`current-state-backup/`. This contradicts the panel's promise to stage only
the migration repo's owned paths and can commit unrelated data.

Remediation: staging now requires the exact directory name or a slash-separated
child. Regression tests cover the lookalike peer paths.

### ZAPP-008: export cancellation deletes the edited path

An export records the path used by the download only in the spawned task. If
the path field was already in editing mode, it remains editable while the
export runs. Cancel reads that current text and removes it, which may be a
different file from the partial export.

Remediation: export state stores the path passed to the download, cancellation
uses only that path, and a running export renders a frozen path rather than the
editable input.

## Accepted finding

### ZAPP-009: elevated forge token is stored for silent Git authentication

The repo picker stores a broad forge token in the plain-token Keychain service
so later Git HTTPS operations can run without another device flow. This is a
deliberate product decision already documented in `ACP-STANDARDS.md`: the
credential is revocable, Keychain-held, host-gated, and never placed in Git
configuration, argv, or ordinary process environment values. This review does
not reopen that accepted tradeoff.

## Checked and clean so far

- Agent app-bridge calls remain capability-authenticated, bounded, and forced
  through a read-only connection, consistent with `ACP-STANDARDS.md`.
- Settings sync preserves existing local connection endpoints and safety
  fields, and new remote connections are forced into a safe posture.
- OAuth device flows use fixed HTTPS authorization and token endpoints; no
  client secret is embedded.
- Git subprocess URL, effective-host, executable-config, and process timeout
  controls remain centralized in `zedb-core`.
- GitNexus PDG analysis found no modeled source-to-sink findings in the audited
  app files. Its closure, property-flow, and implicit-flow limitations mean
  the manual findings above remain authoritative.
