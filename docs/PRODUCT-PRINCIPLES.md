# Product principles

The load-bearing beliefs behind zeDB. Read this before adding a
feature that changes the tool's character, not just its surface.
docs/UI-DESIGN.md governs how things look; this governs what earns a
place at all.

## The spine

**Do it yourself, but your own agents can help natively when things
get annoying and you would be reaching for them anyway.**

zeDB is a hands-on tool first. The user drives: they write the SQL,
read the plan, judge the drift, apply the migration. The craft is
theirs and the tool respects that.

The agent integration is not the product's pitch and never becomes
it. It exists because a ClickHouse practitioner already alt-tabs to
their own installed Claude Code or Codex when a query fails or a
migration gets fiddly; zeDB just removes the alt-tab. It is the
user's own agent, their own credentials, invoked at the exact moment
they were already going to reach for it (a failed query, a gnarly
diff), with the context they would otherwise paste by hand.

## What this rules out

- No AI upsell. Nothing suggests, nudges toward, or advertises agents
  to someone not already using them. The agent surfaces stay hidden
  and unmentioned for users who have no agent CLI installed.
- No agent-first workflows. Every core task has a complete
  hands-on path; the agent is an accelerator on top, never the only
  way through.
- No hosted/managed AI. The user brings their own agent and
  credentials; zeDB does not proxy, host, or bill for inference.
- No doing-it-for-you by default. The agent proposes; the user
  applies. Actions that touch a server stay explicit.

## Why write it down

This is the belief most easily eroded one reasonable-sounding default
at a time. A new "just enable it for everyone" or "suggest the agent
here" is how tools drift from respecting the user into managing them.
When a feature brushes against this, the burden is on the feature to
justify itself against this page, not the other way round.
