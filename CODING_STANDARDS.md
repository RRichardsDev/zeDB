# Coding standards

These are zeDB's merge standards. They are inspired by the discipline used
for Wine core development, adapted to Rust and this repository. The objective
is not to imitate Wine's C formatting. It is to require patches that are small,
auditable, compatible, tested, and straightforward for a maintainer to own for
years.

## Adoption and current refactor

These standards describe the target for new work and future pull requests.
They are not authorization to retrofit the entire existing codebase in the
current `zedb-app` structural refactor.

For that refactor specifically:

- Preserve behavior and establish clear ownership boundaries first.
- Do not fix unrelated naming, error handling, formatting, comments, warning
  suppressions, or historical design choices merely because this document now
  recommends something better.
- Touch an existing standards violation only when it prevents a safe extraction
  or would make the changed boundary misleading or incorrect.
- Record broader cleanup as follow-up work and submit it separately after the
  structural refactor.
- Judge each refactor patch primarily on whether it reduces coupling without
  changing behavior and remains easy to review.

This keeps the refactor auditable. Standards adoption is a subsequent body of
work, not hidden scope inside file movement and state decomposition.

## The merge test

A pull request is mergeable only when a reviewer can answer yes to all of the
following:

1. Is the change necessary and is its purpose clear?
2. Is it the smallest coherent change that solves the stated problem?
3. Does each commit leave the tree buildable and testable?
4. Is the behavior proved by tests or by a documented manual check?
5. Does the code belong to the module that now owns it?
6. Are failures, cancellation, compatibility, and user data handled safely?
7. Can another maintainer understand and modify it without consulting the
   original author?

If any answer is no, the pull request is not ready.

## Patch discipline

- One patch solves one problem. Do not combine a feature, refactor, formatting
  sweep, dependency update, and unrelated cleanup.
- Keep patch series short. Split a large change into independently useful,
  reviewable commits with explicit boundaries.
- Refactors preserve behavior. Behavior changes require a separate commit and
  tests that make the difference visible.
- Do not reformat untouched code. Follow the current style of recently changed
  code in the component.
- Commit messages explain the reason and observable result, not the editing
  process.
- Never hide risk in a large mechanical diff. Move first, change second, or
  change first, move second.
- Do not commit generated artifacts, local state, credentials, signing
  material, or incidental workspace changes.

## Architecture and ownership

- `zedb-core` owns database-independent models, persistence, repository rules,
  and settings-sync policy.
- `zedb-ch` owns ClickHouse protocol behavior, SQL semantics, migration policy,
  and deterministic database analysis.
- `zedb-app` owns GPUI entities, rendering, focus, window actions, and
  presentation state.
- UI modules may schedule a headless operation and present its typed result.
  They must not absorb reusable database or repository policy.
- A feature owns its model, actions, asynchronous commands, rendering, and
  tests behind a narrow `pub(crate)` surface.
- Sibling features do not mutate each other's fields. Cross-feature behavior
  goes through explicit shell methods or typed effects.
- `Workspace` is an application shell, not a general-purpose store. Adding a
  top-level field or another sibling-file `impl Workspace` requires written
  justification in the pull request.
- New `utils`, `helpers`, `common`, or `misc` modules are rejected unless the
  responsibility can be named more precisely.
- Prefer private modules and deliberate re-exports. Public surface area is a
  maintenance commitment.

## Rust implementation rules

- `cargo fmt` output is mandatory.
- Clippy must pass with warnings denied. New warning suppressions require a
  comment explaining why the warning is wrong for that location.
- Production paths do not use `unwrap`, `expect`, `panic`, `todo`, or
  `unreachable` for recoverable input, I/O, network, process, or server errors.
- Errors include the failed operation and enough context to act on them, while
  excluding passwords, tokens, and sensitive SQL values.
- Types encode meaningful states. Avoid clusters of booleans when an enum or a
  state object makes invalid combinations impossible.
- Functions have one responsibility. Long functions are acceptable only when
  splitting them would obscure a single linear protocol or state transition.
- Comments explain invariants, compatibility constraints, or non-obvious
  decisions. They do not narrate syntax.
- `unsafe` code requires a documented safety invariant and focused tests.
- New dependencies require a clear benefit, compatible licensing, and an
  explanation of why the standard library or an existing dependency is
  insufficient.

## Asynchronous and stateful code

- Background work must not block the GPUI thread.
- Every long-running task has an owner and an explicit cancellation or stale
  result strategy.
- Generation counters, epochs, and abort handles are owned by the feature whose
  work they guard.
- Late task results must not overwrite newer selections, connections, tabs, or
  queries.
- Dropping a stream, watch, child process, or temporary resource must clean up
  its external work.
- State transitions should be centralized and testable. Rendering code should
  not quietly perform domain mutations.

## Compatibility and safety

- Preserve stored settings and session compatibility, or provide an explicit,
  tested migration.
- Preserve safe defaults: read-only connections, production visibility,
  bounded results, and confirmation for destructive operations.
- Never weaken a safety rule only in the UI. Database mutation policy belongs
  in the headless execution layer as well.
- Platform-specific behavior is isolated behind a narrow module and tested on
  the relevant platform.
- Do not assume a particular ClickHouse version, topology, transport, shell,
  home-directory layout, or signing identity unless the contract says so.
- Fallback behavior must be deliberate, observable where useful, and tested.
  Silent fallback must not change correctness or safety.

## Tests and evidence

- Bug fixes include a regression test that fails for the old behavior whenever
  practical.
- New behavior includes positive, negative, boundary, and failure-path tests
  proportional to its risk.
- Pure logic is tested beside its owning module without GPUI or a live server.
- ClickHouse semantics are verified against a real supported server when a
  parser or mock cannot prove compatibility.
- Tests are deterministic, isolated, and clean up processes, sockets, files,
  repositories, and database objects they create.
- A skipped or environment-gated test documents exactly what it requires and
  what remains unverified when it does not run.
- Manual checks supplement automated tests. They do not replace automatable
  assertions.

## Documentation and user impact

- User-visible changes update `CHANGELOG.md` under `Unreleased`.
- Internal architectural decisions and significant implementation findings go
  in `docs/devlog.md` or the relevant design document.
- Public types, persisted formats, safety invariants, and surprising fallbacks
  are documented where maintainers will encounter them.
- Finished UI contains product language, not milestone, phase, or
  implementation language.

## Required pre-merge checks

Run from the repository root:

```sh
cargo fmt --all --check
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
git diff --check
```

Also required:

- Run GitNexus impact analysis before changing symbols.
- Review HIGH or CRITICAL blast-radius warnings before editing.
- Run GitNexus `detect_changes` before committing.
- Exercise relevant macOS UI, signing, Docker, or ClickHouse integration paths
  when the patch touches them.
- Confirm the diff contains no unrelated user-owned files.

Passing automation is necessary, not sufficient. Reviewers may still reject a
patch that increases coupling, obscures behavior, lacks evidence, or imposes an
unreasonable long-term maintenance cost.

## Review conduct

- Review the code, not the author.
- Treat every comment as either required, suggested, or a question.
- Address every required comment in code or with concrete technical evidence.
- Do not resolve discussion by making the diff larger than necessary.
- When a design is uncertain, submit the smallest patch that proves the seam
  before migrating the difficult path.

## Wine influence

Wine's upstream culture emphasizes small patches, small merge requests,
submitter responsibility, current component style, compatibility evidence, and
tests. Those principles are the model for this document, while the specific
Rust, GPUI, ClickHouse, and repository rules above are zeDB's own.

## Comments

The type of comment should be clear and consistent. Only ever stating why a particular decision was made, not what the code is doing. And only ever why it would be unclear to the reader.

References:

- [Wine GitLab workflow proposal](https://www.winehq.org/pipermail/wine-devel/2022-April/214894.html)
- [Wine discussion of current component style](https://list.winehq.org/archives/list/wine-devel%40list.winehq.org/message/6JD7DDEAJ4P6F32QH4MNSX6VJ5K5PPYM/)
- [Wine submitter sign-off rationale](https://www.winehq.org/pipermail/wine-devel/2015-September/109428.html)
