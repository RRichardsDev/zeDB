# Phase 3.3: schema intelligence for agents

Phase 3.2 built a per-connection schema cache so the editor could be
instant. Phase 3.1 gave agents a zedb MCP server whose schema tools all
hit ClickHouse live. This phase hands the cache to the agents for the
two jobs where it beats live queries: fleet-wide search and SQL
linting. The live tools stay untouched as the source of truth.

## Why these two and nothing else

- Reconnaissance ("which tables have a `user_id` column?", "where does
  `events_daily` exist?") currently costs an agent a `list_databases`
  plus a `list_tables` and `describe` per database: a dozen round
  trips and a pile of context tokens. One search over the snapshot
  answers instantly.
- Drafted SQL can be checked against the cache before it reaches
  `propose_migration` / `propose_query`, catching hallucinated names
  for free without touching the server.
- Everything else stays live. Serving `list_tables`/`describe` from
  cache would trade authority for a marginal speedup; staleness is the
  one real risk in this design, and the mitigation is that cached
  answers are clearly stamped and the authoritative tools remain.

## Shape

Both tools are pure functions over the on-disk snapshot the app
already maintains (`~/Library/Caches/zedb/schema/<connection>.json`).
The MCP server re-reads the file on every call, so a long-lived agent
session tracks the app's background refreshes without any IPC.

- `schema_search { query }`: case-insensitive substring match over
  database names, table/view names, and column names. Results grouped
  by kind as `db`, `db.table (engine)`, `db.table.column (type)`,
  capped with a "N more matches" line. Footer: "cached as of <time>".
- `lint_sql { sql, database? }`: runs the editor's conservative
  analyzer. Reports unknown tables/columns with line numbers, or a
  clean bill; silence on anything ambiguous or uncached is by design
  and the tool says so. Same freshness footer.

## Milestones

- M0: `schema_search` in zedb-ch as a pure snapshot function, with
  tests alongside the existing intelligence tests.
- M1: both tools in the MCP server, offered only when a cache path is
  configured and the file is readable. App-embedded serve config
  gains the cache path; `zedb mcp` gains `--cache-connection <name>`
  to point at an app-maintained cache by connection name.
- M2: the agent primer mentions the two tools and when to trust them
  (recon and drafting: cache; decisions and DDL: confirm live).

## Done when

An agent in the pane answers "which tables have a sneaky column?"
with one `schema_search` call, and linting a drafted query with a
typo'd column reports it without a ClickHouse query being issued.

## Risks

- Stale cache misleading an agent: mitigated by the freshness stamp,
  the primer guidance, and keeping every live tool available.
- Cache file absent (fresh install, never connected): the tools are
  simply not offered; nothing degrades.
