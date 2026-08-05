# Ideas parking lot

Not commitments. Things deliberately kept out of the spec's v1 scope so
they stop haunting design discussions. Promote to SPEC.md only with a
phase assignment.

## Exploration

- Saved queries / query library (per repo? per connection?).
- Query history with instant recall and fuzzy search.
- Inline charting of result sets.
- EXPLAIN visualization (pipeline / plan graphs).
- Table data preview with server-side filtering and sampling.
- Export result sets (CSV, Parquet, clipboard).

## Migrations / fleet

- zeDB writes to git: commit scaffolded migrations, push branches
  (first follow-up after v1; already noted in SPEC.md).
- Fleet-wide "apply wave" orchestration: staged rollout groups with
  pause/resume and failure isolation.
- Migration authoring assistance: live check-as-you-type against the
  pinned local server.
- Schema timeline: scrub through the migration chain and watch an object's
  DDL evolve.
- Per-database parameter overrides UI (the ancestor's offset inheritance,
  generalized).

## Ops (explicitly out of scope for cluster management, parked hard)

- Read-only ops panels: replication lag, mutation queues, disk, running
  queries.
- Mutating ops actions: kill query, restart replica, user/grant management.

## Drivers

- Guest explorer drivers: Postgres, SQLite, DuckDB (capability: explore
  only).
- Investigate replay-based migration support for engines with a cheap
  embedded form (DuckDB, SQLite) as a proof the capability model works.

## Platform

- Windows support (blocked on GPUI maturity + no native ClickHouse binary;
  WSL2 story instead?).
- Collaboration features (shared sessions, Zed-style). Very far future.
