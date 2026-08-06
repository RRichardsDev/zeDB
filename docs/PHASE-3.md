# Phase 3 plan: the managed lifecycle

Goal: close the loop. BYO git was never meant to be self-managed; it
means the migration repo lives under the user's git remote, with no
forge coupling. The workflow itself belongs to zeDB. After Phase 3 the
app manages the whole migration lifecycle: create a migration, check
it, regenerate current-state, commit and push, deploy across the fleet,
and verify, without dropping to a terminal. The CLI keeps parity for CI
and scripting; the app becomes the daily surface.

## Working rules

- Same as before: every milestone ends buildable, on main, demoable in
  30 seconds; riskiest unknowns first; devlog as we go; UI work follows
  docs/UI-DESIGN.md and reuses existing primitives.
- The GUI calls the same zedb-core/zedb-ch functions as the CLI. New
  lifecycle operations land in core first, then in both clients.
- Git stays plain git. Any remote, any hosting, ssh or https auth from
  the user's existing setup. No PRs, no reviews, no forge APIs.
- zeDB never resolves conflicts, rewrites history, or touches files it
  did not create. When git says no (dirty conflicting state,
  non-fast-forward push), the app explains and stops; the user's normal
  git workflow is the escape hatch, never the required path.
- The safety ladder covers the new ground: deploys keep their existing
  gates, and repo mutations (commit, push) are explicit actions with a
  visible preview of exactly what will be written, never side effects.

## Milestones

### M0. Git awareness

Read-only git status in the app: current branch, clean or dirty, ahead
or behind the remote, surfaced wherever the repo is shown (fleet view,
future authoring surface). Deploying from a dirty or behind checkout
warns with the specifics.

Done when: the fleet view shows the repo's git state at a glance, and
an upgrade attempted from a stale checkout says so before the safety
ladder starts.

### M1. Authoring in the app

Create a migration from the GUI: pick the next number, rollback class,
targeted or chain, and edit upgrade.sql and rollback.sql in the
existing editor surface (highlighting, Vim mode) with checks running
against the pinned local server as you work. Scaffolding reuses
`zedb new`; checks reuse check sql and equivalence.

Done when: a migration authored entirely in the app passes `zedb check
all` from the CLI unchanged, and a deliberate SQL error surfaces in the
editor before anything is written to the chain.

### M2. Codegen in the app

Regenerate current-state after authoring, with the churn shown as a
readable diff (which objects changed, added, removed) before the files
are written. Chain checks (sql, equivalence, lifecycle) runnable from
the app with progress and results as legible as the CLI's.

Done when: authoring a migration and regenerating produces exactly the
tree `zedb regen` produces, the diff view shows the churn honestly
(data-only migrations show zero churn), and a failing lifecycle check
reads as clearly in the app as in the terminal.

### M3. Commit and push

The app commits the migration and its regenerated current-state as one
commit (staging exactly those files, nothing else), with a templated
message the user can edit, and pushes to the user's remote. Auth comes
from the user's existing git setup (ssh agent, credential helper).
Failures (auth, non-fast-forward, diverged remote) surface readably and
leave the working tree exactly as git left it.

Done when: the full write path (scaffold, regen, commit, push) produces
a remote commit indistinguishable from one made by hand, and a rejected
push explains itself without corrupting or half-committing anything.

### M4. The loop

The end-to-end flow on one surface: author, check, regen, commit, push,
deploy through the safety ladder, verify. First against the demo fleet,
then a real migration on the imported repo against real clusters.

Done when: a routine schema change goes from idea to verified-deployed
without leaving the app, and the audit log plus git history tell the
whole story afterwards.

### M5. Lifecycle polish soak

Driven by real use, like Phase 2's M5: the friction found by actually
authoring and shipping migrations through the app for a while, fixed or
parked in IDEAS.md.

Done when: the app is the default way migrations get written and
shipped, and the terminal is for CI and emergencies.

## Order and dependencies

M0 → M1 → M2 → M3 → M4 → M5. Strictly sequential: each milestone is
the next segment of the same loop.

## Explicitly not in Phase 3

- Forge integration (PRs, reviews, checks APIs). BYO git still means
  git, not GitHub.
- Branch management workflows (feature branches per migration, merge
  queues). Phase 3 commits to the current branch; anything fancier is
  the user's git workflow.
- Apply-wave orchestration (IDEAS.md).
- Embedded runners / client SDKs (below).

## Beyond Phase 3: embedded runners (candidate Phase 4, not committed)

Libraries for Java, Python, Node, C++, Rust, and PHP that let an
application deploy its own schema: read the repo's chain and generated
current-state, provision or upgrade the databases it owns at startup or
release time, and stamp the tracking tables when done, with zeDB and
the CLI seeing exactly the same state afterwards.

Design constraint to settle before committing: the engine must not be
reimplemented six times. Candidate shapes, in rough order of appeal:
a C ABI over zedb-core with thin native bindings per language; a
vendored CLI driven as a subprocess with `--json`; or a documented
"dumb client" protocol (render, apply, stamp) that deliberately
excludes replay-dependent operations (regen, checks stay in CI). The
tracking schema in FORMAT.md is already the shared contract either way.

Whatever the shape, runner support is opt-in per repo and per language:
disabled by default, enabled explicitly in zedb.toml (for example a
`[runners]` table listing the languages a repo generates or supports).
A repo that never opts in sees no generated artifacts, no extra config
surface, and no behaviour change. Parked in IDEAS.md until Phase 3
exits.

## Phase exit

Phase 3 is done when M5's done-condition holds: migrations are
authored, shipped, and deployed through the app in real use. That
supersedes the old post-v1 follow-up ("zeDB writes to git"); what
remains afterwards is the release checklist and the follow-ups queue,
with embedded runners first in line for consideration.
