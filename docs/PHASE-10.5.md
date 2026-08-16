# Phase 10.5: ClickHouse Cloud that feels native

Status: PLANNED. Groups the Cloud items from `docs/IRL-ISSUES.md`. Read
`docs/PRODUCT-PRINCIPLES.md` before changing the setup flow's character.

Cloud setup works but feels bolted on beside the normal cluster flow.

## Scope

- Investigate whether ClickHouse Cloud offers an OAuth (or
  device-code) login the app can drive, replacing hand-copied API keys.
  This is a spike with a written answer first; integration only if the
  answer is yes.
- Whatever the answer, fold Cloud setup into the primary connection flow
  so it stops feeling like a separate bolted-on path.
- When connected to a Cloud instance, a 1px border in ClickHouse yellow
  around the editor says "you are on Cloud" at a glance.

## Acceptance

- The spike's findings are recorded (devlog) with a ship or stop call on
  OAuth.
- Cloud connections are created from the same entry point as other
  connections.
- The yellow border appears exactly when the active connection is a
  Cloud service and never otherwise.
