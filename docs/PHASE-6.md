# Phase 6: the ops view

Status: PLANNED. The committed slice of PHASE-6-IDEAS.md, chosen
2026-08-09. The competitive argument lives there; short version: a
good ops cockpit inside a good browser is a hole in the market, and
ClickHouse exposes everything needed as plain system tables the
existing read-only HTTP client already speaks.

## The one-sentence version

One glance answers "what is this cluster doing right now", and a
runaway query dies in two clicks.

## Shape

- A new per-connection top-level view (toolbar icon alongside query,
  fleet, and agent), visible only while connected.
- Everything is a small capped SELECT against system tables, polled
  on a short interval while the view is visible and not at all when
  it is not. Every panel wears an "as of" stamp.
- Read-only by construction, with exactly one action: KILL QUERY,
  and only where the connection's posture already allows writes.
  Read-only connections see the button disabled with an honest
  tooltip, mirroring the existing write-posture ladder.
- Sharding-aware from birth: where Phase 5 topology is known, panels
  can aggregate across shards via the cluster() function with a
  per-node column; unknown topologies show the connected node only.

## Milestones

- **M1: queries now.** system.processes: elapsed, user, memory,
  read rows/bytes, and the query text (truncated, expandable), with
  KILL QUERY behind the write posture. This is the two-clicks
  promise and stands alone as a shippable feature.
- **M2: background work.** system.merges (table, progress bar,
  elapsed) and system.mutations (command, parts remaining, stuck
  mutations surfaced first with latest_fail_reason). Answers "why is
  the disk churning".
- **M3: replication and storage.** system.replicas health flags
  (readonly replicas, absolute delay) and a replication_queue
  summary per table (depth, oldest entry age, last exception);
  system.disks capacity; top tables by on-disk size from
  system.parts. Answers "is anything quietly wrong".
- **M4: cluster-wide.** The same panels fanned across shards for
  known topologies (Phase 5 data + cluster()), with a node column
  and per-shard rollups.

## Non-goals

- Charts, history, and trends: system.query_log analytics belong to
  the EXPLAIN/history ideas, and Grafana exists. This view is *now*,
  not *lately*.
- Alerting or thresholds. It shows; the user judges.
- Any second mutating action beyond KILL QUERY. DROP/DETACH
  partition management is browser-territory for a later phase, if
  ever.

## Risks

- Poll load: every query is LIMITed and aggregated server-side; the
  interval pauses with the view hidden and backs off on errors.
- KILL QUERY on readonly=2 is refused server-side; the UI must not
  promise what the posture forbids.
- system.parts is huge on big clusters: aggregate by table with
  LIMIT, never row-per-part.
- Managed/Cloud variations in system tables: every panel degrades to
  "unavailable" quietly, never an error banner.

## Done when

Connected to the local sharded cluster during a heavy insert, the
view shows the running query, its merges, and both shards' disks
without touching the editor; killing a deliberate runaway takes two
clicks on a write-unlocked connection and is visibly impossible on a
read-only one.
