# `zedb-core` security review plan

## Purpose

Review `zedb-core` as the foundation trust boundary the already reviewed
crates stand on. It parses the migration repository format (often a shared
git checkout, so semi-trusted third-party content), runs the system git as a
subprocess inside that checkout, persists preferences, query history, saved
tabs, and sessions, holds the Keychain secret paths, and implements settings
sync, whose documents are remote input once an account is involved.

The review must answer four questions:

1. Can repository content (zedb.toml, migration SQL, templates, directory
   names, exclusions) escape its intended SQL, filesystem, or UI boundary in
   any consumer?
2. Can a git subprocess run attacker-configured code, follow injected
   arguments, or leak into the UI, given that the checkout itself may carry
   hostile git configuration?
3. Can secrets reach disk, sync payloads, process metadata, or logs outside
   the Keychain, and do local persistence files carry correct private modes?
4. Can remote sync documents, corrupt local files, or oversized content
   crash the app, resurrect filtered settings, or influence execution?

Findings are established before remediation. Every code change receives an
upstream GitNexus impact check and focused regression coverage.

## Security contracts

- `docs/contracts/FORMAT.md` defines the repository format grammar, the
  plain-identifier rules, and scope-name restrictions this crate enforces.
- `docs/contracts/ACP-STANDARDS.md` binds what synced or agent-adjacent state
  may carry between machines and processes.
- The completed `zedb-ch`, `zedb-acp`, and `zedb-cli` reviews are
  authoritative for their crates; shared code is revisited only where
  `zedb-core` supplies the input or the contract.
- `AGENTS.md` requires impact analysis before symbol edits, warnings for High
  or Critical blast radius, and `detect_changes()` before commit.

## Scope and priority

| Priority | Surface | Main risks |
| --- | --- | --- |
| P0 | Repo format parsing (`repo/`) | Traversal via config or directory names, template escapes into SQL, unbounded reads, parser panics from crafted repos |
| P0 | Git subprocess (`git.rs`) | Hostile repo-local git config executing code, argument injection, output injection, missing bounds and deadlines |
| P0 | Secrets and session (`secrets.rs`, `session.rs`) | Plaintext fallbacks, permissive modes, tokens in errors, stale credentials after sign-out |
| P1 | Settings sync (`sync.rs`, `preferences.rs`, `connection.rs`) | Remote documents influencing execution, filtered fields resurrected by merges, secrets serialized into payloads |
| P1 | Local persistence (`history.rs`, `saved_tabs.rs`, `store.rs`) | Credential-bearing SQL on disk with permissive modes, corrupt-file panics, unbounded growth |
| P2 | Value formatting (`value.rs`) | Server-controlled bytes to UI, parsing panics, UTF-8 boundary errors |

## Run sequence

1. Parallel manual audit of the six surfaces above, each traced through its
   consumers in `zedb-app`, `zedb-cli`, and `zedb-ch`.
2. Cross-verify every candidate finding against the code; discard anything
   not reproducible in source.
3. Register findings with severity, CWE, and concrete failure scenarios in
   `security-review.md`.
4. Remediate in severity order behind impact analysis; add regression tests
   beside the existing `repo_format.rs` / `repo_import.rs` suites where the
   format is involved.
5. Full-workspace fmt, clippy, and test evidence; `detect_changes()` scope
   check; one commit.

## Closing requirements

- Add a changelog entry for every user-visible hardening change; use the
  devlog for internal-only work.
- Update `docs/contracts/FORMAT.md` if any enforced grammar changes.
- Record checked-and-clean areas, not only findings.
