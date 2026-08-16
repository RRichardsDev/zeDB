# Phase 10.3: connection-scoped workspace state

Status: PLANNED. Groups the session-scoping items from
`docs/IRL-ISSUES.md` plus two defects found during the 10.1
verification.

What follows the connection and what does not, made deliberate:

## Scope

- SQL history becomes connection-specific.
- Open tabs become connection-specific; switching connections shows that
  connection's tabs, not a shared pool.
- Saved queries stay global (explicitly not scoped).
- Fix: a restored session re-seeds its tails before the restored node
  selection applies, so a tail shows one node's rows while connected to
  another until a manual node flip (devlog 2026-08-16). Restore must
  apply the node selection first, then seed.
- Fix: switching connections freezes a tail (its loop exits on the name
  mismatch and never resumes). With per-connection tabs the tail should
  suspend with its tab and restart cleanly when its connection returns.

## Acceptance

- Two connections show disjoint histories and tab sets; saved queries
  appear in both.
- Relaunch on a multi-node connection seeds every restored tail against
  the restored node; no manual flip needed.
