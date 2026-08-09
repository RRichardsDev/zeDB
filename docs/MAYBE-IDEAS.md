# Maybe ideas

Looser than a phase ideas list: stray thoughts, half-wishes, and
"would that be nice?" notes. No ranking, no commitment, no cleanup
duty. Promote anything that grows legs into the current phase ideas
doc; delete freely.

- Query history drawer: run an entry directly from the drawer
  (today it inserts; a second affordance could execute).
- History: record cancelled runs too, marked as such.
- Hover cards in the history drawer could show rows/duration meta
  inside the card, not just the SQL.
- cmd+click a fully-formed URL in a data-table cell to open it in the
  browser (detect http(s):// values, underline on cmd-hover).
- Onboarding step to opt into the bigger surfaces: fleet view, ops
  view, AI agent threads. Framing matters: this is NOT an AI upsell.
  The agent-thread option is only about surfacing a workflow the user
  ALREADY has (their own installed Claude Code / Codex CLI) inside
  zeDB; if they don't use those, it stays hidden and unmentioned.
  Opt-in, off by default, never nagged.
  This opt-in is also the STRUCTURAL enforcement of the product spine
  (docs/PRODUCT-PRINCIPLES.md): the test "no agent in front of someone
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
  docs/PRODUCT-PRINCIPLES.md): it never fires without a real agent
  present, states a fact about the user's own rule rather than
  selling, and MUST be self-silencing: rare to begin with, each
  Ignore ratchets frequency down, and a few Ignores stop it for good
  (a repeated no is a reaffirmed no; continuing past it is the nag the
  spine forbids). Cadence to settle: user floated ~1-in-5-to-10
  eligible moments before the first ratchet.
- When a query runs past ~30s, quietly suggest the explain ("still
  running… see why: Explain query"), triggering the palette
  command's action directly.
