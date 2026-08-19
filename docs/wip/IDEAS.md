# Ideas parking lot

Not commitments. Things deliberately kept out of scope so they stop
haunting design discussions. Promote to SPEC.md (or a phase doc) only
with a phase assignment. Shipped items get pruned; this is the list
of what is NOT built.

## ClickHouse Cloud (Phase 10.5 remainder)

The front door (10.5a), the truthfulness fixes (10.5b), and the
connection-page dashboard shipped in v0.1.31. Wake-before-connect,
status-bar cost with burn-rate warnings, and the read-only agent
context (the old 10.5c/d core) were promoted to `docs/wip/PHASE-13.md`
(2026-08-19). Still parked here, per `docs/CLOUD-STRATEGY.md`:

- Pre-flight estimates phrased in wake/compute terms; the audit-log
  timeline beside ops; ClickPipes surfaced from the API, not just
  named.
- Backup-restore wired into fleet as migration rehearsal ("rehearse
  this migration on a restored copy"); waits with migrations on the
  analytics-clickhouse-ddl battle-test.
- Query-API Bearer connections: read-only SQL as the signed-in user,
  no database credentials at all, degrading honestly (no native TCP,
  no writes, no session settings).
- Upstream-gated (standing asks to ClickHouse): a zeDB OAuth client
  id; a write-capable audience or key-bootstrap-from-OAuth endpoint;
  JWT-mapped database users (Snowflake-class passwordless sign-in);
  warehouse names in the services API.

## Distributed and workload layer (Phase 12 remainder)

Phase 12 shipped its increments in v0.1.30 (doc harvested into
`docs/SPEC.md` Differentiators, 2026-08-19). Deliberately not built:

- The cross-table workload advisor surface (the Workload tab reasons
  per table; "your whole cluster's traffic, ranked" does not exist).
- MV insert failures drawn onto the Dependencies tab's DAG, so a
  failing edge is visible where the lineage already is.
- An API-minted database credential at Cloud link time was partially
  answered by 10.5 (password provisioning via the state API, primary
  service only); true passwordless minting is upstream-gated below.

## Exploration

- UI scaling with cmd +/- (check what GPUI offers out of the box).
  Was the Phase 11 stub, demoted 2026-08-19; the number was retired
  unused.
- Decide whether native TCP should become the default transport (the
  Phase 10.1 transport work has the facts; this is a judgment call,
  not new infrastructure). Also from the Phase 11 stub.
- Inline charting of result sets.
- EXPLAIN visualization as pipeline / plan graphs (the textual
  EXPLAIN views shipped; the graphs did not).

## Migrations / fleet

- Embedded runners: client libraries (Java, Python, Node, C++, Rust,
  PHP) that read current state, deploy, and stamp tracking from
  application code. Opt-in per repo and per language in zedb.toml,
  disabled by default; one engine, thin bindings. Design sketch lived
  in the retired PHASE-3 doc; see the devlog.
- Fleet-wide "apply wave" orchestration: staged rollout groups with
  pause/resume and failure isolation.
- Migration authoring assistance: live check-as-you-type against the
  pinned local server (the authoring view checks on demand today).
- Schema timeline: scrub through the migration chain and watch an
  object's DDL evolve.
- Per-database parameter overrides UI (the runner honors overrides;
  no UI edits them).

## Agent pane

- Preset prompts as one-click thread starters: explain this
  migration, why is this database drifted, review my draft.

## Ops

- Mutating ops actions beyond KILL QUERY: restart replica, user and
  grant management. (The read-only panels and KILL shipped.)

## Drivers

- Guest explorer drivers: Postgres, SQLite, DuckDB (capability:
  explore only).
- Investigate replay-based migration support for engines with a cheap
  embedded form (DuckDB, SQLite) as a proof the capability model
  works.

## Platform

- Windows support (blocked on GPUI maturity + no native ClickHouse
  binary; WSL2 story instead?).
- Collaboration features (shared sessions, Zed-style). Very far
  future.
