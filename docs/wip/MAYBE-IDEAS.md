# Maybe ideas

Looser than a phase ideas list: stray thoughts, half-wishes, and
"would that be nice?" notes. No ranking, no commitment, no cleanup
duty. Promote anything that grows legs into the current phase ideas
doc; delete freely.

- Badge known bridge clients in the ops and workload views: queries
  arriving via pg_clickhouse (the official Postgres FDW) or the
  postgres/mysql wire protocols already carry client signatures in
  system.processes; a "via pg_clickhouse" tag answers "humans or the
  bridge?" at a glance. String match, ClickHouse-side only; zeDB
  never becomes a Postgres client (2026-08-22 thread-pull verdict).
- Query history drawer: run an entry directly from the drawer
  (today it inserts; a second affordance could execute).
- History: record cancelled runs too, marked as such.
- Hover cards in the history drawer could show rows/duration meta
  inside the card, not just the SQL.
- cmd+click a fully-formed URL in a data-table cell to open it in the
  browser (detect http(s):// values, underline on cmd-hover).
- Auto-update, but ONLY if it captures every single state before it
  quits. The asymmetry is the whole point: an update takes seconds,
  redoing lost work takes hours, so a single dropped in-flight edit
  makes the feature a net negative. It is a state-capture feature
  that happens to update, not an updater.
  There is already a partial base: zedb_core session.json
  (save_session / take_session) persists open query tabs' SQL and the
  active tab. "Every single state" means extending that to the full
  working set: per-tab cursor/selection, unsaved editor buffers
  (already the SQL, keep it exact), which view was showing
  (editor/ops/fleet) and its sub-state (ops tab+scope, history drawer
  open), the connected connection to auto-reconnect, pane sizes
  (already persisted), and any half-written migration in the author
  overlay.
  The honest caveat: some states can't be frozen and resumed, only
  waited out -- an in-flight agent turn, a running export, a
  streaming query. For those, DEFER the auto-update until idle rather
  than interrupting; never trade a running operation for a version
  bump. Rule of thumb: auto-update may cost seconds of waiting, never
  a byte of the user's work.
- Onboarding step to opt into the bigger surfaces: fleet view, ops
  view, AI agent threads. Framing matters: this is NOT an AI upsell.
  The agent-thread option is only about surfacing a workflow the user
  ALREADY has (their own installed Claude Code / Codex CLI) inside
  zeDB; if they don't use those, it stays hidden and unmentioned.
  Opt-in, off by default, never nagged.
  This opt-in is also the STRUCTURAL enforcement of the product spine
  (docs/contracts/PRODUCT-PRINCIPLES.md): the test "no agent in front of someone
  who didn't summon it" becomes a single upstream gate instead of
  each surface checking for itself. Without the opt-in flag, the agent
  pane, the error-bar Ask button, and the cmd+N/cmd+I agent shortcuts
  don't exist at all, rather than each one behaviorally hiding when no
  agent is configured.
- Stale-preference nudge (pairs with the opt-in above). A user who
  said "no" to agents at onboarding may have taken one up since; the
  old click shouldn't trap them into thinking zeDB can't help. When
  the AI-off rule is set AND an agent CLI is actually detected AND we
  are at a moment it would have helped (an error), occasionally flash
  a neutral status-bar line: "AI-off rule enforced; agents detected"
  with Enable / Ignore. This is NOT an upsell (see
  docs/contracts/PRODUCT-PRINCIPLES.md): it never fires without a real agent
  present, states a fact about the user's own rule rather than
  selling, and MUST be self-silencing: rare to begin with, each
  Ignore ratchets frequency down, and a few Ignores stop it for good
  (a repeated no is a reaffirmed no; continuing past it is the nag the
  spine forbids). Cadence to settle: user floated ~1-in-5-to-10
  eligible moments before the first ratchet.
- When a query runs past ~30s, quietly suggest the explain ("still
  running… see why: Explain query"), triggering the palette
  command's action directly.
- Cross-version robustness for `system.*` reads. We query specific
  columns from memory, and they vary by ClickHouse version (e.g.
  `system.merges` had no `total_rows_count` on an older server, which
  errored the merges panel). A version-tolerant approach would probe
  `system.columns` for what exists and degrade gracefully, or keep a
  vetted stable-columns list, applied app-wide. Deferred for now: the
  breakages have been one-off and patchable, so this is a hardening
  pass to do when a version gap actually bites, not speculatively.
