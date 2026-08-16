# Phase 10.5: ClickHouse Cloud that feels native

Status: SPIKE COMPLETE (2026-08-16), design below awaiting direction.
Groups the Cloud items from `docs/IRL-ISSUES.md`. Read
`docs/PRODUCT-PRINCIPLES.md` before changing the setup flow's character.

## Spike findings: the OAuth answer is yes

ClickHouse Cloud now exposes a real OAuth device flow, shipped for
`clickhousectl` (Apache-2.0, so the mechanics are public):

- Device flow against `https://auth.clickhouse.cloud/oauth/device/code`
  with a public embedded client id, scope
  `openid profile email offline_access` (so refresh tokens), audience
  `clickhousectl`. Staging and dev control planes have their own hosts
  and client ids.
- OAuth tokens are read-only for the management API: list
  organizations, services, backups; no create/scale/delete/wake.
- The per-service Query API accepts the user's OAuth Bearer token
  directly: read-only SQL as the user's own identity, no per-service
  credentials at all, with idle-wake confirmation built into the
  endpoint.
- Management writes still need an org API key (HTTP Basic), and the
  management API has
  `PATCH /v1/organizations/{org}/services/{id}/password`: with an API
  key, zeDB can provision the database password itself instead of
  sending the user to the console.

Caveats: an OAuth token cannot wake or mutate; the embedded client id
identifies as clickhousectl (asking ClickHouse for a zeDB client id is
the clean long-term move, and costs nothing to defer); and the Query
API is a narrower surface than the service's own HTTP interface, so a
Bearer-only connection cannot carry native TCP (no instant tails),
writes, or session settings.

## Design: one connection flow, Cloud as a first-class citizen

Today Cloud linking is a separate bolted-on path beside the manual
form. The redesign folds it into the single Add Connection entry:

1. Add Connection offers two doors: "ClickHouse Cloud" (primary) and
   "Self-hosted / direct" (the existing form).
2. The Cloud door signs in via the device flow in-app: show the code,
   open the browser, poll; refresh token in the Keychain. API-key
   entry remains as the fallback door for CI-style orgs and as the
   write-capable credential.
3. Signed in, zeDB lists organizations and services with live state
   badges (running/idle), and the user picks services to link.
4. Per linked service, two access levels:
   - Full access: the database password, either pasted (as today,
     but with everything else prefilled) or provisioned by zeDB via
     the password-reset endpoint when an API key is present, with an
     explicit warning that it rotates the existing password.
   - Read-only, instant (later increment): no password at all; the
     connection runs SQL through the Query API with the OAuth Bearer
     token. Every feature that needs the native surface (tails,
     writes, driver settings) degrades with an honest label.

   A passwordless full-access path (JWT database sessions) was tested
   live and is blocked upstream: the data plane validates JWTs and
   maps them to users, but `CREATE USER ... IDENTIFIED WITH jwt`
   returns "CREATE USER is not supported for JWT" (BAD_ARGUMENTS,
   26.2.1.558); Cloud reserves JWT user mapping for its own console.
   Filed under "ask ClickHouse"; revisit when customer-mapped JWT
   users exist.
5. Any connection linked to a Cloud service wears a 1px border in
   ClickHouse yellow around the editor.

## Increments

- 10.5a: the front door, specified in `docs/PHASE-10.5A.md`.
- 10.5b: Cloud-truthful internals, specified in `docs/PHASE-10.5B.md`
  (the self-audit debts from `docs/CLOUD-STRATEGY.md`).
- 10.5c+: control-plane surfaces and Bearer read-only connections over
  the Query API, per `docs/CLOUD-STRATEGY.md`.

## Acceptance (10.5a)

- A fresh user with only a browser login reaches a connected service
  without ever copying an endpoint URL.
- The password step is either one paste with everything prefilled, or
  zero pastes with an API key (behind an explicit rotation warning).
- The yellow border appears exactly when the active connection is a
  Cloud service and never otherwise.
