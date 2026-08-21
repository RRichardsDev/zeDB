# `zedb-cli` security review

## Review metadata

| Field | Value |
| --- | --- |
| Status | Complete, eight confirmed findings remediated and verified |
| Baseline commit | `ee4c6d8af5b4dd3f2926d2666bf1bc8190564dba` |
| Branch | `security/zedb-cli-2026-aug` |
| Review host | Apple silicon, macOS 26.2.0 |
| Rust | `rustc 1.94.1 (e408947bf 2026-03-25)` |
| Cargo | `cargo 1.94.1 (29ea6fb6a 2026-03-24)` |
| Started | 2026-08-21 |
| Scope | `crates/zedb-cli` and the directly reachable core and ClickHouse command boundaries |

The review starts from the completed `zedb-ch` and `zedb-acp` security
commits. Shared code is revisited where the CLI supplies a new input or safety
contract that was not exercised by the earlier crate-local reviews.

## Evidence status

| Evidence | Status | Notes |
| --- | --- | --- |
| Formatting | Passed | `cargo fmt --all --check` |
| Clippy | Passed | `cargo clippy -p zedb-cli --all-targets -- -D warnings` |
| CLI tests | Passed | All 16 unit tests |
| Core tests | Passed | 30 unit and 24 repository integration tests |
| ClickHouse tests | Passed | 122 unit and 27 integration tests; one live GitHub metadata test remains normally ignored |
| App reachability | Passed | Strict Clippy passed for every app target sharing scaffold and verify |
| Dependency advisory policy | Passed with warnings | Configured cargo-deny command passes; known unmaintained workspace GUI transitives remain warnings |
| GitNexus PDG | Complete with limitations | Final index: 29,289 nodes, 65,468 edges, 194 clusters, 300 flows; targeted scans returned no modeled findings; callbacks, fields, implicit flows, process arguments, and many Rust I/O flows are not modeled |
| Manual threat model | Complete | Command dispatch, write gates, target selection, credentials, migration parameters, tracking import, MCP stdio, repo import, output, and availability traced |
| Adversarial tests | Passed locally | Process argument exposure reproduced with a dummy credential; file, SQL, output, parser, unreachable-peer, and live ephemeral-server regressions pass |
| Change-scope analysis | Reviewed | Critical graph reach across 37 processes matches the planned CLI, runner, verify, import, and scaffold boundaries |

## Threat model

### Assets and authority

- Database credentials and any elevated admin credential.
- ClickHouse data, schema, and migration tracking integrity.
- The user's confidence that dry-run commands do not modify a server.
- Local files outside an imported migration repository.
- Migration parameters, which may include passwords, tokens, or signed URLs.
- Terminal integrity and machine-readable output consumed by automation.

### Actors and failure sources

- The local user, shell history, process inspection, and local monitoring tools.
- A malicious or unexpectedly structured migration repository.
- A compromised or incorrectly targeted ClickHouse server.
- Ambiguous or contradictory command-line input.
- A malicious ancestor repository supplied to `zedb import`.
- Very large or control-character-bearing repository and server data.

### Principal boundaries

| Boundary | Input | Sensitive sink | Required control |
| --- | --- | --- | --- |
| Shell to CLI | Passwords and migration parameters | Process arguments, history, runner configuration | Secret-safe acquisition and no ignored secret options |
| CLI safety flags to runner | `--write` and `--dry-run` | Tracking DDL, tracking rows, migration SQL | One explicit, consistent no-write decision |
| CLI and repo strings to SQL | Cluster, tracking database, import source | ClickHouse query and execute | Strict identifier grammar or correct quoting |
| Repeated options to runner | Parameter names and admin options | Rendered migration and credential selection | Reject duplicates and incomplete combinations |
| Ancestor repo to destination | Files, directories, symlinks | Arbitrary local paths and existing destination content | No symlink traversal and fresh destination requirement |

## Findings register

| ID | Severity | Confidence | Status | Title | CWE |
| --- | --- | --- | --- | --- | --- |
| ZCLI-001 | High | High | Fixed | Secret-bearing arguments are exposed in process metadata and shell history | CWE-214 |
| ZCLI-002 | Medium | High | Fixed | Dry-run commands can still mutate tracking state | CWE-693 |
| ZCLI-003 | Medium | High | Fixed | CLI and repo strings are interpolated into SQL syntax without identifier validation | CWE-89 |
| ZCLI-004 | Medium | High | Fixed | Duplicate and incomplete security-sensitive options are silently accepted | CWE-20 |
| ZCLI-005 | Medium | High | Fixed | Repo import follows source symlinks and can overwrite an existing destination | CWE-59, CWE-73 |
| ZCLI-006 | Low | High | Fixed | Human output emits untrusted terminal control characters | CWE-150 |
| ZCLI-007 | Medium | High | Fixed | Multiline migration descriptions can inject active SQL into scaffolds | CWE-74 |
| ZCLI-008 | Medium | High | Fixed | Read commands construct write-capable database sessions | CWE-250 |

### ZCLI-001: secret-bearing arguments are exposed in process metadata and shell history

**Affected code:** `Command::Pin`, `ConnectionArgs`, `Command::Mcp`,
`ConnectionArgs::options`, and their dispatch in `main::run`.

Database and admin passwords are accepted directly as `--password` and
`--admin-password` values. Migration parameters are also accepted as complete
`name=value` arguments even though imported repositories define a password
parameter and the runner correctly treats arbitrary custom values as possible
secrets when redacting durable errors.

On the review host, a live MCP process launched with a dummy password displayed
that value verbatim in `ps -ww` output. The same arguments are normally retained
by interactive shell history. This disclosure occurs before the transport and
runner protections reviewed in `zedb-ch` can apply.

**Required fix:** remove password values from argv and provide a file-based
secret source suitable for interactive and automated use. Provide a safe path
for secret migration parameters, clearly distinguish it from ordinary
non-secret `--param`, and document the remaining local file trust assumption.

**Resolution:** direct password arguments were removed. Password and admin
password values now come from bounded, non-empty UTF-8 files through
`--password-file` and `--admin-password-file`. Secret template values can use
`--param-file name=FILE`; ordinary `--param` remains for values that are not
secrets. Only the file path appears in argv. The user remains responsible for
the local permissions and contents of a chosen secret file.

### ZCLI-002: dry-run commands can still mutate tracking state

**Affected code:** CLI `ConnectionArgs::options`; runner `upgrade`,
`rollback_to`, `rollback_one`, `stamp`, `import_tracking`, `apply_targeted_inner`,
`ensure_tracking`, and `apply_sql`.

The CLI describes `--dry-run` as printing what would run without executing it.
The runner checks that flag only inside `apply_sql`, after mutating entry points
have called `ensure_tracking`. Upgrade, rollback, and targeted apply can
therefore create the tracking database and tables and seed metadata during a
dry run. Stamp and import-tracking do not consult `dry_run` at all and can write
their normal tracking rows or imported data.

Every affected command still requires `--write`, which limits accidental
reachability but does not restore the explicit preview contract represented by
the second flag.

**Required fix:** branch before every server mutation, including tracking
bootstrap and direct tracking writes. Add regressions proving dry-run emits no
execute request for each mutating command family.

**Resolution:** tracking bootstrap returns before network access in dry-run
mode, stamp emits previews without recording rows, and import-tracking validates
its source then returns without a query or execute. A regression uses an
unreachable endpoint to prove direct tracking dry runs make no request. The
ephemeral-server regression also passed against the reviewed cached ClickHouse
build during the full serial test gate.

### ZCLI-003: CLI and repo strings are interpolated into SQL syntax without identifier validation

**Affected code:** runner `ensure_tracking`, `tracking_table`, and
`import_tracking`, plus verifier `verify_database`, reached through CLI
`--cluster`, `--db`, repository `tracking.database`, and
`import-tracking --from`.

These values are inserted directly into SQL syntax. The normal migration
template renderer validates the built-in database and cluster parameters as
plain identifiers, but tracking setup bypasses that renderer. The import source
is documented as a table name yet can contain an arbitrary table expression or
additional query clauses. The tracking database and cluster inputs can likewise
change the structure of generated statements.

Explicit write consent is required for mutating entry points, but the expanded
query structure can exceed the narrowly reviewed tracking action and use the
migration identity's full authority. Read-only status paths also construct
queries from the configured tracking database.

**Required fix:** centralize and apply strict ClickHouse identifier validation
to tracking database, cluster, and qualified import table names before any
query or execute call. Add adversarial tests for whitespace, comments,
punctuation, table functions, and extra clauses.

**Resolution:** every runner entry validates tracking and cluster names as
plain identifiers, and tracking imports accept only `TABLE` or
`DATABASE.TABLE`. Verification now quotes database names as SQL string
literals. Focused tests reject comments, punctuation, table functions, extra
clauses, and quote-bearing literals before connection or execution.

### ZCLI-004: duplicate and incomplete security-sensitive options are silently accepted

**Affected code:** `parse_param`, `ConnectionArgs::options`, `Command::Pin`,
and `Command::Mcp`.

Repeated `--param` values are collected into a map, so the final value for a
duplicate name silently wins. `--admin-password` without `--admin-user` is
accepted and discarded. Pin accepts user and password arguments when an
explicit version means no server discovery occurs, and MCP accepts credentials
without a server and discards them.

These behaviors make reviewed command text differ from effective authority or
configuration. The ignored password cases also encourage needless secret
exposure through ZCLI-001.

**Required fix:** reject duplicate parameter names and use command-line
requirements and conflicts to reject credentials without their corresponding
identity or endpoint. Cover every rejected combination with parser tests.

**Resolution:** option construction rejects duplicate names across `--param`
and `--param-file`. Parameter names follow the placeholder grammar. Clap now
rejects pin and MCP identities or secret files without their endpoint, admin
secret files without an admin identity, and credentials alongside an explicit
pin version. Read commands use a separate minimal argument set and no longer
accept irrelevant write, dry-run, admin, cluster, or parameter options.

### ZCLI-005: repo import follows source symlinks and can overwrite an existing destination

**Affected code:** core `import::copy_tree`, `pinned_version`, and
`import_repo`, reached by CLI `zedb import`.

The recursive copier uses `Path::is_dir`, which follows symlinks. A symlink in
an ancestor migration tree can therefore copy files from outside that tree or
create a traversal cycle. The destination is considered available whenever it
lacks `zedb.toml`; existing migration files, `exclusions.toml`, and other
matching paths may then be overwritten or merged into the import.

The destination can also be nested under the ancestor. Creating it after
preflight makes the recursive source walk discover its own output. In addition,
the ancestor's `CH_VERSION` value is inserted into generated TOML without a
safe version grammar, allowing configuration text injection.

**Required fix:** reject every source symlink, require the destination to be
absent or empty, and avoid leaving a partially imported tree on validation
failure. Add tests using an outside-file symlink, a directory symlink, and a
pre-populated destination.

**Resolution:** import validates the complete migration tree before creating
the destination, rejects symlinks and special files, requires an absent
destination whose parent already exists, resolves real parent paths to reject
ancestor overlap, and accepts only four-part numeric ClickHouse versions.
Regressions cover file and directory symlinks, existing content, nested output,
configuration injection, and validation-before-creation.

### ZCLI-006: human output emits untrusted terminal control characters

**Affected code:** CLI human report paths and runner progress output.

Database names and errors can originate at a server, while migration headlines,
SQL, paths, and check findings originate in a repository. Human output printed
these strings verbatim, permitting escape sequences to alter the visible
terminal, forge lines, or conceal subsequent output. JSON output must remain
lossless and is not a terminal presentation sink.

**Required fix:** escape control characters at human output boundaries while
preserving intended SQL and diagnostic line layout. Do not alter JSON values.

**Resolution:** shared CLI and runner presentation helpers escape control
characters in fields and preserve only newline and tab where multiline text is
intentional. Human reports and final errors use those helpers. JSON construction
is unchanged. Focused tests cover newline and terminal-clear sequences.

### ZCLI-007: multiline migration descriptions can inject active SQL into scaffolds

**Affected code:** core `scaffold_migration`, reached by CLI `zedb new` and the
app migration authoring flow.

The command documents a one-line description but the shared writer placed any
string directly after a SQL line-comment marker. An embedded newline therefore
starts attacker-selected active SQL in the new `upgrade.sql`. Execution still
requires a later reviewed write action, but the generated artifact violates its
content boundary at creation time.

**Required fix:** reject empty descriptions and every control character before
creating a migration directory. Cover newline and terminal-control inputs.

**Resolution:** shared scaffold validation now enforces one non-empty line and
runs before filesystem creation. The regression covers blank, multiline, and
escape-bearing descriptions and proves no migration directory is left behind.

### ZCLI-008: read commands construct write-capable database sessions

**Affected code:** CLI `Command::Status`, `Command::Verify`, and
`ConnectionArgs::options`.

Status and verify flattened the same connection arguments as mutating commands.
They advertised write, dry-run, admin, cluster, and parameter options they did
not need, then constructed `ChConfig` with `read_only = false`. Their current
call paths intended only queries, but ClickHouse did not enforce that authority
boundary. This increased the impact of any query-construction defect such as
the verify literal issue in ZCLI-003.

**Required fix:** give read commands a minimal argument type and force server
read-only mode. Reject mutation and elevated options at parsing time.

**Resolution:** status and verify now use `ReadConnectionArgs`, which exposes
only endpoint, user, and password-file inputs and always sets
`ChConfig.read_only = true`. ClickHouse HTTP and native transports therefore
apply `readonly=2`. Parser tests prove write, dry-run, and admin options are not
accepted on read commands.

## Review log

- 2026-08-21: created the dedicated review branch and plan.
- 2026-08-21: baseline formatting, strict Clippy, CLI tests, and configured
  dependency advisory checks passed.
- 2026-08-21: GitNexus targeted taint scans returned no modeled findings; the
  manual review retained process, argument, SQL-construction, and filesystem
  boundaries because those flows are outside the model's strongest coverage.
- 2026-08-21: confirmed ZCLI-001 through ZCLI-005 and recorded them before
  remediation.
- 2026-08-21: completed the output, verify, read-authority, scaffold, and path
  overlap passes; confirmed ZCLI-006 through ZCLI-008 and remediated all eight
  findings.
- 2026-08-21: final formatting, strict Clippy for core, ClickHouse, CLI, and app,
  all affected tests, the configured advisory policy, live dry-run proof,
  refreshed PDG scans, and change-scope review passed. Review complete.
