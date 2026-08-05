# Phase 2 plan: the fleet view

Goal: the flagship screen. The GPUI app opens a migration repo and shows
the databases x migrations matrix for a connected cluster: applied,
pending, customised, failed, and drifted per database, filterable at
hundreds-of-databases scale. Read-only lands first and completely; only
then do applies arrive, behind the full safety ladder. Everything renders
data the Phase 1 engine already produces (status, verify, dry-run);
Phase 2 adds no new engine semantics.

## Working rules

- Same as before: every milestone ends buildable, on main, demoable in 30
  seconds; riskiest unknowns first; devlog as we go; UI work follows
  docs/UI-DESIGN.md and reuses existing primitives.
- The GUI calls the same zedb-core/zedb-ch functions as the CLI. If the
  GUI can do it and the CLI cannot (or vice versa), that is a bug in the
  layering.
- BYO git stands: the app opens a migration repo as a local directory
  (any git checkout); committing and pushing stay in the user's normal
  git workflow. zedb writing to git itself remains a post-v1 follow-up
  (SPEC), not Phase 2 scope.
- Mutating anything from the GUI is forbidden until M4. The safety
  ladder must exist before the first mutating action ships, not after
  (SPEC principle 4).
- Demo/test fleet: the committed examples/demo-fleet repo against the
  local two-node docker cluster, seeded with enough databases in varied
  states (current, behind, customised, failed, drifted, excluded) that
  the matrix has something honest to show.

## Milestones

### M0. Open a repo

The app opens a migration repo: a picker (and recent-repos memory in
preferences) pointing at a local checkout; the repo's chain summary is
visible (migration count, pinned version, scopes, exclusion groups), and
repo validation errors surface in the UI as readably as the CLI prints
them. No server interaction yet.

Done when: opening examples/demo-fleet shows its chain and config at a
glance, a broken repo shows the same actionable error the CLI gives, and
the last-opened repo restores on launch.

### M1. The matrix

The flagship. For the active connection and open repo: databases down,
migrations across, one cell per (database, migration) showing applied /
pending / customised / failed, a head-position summary per database, and
excluded databases visibly parked rather than hidden. Filter-by-typing
on database names. Data comes from the runner's status path; the render
must stay smooth at hundreds of databases (the Phase 0 grid findings are
the starting point; this is the riskiest GPUI piece, so it lands before
anything is built on top).

Done when: the demo fleet renders instantly with its mixed states
legible at a glance, filter narrows as you type, and a synthetic
several-hundred-database fleet scrolls without jank.

### M2. Drift in the matrix

Verify integration: on demand per database (and a refresh-all), drift
badges appear in the matrix, and selecting a drifted database shows the
findings (missing, unexpected, mismatch with the expected/live pair) in
a detail panel. Verify runs replay work, so it is explicitly asynchronous
with visible progress, never blocking the matrix.

Done when: the demo fleet's deliberately drifted database shows its
badge and readable findings without freezing the UI, and clean databases
show verified-clean state with a timestamp.

### M3. Dry-run rendering

Selecting a database shows what an upgrade would do: pending migrations
rendered with that database's parameters, statement by statement,
SQL-highlighted in the existing editor surface. This is the read half of
apply: the exact SQL that would run, per target, before anyone is
allowed to run it.

Done when: a behind database shows its pending migrations' rendered SQL,
parameter substitutions visible, and a current database says so instead.

### M4. Applies behind the safety ladder

Mutation reaches the GUI, gated hard: environment tier identity on every
confirmation (the dev/staging/production colors from Phase 0), an
explicit per-connection write unlock (the GUI's --write), the rendered
dry-run diff shown before any apply, rollback-class acknowledgements
(structural warns, irreversible requires typed confirmation), targeted
removal friction, per-database and fleet-wide apply with the exclusion
rules the CLI enforces, live progress per statement, and the audit log
appended exactly as the CLI does. A production-tier apply additionally
requires typing the database (or cluster) name.

Done when: upgrading the demo fleet from the GUI walks the same ladder
the CLI does with nothing skippable, a refused rollback explains itself,
and the tracking table and audit log are indistinguishable from a CLI
run's.

### M5. Fleet daily-driver polish

The gap-closing milestone, driven by real use against the imported repo
and real clusters once the Phase 1 adoption tail lands: keyboard-first
navigation between matrix, detail, and diff; connection and repo
switching without restart; status refresh cadence that does not hammer
servers; empty/error states that do not look like placeholders; and
whatever the first week of real fleet use surfaces, fixed or parked in
IDEAS.md.

Done when: checking fleet state and applying a routine migration is
something you reach for zedb to do, and DBeaver/clickhouse-client/ddl
are not part of the loop.

## Order and dependencies

M0 → M1 → M2 and M3 in either order → M4 (needs all of M1-M3: the
matrix to pick targets, drift to know state, dry-run as the consent
surface) → M5.

## Explicitly not in Phase 2

zedb writing to git (commit/push of scaffolded migrations; post-v1
follow-up), apply-wave orchestration with pause/resume (IDEAS.md),
migration authoring assistance, ops panels (replication lag, queues;
parked hard in IDEAS.md), guest-driver fleets, Windows.

## Phase exit

Phase 2 is done when M5's done-condition holds on real fleet use. That
completes the v1 scope from SPEC.md: explorer, migration engine, fleet
view. What follows is the v1 release checklist (naming, license,
packaging) and the follow-ups queue, starting with zedb writing to git.
