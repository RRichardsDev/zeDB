# Phase 16: what the die-hard is still missing

Status: CANDIDATES (2026-08-22). Not a commitment; a ranked map of
the gaps between zeDB-as-built (shaped by its author's own use) and
the daily life of a die-hard ClickHouse practitioner. Written after
the v0.1.34 security release, when the question "who else is this
for?" became worth answering deliberately. Pick one track for the
phase; the others stay here.

The frame: four personas, all the same person on different days.

## Track A: the performance whisperer (query_log analytics)

The biggest gap. The ops view answers "what is happening now"; the
workload tab answers "how is this table used"; nothing answers the
die-hard's actual daily question: "what happened over time, and why
was it slow?"

- Normalized query fingerprints (`normalizeQuery` /
  `normalized_query_hash` are built in) with p50/p95/p99 duration,
  memory, and read-bytes trends per fingerprint; error rates by shape;
  "what changed since yesterday".
- The killer feature: click any query (history drawer, ops view, or a
  fingerprint) and see its ProfileEvents breakdown, the testimony to
  EXPLAIN's prediction: rows/bytes read and why, index usage, time
  split across read/compute/network. "Why was this slow" answered
  from evidence, not vibes.
- Infra already in place: query history drawer, EXPLAIN/estimate
  surfaces to hang it off, and the ops polls now carry log_queries=0
  so query_log is not polluted by the watcher.
- Honest ceiling: query_log is per-node; cluster scope needs
  clusterAllReplicas like the ops view already does.

## Track B: the data wrangler (clickhouse-local on files)

The sleeper with unfair synergy: zeDB already ships, verifies, and
continuity-checks the ClickHouse binary (the pin cache); die-hards
already run `clickhouse local` against Parquet/CSV by hand daily.

- Drop a file (Parquet, CSV/TSV, JSON/NDJSON, ORC, ...) onto zeDB:
  it appears as a local table in the sidebar (schema via
  `DESC file(...)`, format auto-detected), queryable in the normal
  editor with completions, grid, and export.
- Same engine as production: the dialect, types, and functions match
  the user's server exactly, so a query prototyped on a local file is
  already valid for the eventual server-side INSERT SELECT. This is
  the DuckDB experience without the dialect translation tax, and no
  ClickHouse GUI has it.
- Local-to-server joins stay hands-on and app-mediated: the app can
  upload a local result into a temp table over its existing client
  when asked. The local process NEVER receives server credentials;
  this keeps the v0.1.34 boundary (secrets do not enter spawned
  processes) intact by construction.
- Infra already in place: pin cache with trust manifest and
  continuity digest, LocalReplay's process handling and bounded
  output, the grid, export, schema-provider completions.
- Open design question: one-shot `clickhouse local` per query
  (stateless, simplest) vs a session-scoped EphemeralServer over a
  scratch dir (enables multi-statement work and temp state; heavier).

## Track C: the cluster operator (the missing weekly rituals)

Three small-to-medium surfaces, each a known weekly ritual with no
home today:

- Settings drift: `system.settings WHERE changed` vs defaults, and
  the same setting compared across replicas ("do my nodes agree?").
  Small, and squarely inside the truthful-state review bar.
- Backups: `system.backups` status/history surface. Die-hards run
  BACKUP/RESTORE and zeDB cannot currently even show one exists.
  (Triggering them is mutation-ladder territory; showing them is
  not.)
- Part lifecycle forensics: `part_log` history (why did merges storm
  at 03:00), detached parts awaiting recovery. Extends the ops view
  backwards in time.

## Track D: the admin (RBAC), deliberately last

Users, roles, grants, quotas, row policies: a total blank in zeDB,
and the largest genuine gap on shared clusters. Ranked last on
purpose: it is also the largest liability surface (every write is a
security decision), and nobody has asked yet. A read-only "who can
touch this table" viewer is the only version worth considering before
real demand exists. Do not build write paths speculatively.

## Honorable mentions (stay unranked)

- `system.text_log` tailing: the tail machinery pointed at the
  server's own log; nearly free.
- Dictionary status / last-error / reload surface.
- Cross-connection schema diff (staging vs prod outside the
  migration repo's scope).

## Ranking, and why

A first: it is the heart of the craft this tool exists to serve, and
its absence is what a die-hard would notice within an hour. B second:
cheapest delight per unit work, unique among ClickHouse GUIs, and it
compounds A (local files join the same analysis surfaces). C third:
three small honest wins. D only on demand.
