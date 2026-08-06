# Phase 3.2 plan: schema intelligence

Goal: the query editor knows the schema. A per-connection object cache
(databases, tables, columns, types, engines, comments) makes typing a
query feel informed: invalid table and column names are visibly wrong,
valid ones complete as you type, and hovering an identifier says what
it is. The defining constraint is the product's defining constraint:
snappy. Nothing on the keystroke path ever waits on the network or a
lock; the cache answers from memory or not at all.

## Performance budgets (the point of the phase)

- Identifier lookups are in-memory hash lookups, sub-millisecond,
  never blocking the render thread.
- No network on the keystroke path, ever. Scans, refreshes, and
  invalidation fetches happen on background tasks and land as atomic
  cache swaps.
- Warm-up is hybrid: databases and tables load eagerly the moment a
  connection lands (one system.tables sweep, cheap even on big
  clusters); columns load per-database in the background, prioritized
  by what the user touches (schema tree selection, databases named in
  the editor). A million-column production cluster must not get a
  million-row system.columns query on connect.
- The cache persists per connection under the app data directory, so
  a relaunch starts warm from the last snapshot and refreshes behind
  the scenes; cold start never blocks on a scan.

## Freshness

- DDL the user runs through the editor invalidates the affected
  objects immediately (statement classification reuses the existing
  split/parse machinery; on success, the touched database refreshes in
  the background).
- A periodic background re-scan on the health-poll cadence picks up
  changes made elsewhere; the schema tree's manual refresh also feeds
  the cache. Staleness between ticks is acceptable and must degrade
  politely: a table that exists but is not yet cached must never be
  flagged as wrong loudly.

## Working rules

- Same as before: milestones end buildable, on main, demoable in 30
  seconds; riskiest first; devlog as we go; UI per docs/UI-DESIGN.md.
- Cache core lives in zedb-ch (engine-specific, headless, tested
  without a GUI); the editor consumes it through the same interfaces
  any client could.
- False positives are worse than false negatives everywhere: when
  alias resolution or staleness leaves doubt, say nothing.

## Milestones

### M0. The cache core

SchemaCache in zedb-ch: the data model, the hybrid warm-up, the
per-connection disk snapshot (load, refresh, atomic swap), touch-based
column prioritization, DDL invalidation hooks, and the periodic
refresh, all headless with tests including a synthetic
hundreds-of-databases fleet. The budgets above are asserted in tests
where they can be (lookup cost, no-network-on-lookup by construction).

Done when: connect on the demo cluster yields a fully warm cache
before a human could start typing, a synthetic large fleet warms
tables-first without hammering columns, and a relaunch answers from
the snapshot instantly while refreshing behind the scenes.

### M1. Validity marking

The editor flags unknown identifiers: table and column references
extracted from the existing tree-sitter parse, checked against the
cache off-thread with a debounce, rendered as a subtle wrongness cue
(not an error explosion). Alias- and CTE-aware enough to stay quiet
when unsure; uncached-but-real must read as neutral, not wrong.

Done when: typing a misspelled table reads as wrong within a beat,
fixing the spelling clears it, aliases and CTEs do not false-flag,
and keystroke latency is indistinguishable with the feature on.

### M2. Autocomplete

Completions from the cache through gpui-component's existing
completion machinery (the input's LSP-shaped layer, fed by an
in-process provider instead of a language server): databases and
tables after FROM/JOIN/INTO, columns after a resolved alias or table
qualifier, keyboard-driven, ranked simply (prefix match, then touch
recency). No network, no debounce-lag popup.

Done when: FROM ze<tab> lands a table on the demo fleet instantly,
alias.col completes columns for the right table, and the popup never
stalls the editor.

### M3. Hover info

Hovering a known identifier shows what the cache knows: type and
codec for columns, engine and row estimate for tables, comments where
present, through the input component's existing hover machinery.

Done when: hovering a column answers the what-type-is-this question
without leaving the editor, and hovering something unknown shows
nothing rather than a shrug.

### M4. Scale soak and polish

The synthetic large fleet and the real imported repo's cluster,
driven daily: cache size bounds (evict cold databases beyond a cap),
snapshot format versioning, a quiet cache status line in the schema
pane (warmed N of M databases), and whatever real typing surfaces,
fixed or parked in IDEAS.md.

Done when: schema intelligence on a hundreds-of-databases connection
feels identical to a two-database one, and the feature never once
makes the editor feel slower than before the phase.

## Order and dependencies

M0 → M1 → M2 → M3 → M4. Independent of Phase 3.1; whichever lands
first, the agent pane's ClickHouse tools and the cache share nothing
but the client, deliberately (the cache serves keystrokes, the tools
serve conversations; coupling them would put a conversation's load on
the keystroke path).

## Explicitly not in Phase 3.2

- Semantic SQL validation (type checking, function signatures); the
  cache knows names, not semantics.
- Query linting or formatting.
- Cross-connection caches or sharing; one cache per connection.
- Feeding the cache to agents (they have their own tools with their
  own budgets).

## Phase exit

Phase 3.2 is done when M4's done-condition holds in real use. It
slots between the agent pane and the Phase 4 runners decision, and
whichever order 3.1 and 3.2 actually land in, both must hold the same
line: the UI stays instant.
