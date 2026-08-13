# Phase 10.1: ClickHouse 26.6 `STREAM` spike

Test whether ClickHouse 26.6 streaming queries can extend Phase 10's instant
tail while retaining `WATCH` for older ClickHouse versions that still support
Live Views and preserving the existing polling path.

Status: IMPLEMENTED. Verification in progress. Builds on Phase 10
(`docs/PHASE-10.md`).

## Decision this phase must produce

This is a gated spike, not a promise to ship an experimental ClickHouse
feature.

Ship `STREAM` as an opt-in preferred instant-tail mechanism only if it:

- delivers every inserted row once in the tested tail model;
- works through zeDB's native TCP client on a read-only connection;
- stops promptly and leaves no query or connection behind;
- recovers from disconnects without losing rows;
- uses materially less repeated query work than 400 ms keyed polling; and
- falls back cleanly on older or unsupported ClickHouse servers.

If any of those gates fail, keep `WATCH` on servers that support it and retain
native fast polling plus HTTP polling. Native TCP remains useful independently
of `STREAM`.

## Why this is worth testing

ClickHouse 26.6 introduces experimental
[`SELECT ... FROM table STREAM`](https://clickhouse.com/videos/clickhouse-release-26-06)
queries for real-time monitoring. The older ClickHouse streaming-query
[RFC](https://github.com/ClickHouse/ClickHouse/issues/42990) describes the
intended direction as continuous queries over arriving data and identifies
Live Views and Window Views as mechanisms it could replace.

That is a substantially better fit for tail than the current instant-mode
prototype:

1. `upgrade_tail_instant` creates a temporary Live View.
2. `NativeClient::stream_blocks` holds a native connection open on
   `WATCH ... EVENTS`.
3. Each event triggers the existing keyed tail query.
4. Cleanup must drop the Live View.

The prototype therefore still runs a `SELECT` for every notification, needs
DDL privileges, and depends on an experimental, semi-deprecated object. A
direct streaming query could remove the DDL and the notification-triggered
read. The spike must prove that it actually does so.

## Standing constraints

- The normal tail remains useful on ClickHouse versions before 26.6.
- HTTP polling remains the universal fallback.
- `WATCH` remains the first fallback on servers that support Live Views.
- Native fast polling remains the next fallback when TCP is available.
- The `STREAM` path must not create, alter, or drop any database object.
  The older `WATCH` path still owns its temporary Live View and cleanup.
- `STREAM` is disabled by default and must be enabled in Preferences.
- One active tail may own one dedicated native connection. It must not consume
  a pooled connection indefinitely.
- The existing monotonic-key cursor remains the source of truth for catch-up,
  deduplication, and reconnects.
- Result retention stays bounded by the cap selected when the tail starts.
- No database or network work runs on the GPUI render thread.
- No Docker volume is deleted during the upgrade. In particular, never use
  `docker compose down -v` for this work.

## Increment 1: isolated 26.6 environment

Do not upgrade the existing 26.3 data volumes in place for the first test.
Downgrading data directories after a newer server has written to them is not a
safe rollback strategy.

1. Resolve the current official 26.6 patch for both
   `clickhouse/clickhouse-server` and `clickhouse/clickhouse-keeper`.
2. Pin the exact patch in all three compose locations: server, Keeper, and
   bootstrap. Do not leave a moving `26.6` tag in the committed file.
3. Stop the normal compose project without removing its volumes.
4. Start the same compose file under a separate project name, for example
   `docker compose -p zedb-clickhouse-266 up -d`. This creates fresh 26.6
   volumes while retaining the 26.3 volumes for rollback.
5. Confirm both servers, all three Keepers, replication, and bootstrap data are
   healthy.
6. Record the exact server build and image digests in the implementation
   notes.

Rollback is then mechanical: stop the 26.6 project, restore the 26.3 image
pin, and start the original project with its untouched volumes.

### Increment 1 acceptance

- Both nodes return the same 26.6 patch from `SELECT version()`.
- Ports 8123/9000 and 8124/9001 are reachable.
- `system.replicas` reports no read-only or session-expired replica.
- The bootstrap fixtures exist on both nodes.
- An ordinary HTTP query and an ordinary native TCP query still decode the
  same result.

## Increment 2: characterize `STREAM` outside the app

Before changing zeDB, use `clickhouse-client` and the `explain.tail_demo`
fixture to answer the questions below. Query `system.settings` and the server's
help output first, because the feature is experimental and its setting name
and defaults are part of what we are testing.

### Semantics

- Does a stream begin at the current end, replay existing rows, or do something
  selectable?
- Are newly inserted rows emitted in insertion order, primary-key order, or
  part order?
- Are rows ever duplicated when parts merge?
- Can `WHERE`, column projection, aliases, and `LIMIT` be used?
- Can a cursor predicate such as `WHERE order_id > last_seen` be used on
  reconnect?
- Does it work for local MergeTree, ReplicatedMergeTree, and Distributed
  tables, and what duplication occurs through a Distributed table?
- Does the query emit data rows directly, or only change notifications?

### Lifecycle and permissions

- Does it work for the compose user's read-only profile?
- Does cancelling the client remove the query promptly from
  `system.processes`?
- What happens when the server, Keeper session, or TCP connection disappears?
- Does the client receive a usable error when the feature is disabled?

### Cost

Run a controlled insert at 10 rows per second for 20 seconds and compare:

- HTTP polling at the baseline cadence;
- native polling at the fast cadence; and
- one native `STREAM` query.

Capture query count, selected rows and bytes, CPU time, memory, and observed
row latency from `system.query_log` and `system.processes`. The important claim
to prove is that streaming is one long-lived query and does not hide a repeated
table scan inside the server.

### Increment 2 acceptance

- Exactly 200 fixture rows arrive, with no missing or duplicate key.
- Median visible latency is below the fast-poll cadence.
- Cancellation clears the server query within two seconds.
- No Live View or other database object is created.
- The measured cost is documented before app integration begins.

If the 200-row correctness check fails, stop the phase here.

## Increment 3: native-client streaming primitive

The library surface belongs in `crates/zedb-ch/src/native.rs` and should remain
independent of GPUI and tail policy.

- Reuse the existing dedicated `NativeClient` connection and block decoder.
- Add explicit cancellation ownership rather than relying only on a callback
  returning `false`.
- Preserve query identity so cancellation and diagnostics can find the server
  query.
- Distinguish a clean user stop, an unsupported feature, a transport failure,
  and a server query error.
- Add a ClickHouse 26.6 integration test that consumes inserted fixture rows
  and then cancels the stream.
- Keep ordinary pooled native queries unchanged.

Before editing these symbols, run GitNexus impact analysis for
`NativeClient::stream_blocks`, `NativeClient::connect`, and any shared decoder
that changes. Warn before proceeding if the reported risk is high or critical.

## Increment 4: integrate ahead of the Live View tail path

Add the new instant-mode event source without deleting the existing one. Keep
the seed, keyed cursor, row cap, grid prepend, pause, update, and stop behaviour.

The target state machine is:

```text
Get instant updates
    -> opt-in STREAM on ClickHouse 26.6+ for a compatible direct table query
    -> WATCH Live View on a writable server that supports it
    -> native fast polling
    -> HTTP polling when native TCP is unavailable
```

Implementation rules:

- Gate the attempt by server version 26.6 or newer, then treat the actual query
  result as authoritative. Syntax probing alone is insufficient because 26.3
  parses trailing `STREAM` as the table alias `AS STREAM`.
- Build streaming SQL only for query shapes proven in Increment 2. Do not use
  fragile text replacement to inject the keyword into arbitrary SQL.
- Feed streamed rows through the same key extraction, ordering, deduplication,
  and bounded retention rules as polled rows.
- On reconnect, run one keyed catch-up query from `last_seen` before reopening
  the stream. This closes the disconnect gap.
- Stop, tab close, query update, and connection switch must cancel the old
  stream before starting another.
- Read-only connections must be allowed to use instant mode if the server
  permits `STREAM`.
- Preserve Live View creation, `WATCH`, generated-view naming, and drop-view
  cleanup for versions where that mechanism remains supported.

Expected primary integration points are:

- `Workspace::upgrade_tail_instant`
- `Workspace::start_tail_watch_consumer`, retained beside the new stream
  consumer
- `Workspace::start_tail_loop`
- `Workspace::tail_poll_once`
- `TailState` and `TailPush`
- `NativeClient::stream_blocks`

Run GitNexus impact analysis on every symbol before editing it. The current
graph shows `upgrade_tail_instant` is called from the tail strip and the edited
tail restart path, while `tail_poll_once` is shared by start, timer, resume, and
the current watch consumer. Those callers form the minimum regression set.

## Increment 5: fallback, lifecycle, and UX verification

Reuse the current instant-tail control and status strip. Before any UI change,
read `docs/UI-DESIGN.md` and extend the existing primitives rather than adding
a separate streaming control.

Required behaviours:

- ClickHouse before 26.6 never attempts `STREAM` and may continue to use
  `WATCH` when Live Views are supported.
- A disabled or rejected experimental feature falls back once, with one useful
  notice rather than repeated errors.
- Native connection loss catches up and resumes, or falls back to polling,
  without losing the tail.
- Pause and resume do not lose rows. The exact mechanism, buffering or
  cancel-and-catch-up, is chosen from Increment 2's backpressure results.
- Stop and tab close release the dedicated connection and clear the query from
  `system.processes` within two seconds.
- Editing and applying the tail query replaces the old stream without leaving
  a second server query.
- The status text distinguishes streaming, native fast polling, and HTTP
  polling accurately.
- A small experimental icon beside "Get instant updates" opens Preferences,
  where `STREAM CURSOR` can be opted into or disabled.

## Test matrix

### Automated

- SQL builder tests for every supported and rejected tail shape.
- Capability tests covering 26.3 alias parsing, 26.6 streaming, disabled
  experimental support, and server-query failure.
- Native integration test for receive, cancellation, and reconnect.
- State tests for fallback ordering and generation-based stale-result rejection.
- Existing tail and native query tests remain green.
- `cargo fmt --check`, focused crate tests, then the full workspace tests.

### Manual

- Tail `explain.tail_demo`, insert 10 rows per second for 20 seconds, and see
  exactly 200 new rows.
- Repeat while instant mode is not selected to verify the polling baseline.
- Pause halfway through, resume, and verify no gap or duplicate.
- Stop one ClickHouse node, restore it, and verify catch-up.
- Tail through Node 1 and Node 2 separately.
- Repeat with a read-only connection.
- Close the tail tab and confirm no matching row remains in `system.processes`.
- Confirm no `zedb_tail_*` objects exist in `system.tables`.

## Observability required for the decision

For the spike, log or expose enough state to answer:

- selected tail transport;
- server version and why streaming was accepted or rejected;
- native query id;
- reconnect and fallback count;
- last accepted key and rows discarded as duplicates; and
- time from insert to row delivery in the controlled test.

Permanent debug UI is not required. Structured application logs and repeatable
SQL measurements are sufficient for the decision.

## Expected files when implementation lands

- `docker-compose.yml`: exact 26.6 server and Keeper patch.
- `crates/zedb-ch/src/native.rs`: cancellable continuous query support.
- `crates/zedb-ch/tests/native.rs`: 26.6 streaming integration coverage.
- `crates/zedb-app/src/tail.rs`: supported streaming query construction and
  cursor rules.
- `crates/zedb-app/src/main.rs`: tail transport state and lifecycle integration.
- `docs/PHASE-10.md`: update the mechanism decision after the spike.
- `docs/devlog.md`: measurements and internal implementation notes.
- `CHANGELOG.md`: user-facing instant-tail behaviour under `## Unreleased` if
  the feature ships.

## Final ship or stop review

Before declaring the phase complete:

1. Run `detect_changes()` against `main` and inspect every affected execution
   flow.
2. Confirm the full test matrix on a clean 26.6 compose project.
3. Compare the measured cost and latency with both polling modes.
4. Record unsupported SQL shapes and server configurations.
5. Decide explicitly:
   - **Ship:** opted-in `STREAM` becomes the preferred instant mode, with
     automatic fallback through `WATCH` and polling.
   - **Hold:** keep it behind a development-only switch for more ClickHouse
     releases.
   - **Stop:** remove the `STREAM` path, retain `WATCH` and polling.

Experimental status alone does not force a stop, but correctness, cleanup, and
fallback are non-negotiable.
