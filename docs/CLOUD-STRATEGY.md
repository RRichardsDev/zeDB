# zeDB and ClickHouse Cloud: the case for being the tool

Written 2026-08-16 from the Phase 10.5 investigation: a live auth
experiment against a real service, a sweep of the Cloud management API
surface, a competitive landscape review, and a feature-by-feature audit
of zeDB's own Cloud behaviour. Sources and detail live in the phase
docs and devlog; this file is the argument.

## The thesis

Nobody has built a Cloud-native ClickHouse client. Every desktop tool
on the market treats a Cloud service as a dumb HTTPS endpoint: no org
or service awareness, no wake-from-idle handling, no Cloud auth, no
SharedMergeTree awareness in system queries, and (for the JDBC crowd)
recurring driver breakage. ClickHouse's own console is a browser tab
with basic charts, no alerting, no migration story, no local files, one
org at a time; their new CLI is ops-shaped, not a daily driver. The
lane "keyboard-driven native client that genuinely understands Cloud"
is empty.

zeDB is already closer than anything shipping: service state badges,
wake-before-connect, idle-aware probes, Cloud build pins in the
migration harness, and Shared*MergeTree rewrites normalized in drift
detection. The distance from "closer than anyone" to "obviously the
best" is measured and finite, and most of it is listed below.

## What the investigation established

1. Auth. The device flow works against auth.clickhouse.cloud with a
   public client id (proven live: token acquired, org listed). The
   data plane itself validates JWTs: our probe failed only on "no user
   with such name", meaning a JWT-identified database user closes the
   loop to Snowflake-class passwordless sign-in. Management writes
   (wake included) still need an API key. The read-only-ness of OAuth
   is upstream policy, not a missing scope: the device flow's scopes
   are identity scopes, authorization hangs on the audience, and
   ClickHouse set the clickhousectl audience to read-only; even key
   creation needs an existing admin key (bootstrap is console-only by
   design). Standing asks to ClickHouse: a zeDB client id, a
   write-capable audience or a key-bootstrap-from-OAuth endpoint,
   JWT-mapped database users, and warehouse names in the services
   API (only dataWarehouseId is exposed; the console-side warehouse
   name is unreachable). Softener: querying an idle service
   wakes it in both auth modes, so OAuth-only users wake services by
   connecting; only the explicit start command is key-gated.
2. Control plane. The management API exposes per-service cost
   (usageCost), credit balances, a full typed audit log, Prometheus
   metrics per service, backup config plus restore-as-new-service,
   ClickPipes CRUD with schema discovery, warehouse (compute-compute)
   creation, an explicit awake command, and schema-validated server
   settings. Almost none of it is surfaced by any client tool.
3. The console's own gaps. No alerts, basic charts, no schema/version
   tooling, no local file joins, single org. Query insight exists in
   the console but has no API; zeDB's SQL-side workload analysis is
   the only path any external tool has, and zeDB's is already built.
4. Migrations are wide open. Every existing option is a generic
   framework driver or a young single-vendor tool; none is
   Cloud-aware. zeDB's fleet system already handles Cloud build pins
   and engine rewrites; it is the only migration tool that replays
   chains through the real pinned binary.

## The three moats to build on

- Migrations as a product (fleet): already ahead, no competitor.
- Workload-measured advice (indexes, projections, storage codecs,
  pre-flight cost): no console equivalent, no API for others to copy
  from cheaply.
- The daily driver itself: native, offline-capable schema cache,
  million-row grids, tails, agent pane with the user's own agent under
  read-only caps.

## Cloud-truthfulness debts found in our own code

The self-audit found places where zeDB is wrong or silently degraded
on Cloud today, in rough order of impact:

1. The ops view's cluster fan-out filters out any cluster named
   `default`, which is exactly Cloud's cluster name: cluster scope,
   the ops dropdowns, and node attribution are dead on Cloud
   (features/operations/actions.rs).
2. The workload advisor reads system.query_log on whichever replica
   the load balancer picks: on multi-replica services it reports a
   fraction of traffic as if it were all of it (needs
   clusterAllReplicas fan-out).
3. The replication tab measures ZooKeeper-era signals; on
   SharedMergeTree it reports healthy regardless. Needs an SMT branch
   (and the storage tab's disk-percentage bar is meaningless against
   object storage).
4. Cloud service prefill discards the nativesecure port the control
   plane just told us (instant tails then rely on runtime discovery),
   and the read-only default silently disables Tier-3 measured codec
   savings and KILL QUERY with generic messaging.
5. Regen synthesis recognizes Replicated* engines but not Shared*,
   asymmetric with drift normalization.
6. A deleted Cloud service fails as a plain timeout instead of being
   marked dead. Service state also never refreshes while the app sits
   focused and idle.

## Roadmap shape

- 10.5a: the front door. Device-flow sign-in, unified Add Connection,
  service discovery, JWT-first full access (pending the mapped-user
  experiment), password provisioning and paste as fallbacks, yellow
  Cloud border.
- 10.5b: Cloud-truthful internals. Fix the debts above; teach ops,
  workload, and advisors what SharedMergeTree and object storage are.
- 10.5c: control-plane surfaces nothing else has. Cost of this
  service in the status bar with burn-rate warnings; pre-flight
  estimates phrased in wake/compute terms; the audit timeline beside
  ops; wake-before-connect everywhere; backup-restore-as-scratch
  wired into fleet ("rehearse this migration on a restored copy").
- 10.5d: agent and MCP. Cloud control-plane context (state, tier,
  cost) exposed read-only to the in-app agent, and the byte caps
  re-reasoned as billing ceilings.

Everything here stays inside docs/PRODUCT-PRINCIPLES.md: hands-on
first; the agent surfaces stay read-only or propose-only.
