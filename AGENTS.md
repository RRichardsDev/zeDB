<!-- gitnexus:start -->
# GitNexus: Code Intelligence

This project is indexed by GitNexus as **zeDB** (3601 symbols, 10669 relationships, 295 execution flows). Use the GitNexus MCP tools to understand code, assess impact, and navigate safely.

> Index stale? Run `node .gitnexus/run.cjs analyze` from the project root; it auto-selects an available runner. No `.gitnexus/run.cjs` yet? `npx gitnexus analyze` (npm 11 crash → `npm i -g gitnexus`; #1939).

## Always Do

- **MUST run impact analysis before editing any symbol.** Before modifying a function, class, or method, run `impact({target: "symbolName", direction: "upstream"})` and report the blast radius (direct callers, affected processes, risk level) to the user.
- **MUST run `detect_changes()` before committing** to verify your changes only affect expected symbols and execution flows. For regression review, compare against the default branch: `detect_changes({scope: "compare", base_ref: "main"})`.
- **MUST warn the user** if impact analysis returns HIGH or CRITICAL risk before proceeding with edits.
- When exploring unfamiliar code, use `query({search_query: "concept"})` to find execution flows instead of grepping. It returns process-grouped results ranked by relevance.
- When you need full context on a specific symbol (callers, callees, which execution flows it participates in), use `context({name: "symbolName"})`.
- For security review, `explain({target: "fileOrSymbol"})` lists taint findings (source→sink flows; needs `analyze --pdg`).

## Never Do

- NEVER edit a function, class, or method without first running `impact` on it.
- NEVER ignore HIGH or CRITICAL risk warnings from impact analysis.
- NEVER rename symbols with find-and-replace; use `rename` which understands the call graph.
- NEVER commit changes without running `detect_changes()` to check affected scope.

## Resources

| Resource | Use for |
|----------|---------|
| `gitnexus://repo/zeDB/context` | Codebase overview, check index freshness |
| `gitnexus://repo/zeDB/clusters` | All functional areas |
| `gitnexus://repo/zeDB/processes` | All execution flows |
| `gitnexus://repo/zeDB/process/{name}` | Step-by-step execution trace |

## CLI

| Task | Read this skill file |
|------|---------------------|
| Understand architecture / "How does X work?" | `.claude/skills/gitnexus/gitnexus-exploring/SKILL.md` |
| Blast radius / "What breaks if I change X?" | `.claude/skills/gitnexus/gitnexus-impact-analysis/SKILL.md` |
| Trace bugs / "Why is X failing?" | `.claude/skills/gitnexus/gitnexus-debugging/SKILL.md` |
| Rename / extract / split / refactor | `.claude/skills/gitnexus/gitnexus-refactoring/SKILL.md` |
| Tools, resources, schema reference | `.claude/skills/gitnexus/gitnexus-guide/SKILL.md` |
| Index, status, clean, wiki CLI commands | `.claude/skills/gitnexus/gitnexus-cli/SKILL.md` |
<!-- gitnexus:end -->

## Running the app

- Launch through `.claude/skills/run-zedb/SKILL.md` (signed build via
  `scripts/run-signed-macos.sh`, quit-before-relaunch). Bare debug
  builds cannot read the protected Keychain, and `open` silently
  no-ops while an instance is already running.

## Product principles

- `docs/contracts/PRODUCT-PRINCIPLES.md` states the tool's spine: hands-on
  first, with the user's own agents helping natively only where they
  were already reaching for them. Not an AI upsell. Read it before
  any feature that changes the tool's character (especially anything
  that surfaces, defaults, or nudges the agent integration).

## Agent (ACP) integration

- `docs/contracts/ACP-STANDARDS.md` is the contract for the in-app agent pane:
  which tools exist, why every one is read-only or propose-only, what
  the agent may never reach (server writes, the write lock), and the
  checklist for adding a tool. Read it before touching
  `features/agent/`, `crates/zedb-acp`, or `crates/zedb-ch/src/mcp*`.

## UI work

- Before changing the user interface, read and follow `docs/contracts/UI-DESIGN.md`.
- Reuse or extend existing UI primitives before introducing one-off controls.
- `vendor/gpui-component` carries local patches, each marked with a
  `zeDB patch` comment and cataloged in `docs/contracts/VENDOR-PATCHES.md`.
  Keep both in sync when patching the vendor; read that file before
  any vendor rebase.

## Migration repo format

- `docs/contracts/FORMAT.md` specifies the format-1 migration repo (zedb.toml,
  migrations/YYYY/MM/NNNNN, rollback classes, current-state/); the
  vision and differentiator ranking live in `docs/contracts/SPEC.md`.

## Working docs

- `docs/wip/` holds the working state: active and deferred phase
  docs, IDEAS.md / MAYBE-IDEAS.md parking lots, IRL-ISSUES.md (the
  raw inbox), and in-flight refactor notes. Top-level `docs/` is the
  durable contracts only. Retired phase docs are deleted, not moved.

## Changelog

- Every user-facing change adds a line under `## Unreleased` in
  `CHANGELOG.md` as part of making it. User-facing means a user of the
  app or CLI would notice; internals go in `docs/devlog.md` instead.
- Cutting a release renames Unreleased to `## vX.Y.Z - date` and
  CURATES the accumulated bullets into grouped, readable notes (###
  subheadings by theme, tight bullets, no mid-sentence patches); the
  raw accumulation is drafting material, not the final notes. The
  release workflow publishes that section verbatim as the release
  notes. A fresh empty Unreleased is not needed; the next change
  recreates it.
