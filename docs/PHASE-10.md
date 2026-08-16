# Phase 10: Live tail

A `tail -f` for a ClickHouse table. ClickHouse eats logs and events;
this turns the explorer into a real-time console (north-star #5,
`docs/NORTH-STAR.md`). Reuses the streaming execution + progress plumbing
already built.

Status: IN PROGRESS. Phase 10.1 (native instant-tail transports) is
COMPLETE and shipped its decision: opt-in `STREAM` is the preferred
instant mode. Follows Phase 9 (doc retired; history in the devlog and releases).

**Increment 1 DONE** (branch `phase-10-live-tail`): poll-over-HTTP tail from
the schema sidebar's table context menu, on the leading ORDER BY key, off
the main thread. Retained-row cap chosen up front (20/50/100/500/1000/
Unlimited); the initial load is always ~20 rows. Newest-first (rows land at
top, oldest trimmed past the cap), Pause / Resume / Stop, and a live strip
above the results. Native-port discovery (9440/9000, off-thread) surfaces a
"Get instant updates" button. The switch now prefers opt-in ClickHouse 26.6
`STREAM CURSOR`, retains Live View `WATCH` on versions that support it, and
falls back to fast native polling. Core SQL/key logic lives in
`crates/zedb-app/src/tail.rs` (unit-tested). Still open below.

## Mechanism: layered delivery (amended by Phase 10.1)

The monotonic-key polling path remains the universal baseline. Native TCP adds
optional lower-latency paths without making the tail depend on them:

- **HTTP polling.** A periodic
  `SELECT ... WHERE key > :last ORDER BY key LIMIT n`, cadence ~1-2s.
  Each poll is an ordinary query over the **HTTP interface zeDB already
  uses** (reqwest + rustls). No new protocol, no persistent socket. It
  is robust for free: it survives proxies, idle timeouts, and reconnects
  because there is nothing long-lived to drop. For the log/event use case
  this remains a dependable default.
- **Native fast polling.** The same keyed query can ride a pooled native
  connection at a shorter cadence when push is unavailable.
- **Live View `WATCH`.** On writable ClickHouse versions that still support
  Live Views, one dedicated native connection receives change events and
  triggers the keyed fetch. This path remains for compatibility.
- **Experimental `STREAM CURSOR`.** When explicitly enabled in Preferences,
  ClickHouse 26.6 or newer can return inserted rows directly for compatible
  single-table tails. A persisted block cursor supports exact resume. Any
  unsupported version, query shape, or server rejection falls through to
  `WATCH`, then native polling.

Note: "TLS vs HTTP" is not the real axis; TLS is just encryption and
rides on either interface. The axis is native TCP vs HTTP. HTTP remains the
baseline; native TCP is an optional latency upgrade.

## The one thing that matters for cost

**Tail on a monotonic key, not `OFFSET`.** Track the last-seen value of
an incrementing column (a timestamp or an ID) and poll
`WHERE key > :last`, so each poll prunes by the primary key instead of
rescanning from the top. `OFFSET`-based paging would rescan and get
quadratically worse as the tail runs. This keeps it cheap regardless of
protocol.

- Pick the monotonic key: prefer a column in the table's `ORDER BY`
  (`system.tables.sorting_key`); let the user override.
- Handle the empty case (no new rows -> no-op poll) and the burst case
  (`LIMIT n`, keep the last key, continue next poll).

## Constraints (standing)

- **Off the main thread.** Polling runs on a timer through the async
  query path and delivers rows back to the entity; the render thread
  never blocks. Start/stop cleanly; stop the timer when the view closes.
- Reuse the streaming result rendering; append rows, cap the retained
  buffer (e.g. keep the last N thousand) so a long-running tail does not
  grow unbounded.

## Open questions

- Auto-scroll vs pause-on-scroll-up (follow tail, but let the user pin).
- Filter while tailing (add a `WHERE` on top of the key predicate).
- Multiple concurrent tails (per query tab?).
