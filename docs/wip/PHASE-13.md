# Phase 13: Cloud control plane, continued

Wake-aware connecting, cost awareness, and agent context: the part of
the 10.5c/d remainder (`docs/wip/IDEAS.md`, `docs/contracts/CLOUD-STRATEGY.md`)
worth doing now, under its own number since 10.5 shipped two releases
ago.

Status: IN PROGRESS (2026-08-19). The dashboard's wake-state
management (per-service
Wake, Wake all, waking watch with control-plane polling) landed first
and is the shared machinery for slice 1.

The scope is deliberately the two control-plane surfaces that ride on
what already exists, plus the read-only agent exposure. The audit-log
timeline, ClickPipes surfacing, and backup-restore-as-migration-
rehearsal stay in IDEAS.md: the rehearsal item couples to migrations,
which wait on the analytics-clickhouse-ddl battle-test.

Facts that shape this (hard-won in 10.5a/b, do not re-derive): state
PATCH commands are start/stop/awake and awake is what an idled service
needs; waking is a management write only an API key can do (sign-in
tokens are read-only); the plane keeps reporting `idle` for a while
after accepting a wake, so watches outlive the state string.

## Slice 1: wake-before-connect

Status: BUILT (2026-08-19). The probe-failure explanation path now
wakes an asleep service (or watches a waking one), keeps the
connecting state honest, and retries the connect when the control
plane reports running; bounded at ~6 minutes, abandoned if another
connect starts.

Connecting to an asleep Cloud service should offer to wake it and then
connect, instead of timing out with an explanation.

- On connect, when the linked service is asleep and the org has a key:
  wake it (the dashboard's watch machinery), show honest progress
  ("waking, takes a few minutes"), and complete the connect when the
  state settles.
- Without a key, keep today's honest failure (name the cause, point at
  linking a key).
- No auto-wake without an explicit user action; waking costs money.

## Slice 2: cost in the status bar, burn-rate aware

The dashboard already fetches the warehouse-scoped 30-day cost. Put a
quiet daily figure in the status bar for Cloud connections, with a
warning accent only when the recent burn rate is clearly above the
month's norm. Muted semantic colors per `docs/contracts/UI-DESIGN.md`; no
persistent alarm badges.

## Slice 3 (10.5d): control-plane context for the agent

Expose state, tier, and cost read-only to the in-app agent and MCP,
per the `docs/contracts/ACP-STANDARDS.md` checklist (read-only tool, no server
writes, no wake). Re-reason the byte caps as billing ceilings using
slice 2's burn-rate numbers.

## Review bar (learned building slices 0-1; apply first time)

Every remaining slice gets checked against these; they are what the
user actually inspects:

- **No label may lie or mislead.** Words carry exact truth: "not
  tested" became "not connected" because the old word implied it had
  never worked. If a state is optimistic (shown before the control
  plane confirms), the design must either be honest about it or
  revert visibly on refusal.
- **One state, every surface.** A transition started anywhere
  (dashboard, sidebar, connect flow, Cloud page) must show everywhere
  that state is displayed, immediately, and keep updating until
  settled. The wake buttons going stale while connect woke the
  service was the bug that taught this.
- **Transitions are watched, bounded, and abandoned.** Anything
  in-flight polls until it settles, gives up after a stated bound
  with a message, and stops when a newer action supersedes it.
- **Confirms must be unmissable.** An armed destructive action says
  what the next click does in plain text where the eye already is
  (not only a tooltip or a color), and disarms itself when abandoned.
- **Disabled means explained.** A control that cannot work (no API
  key, upstream rule) renders disabled with the reason in its
  tooltip; it never lets a click bounce off a 4xx.
- **Test upstream rules before encoding them.** Docs and plausible
  theories both got the primary-stop rule wrong until curl against
  the live control plane settled it (400 in every secondary state).
  A rule shipped in the UI cites the live test, not the docs.
- **Icons follow the app's icon grammar.** Quiet utility icons
  (group_hover recoloring, color set on the svg element), hover color
  announces intent (green = wake, red = stop), primary actions with a
  cost keep words.

## Acceptance

- Connecting to an idle service with a linked key is one flow: wake,
  watch, connect; the user never sees a raw timeout.
- The status bar shows Cloud cost quietly; the warning state has a
  defined threshold and a tooltip that explains it.
- The agent can read Cloud context but cannot change service state;
  the ACP doc's tool table gains the new tool with its rationale.
