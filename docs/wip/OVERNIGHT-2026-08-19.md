# Overnight run, 2026-08-19 -> 20 (Phase 13 slices 2-3)

Working notes from the unattended session. Delete after reading.

## Shipped (local commits only, NOT pushed, per instruction)

- `c285a6a` slice 2: status-bar Cloud cost with burn-rate warning.
- `52918bb` slice 3: read-only `cloud_context` for the agent/MCP.
- Both pass fmt, clippy, and unit tests (zedb-app 98, zedb-ch 84).

## Decisions I made without you (review these)

1. **Burn-rate rule**: warn when the last COMPLETE day (yesterday)
   exceeds 1.5x the median of the prior complete days, with a 1 CHC
   floor and a >= 7 complete-days history guard. Yesterday, not
   today-extrapolated: extrapolation guesses, and the review bar says
   no lying. Your warehouse (3 days old) correctly stays quiet.
2. **Chip wording**: "cloud 5.35 CHC today" + " . high burn" in amber
   when breached. Click opens the connection page's Cost tab.
3. **cloud_context answers from app state** (per call, freshness
   stated) rather than fetching inline: the bridge is synchronous.
   Each call kicks the background refreshes so a follow-up call is
   fresher.
4. **Byte-caps-as-billing-ceilings** (the 10.5d note) interpreted as
   documentation, not math: the tool's reply states that run_query's
   10 GiB bytes-to-read cap is paid compute on Cloud, a per-query
   billing ceiling. Converting bytes to credits would invent pricing.

## Not verified (needs you or a live agent session)

- **The chip with real data over a real connect**: connecting needs
  the protected Keychain (Touch ID prompt appeared; I cancelled it).
  Verified instead via (a) the live cost API: expected figures today
  5.35 / yesterday 0.18 / median 17.51 CHC over 3 days, and (b) a
  temporary debug injection for visuals, reverted before commit.
  Screenshots: /tmp/zedb-night-06-quiet.png (quiet),
  /tmp/zedb-night-07-burn.png (high burn + tooltip).
  First real connect this morning shows it live.
- **cloud_context through a real agent session**: the tool is unit-
  tested for advertisement and wired through the bridge, but I could
  not run your ACP agent. Open the agent pane and ask "what's my
  cloud context?" to exercise it.

## Cloud state on wind-down

Service 1 idle, Service 2 stopped, "me" idle: everything asleep,
nothing burning. (Service 1's earlier run auto-idled on its own
timeout; I stopped Service 2 during the primary-stop rule test before
you left.)

## Loose end

- /tmp screenshots are temp files per your note; they vanish on
  reboot. Nothing else uncommitted; AGENTS.md/CLAUDE.md were kept
  clean of the gitnexus regeneration clobber throughout.
