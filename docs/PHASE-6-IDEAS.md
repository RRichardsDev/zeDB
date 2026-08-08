# Phase 6 ideas: toward a first-class browser and migration manager

Status: IDEAS, not committed scope. A ranked shopping list from the
2026-08-08 soak review; each item stands alone and none is promised.
Pick by appetite.

## Unfinished thread (small, do soon)

- **Array values in the data view.** Started 2026-08-08 and derailed
  by the empty-body DDL bug: `sat.arrayValues` (values Array(String))
  was created to see how arrays render in the results grid, and
  nobody ever looked. Audit grid rendering for Array / Map / Tuple /
  Nested / JSON values: readable inline formatting, sensible
  truncation, and copy behavior. Pairs with the cell inspector below.

## Browser: daily-driver gaps

- **Query history + saved queries.** The largest absence in the app:
  every executed query is gone forever. Searchable per-connection
  history (system.query_log helps), pinned/saved queries with names.
  Highest daily-feel payoff of anything on this list.
- **Export to file.** Stream FORMAT Parquet/CSV/JSONEachRow straight
  to disk, bypassing decode and the grid; near wire speed with the
  compression work already shipped. Parked twice; still the right
  answer for big pulls.
- **Ops view.** SHOW PROCESSLIST with kill, merges/mutations in
  flight, replication queue health, disk usage by table/partition.
  The topology cards from Phase 5 are the seed of a real cockpit.
- **EXPLAIN, visualized.** Plan/pipeline view plus the query_log
  aftermath (read vs result rows, memory, spill). ClickHouse makes
  people guess why queries are slow; showing them wins loyalty.
- **Cell inspector.** Click a long value (JSON blob, array, huge
  string) and see it pretty-printed in a side pane. Small, daily.

## Migration manager: growing up

- **Promotion ladder.** Tiers are badges today; make them policy:
  dev applies freely, staging confirms, prod shows the exact diff
  and demands typed confirmation. The safety story teams adopt for.
- **Drift -> migration.** Detection exists; the killer move is
  "adopt this drift": generate the migration that legitimizes or
  reverts a hand-edited server. Nobody closes this loop well.
- **CI recipe.** zedb-cli already runs checks headlessly; document a
  GitHub Action gating PRs on chain/regen checks. The honest small
  kernel of the parked Phase 4.
- **Per-shard apply awareness.** Phase 5 meets the fleet: the apply
  matrix understanding ON CLUSTER propagation per shard.

## Platform bets (bigger, later)

- **Linux/Windows.** gpui allows it; the macOS Keychain layer is the
  abstraction to crack (secrets backend per platform).
- **ClickHouse Cloud ergonomics.** Endpoint conventions, wake-up
  handling, no-visible-shards polish.
- **Grid memory.** Columnar/interned value storage to cut the ~2.4x
  decoded-vs-wire blow-up seen on the 193M-row pull.
- **Light theme tuning.** Shipped as a first draft; soak the shades.

## A suggested first bite

Query history for feel, the ops view for power, drift->migration for
having something nobody else has.
