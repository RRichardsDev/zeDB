# Structure improvements

Status: accepted direction, incremental refactor in progress.

Date reviewed: 2026-08-13.

## Progress

- Query-buffer parsing, cursor targeting, and local variable expansion now
  live behind `features/query`, with focused tests beside the implementation.
- Connection forms, drafts, topology and health models now belong to
  `features/connections`; schema cache projections and inspector state belong
  to `features/schema`; query tabs and tail lifecycle state belong to
  `features/query`.
- `Workspace` now composes `ConnectionState`, `SchemaState`, and `QueryState`
  instead of storing those features as several dozen unrelated flat fields.
  Existing orchestration still lives on `Workspace` for now, which keeps this
  tranche structural and behavior-preserving.
- Query history, saved tabs, drawer filtering, rename state, and drawer layout
  now share a `HistoryState` owner instead of adding another set of fields to
  the application shell.
- Query editor lifecycle, tab management, tailing, input handling, and query
  execution now live in focused `features/query` modules. This removes more
  than 3,200 lines from `main.rs`; the crate root retains shell composition and
  cross-feature rendering rather than the detailed query controller.
- Schema loading, cardinality analysis, parts and merge inspection,
  dependencies, projections, storage advice, and the object inspector now live
  in `features/schema`. Together with the query extraction, this reduces
  `main.rs` from 11,755 lines to 5,948 lines in behavior-preserving moves.
- Connection form lifecycle, persistence, probing, health polling, disconnect,
  and node selection now live in `features/connections/controller.rs`, reducing
  the application shell by another 990 lines.
- Connection form rendering, the connected toolbar, node scope selector,
  cluster overview, and topology rendering now live in
  `features/connections/view.rs`. `main.rs` is now 3,997 lines, down from
  11,755 before the feature-controller extractions.
- The blocking settings-sync workflow now belongs to `zedb-core::sync`.
  `zedb-app` schedules it and translates its typed result into UI state.
- `Workspace::new` still performs the same initialization, but delegates the
  three extracted feature aggregates to their own constructors. Further
  decomposition remains deliberately deferred because this constructor has a
  critical blast radius.

## Executive assessment

The repository is not structurally unhealthy. Its top-level crate direction is
clear, GitNexus reports no circular file imports, the ClickHouse engine is
usable without GPUI, and important safety rules live below the UI. Those are
good foundations.

The maintainability concern is real, but it is concentrated rather than
systemic. `zedb-app` is becoming the integration point for almost every kind of
state, policy, asynchronous work, and rendering. Files have been split by
feature, but many of those files still extend and mutate the same `Workspace`
type. The resulting boundaries are visual more than enforceable. At the same
time, some deterministic, headless logic has accumulated in the app binary,
which weakens the project's stated "headless core, thin clients" principle.

The recommended response is an incremental boundary correction, not a rewrite
and not a large new framework. First improve CI coverage and stop adding new
responsibilities to `Workspace`. Then extract one or two well-understood
vertical slices, using the result to establish a repeatable feature shape.

## Evidence snapshot

These numbers are observations, not quality targets. They are included so the
proposal can be checked against the current repository rather than argued from
taste.

| Observation | Current evidence | Why it matters |
|---|---:|---|
| `zedb-app/src/main.rs` size | 11,431 lines | Navigation, review, and conflict risk are concentrated in one file. |
| `Workspace` state | 71 fields | Unrelated features share one mutable lifetime and one construction path. |
| Methods in `main.rs` matching the current feature style | about 158 | The shell owns both coordination and detailed feature behavior. |
| Files containing `impl Workspace` | 12 files, 13 blocks | Moving code into another file has not created an ownership boundary. |
| First-party source size | app 28,655; ClickHouse 10,217; core 2,616; ACP 968; CLI 785 lines | Most product policy now has a strong incentive to land in the app crate. |
| Large non-app modules | `schema_intelligence.rs` 2,095; `regen.rs` 1,263; `runner.rs` 1,172 lines | A few engine modules are also approaching multi-responsibility size. |
| Vendored surface | 393 tracked files, about 10 MB, versus 125 tracked files under `crates/` | Repository-wide searches and upgrades carry substantial third-party noise. |
| Planning documents | 17 `PHASE-*.md` files; `devlog.md` is 784 lines | It can be difficult to tell current contract from historical plan. |
| File import cycles | 0 reported by GitNexus | The basic dependency graph is currently sound and should be preserved. |

The GitNexus index was refreshed before this review. Two representative flows
also confirm that cross-crate calls generally go in a sensible direction:

- Fleet execution travels from `zedb-app/src/fleet.rs` through
  `zedb-ch::runner` to `zedb-ch::client`.
- Settings synchronization travels from `zedb-app/src/settings_sync.rs` into
  `zedb-core::git`.

## What should be preserved

Several existing choices are helping maintainability and should not be lost in
a cleanup:

- The workspace crates have understandable purposes, and `zedb-core` does not
  depend on UI or ClickHouse implementation code.
- Mutating database operations are guarded in `zedb-ch::runner`, not only by
  buttons and confirmation overlays.
- `zedb-acp` is already a headless protocol client with a fake-agent test
  boundary.
- The vendor patch inventory is unusually explicit. Each local GPUI change has
  a location, rationale, and removal condition in `VENDOR-PATCHES.md`.
- Pure rule modules such as query and storage advice already have substantial
  unit tests. Their placement is questionable, but their design is a useful
  extraction seam.
- CI separates Linux-capable headless code from the macOS GPUI build.

## Improvement 1: turn `Workspace` back into a shell

Priority: high.

`Workspace` currently owns connection state, schema loading, health polling,
query tabs, tails, history, fleet operations, operational monitoring, agent
sessions, GitHub authentication, settings sync, updates, preferences, overlays,
and layout state. Feature files such as `fleet.rs`, `ops.rs`,
`settings_sync.rs`, and `agent_pane.rs` define their own state types, but they
also implement methods directly on `Workspace` and can reach all of its fields.

This creates several foot guns:

- A feature can silently depend on another feature's private state.
- Cancellation generations, notices, selection, and connection identity have
  no single owner.
- Construction and reset behavior are centralized in a very large `new` path.
- Tests for orchestration tend to require the GPUI application type even when
  the behavior itself is headless.
- Splitting another method into another file improves scrolling but not
  coupling.

The desired end state is a small application shell that composes feature
models and routes explicit outcomes between them. A practical shape is:

```text
Workspace
  ShellState
  ConnectionFeature
  SchemaFeature
  QueryFeature
  FleetFeature
  OpsFeature
  AgentFeature
```

This does not require an event bus, a Redux-style store, or a trait per
feature. Each feature can be an ordinary Rust struct with narrow methods. A
feature operation should receive the data and service handles it needs, then
return a small outcome such as `QueryEffect::RefreshSchema` or
`ConnectionEffect::Disconnected`. `Workspace` applies cross-feature outcomes
and owns only genuinely global concerns such as active view, window layout,
theme, and top-level notices.

Start with a low-risk slice, not the query editor. Settings sync is a good
candidate because it already has `SettingsSyncState`, a mostly headless
`run_tick`, and a small number of explicit triggers. Fleet is a good second
candidate once the pattern is proven.

### Guardrail while extraction is in progress

Do not add another top-level `Workspace` field or another sibling-file
`impl Workspace` without first checking whether the responsibility belongs to
an existing feature state. This is a review rule, not a permanent numeric
limit.

## Improvement 2: restore the headless-core contract

Priority: high.

The documentation says all logic lives in library crates and the GUI and CLI
are thin clients. The migration engine largely honors this. Newer explorer
features do so less consistently.

Examples of headless or mostly headless logic currently under `zedb-app`:

- `query_advisor.rs` derives deterministic ClickHouse recommendations from
  execution facts and explain plans.
- `storage_advisor.rs` ranks ClickHouse storage recommendations.
- `tail.rs` contains query construction and validation rules.
- `settings_sync.rs::run_tick` contains reconciliation orchestration and git
  policy.
- `fleet.rs` assembles runner, verification, targeting, and safety behavior
  around the view state.

The rule should be based on ownership, not on whether a function happens to be
pure:

- ClickHouse semantics and deterministic advice belong in `zedb-ch`.
- Database-independent models, persistence, repository rules, and sync policy
  belong in `zedb-core`.
- GPUI entities, focus, rendering, window actions, and presentation-specific
  state belong in `zedb-app`.
- Adapter parsing and output formatting belong in the adapter that exposes
  them.

Move the query and storage advice rule engines first. They are already pure,
well tested, and engine-specific. Keep their panels and GPUI state in the app.
This gives immediate headless testability without inventing a generic
`services` or `utils` crate.

Settings sync should be split similarly: `zedb-core` owns the reconcile and git
workflow, while `zedb-app` owns triggers, progress state, input controls, and
notifications.

## Improvement 3: give frontends typed use cases

Priority: medium to high.

The CLI, MCP server, and app often construct `RunnerOptions`, `Targets`,
`Verifier`, `Regenerator`, and clients directly. This is workable today, but it
allows adapter-specific defaults and error presentation to drift. The MCP
transport is also located in `zedb-ch::mcp`, even though JSON-RPC transport and
tool schema are adapter concerns rather than ClickHouse driver concerns.

Introduce a small typed use-case surface inside the existing library crates
before considering another crate. Examples might include:

```text
FleetService::status(request)
FleetService::upgrade(request)
SchemaService::snapshot(connection)
MigrationService::regenerate(repo, mode)
AdviceService::query_findings(facts)
```

Requests should carry safety-relevant choices explicitly. Results should be
structured values, not preformatted terminal or UI strings. The CLI, MCP, and
app can then remain responsible for argument parsing, protocol translation,
rendering, and user interaction.

Only create a separate `zedb-mcp` crate if the MCP server gains its own release
lifecycle, dependencies, or entry point. Until then, moving it under a clearly
named adapter module, or keeping the transport in the CLI package, is enough.
The goal is ownership clarity, not a higher crate count.

## Improvement 4: organize the app by owned features

Priority: medium.

The app currently has a flat list of more than twenty sibling modules. Several
names describe a screen, several describe a domain capability, and several are
shared implementation details. The flat layout makes every new feature look
equally global.

A useful target, reached gradually, is:

```text
crates/zedb-app/src/
  main.rs                 # process startup only
  shell/
    mod.rs                # Workspace composition and routing
    layout.rs
    notices.rs
  features/
    connections/
    schema/
    query/
    fleet/
    ops/
    agent/
    settings/
  ui/
    text_input.rs
    theme.rs
```

Each feature directory should own its model, actions, asynchronous commands,
and rendering. Its `mod.rs` should expose a narrow `pub(crate)` surface. A
feature should not reach into a sibling feature's fields. Cross-feature changes
go through shell-level effects or explicit method calls.

Do not perform a mechanical file shuffle first. Move a file only as part of
establishing or enforcing its ownership boundary, otherwise the change creates
review churn without reducing coupling.

## Improvement 5: split large engine modules by responsibility

Priority: medium.

`zedb-ch` has coherent external ownership, but some internal modules now span
several reasons to change:

- `schema_intelligence.rs` contains snapshot analysis, tokenization,
  resolution, and recommendation-building helpers.
- `regen.rs` combines SQL statement classification, state tracking, replay,
  file planning, and output verification.
- `runner.rs` combines target resolution, safety policy, tracking, audit, and
  every live migration operation.

These should remain one crate for now, but can become private submodules with a
small public facade. For example:

```text
zedb-ch/src/
  schema_intelligence/
    mod.rs
    snapshot.rs
    resolve.rs
    lint.rs
    advice.rs
  migrations/
    mod.rs
    targets.rs
    safety.rs
    tracking.rs
    execute.rs
    regenerate.rs
```

Split only where there is a named responsibility and an API boundary. A line
count alone is not a reason to create a file.

While doing this, prefer private modules and deliberate re-exports. The current
`zedb-ch` root exposes many implementation modules directly, which makes it
easy for adapters to couple to internals and makes later movement expensive.

## Improvement 6: close CI coverage gaps

Priority: high and low effort.

The CI structure is sensible, but two first-party test suites are not executed:

- The Linux job runs Clippy and tests for `zedb-core`, `zedb-ch`, and
  `zedb-cli`, but omits `zedb-acp`.
- The macOS job runs Clippy with all app targets and builds `zedb-app`, but it
  does not run the app's unit or integration tests.

`--all-targets` asks Clippy to compile test targets; it does not execute them.
This is an easy place for maintainability work to create false confidence.

Add `zedb-acp` to the Linux Clippy and test commands. Run
`cargo test -p zedb-app` in the macOS job, separating tests that truly require a
window server if any prove unsuitable for CI. Once that is reliable, prefer a
workspace-wide headless test command plus the dedicated app job so a newly
added crate is not silently omitted.

Architecture rules can also be checked cheaply. A small repository test can
assert the allowed crate dependency direction from `cargo metadata`. Avoid
introducing a large policy tool until more rules justify it.

## Improvement 7: treat vendoring as a maintained subsystem

Priority: medium.

Vendoring is justified here. The app depends on behavioral patches that are
not available in released upstream crates, and `VENDOR-PATCHES.md` records them
well. Removing the vendor directory immediately would trade visible burden for
missing product behavior.

The foot gun is that the vendored tree is larger, by file count, than all
first-party crates combined. It can dominate search results, code indexing,
review statistics, and broad mechanical changes.

Keep the current inventory, then add a reproducible refresh procedure:

1. Record the exact upstream commit for each vendored crate in one
   machine-readable place.
2. Provide a script that imports that revision and reapplies or verifies the
   local patch set.
3. Run the focused tests named by `VENDOR-PATCHES.md` after a refresh.
4. Configure repository analysis and search tools to exclude `vendor/` by
   default unless vendor work is explicitly requested.
5. Review each patch's removal condition during dependency upgrades.

Patch files layered over a pristine upstream tree may eventually make upgrades
easier, but converting the current working vendor should be evaluated against
the size and interaction of the multi-cursor changes. It is not automatically
an improvement.

## Improvement 8: distinguish current contracts from historical plans

Priority: medium.

The phase documents and devlog contain valuable reasoning, but the repository
does not make their lifecycle obvious. A contributor can find `SPEC.md`,
`NORTH-STAR.md`, `PRODUCT-PRINCIPLES.md`, many completed phase plans, idea
files, and the devlog, then still need to infer which document describes the
current architecture.

Create one short `docs/ARCHITECTURE.md` that describes only the current system:

- crate and module responsibilities;
- allowed dependency direction;
- important runtime flows;
- state and async ownership in the app;
- safety boundaries;
- links to deeper current contracts.

Add a small index to `docs/` marking documents as current contract, active
plan, historical plan, idea backlog, or investigation log. Completed phase
documents can remain in place or move under `docs/history/`; the important part
is that they no longer compete with current documentation.

Use short architecture decision records for lasting choices that have real
alternatives, such as vendoring GPUI, hand-rolling RowBinary, or locating MCP
transport. Keep transient debugging discoveries in `devlog.md`. This stops the
devlog from becoming the only place where a future maintainer can recover a
load-bearing decision.

## Proposed dependency rules

The following rules preserve the good parts of the current graph while making
future placement decisions easier:

```text
zedb-core   -> standard library and general-purpose dependencies
zedb-ch     -> zedb-core
zedb-acp    -> protocol dependencies, no app dependency
zedb-cli    -> zedb-core and zedb-ch use cases
zedb-app    -> zedb-core, zedb-ch, and zedb-acp use cases
vendor      -> patched upstream code, no zeDB product dependencies
```

Within `zedb-app`:

- Feature modules may depend on shared UI primitives and library use cases.
- Feature modules do not read or mutate sibling feature state directly.
- Only the shell coordinates cross-feature effects.
- Pure ClickHouse policy does not depend on GPUI types.
- Background work returns typed results; GPUI updates happen at the boundary.

Avoid a generic `common`, `helpers`, or `utils` module. Shared code should have
a named owner based on what it knows, not merely how many callers it has.

## Suggested sequence

### Stage 0: guardrails

- Add the missing ACP and app test execution to CI.
- Document allowed crate dependencies.
- Adopt the temporary review rule against new `Workspace` fields and extension
  blocks.
- Add a docs index that marks historical phase plans.

### Stage 1: prove a headless extraction

- Move query and storage advice rules and tests into `zedb-ch`.
- Leave their GPUI panels and presentation state in `zedb-app`.
- Split settings-sync policy from its GPUI state and triggers.

Success means these behaviors can be tested without compiling or constructing
the desktop app, with no user-visible change.

### Stage 2: extract feature ownership

- Make settings sync the first feature that no longer extends `Workspace`.
- Repeat with fleet or ops.
- Introduce explicit cross-feature effects only where direct ownership cannot
  express the flow.
- Move startup out of the current app body so `main.rs` trends toward process
  wiring rather than becoming a different large module.

### Stage 3: narrow engine and adapter APIs

- Put typed use-case facades in front of runner, verifier, regeneration, and
  schema operations.
- Migrate CLI, MCP, and app call sites one operation at a time.
- Make implementation modules private once callers use the facade.
- Reconsider MCP's package location after its lifecycle is clearer.

### Stage 4: reduce recurring repository friction

- Add the vendor refresh and verification workflow.
- Split the largest `zedb-ch` modules along proven responsibility boundaries.
- Create the current-state architecture document and archive or label completed
  plans.

## What not to do

- Do not rewrite the app around a new state-management framework.
- Do not create a crate for every screen or every pure function.
- Do not split files solely to meet a line limit.
- Do not add traits where there is only one implementation and no testing seam
  that benefits from substitution.
- Do not move safety checks out of `zedb-ch` while thinning UI code.
- Do not combine this work with broad UI changes or vendor upgrades.
- Do not attempt one large "architecture PR". Each extraction should preserve
  behavior, carry its tests, and leave the repository simpler at the end.

## Review questions

1. Is the "headless core" principle still intended to cover explorer advice,
   tails, settings sync, and fleet orchestration, or only the migration engine?
2. Should MCP remain a mode of the CLI, become an independently packaged
   adapter, or simply move behind a clearer internal boundary?
3. Which feature has the highest current change rate? That feature may be a
   better first `Workspace` extraction than settings sync despite higher risk.
4. Are completed phase documents meant to remain active specifications? If so,
   they need an index and explicit precedence rules rather than archival.
5. Which GPUI vendor patches are expected to survive for more than one upstream
   release? Those deserve the strongest automated refresh coverage.

## Definition of improvement

This proposal has worked when a normal feature change usually touches one
feature module plus a typed library use case, can be tested without constructing
the whole GPUI workspace, and does not require knowing the fields of unrelated
features. Smaller files are welcome, but reduced knowledge and a narrower blast
radius are the actual goal.
