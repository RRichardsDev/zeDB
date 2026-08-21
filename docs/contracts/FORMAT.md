# zeDB migration repo format (version 1)

The second generation of the analytics-clickhouse-ddl layout. Everything
here is what `zedb-core` reads and writes; the ancestor is the reference
implementation and divergences are called out with their reasons.

## Repo root

```
zedb.toml            # repo config (required; marks the repo root)
exclusions.toml      # fleet exclusion groups (optional)
migrations/          # the ordered chain
current-state/       # generated, never hand-edited
```

## zedb.toml

New in this generation (the ancestor had conventions, not a config file).
TOML, versioned explicitly so later format changes are detectable:

```toml
format = 1

[engine]
kind = "clickhouse"
# Version pin source: "server" discovers via SELECT version() and records
# the result here; a literal string pins explicitly.
version = "25.3.2.1"

[tracking]
# Database that holds the tracking tables. The tables themselves are
# zedb_migrations and zedb_meta (see Tracking below). One tracking
# database per cluster is the expected shape; the repo identity below
# keeps several repos apart within it. Never a migration target.
database = "zedb_config"
# Optional: when set, tracking DDL and migrations run ON CLUSTER ${cluster}.
cluster_param = "cluster"
# Optional: this repo's identity in the tracking rows; defaults to the
# repo directory's name. Lets repos share a tracking database without
# reading each other's history.
repo = "org-fleet"

[fleet]
# Optional SQL returning one database name per row. When absent, the
# fleet defaults to every non-system database except `default` and the
# tracking database; set this to narrow it. The tracking database is
# excluded from targeting even when the query matches it. The ancestor
# hardcoded this per deployment.
registry_query = "SELECT name FROM system.databases WHERE name LIKE 'org_%'"

[scopes]
# Which template scopes exist and how they map to targets. "global" runs
# once per cluster; "db" runs once per discovered database.
global = { }
db = { param = "db" }

[replay]
# Databases created by cluster bootstrap rather than the chain; ephemeral
# replays pre-create them and isolate them per replay side.
shared_databases = ["org_to_slug_mappings"]

[params]
# User-declared template parameters. ${db} and ${cluster} are built-in.
# The ancestor's analytics-specific offsets become declarations like:
refresh_offset_expr = { dummy = "1 HOUR 42 MINUTE", sentinel = "2 HOUR 53 MINUTE", description = "per-db refresh stagger" }
# Fields: `default` is the runtime value when no --param override is
# passed (omit to force explicit overrides at apply time); `dummy` is what
# checks render with (falls back to default, then a number); `sentinel` is
# the collision-proof value for regen's replay round trip, needed when a
# generated number is invalid in the parameter's position (for example
# interval expressions).
```

Tracking database and runtime cluster names are plain ClickHouse identifier
chunks: an ASCII letter or underscore followed by ASCII letters, digits, or
underscores. Ancestor tracking imports accept a table name in `TABLE` or
`DATABASE.TABLE` form under the same grammar.

Scope names are directory names under `current-state` and must match
`[a-z0-9_]+`. Absolute paths, separators, parent components, uppercase letters,
and punctuation are rejected when the repo opens.

- `${param}` placeholders may appear in identifier or expression position.
  Identifier-position values must match `[A-Za-z_][A-Za-z0-9_]*` at render
  time; anything else is a hard error (ClickHouse cannot parameterize
  identifiers, so values land verbatim in DDL).
- Per-database parameter overrides and live extraction rules (the
  ancestor's offset inheritance) are runtime concerns, not format; they
  arrive with fleet execution and are recorded in the tracking tables, not
  in files.

## Migrations

Unchanged from the ancestor, because live databases track these numbers
and the shape is proven:

```
migrations/YYYY/MM/NNNNN/upgrade.sql
migrations/YYYY/MM/NNNNN/rollback.sql    # optional
migrations/YYYY/MM/NNNNN/targeted.toml   # optional marker
```

- `NNNNN` is five digits, globally unique and strictly increasing across
  the whole tree. New migrations are numbered in increments of 100 so
  hotfixes can be inserted between existing numbers when unavoidable.
- Month directories must not sort against migration order: a later number
  may not live in an earlier `YYYY/MM`.
- Plain SQL, statements split on `;`. No annotations, no redeclarations.
  A leading `-- migration NNNNN: description` comment is conventional but
  not parsed.
- `rollback.sql` line 1 must declare its class:
  `-- rollback-class: clean | structural | irreversible`. Enforced at
  check time and run time. A migration without `rollback.sql` is treated
  as irreversible.
- `targeted.toml` marks an opt-in migration: applied per database with
  `zedb apply`, skipped by `zedb upgrade`, invisible to current-state.
  Optional `allow_list = ["db1", ...]` restricts which databases may be
  targeted (a guardrail; the tracking table is the record of where it is
  actually applied).

## current-state/

Generated by `zedb regen` replaying the chain through the pinned
`clickhouse local`; committed; verified by `zedb regen --check` in CI;
never hand-edited.

```
current-state/<scope>/NNNNN_SS_description.sql
```

One canonical statement per file: `NNNNN` is the migration that last
shaped the object, `SS` a stable per-migration sequence, and the
description derived from the statement. Scope directories come from
`[scopes]` in zedb.toml (the ancestor's `global/` and `org/` generalize to
this). Template placeholders appear un-rendered (`${db}`) in canonical
output. Data-only migrations produce zero churn here.

## exclusions.toml

The ancestor's `exceptions.toml`, renamed for what it means:

```toml
[groups.<name>]
reason = "why these databases are excluded from fleet operations"
databases = ["db_a", "db_b"]
```

`--all` operations skip every listed database and say so; `--group <name>`
and `--db <name>` target them deliberately.

## Tracking

Two tables in the configured tracking database, created on first contact.
Diverges from the ancestor's single `default.schema_migrations` by
versioning the tracking schema explicitly:

```sql
CREATE TABLE zedb_meta
(
    key   LowCardinality(String),   -- 'tracking_version', 'format'
    value String,
    recorded_at DateTime64(3) DEFAULT now64(3)
)
ENGINE = MergeTree ORDER BY key;

CREATE TABLE zedb_migrations
(
    repo          LowCardinality(String) DEFAULT 'default',
    db            String,
    migration     UInt32,
    action        LowCardinality(String),  -- upgrade | rollback | stamp | apply
    status        LowCardinality(String),  -- started | success | failed
    error         Nullable(String),
    recorded_at   DateTime64(3) DEFAULT now64(3),
    duration_secs Decimal(9, 2) DEFAULT 0,
    run_id        UUID,
    params        Map(String, String)      -- rendered parameter values
)
ENGINE = MergeTree ORDER BY (repo, db, migration);
```

- `params` is new: the rendered parameter values for the run, making
  per-database overrides auditable without a side channel.
- `action` gains `apply` (targeted runs were folded into upgrade rows in
  the ancestor).
- When `tracking.cluster_param` is set, both tables are created
  `ON CLUSTER ${cluster}` with `ReplicatedMergeTree`, matching the
  ancestor's deployment shape.
- `zedb import` maps ancestor `schema_migrations` rows into
  `zedb_migrations` one to one and writes `zedb_meta` rows recording the
  import.

## Divergence summary vs analytics-clickhouse-ddl

| Area | Ancestor | Format 1 | Why |
|------|----------|----------|-----|
| Repo config | conventions in code | `zedb.toml` with `format = 1` | multiple repos, explicit versioning |
| Numbering | `YYYY/MM/NNNNN` +100 | unchanged | proven; live DBs track numbers |
| Rollback classes | line-1 marker | unchanged | plain SQL stays plain |
| Targeted marker | `targeted.toml` | unchanged | proven |
| Scopes | hardcoded `global`/`org` | declared in `[scopes]` | not analytics-specific |
| Params | built-in analytics offsets | declared in `[params]` | not analytics-specific |
| Exclusions | `exceptions.toml` | `exclusions.toml`, same shape | clearer name |
| Tracking | `default.schema_migrations` | `zedb_migrations` + `zedb_meta`, configurable db | versioned tracking schema, auditable params |
