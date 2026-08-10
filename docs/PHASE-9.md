# Phase 9: Query advisor + visible MergeTree machinery + MV/projection DAG

Three north-star differentiators (`docs/NORTH-STAR.md` #2, #3, #4) that
all build directly on shipped surfaces: the EXPLAIN visualizer, the
cluster-fanning ops view, and the existing tree rendering. Phase 9 turns
the plan into advice, makes the MergeTree lifecycle (parts, merges,
mutations, TTL) visible, and draws the materialized-view / projection
dependency graph.

Status: PLANNED. Not started. Follows Phase 8 (`docs/PHASE-8.md`).

## Standing constraints (same as Phase 8)

- **Rules-first, explainable.** The advisor is deterministic logic over
  `system.query_log` / `ProfileEvents`, not a model. Every finding shows
  *why* it fired and the DDL that fixes it. The existing "ask your agent"
  error flow stays available as an **optional** hand-off for "explain
  this slow query," never a required or default step (product spine:
  `docs/PRODUCT-PRINCIPLES.md`).
- **Nothing heavy on the main thread.** All `system.*` reads (query_log,
  parts, merges, mutations) go through the async ClickHouse query path
  and deliver back to the entity; the render thread never blocks. Any
  live/auto-refreshing panel polls off-thread on a timer.
- **Conservative.** Only surface a finding we are sure of; a wrong
  "add this index" erodes trust fast.

## Part A — Query advisor (north-star #2)

Close the EXPLAIN loop: we already visualize the plan (tree + index
pruning bars); now turn it into a ranked recommendation.

Inputs, after a query runs or picked from history / `system.query_log`:

- `read_rows` / `read_bytes` vs `result_rows`, `memory_usage`.
- `ProfileEvents`: `SelectedParts`, `SelectedRanges`, `SelectedMarks`
  vs totals; whether partitions pruned.
- Table `sorting_key` / `partition_key` from `system.tables`.

Findings (each = plain-language diagnosis + fix DDL, ranked by estimated
rows saved):

- scanned billions to return a handful -> `WHERE` not covered by the
  primary key / `ORDER BY`.
- filter column with no index -> suggest a **skip index** (`minmax`,
  `set(N)`, `bloom_filter`) sized to the data.
- repeated aggregate over a large scan -> suggest a **projection** or
  materialized view.
- partition not pruned -> suggest a partition-aligned predicate.

The suggested DDL is copyable, and (once drift -> migration lands,
`docs/PHASE-7.2-IDEAS.md`) stageable as a migration rather than pasted.

Reuses: the EXPLAIN read path and result rendering; the history / query
picker already exists.

## Part B — Visible MergeTree machinery (north-star #3)

The MergeTree lifecycle is opaque in every generic tool. Make it a
surface, the storage-side twin of the ops view.

- **Parts / partitions**: parts per partition, sizes, level, rows and
  compressed size per partition, a "too many parts" warning
  (`system.parts`).
- **Live merges** (`system.merges`): what is merging now, progress,
  memory. Auto-refreshing (off-thread poll).
- **In-flight mutations** (`system.mutations`): long-running `ALTER`s
  with progress and failures; surface stuck/failed mutations.
- **TTL moves**: hot/cold disk tiering, what is scheduled to move
  (`system.parts.move_ttl_info`, `system.storage_policies`).

Reuses: the ops view's cluster-fanning read pattern
(`clusterAllReplicas()`), so these can be cluster-wide too; the storage
tab's `system.parts`-shaped queries.

## Part C — Materialized-view & projection DAG (north-star #4)

MVs and projections are ClickHouse's secret weapon and totally opaque in
generic tools. Draw the dependency graph so users can see what feeds
what:

- **MV dependency graph**: source table -> materialized view -> target
  table, from `system.tables.dependencies_table` /
  `dependencies_database` and each MV's `as_select`.
- **Projections**: which projections are attached to a table
  (`system.projection_parts` / table DDL), and their size/coverage.
- **Staleness / health**: flag MVs whose target has drifted or whose
  source no longer exists; a broken MV silently drops inserts, so
  surfacing it is high value.

Reuses: the tree/graph rendering already used for EXPLAIN and the schema
inspector; the dependency data is a `system.tables` query behind it.

## Sequencing

The three parts are independent; any can go first.

- **Part A (query advisor)** is the biggest "whoa" and closest to the
  EXPLAIN work.
- **Part B (MergeTree machinery)** is more read-and-render (lower risk,
  high operator utility).
- **Part C (MV/projection DAG)** is read-and-render over the existing
  tree UI.

Ship in increments, each releasable: for A, one finding type at a time;
for B, parts view then merges then mutations/TTL; for C, the MV graph
then projections then staleness.

## Open questions

- query_log availability: it must be enabled server-side; detect and
  degrade gracefully (some managed setups restrict it), mirroring how
  ops degrades when distributed queries are refused.
- Advisor scope: only the last run, or mine `system.query_log` for the
  worst recent queries and advise across them?
- Estimated-rows-saved: heuristic from `ProfileEvents`, labelled as an
  estimate (same honesty rule as Phase 8's size estimates).
