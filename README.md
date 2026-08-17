# zeDB

A fast, native ClickHouse explorer, fleet migration tool, and
ClickHouse Cloud client for macOS. Built with [GPUI](https://www.gpui.rs)
(the UI framework behind the Zed editor), so it feels like an editor,
not an electron app: instant launch, native rendering, and million-row
result sets that scroll without jank.

![The query view: sort and filter straight from the results grid](docs/screenshots/query.png)

## What you get

**A serious query workbench.**
Connect to clusters with tiered identities (dev / staging / production),
read-only by default, passwords in the macOS Keychain. Write SQL with
schema-aware completions, hover cards, and typo underlines powered by a
per-connection schema cache that persists across launches. Stream
results into a virtualized grid; sort and filter by clicking the
headers, and zeDB rewrites the actual SQL (a real `ORDER BY`, a real
`WHERE`) and re-runs just that statement, so what you see is always what
the server did. EXPLAIN any statement against the live server, and keep
a searchable per-connection history with saved queries. Tabs are
connection-scoped: switch clusters and your workspace follows.

![Filter popovers offer checkboxes when a column has few distinct values](docs/screenshots/filtering.png)

Ask for the plan and zeDB renders EXPLAIN as the server's own tree,
with the index-pruning verdict called out: which parts and granules
the primary key actually kept.

![The query plan: reads show index pruning](docs/screenshots/query-plan.png)

**Live tails, instantly.**
Right-click any table and tail it, `tail -f` for ClickHouse. Rows
stream in newest-first with a live strip, pause/resume, and a retained
cap you pick up front. On servers with streaming support one click
upgrades the tail to instant push updates (`STREAM`), millisecond
latency instead of polling, with graceful fallback everywhere else.

![A live tail with instant STREAM updates](docs/screenshots/tail.png)

**Advice measured on your data, not folklore.**
The table inspector analyses per-column cardinality and compression and
suggests better codecs and types, with the estimated savings measured
on a sample of your actual data: left-click applies the change,
right-click drafts the `ALTER` into the editor instead.

![Per-column analysis: measured savings, one click to apply](docs/screenshots/columns-advisor.png)

The Workload tab reads your real `system.query_log`, EXPLAINs the
heaviest query shapes, and tells you run-weighted truths: whether the
primary key actually prunes, which skip indexes earn their keep, which
projections are never hit, and what to change, with the scope honestly
labelled.

![Workload-measured findings: does the primary key actually prune?](docs/screenshots/workload.png)

**A migration engine that understands fleets.**
Migrations are plain SQL files in a plain git repository. zeDB replays
them through a real, version-pinned ClickHouse rather than parsing your
SQL, then shows you, across every database on the cluster, exactly what
is applied, pending, customised, or drifted, and what an apply would do
before you run it. Applying to production takes layered, explicit
consent, not a confirmation dialog.

![The fleet view: one row per database, one column per migration](docs/screenshots/fleet.png)

**A ClickHouse Cloud client that actually understands Cloud.**
Sign in with your ClickHouse Cloud account (browser approval, no API
key needed to look) and your organizations appear with live service
state. zeDB knows what a warehouse is: compute grouped over shared
data, connections built per warehouse with every field prefilled from
the control plane, one shared password (which zeDB can provision for
you, API key permitting), and `ON CLUSTER` never emitted where a
shared catalog makes it wrong. Idle services wake on demand, deleted
ones say so instead of timing out, and the editor wears a thin
ClickHouse-yellow border whenever you are on a live Cloud service.

![The Cloud panel: signed in, warehouse-grouped services, one-click connections](docs/screenshots/cloud-panel.png)

Every Cloud connection's page is a control-plane dashboard: per-compute
state, version, and sizing; the last 30 days of credits as a daily
chart with warehouse and organization totals; backups; and the
service's key metrics. The ops and workload views are
SharedMergeTree-aware, so what they report on Cloud is true, not a
self-hosted view wearing a trench coat.

![The connection page doubles as a Cloud dashboard: cost, backups, metrics](docs/screenshots/cloud-dashboard.png)

**Eyes on the cluster.**
The ops view is a live read of what the server is doing: running
queries (with KILL behind explicit write consent), connections, merges
and mutations, replication health, Kafka consumers and async inserts,
disks, and the largest tables, node-scoped or fanned out across the
cluster.

**Your AI agents, inside the app.**
Open the agent pane (`cmd-i`) and run Claude Code, Codex, or any
ACP-speaking agent with the auth you already have. Agents see what you
see, query through zeDB's read-only, capped MCP tools, search the
schema cache instantly, and can draft migrations or queries into the
app for your review; they cannot write anything themselves.

## Install

Grab the latest DMG from
[Releases](https://github.com/RRichardsDev/zeDB/releases). zeDB checks
for updates itself and installs them in place.

## Quick start

1. Click `+` in the sidebar and pick your door: **ClickHouse Cloud**
   (sign in and pick services) or **Self-hosted cluster** (name,
   nodes, tier, and whether it may ever write).
2. Connect; you land in a query tab with the schema sidebar warming
   itself in the background.
3. Write SQL and `cmd-enter`, or click around the results grid: header
   clicks sort, right-click filters, dividers resize. Right-click a
   table to tail it live.
4. For migrations, open the fleet view (grid icon) and point it at a
   migration repo, or paste a git URL and let zeDB clone and manage the
   checkout for you.

## Development

Everything about building, architecture, and design decisions lives in
[DEV_README.md](DEV_README.md). The full design document is
`docs/SPEC.md`; user-facing changes land in
[CHANGELOG.md](CHANGELOG.md).

## License

Source-available under the [PolyForm Noncommercial License
1.0.0](LICENSE): free to use, modify, and share for any **noncommercial**
purpose, but not for commercial use. Copyright 2026 Rhodri Richards; all
commercial rights reserved.

Third-party components under `vendor/` (gpui, gpui-component, ...) remain
under their own licenses (Apache-2.0).
