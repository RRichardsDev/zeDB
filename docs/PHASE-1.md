# Phase 1 plan: the migration engine (headless)

Goal: `zedb-core` + `zedb-cli` become a working second generation of
analytics-clickhouse-ddl. Plain-SQL migrations, replayed through a pinned
`clickhouse local`, with generated current-state, checks, and live
upgrade/rollback/status/stamp/verify. No GUI work in this phase; the CLI is
the only new surface, and CI is a first-class consumer.

The proof for the whole phase is the importer: the real
analytics-clickhouse-ddl repo must import, regenerate, and report status
identically to its ancestor tooling.

## Working rules

- Same as Phase 0: every milestone ends buildable, on main, demoable in 30
  seconds; riskiest unknowns get spiked first; keep `docs/devlog.md` going.
- All logic in `zedb-core` behind functions the CLI calls; `zedb-cli` stays
  a thin argument parser. Anything the CLI can do must be testable without
  it.
- Tests run against fixture repos in-tree plus an ephemeral seeded
  `clickhouse server`, exactly like the Phase 0 integration tests. The real
  fleet is touched read-only, and only from M8 on.
- The ancestor (`~/code/analytics/analytics-clickhouse-ddl`) is the
  reference implementation. Port semantics deliberately, not blindly: every
  place the new format diverges gets a line in FORMAT.md explaining why.

## Milestones

### M0. Format RFC

Resolve SPEC.md's open format decisions in a committed `docs/FORMAT.md`:
directory layout and numbering, tracking table name and schema (explicitly
versioned this time), repo config file (name, format, contents), rollback
class markers, targeted-migration markers, template parameter model
(user-defined per repo, replacing the ancestor's built-in analytics
offsets), and exclusion-group config. Includes a small hand-written example
repo as a test fixture.

Done when: FORMAT.md is committed, SPEC.md's open-decisions list shrinks
accordingly, and the fixture repo exists with chain, targeted migration,
templating, and exclusion groups all represented.

### M1. Repo model in core

`zedb-core` reads and writes the format: discover and order the chain,
parse rollback classes and targeted markers, load repo config and template
parameters, validate structure (gaps, duplicates, malformed markers) with
readable errors. `zedb-cli` is born: `zedb init` scaffolds a new repo,
`zedb new` scaffolds a migration, `zedb ls`/`zedb show` print the chain.

Done when: init + new produce a valid repo from nothing; the fixture repo
round-trips through parse and re-render; structural corruption in a fixture
variant produces errors a human can act on.

### M2. Pinned binary management

Discover the target version from a server (`SELECT version()`), download
the matching `clickhouse` binary into a per-user cache on demand, and run
`clickhouse local` replays against it. `zedb pin` pins a repo to a version.
Graceful degradation when no binary exists for a version/platform: a clear
message and text-only mode, never a silent pass.

This and M3 are the riskiest ports, so they come before the features that
depend on them. Binary URLs, checksums, and cache invalidation are the
unknowns; findings go in the devlog.

Done when: pointing `zedb pin` at the local dev server fetches and caches
the right binary, a smoke replay executes against it, and an impossible
version/platform pair fails with a message that says what to do instead.

### M3. Replay, canonicalization, regen

The heart. Replay the chain through the pinned `clickhouse local`, read
back canonical DDL (`SHOW CREATE`), un-render template placeholders back to
`${param}` form, and write the generated `current-state/` tree. `zedb
regen` regenerates; `zedb regen --check` fails when the committed tree does
not match. Data-only statements cause zero churn. The ancestor's
`canonical.py` and `regen.py` are the reference; divergences are format
decisions, not accidents.

Done when: regen on the fixture is byte-stable across runs, a migration
that only inserts data changes nothing, and `regen --check` catches a
hand-edited current-state file.

### M4. Checks

`zedb check` with the ancestor's three layers: `sql` (each migration
replays cleanly, statement by statement, with server errors reported at
file and line), `equivalence` (upgrade followed by rollback returns to the
prior schema for clean/structural classes), and `lifecycle` (the chain
walks up and down end to end under realistic grants, irreversible
migrations enforced as such). All against the pinned local binary; CI runs
`zedb check` with no server.

Done when: the fixture passes all checks; fixture variants broken in each
dimension (bad SQL, non-reverting rollback, wrong class declaration) each
fail their check with a readable message.

### M5. Live execution

Against real servers (local dev ClickHouse in tests): versioned tracking
table bootstrap, `zedb status`, `zedb upgrade`, `zedb rollback` with
rollback-class enforcement, `zedb stamp` to adopt existing schemas, and
targeted `zedb apply`. Safety from day one: read-only connections refuse
mutation with a clear message, every mutating run records to a local audit
log, and rollback of an irreversible migration requires explicit
acknowledgement.

Done when: a seeded local server walks upgrade, status, rollback, stamp
through the fixture chain; the tracking table records match the ancestor's
semantics; a read-only connection cannot mutate anything.

### M6. Fleet targeting

Databases discovered live from the server (system tables plus the
per-repo registry query), template rendering per target, exclusion groups
honored: `--all` skips excluded databases and says so, `--group` and
`--db` target deliberately. Targeted migrations apply per database and are
ignored by fleet operations.

Done when: a local server seeded with several databases (including
excluded ones) behaves correctly under `--all`, `--group`, and `--db`, and
the skip list is printed, not silent.

### M7. Verify

`zedb verify`: diff each database's live schema against the expected state
for its applied chain position, report drift per object with a readable
diff. This is the read half of what Phase 2's fleet view will render.

Done when: introducing manual drift on the local server (an added column, a
changed view) is detected and shown as a diff naming the object and the
difference.

### M8. Importer

`zedb import` converts an analytics-clickhouse-ddl repo: layout, rollback
classes, targeted migrations, exceptions.toml, template placeholders, and
existing tracking rows mapped into the new tracking schema. This is the
bridge to reality and the acceptance test for every milestone above it.

Done when: the real ancestor repo imports without hand-editing, `zedb
regen --check` passes on the imported repo, `zedb check` passes, and `zedb
status` against staging (read-only) agrees with the ancestor's `ddl
status` for the same databases.

### M9. CI and daily-driver hardening

The gap-closing milestone, driven by using zedb instead of the ancestor
tooling for real migration work. Expected contents: a GitHub Actions
recipe (regen --check + check on PRs, matching the ancestor's CI role),
error and help text polish, `--json` output where CI wants it, and
performance sanity on the real repo's chain length. Anything still forcing
a fall back to `ddl` is fixed or parked in IDEAS.md.

Done when: one real migration has gone from `zedb new` through checks,
staging apply, and production apply without touching the ancestor tooling,
and CI runs zedb on the migration repo.

## Order and dependencies

M0 → M1 → M2 → M3 → M4 → M5 → M6 → M7 → M8 → M9. M2 can start alongside
M1 (binary management is independent of the repo model). M6 and M7 can
land in either order. Nothing before M8 touches a non-local server;
M8's staging use is read-only.

## Explicitly not in Phase 1

Any GUI for migrations (that is Phase 2: fleet view first read-only, then
applies behind the safety ladder), zeDB writing to git (commit/push stays
with the user), apply-wave orchestration, migration authoring assistance,
guest-driver migration support, Windows.

## Phase exit

Phase 1 is done when M9's done-condition holds: the ancestor tooling is no
longer needed for the real fleet's migration workflow. The next step is
Phase 2 (the fleet view), which starts by rendering M7's verify and M5's
status data as the databases x migrations matrix.
