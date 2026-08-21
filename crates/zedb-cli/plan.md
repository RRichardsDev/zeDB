# `zedb-cli` security review plan

## Purpose

Review `zedb-cli` as the user-controlled authority boundary over the already
reviewed ClickHouse, repository, and MCP implementations. The CLI accepts
credentials, server endpoints, database targets, template values, repository
paths, migration numbers, and write consent before dispatching read, write,
filesystem, subprocess, and stdio-server operations.

The review must answer four questions:

1. Can secrets leak through process arguments, environment, diagnostics,
   machine-readable output, or child processes?
2. Can parsing, defaults, conflicting flags, or target selection cause a write
   broader than the user explicitly authorized?
3. Can crafted paths, repository contents, template values, or table names
   escape the intended filesystem or SQL boundary?
4. Can malformed input, stalled dependencies, or excessive output compromise
   availability or produce misleading success?

Findings are established before remediation. Every code change receives an
upstream GitNexus impact check and focused regression coverage.

## Security contracts

- `docs/contracts/FORMAT.md` defines valid repository layout, migration
  targeting, rollback classes, parameters, and tracking semantics.
- `docs/contracts/PRODUCT-PRINCIPLES.md` requires the user to drive every
  server mutation explicitly.
- The completed `zedb-ch` review is authoritative for ClickHouse transport,
  runner, binary, replay, and MCP internals. This review verifies that CLI
  construction and dispatch cannot bypass those controls.
- `AGENTS.md` requires impact analysis before symbol edits, warnings for High
  or Critical blast radius, and `detect_changes()` before commit.

## Scope and priority

| Priority | Surface | Main risks |
| --- | --- | --- |
| P0 | Credentials and endpoints | Shell history and process-list disclosure, error leakage, endpoint confusion, secret inheritance |
| P0 | Write consent and target selection | Accidental all-fleet writes, conflicting flags, unsafe defaults, dry-run or write-control bypass |
| P0 | Upgrade, rollback, stamp, apply, and tracking import dispatch | Wrong operation or migration, irreversible action ambiguity, unchecked source table input |
| P1 | Repository init, import, scaffold, show, and regeneration | Path traversal, symlink following, overwrite, source-destination overlap, terminal injection |
| P1 | MCP stdio command | Credential disclosure, write-capable construction, framing and shutdown behavior |
| P1 | JSON and human output | Secret or control-character leakage, invalid JSON on errors, misleading exit status |
| P2 | Availability and dependency policy | Missing deadlines, excessive output, subprocess hangs, test and CI gaps |

## Run sequence

### 0. Establish the baseline

- Record branch, commit, host, Rust, Cargo, and clean worktree state.
- Run formatting, strict CLI Clippy, CLI tests, the configured advisory gate,
  and a reachable dependency tree.
- Refresh and query the PDG index; record both findings and analysis limits.
- Inventory every command, option, default, conflict, output mode, and exit.

### 1. Map authority and data flow

- Trace `Cli::parse` through `run` into every command module and shared crate.
- Classify each input as secret, identifier, path, selector, content, endpoint,
  consent, or presentation-only.
- Identify every database write, filesystem write or deletion, process launch,
  network connection, stdout/stderr write, and MCP response sink.

### 2. Review credentials and endpoint handling

- Test password and admin-password exposure through argv, help, errors,
  debugging, environment, process inheritance, and child command lines.
- Verify alternatives suitable for scripts and interactive use, with explicit
  precedence and no ambiguous empty-secret behavior.
- Confirm URL validation, redirects, read-only construction, and credential
  redaction inherit the reviewed ClickHouse controls.

### 3. Review writes, targets, and confirmation

- Build a decision table for every mutating command across `--write`,
  `--dry-run`, database, group, all, cluster, no-cluster, admin, targeted,
  irreversible, number, and `--to` combinations.
- Prove omitted target flags do not silently widen scope and conflicting flags
  fail closed.
- Verify CLI parsing cannot bypass the runner's write lock, targeting,
  rollback, tracking, parameter, or elevated-routing controls.

### 4. Review paths, repository input, and output

- Exercise absolute, parent, symlink, overlapping, existing, special-file,
  Unicode, control-character, and excessive path and description inputs.
- Verify imports and scaffolds fail atomically without overwriting unrelated
  content.
- Ensure human and JSON output remain bounded, structurally valid, redacted,
  and paired with truthful exit codes.

### 5. Review MCP and availability

- Confirm the CLI MCP command is structurally read-only and cannot expose a
  write client or credentials through responses or logs.
- Bound waits and output at every CLI-owned boundary not already enforced by
  reviewed shared code.
- Test broken pipes, child failures, malformed repos, unavailable servers,
  interrupted operations, and partial output.

### 6. Remediate and close

- Record severity, confidence, affected code, exploit narrative, required fix,
  resolution, regression evidence, and residual risk for every finding.
- Add user-facing changes to `CHANGELOG.md` and internal-only changes to the
  development log where appropriate.
- Pass formatting, strict Clippy, CLI and affected shared-crate tests,
  dependency policy, adversarial regressions, final PDG analysis, and staged
  `detect_changes()` before committing.
