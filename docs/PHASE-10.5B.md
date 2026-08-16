# Phase 10.5b: Cloud-truthful internals

Status: PLANNED. Child of `docs/PHASE-10.5.md`; the debts come from the
self-audit recorded in `docs/CLOUD-STRATEGY.md`. Every item here is a
correctness fix: zeDB currently shows Cloud users wrong or silently
partial numbers.

## Increment 1: unblock the cluster named `default`

- `features/operations/actions.rs` drops any cluster membership named
  `default`; that name is exactly what ClickHouse Cloud uses, so ops
  cluster scope, the scope dropdown, and node attribution are dead on
  Cloud. The filter's original intent (hide the degenerate self-only
  cluster on single-node installs) survives as a shape check (skip
  only when the cluster contains just the local node), not a name
  check.
- The fleet's own cluster list does not filter; after the fix the two
  surfaces agree.

## Increment 2: workload advisor reads every replica

- `crates/zedb-ch/src/workload.rs` reads bare `system.query_log`,
  which on a multi-replica service is one replica's slice presented as
  the whole workload. Fan out with `clusterAllReplicas(default, ...)`
  when a cluster exists, deduplicating by query id; fall back to the
  bare table when it does not (single node, no cluster access).
- The Workload tab states its scope either way ("all replicas" /
  "this node only"), so a partial view is labelled, never implied.

## Increment 3: SharedMergeTree-aware ops view

- Replication tab: on services whose tables are `Shared*MergeTree`,
  the ZooKeeper-era signals (`system.replication_queue`,
  `is_session_expired`, `system.zookeeper_connection`) are not the
  coordination surface and currently produce a green "replication
  healthy" regardless of reality. Detect the engine family and switch
  to what SMT does expose (parts, merges, mutations activity, and the
  SMT system tables that exist on the pinned Cloud builds), with an
  honest "coordination is Cloud-managed" strip instead of fabricated
  green.
- Storage tab: object-storage disks make a percentage-full bar
  meaningless. When disk metadata says object storage, show data size
  and growth instead of free-space percentage.
- Ingestion tab: name ClickPipes as a source; a Cloud user with pipes
  currently stares at an empty Kafka section. (Full ClickPipes API
  integration is 10.5c; this is just honest labelling from SQL-side
  signals.)

## Increment 4: engine-rewrite symmetry in regen

- `crates/zedb-ch/src/regen.rs` transplants `Replicated*MergeTree`
  prefixes during synthesis but not `Shared*`, asymmetric with
  `replay.rs` normalization; align them, and widen the replay regex to
  tolerate a bare `SharedMergeTree` without the two-argument
  coordination clause.

## Increment 5: service lifecycle honesty

- A deleted Cloud service currently fails as a plain timeout; a linked
  connection whose service id no longer appears in discovery gets
  marked dead with a saying-so message.
- Service state polls while the app is focused (gentle cadence, not
  only on refocus), so "running" cannot go stale while the user
  watches.
- KILL QUERY on a Cloud read-only connection explains the Cloud
  posture instead of the generic read-only message.

## Acceptance

- On a multi-replica Cloud service: ops cluster scope works, the
  workload tab says "all replicas" and its totals change accordingly,
  the replication tab never claims ZooKeeper health, and the storage
  tab shows no meaningless percentage.
- `zedb verify` and regen agree on a table Cloud rewrote to
  `SharedMergeTree`, both directions.
- Deleting a linked service in the Cloud console produces a marked-dead
  connection with a clear message on next refresh.
- All existing self-hosted behaviour is unchanged (the `default`
  filter's single-node intent is preserved by the shape check).

## Test notes

- Unit tests for the cluster shape check (single-node self cluster
  skipped; multi-replica `default` kept).
- Workload fan-out: fixture test that per-replica rows deduplicate by
  query id; label test for both scopes.
- Regen/replay symmetry: round-trip fixture with `SharedMergeTree`
  with and without coordination args.
- SMT detection: engine-family predicate unit tests.
