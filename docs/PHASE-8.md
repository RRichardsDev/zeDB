# Phase 8: Column-level storage intelligence

Make the columnar nature of ClickHouse a first-class, visible thing:
per-column compression, and an **explainable, rule-based** codec
advisor. This is north-star differentiator #1 (see `docs/NORTH-STAR.md`)
and the closest "whoa" feature to what is already shipped (the storage
tab and the type-colored columns tab).

Status: PLANNED. Not started.

## Non-negotiables

- **Rules are the driver; AI is optional, never the main thing.** The
  advisor is a deterministic rules engine and must work fully with zero
  AI, that is the default and the core. An **optional** agent hand-off is
  welcome for users who want it, exactly like the error bar's "ask agent"
  flow (silent, opt-in, their own agent): e.g. "explain this suggestion"
  or "ask my agent about this column." It is off by default, never
  required, and never the driver. This keeps the product spine
  (`docs/PRODUCT-PRINCIPLES.md`): explainable recommendations the user
  chooses to apply, hands-on first, with the agent available where they
  were already reaching for it, not an upsell. Building the rules first
  also keeps the core feature small.
- **Nothing heavy on the main thread.** Any work that touches data (the
  cardinality probe, any codec trial) runs through the existing async
  ClickHouse query path and delivers results back to the entity; it must
  never block the gpui render thread. The rules engine itself is a
  `match` over a few dozen columns (trivial CPU) and may run inline; the
  moment something scans rows, it goes off-thread. See "Threading" below.
- **Probes are opt-in and cached.** The base view (sizes, codecs, ratio)
  is instant because it reads `system.columns` only. The cardinality
  scan runs only when the user asks ("Analyze cardinality"), and its
  result is cached per (table, part-set) so it is not re-run on every
  glance.
- **Conservative and explainable suggestions.** Only fire a rule we are
  sure of, and always show *why* it fired. A wrong codec suggestion
  erodes trust fast; silence beats a bad guess.

## The data (queries only, no scans in Tier 1)

- **Per-column size + codec**: `system.columns` already carries
  `data_compressed_bytes`, `data_uncompressed_bytes`, and
  `compression_codec` per column (aggregated over active parts). One
  query per table. Ratio = uncompressed / compressed.
- **Cardinality (Tier 2, opt-in, off-thread)**: one pass,
  `SELECT uniqCombined(col1), uniqCombined(col2), ... FROM t`, gets every
  column's distinct estimate in a single scan. `uniqCombined` is
  approximate and cheap-ish; still a full scan, so opt-in + cached +
  async. Distinct-ratio = distinct / total_rows.
- **Real measurement (Tier 3)**: trial a candidate codec by creating a
  temp table with that codec, inserting a sample, and reading back the
  compressed size. Turns a heuristic estimate into a measured one. Also
  off-thread. This is where the real complexity lives, but it is part of
  Phase 8, not a someday-maybe: the advisor's numbers should ultimately
  be measured, not guessed.

## The rules engine (pure logic)

Input per column: data type, total rows, distinct estimate (if probed),
current codec, current ratio, and whether the column is in the table's
`ORDER BY`. Output: an optional suggestion + a one-line reason + a
copyable `ALTER TABLE ... MODIFY COLUMN ... CODEC(...)`.

Start with a few high-confidence rules only:

- low distinct-ratio `String` (and not already `LowCardinality`) ->
  `LowCardinality(String)`.
- `DateTime`/`Date`, or an integer that is the `ORDER BY` key ->
  `Delta, ZSTD` (or `DoubleDelta` for steadily increasing).
- float time-series -> `Gorilla`.
- already high-entropy / high existing ratio -> "leave it; a heavier
  codec buys nothing" (an explicit no-op suggestion is a feature).

Size-drop estimate in Tier 2 is heuristic and must be labelled as such
("typically ~Nx"); Tier 3 replaces it with a measured number.

## Tiers (all three are Phase 8; each is shippable on its own)

Phase 8 delivers all three. They are sequenced, not optional: ship Tier
1, then Tier 2, then Tier 3, each releasable on its way so the phase
lands in usable increments rather than one big drop.

**Tier 1 — see your compression.** One `system.columns` query; render
compressed / uncompressed / ratio / codec in the existing columns tab.
No cardinality, no suggestions. Already beats generic tools. Small.

**Tier 2 — the advisor.** Add the opt-in cardinality probe (off-thread,
cached) and the rules engine with copyable `ALTER` DDL. The "whoa." The
rules are little code; the effort is honest thresholds.

**Tier 3 — measured, not estimated.** Codec trial via temp table for
real size numbers, replacing Tier 2's heuristic estimates with measured
ones. This is where the complexity is, and it is what makes the advisor
trustworthy rather than hand-wavy, so the phase is not done until it
lands.

Sequencing: **Tier 1** first (one sitting, ship it), then Tier 2, then
Tier 3.

## Threading

The app already runs ClickHouse queries asynchronously and streams
results back to the UI entity (the query execution + streaming-progress
path). Phase 8 reuses that path:

- Tier 1's `system.columns` query is small but still goes through the
  async query path like any other query; results land via the normal
  entity update, not a blocking call.
- The Tier 2 cardinality probe and any Tier 3 codec trial are ordinary
  async queries dispatched on demand; the UI shows a pending state and
  updates when they return. No `uniq*` or trial runs on the render
  thread, ever.
- The rules engine is pure CPU over the already-fetched column rows
  (tens of items); it runs inline when building the view. If a table
  ever has a pathological column count, revisit, but this is not a real
  concern.

## Open questions

- Cardinality cache key: per (table, max part, row count)? Invalidate on
  detected schema/parts change.
- `ORDER BY` membership: read from the table DDL / `system.tables`
  `sorting_key`.
- How to present the suggestion: inline in the columns tab, or a
  dedicated "storage advisor" panel? Lean on the existing storage tab.
- Later: feed accepted suggestions into the drift -> migration flow
  (`docs/PHASE-7.2-IDEAS.md`) so an `ALTER` can be staged safely rather
  than pasted.
