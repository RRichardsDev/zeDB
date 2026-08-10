# North star: the first-in-class ClickHouse explorer

The thesis, in one line: **every generic tool (DBeaver, DataGrip,
TablePlus) treats ClickHouse as "just another SQL database." First-in-
class means leaning hard into what ClickHouse actually is: columnar,
MergeTree, distributed.**

We are not building a SQL client that happens to connect to ClickHouse.
We are building the tool that understands columnar storage, the
MergeTree lifecycle, and distributed execution as first-class concepts.
The features below are the ones a generic tool structurally cannot do,
because doing them requires modelling ClickHouse's internals rather
than SQL-in-general.

This doc is the vision and the ranking, not a committed plan. Near-term
backlog lives in `docs/PHASE-7.2-IDEAS.md`; loose thoughts in
`docs/MAYBE-IDEAS.md`. It is deliberately opinionated so future feature
work can ask "does this move us toward the north star, or is it generic
polish?"

## What we already have (the bones)

The differentiators below are close reach precisely because the
infrastructure exists:

- **Streaming query execution** with live progress (built for volume).
- **EXPLAIN visualized**: plan tree + per-read index-pruning bars
  (parts/granules selected vs initial). We already read the plan.
- **Ops view**: running queries fanned across shards via
  `clusterAllReplicas()`, kill `ON CLUSTER`. The cluster-aware read
  path exists.
- **Storage tab**: per-object size/rows, container-vs-leaf structure.
  We already query `system.parts`-shaped data.
- **Sharding awareness**, schema intelligence (typed hover/completion),
  composite/JSON/array rendering, query history, streaming export.

Every idea below is "the next layer" on one of these, not a green
field.

## The differentiators, ranked

### 1. Column-level storage intelligence (start here)

The biggest gap in every other tool, and pure ClickHouse. Because the
engine is columnar, surface the column as the unit of storage:

- Per-column **compressed vs uncompressed size**, compression ratio,
  **codec**, and cardinality. Source: `system.columns` +
  `system.parts_columns` (`data_compressed_bytes`,
  `data_uncompressed_bytes` per column), cardinality via a sampled
  `uniqCombined`/`uniqExact` on a `SAMPLE` or `LIMIT`.
- **Codec advice** from the shape of the data:
  - high row count, low distinct ratio on a `String` ->
    `LowCardinality(String)` (show estimated size drop).
  - monotonic / slowly-changing integer or timestamp -> `Delta` or
    `DoubleDelta` then `ZSTD`.
  - float time-series -> `Gorilla`.
  - already-random / high-entropy -> leave it, note that a heavier
    codec buys nothing.
- Present it as "this column is X now, would be ~Y as Z" with the
  `ALTER TABLE ... MODIFY COLUMN ... CODEC(...)` ready to copy (and,
  once migrations land, to stage as a migration).

Why first-in-class: no generic tool models compression at all. The
storage tab already fetches sizes; this is the per-column twin with a
recommendation attached.

Rough build: extend the storage/columns query to join
`system.parts_columns`, add a cardinality probe (cache it, it is the
expensive part), and a small rules engine for the codec suggestion.
Keep the rules explainable (show *why* each suggestion fires); never
auto-apply.

### 2. Query advisor on `system.query_log` (close the EXPLAIN loop)

We visualize the plan; the next step is turning it into a
recommendation. After a query runs (or by picking one from history /
`system.query_log`):

- Read `read_rows` / `read_bytes` vs `result_rows`, memory,
  `ProfileEvents` (e.g. `SelectedParts`, `SelectedRanges`,
  `SelectedMarks` vs total).
- Diagnose the classic misses:
  - scanned billions to return a handful -> the `WHERE` is not covered
    by the `ORDER BY` / primary key.
  - a filter column with no index -> suggest a **skip index**
    (`minmax`, `set(N)`, `bloom_filter`) sized to the data.
  - repeated aggregate over a large scan -> suggest a **projection** or
    materialized view.
  - partition not pruned -> suggest a partition-aligned predicate.
- Output: plain-language finding + the DDL to fix it, ranked by
  estimated rows saved.

Why first-in-class: this is the "whoa" demo feature and it is a short
hop from the EXPLAIN work already shipped. The existing "ask your agent"
error flow is the template for an optional "explain this slow query"
hand-off.

### 3. Make the invisible MergeTree machinery visible

Parts, merges, mutations, TTL: opaque in every other tool.

- **Parts / partitions view**: parts per partition, sizes, level, a
  "too many parts" warning, rows and compressed size per partition
  (`system.parts`).
- **Live merge activity** (`system.merges`): what is merging now,
  progress, memory.
- **In-flight mutations** (`system.mutations`): long-running `ALTER`s
  with progress and failures.
- **TTL moves**: hot/cold disk tiering, what is scheduled to move
  (`system.parts` `move_ttl_info`, `storage_policies`).

Why first-in-class: this is the storage-side twin of the ops view,
which already fans across shards. Same read-and-render muscle.

### 4. Materialized-view & projection DAG

MVs and projections are ClickHouse's secret weapon and totally opaque
elsewhere. Draw the dependency graph: source table -> MV -> target
table, plus projections attached to a table. Show what feeds what and
what is stale. We already render trees (EXPLAIN, schema); this is the
same rendering with a dependency query behind it
(`system.tables.dependencies_table`, MV `as_select`).

### 5. Live tail

ClickHouse eats logs and events. A `tail -f` on a table: poll new rows
by a monotonic key, or `WATCH` a live view. Turns the explorer into a
real-time console. The streaming execution path is already built for
exactly this cadence.

### 6. ClickHouse-correct migrations (the drift -> migration roadmap)

Potentially the feature with no peer. ClickHouse migrations are not
naive `ALTER`:

- many changes are **async** (mutations) and need progress tracking;
- some need **create new + insert + rename**, not in-place alter;
- **`ON CLUSTER`** coordination and **`ReplicatedMergeTree`** awareness;
- codec / `ORDER BY` / partitioning changes have real rewrite cost.

A migration manager that generates **ClickHouse-shaped** migrations
from detected schema drift, not the generic SQL a normal tool emits, is
first-in-class on its own. This is where the migration-manager half of
the product and the explorer half meet: the storage and query advisors
(1 and 2) produce the very `ALTER`s this would stage safely.

## Where to start

**#1 and #2** are the highest reward for the least new infrastructure,
and they are the two features that make someone's jaw drop in a demo.
They also reinforce the product spine (see `docs/PRODUCT-PRINCIPLES.md`):
hands-on, explainable recommendations the user chooses to apply, with
their own agent available for the hand-off, never an auto-magic black
box. Related: `docs/PHASE-7.2-IDEAS.md`.
