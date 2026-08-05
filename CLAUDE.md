<!-- gitnexus:start -->
# GitNexus : Code Intelligence

This project is indexed by GitNexus as **zeDB** (440 symbols, 1244 relationships, 31 execution flows). Use the GitNexus MCP tools to understand code, assess impact, and navigate safely.

> Index stale? Run `node .gitnexus/run.cjs analyze` from the project root : it auto-selects an available runner. No `.gitnexus/run.cjs` yet? `npx gitnexus analyze` (npm 11 crash → `npm i -g gitnexus`; #1939).

## Always Do

- **MUST run GitNexus only when preparing to commit.** Do not run per-symbol impact analysis during routine implementation. Before committing, run `detect_changes()` to verify the completed change affects only expected symbols and execution flows. For regression review, compare against the default branch: `detect_changes({scope: "compare", base_ref: "main"})`.
- **MUST warn the user** if the pre-commit GitNexus review returns HIGH or CRITICAL risk before committing.

## Never Do

- NEVER run GitNexus impact analysis for each routine symbol edit unless the user explicitly requests it.
- NEVER ignore HIGH or CRITICAL risk warnings from the pre-commit GitNexus review.
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
