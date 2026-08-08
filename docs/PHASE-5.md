# Phase 5: sharding awareness

Status: PLANNED, not started. The migration engine is already
cluster-aware (${cluster} templating, ON CLUSTER rendering,
system.clusters replica discovery, declustered single-node checks);
this phase brings the interactive side up to the same honesty.

## The problem

zeDB speaks to one node at a time, which is exactly right for a
sharded ClickHouse cluster queried through Distributed tables: the
server scatters and merges, and zeDB needs no sharding knowledge.
The dishonesty is at the edges:

- A connection's node list conflates interchangeable endpoints
  (load balancers, replicas) with nodes of *different shards*. For a
  local table, switching nodes (by hand or via health failover)
  silently switches which slice of the data you see. This is the
  sharpest edge and the reason this phase exists.
- Schema-sidebar sizes and row counts come from the connected node's
  system.tables: one shard's share presented as if global, and
  usually nothing at all for the Distributed table actually being
  queried.
- Filter probes and hover metadata are similarly node-local truths.

## Shape

Topology is read, never configured: system.clusters on the connected
node already knows the shard/replica layout. zeDB displays what the
server says and stops implying equivalence it cannot verify. No new
protocol, no scatter-gather in the client, ever; Distributed tables
remain the query path.

## Milestones

- **M1: topology discovery.** On connect, read system.clusters and
  match the connection's endpoints to shard/replica entries where
  possible (by host). Store per-connection: cluster name(s), shard
  count, which configured nodes belong to which shard, and which are
  unmatched (LBs, Cloud). Zero UI change; the data rides the
  connection state.
- **M2: honest node picker.** Group the node dropdown by shard
  ("Shard 1: node-a, node-b / Shard 2: node-c"), and when the user
  switches to a node of a *different shard* (or health failover
  does), surface a one-line notice: local-table queries now see a
  different slice. Unmatched/LB nodes behave exactly as today.
- **M3: honest sidebar.** Badge Distributed tables (engine is
  already known) and, for local tables on a multi-shard cluster,
  label size/rows as this-shard values. Optionally sum shard sizes
  fleet-wide via one system.tables query per shard, cached like the
  rest of the schema cache.
- **M4 (only if wanted): topology view.** A read-only
  shards-and-replicas panel on the connection screen, powered
  entirely by M1 data. Pure presentation.

## Non-goals

- Client-side scatter/gather or cross-shard query rewriting: that is
  the Distributed engine's job.
- Any sharding configuration UI: zeDB reads topology, it does not
  define it.
- ClickHouse Cloud awareness beyond "no visible shards, nothing to
  show": Cloud abstracts this correctly already.

## Risks

- system.clusters can list clusters the connection's nodes are not
  part of, and hostnames may not match configured endpoints (DNS
  aliases, LBs, ports). Matching must degrade gracefully to
  "unknown", which behaves exactly like today.
- The M2 slice-change notice must not fire for replicas of the same
  shard or matched LBs, or it becomes noise and trains users to
  ignore it.

## Done when

On a real sharded cluster, the node picker shows which nodes are the
same data and which are not, switching shards on a local-table query
says so once, and the sidebar never presents a shard-local number as
a global one.
