# `zedb-app` security review plan

## Purpose

Review `zedb-app` as the final application boundary after the `zedb-ch`,
`zedb-acp`, `zedb-cli`, and `zedb-core` reviews. The app joins those crates to
the desktop UI, owns user confirmation state, talks to OAuth and Cloud control
planes, clones repositories, installs updates, and launches hidden helper
modes.

The review is about hostile or malformed external input and normal user
workflows. A user deliberately running arbitrary commands, or an unrelated
process that already has the same user's full filesystem authority, is not
treated as a useful attacker model.

## Security contracts

- `docs/contracts/PRODUCT-PRINCIPLES.md` keeps every server write explicit and
  user-driven.
- `docs/contracts/ACP-STANDARDS.md` keeps agent tools read-only or
  propose-only, with live app state behind an authenticated capability.
- `docs/contracts/UI-DESIGN.md` requires production and destructive risk to be
  clear at the moment it matters.
- The completed crate reviews remain authoritative for their internals. This
  review revisits a dependency only when the app supplies unsafe input or
  relies on a security property the dependency does not provide.

## Scope and priority

| Priority | Surface | Main risks |
| --- | --- | --- |
| P0 | Self-update | Signature confusion, archive abuse, unbounded download, unsafe replacement |
| P0 | Server mutations | Stale connection context, UI-only confirmation, production guard bypass |
| P0 | Cloud password provisioning | Rotation without live confirmation, result attached to the wrong form |
| P1 | Managed Git checkouts and settings sync | Reusing a checkout from a different remote, disclosure to the wrong repo |
| P1 | Schema analysis and query export | Destructive temporary-name collision, stale file deletion |
| P1 | OAuth and Cloud clients | Token disclosure, redirect handling, unbounded replies |
| P2 | App persistence and hidden helper modes | Secret persistence, helper authority, malformed local state |

## Run sequence

1. Map the app's security-sensitive execution flows with GitNexus and inspect
   the updater, mutation, OAuth, Cloud, export, Git, and agent boundaries.
2. Reproduce candidate findings where practical and discard issues that need
   a same-user attacker with equivalent existing authority.
3. Record confirmed findings and reviewed-clean areas in `security-review.md`.
4. Completed upstream impact analysis before symbol edits, then remediated in
   severity order with focused regression coverage.
5. Run final workspace tests, advisory checks, and GitNexus change-scope
   analysis before handing the reviewed change set back for commit.

## Closing requirements

- User-visible hardening is recorded under `CHANGELOG.md` Unreleased.
- Production and destructive actions enforce their checks at the execution
  sink, not only through rendered button state.
- Update authenticity is verified with a code-signing requirement, not parsed
  display text.
- Managed checkout identity includes the complete requested remote.
