# Phase 7.2 ideas: toward a first-class migration manager

Status: IDEAS, not committed scope. What remains of the 2026-08-08
soak-review shopping list after the browser daily-driver gaps were
closed (docs/PHASE-7.1.md, v0.1.15): the migration-manager growth
and the bigger platform bets. Each item stands alone and none is
promised. Pick by appetite.

## Migration manager: growing up

- **Promotion ladder.** Tiers are badges today; make them policy:
  dev applies freely, staging confirms, prod shows the exact diff
  and demands typed confirmation. The safety story teams adopt for.
- **Drift -> migration.** Detection exists; the killer move is
  "adopt this drift": generate the migration that legitimizes or
  reverts a hand-edited server. Nobody closes this loop well.
- **CI recipe.** zedb-cli already runs checks headlessly; document a
  GitHub Action gating PRs on chain/regen checks. The honest small
  kernel of the parked Phase 4.
- **Per-shard apply awareness.** Phase 5 meets the fleet: the apply
  matrix understanding ON CLUSTER propagation per shard.

## Platform bets (bigger, later)

- **Linux/Windows.** gpui allows it; the macOS Keychain layer is the
  abstraction to crack (secrets backend per platform).
- **ClickHouse Cloud ergonomics.** Endpoint conventions, wake-up
  handling, no-visible-shards polish.
- **Grid memory.** Columnar/interned value storage to cut the ~2.4x
  decoded-vs-wire blow-up seen on the 193M-row pull.
- **Light theme tuning.** Shipped as a first draft; soak the shades.

## A suggested first bite

Drift->migration, for having something nobody else has.
