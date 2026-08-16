# Phase 10.7: git accounts per cluster instance (DEFERRED)

Status: DEFERRED by explicit decision (2026-08-11): design notes only,
no implementation until called for.

Free switching between git accounts, bound where the user thinks about
them: the cluster instance.

## Direction when picked up

- Multi-account logins: several stored identities per provider.
- At repo setup on a cluster, the user picks which account that cluster
  instance uses; the credential broker resolves per cluster, not
  globally.
- The existing broker (GIT_ASKPASS to own binary, per-host Keychain
  tokens) becomes per-account keyed; `docs/IRL-ISSUES.md` and the git
  broker devlog entries hold the constraints.

## Why deferred

Requirements are expected from the analytics-clickhouse-ddl production
battle-test; designing ahead of them risks the wrong shape.
