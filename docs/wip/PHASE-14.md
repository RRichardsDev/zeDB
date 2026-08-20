# Phase 14: the ops view on the fast transport

Status: PLANNED (2026-08-20). The ops view still does what it did the
day it shipped: re-run a handful of `SELECT`s against system tables
over HTTP every 2 seconds (`POLL_SECS`, `features/operations`). The
native TCP client, its pool, and the discovery that finds a native
port all exist and work, but only the tail uses them. This phase puts
the ops view on the same transport ladder the tail climbs.

## What transfers from the tail, and what does not

The tail's layered delivery (Phase 10.1) is three rungs: HTTP polling
as the universal baseline, native TCP polling for lower latency, and
server push (`STREAM CURSOR` on 26.6+, Live View `WATCH` where
supported) when the shape of the data allows it.

The first two rungs transfer to the ops view. **The third does not,
and the reason is the data, not the plumbing**: push semantics need an
append-only table with a monotonic cursor. The ops view reads
transient snapshots, where rows appear *and vanish* as queries finish
and merges complete. Checked on 25.8: `system.processes` is engine
`SystemProcesses`, `system.merges` is `SystemMerges`,
`system.replication_queue` is `SystemReplicationQueue`. None are
MergeTree-family, none carry a cursor, and a disappearing row cannot
be expressed as a stream increment. (25.8 has no `STREAM` keyword at
all, so the negative could not be tested end to end there; confirm
against the 26.6 dev cluster before the doc claims it as tested.)

So this phase is honest about its ceiling: **fast polling, not push**.
Anything claiming otherwise in the UI would be a lie by the review
bar's first rule.

## Slice 1: native transport for the ops polls

- When the connection has a native port (the tail's discovery already
  finds and caches it), run the ops queries through the pooled
  `NativeClient` instead of HTTP; fall back to HTTP silently when
  there is no port, the handshake fails, or the socket dies.
- One pooled connection serves the whole view rather than N HTTP
  requests per tick; `is_read_statement` keeps the read-only posture.
- No behaviour change beyond speed: same queries, same cadence, same
  rendering.

## Slice 2: spend the savings

A native poll costs a fraction of an HTTP round trip, which buys
either a faster view or a cheaper one. Decide with numbers, not
taste: measure a tick's wall time and bytes on both transports first.

- Candidate: tighten the cadence for the cheap queries (processes,
  merges) while leaving the slow ones on their 5-tick divisor.
- Candidate: back the cadence off when the window is unfocused, the
  way the Cloud state refresh already does.
- On Cloud, fewer requests also means less paid compute; if slice 2
  measurably lowers a warehouse's burn, say so in the release notes.

## Slice 3: say which transport is live

The tail already tells the user when it is on the fast path. The ops
view should do the same, in the ops header, in the same quiet
register: native vs HTTP, and (if the numbers earn it) the observed
tick time. One line, no badge spam, absent rather than guessing when
the transport is still being discovered.

## Deliberately not in this phase

- `STREAM CURSOR` / `WATCH` for the snapshot tables, per the reasoning
  above. If a future ops surface reads an append-only table
  (`query_log`, `part_log`, a merges *history* rather than the live
  merge set), that surface can climb the third rung on its own merits;
  it is a different feature, not this one.
- Any change to what the ops view shows. This phase moves bytes, not
  meaning.

## Acceptance

- With a native port available, the ops view polls over TCP and says
  so; killing the native path (or connecting to a Cloud service with
  no native port) falls back to HTTP with no visible breakage and no
  stale claim in the header.
- A measured before/after for one tick (wall time and bytes) is
  recorded in the devlog; slice 2's cadence change cites it.
- The claim about push semantics is verified against the 26.6 dev
  cluster before it appears in any doc or UI copy.
- Everything in `docs/wip/PHASE-13.md`'s "Review bar" applies here,
  in particular: no label may lie (the transport indicator must never
  claim native while falling back), transitions are watched and
  bounded (discovery is async: absent, not guessed), and upstream
  behaviour is tested before it is encoded.
