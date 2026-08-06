# Phase 4 draft: embedded runners

Status: DRAFT. Not committed scope. This is a parking spot for runner
design thinking, written before Phase 3 has landed, and it violates the
usual rule of planning a phase only when the previous one is ending.
Everything after M0 is conditional on M0's outcome; expect this file to
be rewritten, possibly heavily, at Phase 3 exit. The honest version of
this plan lives in one sentence: applications should be able to deploy
their own schema from the migration repo, in their own language, and
zeDB should not be reimplemented to make that possible.

## Goal

Libraries for Java, Python, Node, C++, Rust, and PHP that let an
application own its databases' schema lifecycle at startup or release
time: read the repo's chain and generated current-state, provision new
databases from current-state, upgrade existing ones through the chain,
and stamp the tracking tables when done. A fleet deployed by runners
and a fleet deployed by zeDB must be indistinguishable: same tracking
rows, same verify results, same status matrix.

Opt-in per repo and per language, disabled by default: a `[runners]`
table in zedb.toml lists what a repo supports; a repo that never opts
in sees no generated artifacts, no config surface, no behaviour change.

## The fork that decides everything (M0)

Three candidate shapes, genuinely different products:

1. **C ABI over zedb-core.** A `zedb-ffi` crate exposing a small stable
   C surface (open repo, plan, apply, stamp), with thin native bindings
   per language. Best fidelity (it IS the engine); cost is six binding
   packages, cross-compilation matrices, and an ABI to keep stable.
2. **Vendored CLI subprocess.** Each library bundles or locates the
   `zedb` binary and drives it with `--json`. Cheapest to build and the
   engine stays one artifact; cost is binary distribution per platform,
   subprocess management in six ecosystems, and awkwardness in
   containers and locked-down runtimes.
3. **Dumb-client protocol.** A documented spec (read chain, render
   parameters, apply statements, write tracking rows) that each library
   implements against its ecosystem's existing ClickHouse client.
   Lightest artifacts and most idiomatic per language; cost is six
   implementations of the protocol, conformance testing to keep them
   honest, and a hard line: anything replay-dependent (regen, checks,
   equivalence) is excluded and stays in CI.

Notes to accumulate here as Phase 3 teaches us things: how painful the
tracking-write path is in practice, whether admin routing matters to
runners (it likely does: OPTIMIZE and SYSTEM statements appear in real
chains), and whether current-state provisioning alone (shape 3 minus
upgrades) would cover most application needs.

## Milestones (all conditional on M0)

### M0. Settle the binding shape

Build the smallest end-to-end proof of each viable shape against the
demo fleet: one language (probably Python, the fleet's lingua franca),
one operation path (provision from current-state, upgrade one pending
migration, stamp). Measure what actually hurts: distribution, auth,
error surfaces, the tracking contract. Write the decision down with the
rejected shapes' reasons, FORMAT.md gaining whatever the chosen shape
needs (a conformance section for shape 3, an ABI version for shape 1).

Done when: the shape is chosen for reasons demonstrated rather than
argued, and a design doc exists that a contributor could implement a
seventh language from.

### M1. The reference runner

The chosen shape, productionized, in one language: full operation set
(plan, provision, upgrade, stamp, status), the safety subset that makes
sense embedded (runners are automation, so the interactive ladder does
not apply, but dry-run, refusal on excluded databases, and audit-style
logging do), and a conformance suite runnable against any future
runner: same repo in, same tracking rows and fleet state out, verified
by `zedb verify` and the status matrix.

Done when: an example application deploys and upgrades its own
databases via the reference runner, and zeDB's fleet view cannot tell
it was not the CLI.

### M2. The language spread

The remaining languages, in demand order rather than all at once, each
passing the same conformance suite. Packaging per ecosystem (Maven,
PyPI, npm, crates.io, Packagist, and whatever C++ deserves that year).
Per-language enablement in zedb.toml drives what is generated or
supported; everything stays disabled by default.

Done when: each shipped language passes conformance, and adding a
language is a documented, bounded exercise rather than a research
project.

### M3. Runner soak

Real applications using runners against real clusters, the fleet view
watching. Whatever the collision of application deploy schedules and
fleet-wide migration operations teaches (locking? stamp races?
concurrent upgrades of one database?) gets designed for deliberately
rather than patched.

Done when: runners in real use coexist with fleet operations without
surprises, and the concurrency story is written down.

## Explicitly not Phase 4

Forge integration (still), ops actions from runners (kill query, grants;
still parked hard), runners performing regen or checks (replay stays in
CI regardless of shape), guest-driver runners.

## Phase exit

Runners are a shipped, conformance-tested way for applications to
deploy their own schema in at least the languages people actually
asked for, and the spec's follow-ups queue is empty enough that what
remains is release engineering and IDEAS.md.
