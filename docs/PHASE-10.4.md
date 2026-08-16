# Phase 10.4: explicit native port per node

Status: COMPLETE (2026-08-16). From `docs/IRL-ISSUES.md`, raised during
the 10.1 node-2 verification.

Native (TCP) port discovery is advertised-port plus the HTTP remap
offset, identity-checked. Good heuristics, still heuristics: asymmetric
mappings or a forwarder that only publishes HTTP defeat them.

## Scope

- Connection settings gain an optional native (TCP) port, and where the
  UI supports several nodes, one per cluster node.
- An explicit port is tried first and trusted as configuration; the
  discovery heuristics remain the fallback when unset.
- The serverUUID identity check stays mandatory for every candidate,
  explicit or discovered.

## Acceptance

- A cluster whose native port matches no heuristic gets instant updates
  by setting the port explicitly.
- An explicit port answering as a different server is refused with the
  existing honest error.
