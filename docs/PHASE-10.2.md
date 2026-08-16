# Phase 10.2: fleet lock and regen flow

Status: PLANNED. Groups the fleet-view items from `docs/IRL-ISSUES.md`.

Small, high-touch polish on the migration fleet view, the daily-driver
surface.

## Scope

- "Up to date" shows its green tick only while the connection is
  write-locked. The tick currently reads as "nothing to do", but an
  unlocked connection can still act, so the states must differ.
- Unlocked shows the action row instead: Up to date / Upgrade all and
  friends, matching what the user can actually do.
- A successful regen closes its panel automatically and reruns the chain
  check in the background without opening the checks UI; only a failure
  keeps the panel open. Mirrors the existing auto-run behaviour of the
  chain check itself.

## Acceptance

- Locked and up to date: tick, no action buttons.
- Unlocked and up to date: action row visible, no tick.
- Regen success: panel closes itself, chain check reruns silently, and
  the regen button returns to its normal state; regen failure keeps the
  panel open with the error.
