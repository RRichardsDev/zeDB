# Phase 4 draft: repo-as-package provisioning

Status: DRAFT, and explicitly undecided. Nothing here is committed
scope; this file records the redesigned plan so the thinking is not
lost. Do not build any of it without a fresh decision.

## The one-sentence version

There are no SDKs: zeDB's codegen scaffolds packaging into the user's
own migration repo, so the repo itself becomes the artifact that
automated provisioning consumes.

This replaces the earlier draft (embedded runner libraries for six
languages), which promised a maintenance surface out of proportion to
the problem. The redesign copies a pattern proven in
analytics-clickhouse-ddl.

## The pattern being copied

In analytics-clickhouse-ddl, the migration repo doubles as a Python
package:

- The wheel bundles `current-state/` and `migrations/` as package
  data. One object per file; sorted filename order is provisioning
  order, so no manifest is needed.
- The API is three tiny helpers: `current_state_dir()`,
  `current_state_version()` (head migration of the bundled state,
  targeted migrations excluded), and
  `stamp_current_state(client, db, cluster)` (write the tracking rows
  once the whole provisioning sequence succeeded). Imports are lazy so
  template-only consumers have zero dependencies.
- The consumer brings its own ClickHouse client, loops the files with
  `${db}`/`${cluster}` substitution, and stamps on success. It never
  replays the migration chain: new databases come from current-state;
  existing databases stay the tool's job. No statement splitting, no
  rollback classes, none of the fragile logic.
- CI is the distribution story: a `RELEASE-*` tag runs the release
  checks, builds the wheel, and opens an automatic version-bump PR at
  the consuming service. Provisioning is pinned to a tagged, checked
  snapshot of the schema, never to "whatever main says", and
  versioning rides the package ecosystem instead of a custom protocol.

## What zeDB would do (if this proceeds)

An opt-in, disabled-by-default codegen target: `[package.python]` (or
similar) in the repo config. When enabled, regen additionally
maintains in the migration repo:

- `pyproject.toml` wiring the current-state and migrations directories
  in as package data, plus a package version derived from the head
  migration number.
- A tiny generated module with the three helpers above, written
  against the tracking schema zeDB itself uses (`zedb_config`, repo
  identity column), so a database provisioned-and-stamped by the
  package is indistinguishable from one zeDB set up: same tracking
  rows, same verify results, same fleet matrix.
- A CI workflow template for the release-tag flow, offered not forced.

zeDB generates roughly a hundred lines of scaffolding and maintains no
client libraries. Other languages become "add another scaffolding
template if anyone ever asks", an honest non-commitment.

## Open questions parked with the draft

- Is this appropriate in a generic tool at all, or is it one user's
  workflow? (The counter-argument: it is scaffolding the repo owner
  opts into, not a runtime zeDB must support forever.)
- Ordering beyond sorted filenames if current-state ever grows
  cross-database dependencies.
- Whether the stamp helper should verify checksums before writing, or
  trust the provisioning loop's success signal as the analytics repo
  does.
