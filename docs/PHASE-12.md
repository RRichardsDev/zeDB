# Phase 12: The distributed and workload layer

The next round of north-star differentiators, extending the same
thesis as Phases 8-10: model what ClickHouse actually is (columnar,
MergeTree, distributed) instead of SQL-in-general. These four are new
additions beyond `docs/NORTH-STAR.md`'s original ranked list; each is
"the next layer" on infrastructure that already exists, not a green
field.

Status: BUILT (first increments, branch `phase-12`). All five
sections shipped an increment: D (estimate command + strip), E (Cloud
linking, service list with state, prefilled add, start-idle, probe
explanation, sidebar idle marker), A (Workload tab on the table
inspector), B (ops health strip + Keeper sessions), C (ops Ingestion
tab). Deliberately not built yet: E's API-minted database credential
(open question below), A's cross-table advisor surface, C's failures
drawn onto the Dependencies DAG. Migrations (north-star #6) stay
held; see the note at the end.

Ranked below by reward-per-infrastructure, except E (ClickHouse Cloud
linking), which is a different axis entirely: adoption rather than
capability; every differentiator is worthless to a user who bounced
off setup.

## A. Skip-index and projection effectiveness, measured (start here)

Phase 9's advisor diagnoses one query. The next class up is grounding
advice in the user's **actual workload**: aggregate
`system.query_log` ProfileEvents over real traffic and answer
"is this index/projection earning its keep?"

- Per skip index: how often queries touched its column in a `WHERE`,
  and how much it actually pruned (`SelectedMarks` vs total across
  matching queries). Surface "this `bloom_filter` never prunes
  anything" and "this `minmax` carries 40% of your reads."
- Per projection: hit rate from `projections` in `query_log` /
  EXPLAIN, vs its storage cost (Phase 9C already lists projections
  and Phase 8 already measures sizes; this joins the two).
- Inverse direction: frequent filter columns with **no** index at
  all, ranked by rows that would have been saved. This upgrades the
  Phase 9 skip-index suggestion from "this query would benefit" to
  "your last 7 days of traffic would benefit."
- Output in the existing advisor voice: explainable finding, the
  DDL (`ALTER TABLE ... DROP INDEX` / `ADD INDEX` /
  `ADD PROJECTION`), never auto-applied.

Why first-in-class: every advisor anywhere (including ours today)
reasons from a single query. Workload-measured effectiveness needs
query_log modelling plus the storage and projection surfaces we
already built. Nothing else has the pieces.

Rough build: a bounded `query_log` aggregation (time window picked by
the user, default 7 days), joined against `system.data_skipping_indices`
and `system.projections`, feeding the Phase 8-style rules engine.
Cache it; the aggregation is the expensive part.

## B. Replication and Keeper health

The distributed twin of the ops view. During an incident this is what
people open three terminals for.

- **Replica status** per table: `system.replicas` fanned via
  `clusterAllReplicas()` (the read path Phase 5/6 built). Absolute
  delay, queue size, `is_readonly`, `is_session_expired`, last
  exception.
- **Replication queue**: `system.replication_queue` entries with age,
  retries, and the stuck ones highlighted (`num_tries` high,
  `last_exception` set).
- **Keeper session state**: connected/expired per replica, and
  `system.zookeeper_connection` where available.
- A cluster-level health strip in the ops view: green until a replica
  is readonly, lagging past a threshold, or has stuck queue entries.

Why first-in-class: generic tools cannot even see replication; it is
not SQL-in-general. Same read-and-render muscle as the ops view.

## C. Ingestion visibility

"Where did my rows go" is the top ClickHouse support question and
nothing visualizes it.

- **Kafka consumers**: `system.kafka_consumers` (assignments, lag-ish
  signals, last poll, exceptions per consumer).
- **MV insert failures**: `system.query_views_log` for materialized
  views that threw during insert, surfaced on the Dependencies tab's
  DAG (Phase 9C) so a failing edge is visible where the lineage
  already is.
- **Async insert queue**: `system.asynchronous_inserts` (pending
  batches, bytes, first-update age).
- Errors ranked by recency; each row links to the table / MV it
  belongs to in the schema sidebar.

Why first-in-class: ClickHouse eats streams; the ingestion half of
that story is opaque in every tool. The DAG rendering and the
system-table read patterns both exist.

## D. Pre-flight cost estimate

Before running, show what a query is about to cost.

- On demand (not on every keystroke): run
  `EXPLAIN ESTIMATE` / `EXPLAIN indexes = 1` and surface estimated
  parts, granules, and rows to be read, reusing the Phase 7 pruning
  bars in miniature.
- A quiet warning when the estimate crosses a threshold ("about to
  read ~4.2B rows; the WHERE is not covered by the primary key"),
  with the advisor one click away.
- Never blocks execution; it informs, the user drives.

Why first-in-class: it moves the advisor from post-mortem to
pre-flight, at the exact moment behavior can change. Cheapest of the
four; almost entirely built from the existing EXPLAIN path.

## E. ClickHouse Cloud quick setup and linking

Time-to-first-query is the whole ballgame for a new user, and most
new ClickHouse users today start on Cloud. The current first
experience is "go dig hostname, port, and password out of the Cloud
console and paste them into a form." Kill that.

- **Link an organization**: the user pastes a Cloud API key (or we
  guide creating one, deep-linking to the right console page). zeDB
  calls the Cloud API to list organizations and services and offers
  each service as a one-click connection: hostname, port, and TLS
  come from the API; only the database password is typed (or a
  per-service credential is created where the API allows it).
- **Service state awareness**: Cloud services idle. Show
  running/idle/provisioning state in the connection list, and on
  connect to an idled service show "waking service..." instead of a
  bare connection timeout. This is the single most confusing Cloud
  failure mode for newcomers.
- **Stay current**: services added or removed in the console appear
  on refresh; a deleted service gets marked dead instead of failing
  cryptically.
- **Plain setup stays first-class**: the manual connection form
  remains the front door for self-hosted; Cloud linking is an
  accelerator beside it, not a wrapper around it. Credentials go in
  the Keychain like every other secret; the API key is stored once
  per organization.
- Later, once linked: surface Cloud-only context where relevant
  (service tier and size on the ops view, "this instance cannot be
  killed, it is Cloud-managed" style affordances).

Why it matters: not a differentiator in the north-star sense, but
the adoption multiplier for all of them. DBeaver and TablePlus make
you paste connection strings too; "sign in and pick your service" is
a first-run experience no ClickHouse tool has.

Rough build: a small Cloud API client (`api.clickhouse.cloud`, REST,
org API keys), a service-picker step in the existing connection flow,
and state polling reused from the connection health machinery.

## Constraints (standing)

- All reads are ordinary queries over the existing connection paths;
  cluster-wide reads use the established `clusterAllReplicas()`
  pattern and degrade gracefully on single-node servers or missing
  system tables (older versions, restricted grants).
- Advice stays explainable and is never auto-applied
  (`docs/PRODUCT-PRINCIPLES.md`). No new agent surfaces; the existing
  hand-off is enough.
- Expensive aggregations (A) run off-thread and cache their results.

## Not in this phase: migrations

North-star #6 (ClickHouse-correct migrations) remains the biggest
prize and remains deliberately held. Its requirements come from the
`analytics-clickhouse-ddl` production battle-test that started
2026-08-11, not from design-ahead. Revisit promoting it once the real
kinks are known; A and D above widen the funnel of DDL it would one
day stage safely.

## Open questions

- Where does A live: a new "Advisor" surface, or folded into the
  existing storage/query advisor tabs per table?
- B and C overlap the ops view; one "cluster health" umbrella tab or
  separate tabs per concern?
- Threshold defaults for D's warning (rows? bytes? relative to table
  size?).
- E: can the Cloud API mint a database credential so the user never
  types a password, or does the DB password stay a manual step? Also
  whether linking lands before or after A given the adoption
  argument.
