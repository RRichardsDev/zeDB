# Phase 17: the server-aware editor

Status: NEXT UP (2026-08-26). Queue-jumps every unimplemented phase
(15's test seams, 16's remaining tracks, 10.7): v0.1.36 proved the
pattern and the appetite, and this phase finishes the thought while
the machinery is warm.

The organizing idea, proven by the v0.1.36 release: the editor knows
the server you are connected to, version-true, and says so in place.
Settings and functions were the first two catalogs. This phase sweeps
the rest of what a running ClickHouse can tell us about itself, and
adds the one live-probe feature that changes behavior: cost honesty
before Run.

## The performance contract (non-negotiable)

Nothing in this phase may make the editor feel slower, even a tiny
amount. Concretely:

- Catalog features (B through F) follow the settings/functions
  pattern exactly: swept once per schema refresh into the snapshot,
  read lock-free from the ArcSwap, cached to disk per connection.
  Zero per-keystroke network. A sweep failure degrades to silence,
  never to lag or noise.
- Completion, hover, and diagnostics stay pure functions over the
  snapshot: no await anywhere on the typing path.
- The one live-probe feature (A) is debounced behind the existing
  schema-analysis debounce, async, generation-guarded, and renders
  into a passive surface (status bar). Typing never waits for it; a
  stale estimate is dropped, never shown. If the probe cannot be made
  imperceptible, it ships behind an off-by-default toggle rather than
  slow.
- Render-path work stays O(visible) and cached, like the grid's
  highlight cache.

## A. Cost honesty before Run (the headline)

Before the user runs anything, the status bar answers "what will this
cost": estimated rows/bytes to read, whether the primary key prunes,
FINAL in play. `EXPLAIN ESTIMATE` / plan-index reads are cheap and the
advisor already parses them after the fact; this moves the read to an
edit-pause probe (debounced on the analysis timer, log_queries=0,
never blocking). Display: one dim status-bar segment, e.g.
"~2.1 GB · PK prunes · 3 parts". Clicking it opens the advisor.
SELECT-only, current statement only, silent on anything it cannot
estimate. This is the only feature in the phase allowed to talk to
the server outside the sweep, under the contract above.

## B. Table functions

`system.table_functions` is the sibling catalog: s3, url, remote,
cluster, file, and friends, with descriptions on modern servers.
Completion in FROM position and hover on the call, same card shape as
functions (separator, quiet links). Die-hards live in `s3()`.

## C. Types and codecs

`system.data_type_families` (with case-insensitive alias flags) and
the codec set. Type position in CREATE completes real types; hovering
`DateTime64(3)` explains precision and range; `CODEC(` completes
Delta/DoubleDelta/Gorilla/ZSTD/LZ4 with a one-line "good for" each.
Codec guidance is stable knowledge (hand-written, like combinator
explanations); type names come from the server.

## D. Dictionary intelligence

`dictGet('dict', 'attribute', key)` is stringly-typed misery
everywhere else. `system.dictionaries` knows names, key types,
attributes, and status: complete the dictionary name and its
attributes inside dictGet/dictHas/dictGetOrDefault, squiggle a wrong
attribute, and hover a dictionary name for its card (source, layout,
status, last exception). Also fills Phase 16's honorable mention.

## E. Cluster and macro awareness

`ON CLUSTER ` completes from system.clusters (topology is already
fetched); hovering a cluster name shows shards/replicas. `{shard}`
and `{replica}` in Replicated paths validate against system.macros,
with completion of defined macros inside `{}` in ENGINE arguments;
the thing everyone gets wrong once per cluster.

## F. Type-time performance hints

The advisor's knowledge surfacing while typing, in the mold of the
PARTITION BY warning: `FINAL` on a table the snapshot knows is large;
`ALTER TABLE ... UPDATE/DELETE` noting it is an async mutation that
rewrites parts; a `PREWHERE` nudge where the advisor would suggest
one. Hints, never errors; each one names its reasoning like the
partition warning does.

## Also swept in (small, same pattern)

- FORMAT clause completion from system.formats with input/output
  capability flags (from IDEAS).
- system.* table literacy: system table columns carry rich comments
  server-side; the column sweep already stores comments, so hovering
  `system.query_log` columns explaining themselves may need only
  cache-priming for the system database (verify, then it is free).

## Order and shape

B, C, and the FORMAT clause are one pattern-stamping pass (catalog +
completion + hover + tests each); D and E add small context detection
like the SETTINGS clause did; F is analysis-only; A is its own animal
and lands last so the probe plumbing gets the most careful review.
Every feature ships with its unit tests and a window test where a new
surface appears, per the testing contract.

## Explicitly not in this phase

- Occurrence highlighting for columns/CTEs beyond the current scopes
  (fine as is until someone misses it).
- GRANT/RBAC intelligence (Phase 16 Track D stays last).
- Cross-version honesty needs no work: every check already runs
  against the connected server's own catalog. Verify with a saved
  tab against two server versions and then advertise it.
