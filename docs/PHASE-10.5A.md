# Phase 10.5a: the Cloud front door

Status: BUILT (2026-08-16), awaiting live verification against a real
Cloud org (sign-in, discovery, provisioning). The OAuth module lives
in `zedb-app/src/platform/cloud_oauth.rs` beside the forge sign-in,
not zedb-core as first specced: zedb-core carries no HTTP or async
runtime, and `platform/` is where the GitHub/GitLab device flow
already lives. Child of `docs/PHASE-10.5.md`; strategy in
`docs/CLOUD-STRATEGY.md`. Read `docs/PRODUCT-PRINCIPLES.md` before
changing the setup flow's character.

Goal: a fresh user with only a browser login reaches a connected Cloud
service without copying an endpoint, and the whole path lives inside
the one Add Connection flow instead of beside it.

## Increment 1: OAuth device flow in zedb-core

- A `cloud_oauth` module owning the device flow proven in the spike:
  `POST {auth_host}/oauth/device/code` with the public client id,
  scope `openid profile email offline_access`, audience
  `clickhousectl`; poll `POST {auth_host}/oauth/token` on the returned
  interval; refresh via `grant_type=refresh_token`.
- Host/client-id table for production, staging, and dev control planes
  (mirroring clickhousectl's `KNOWN_CONFIGS`), production default.
- Refresh token in the Keychain (`zedb-clickhouse-cloud-oauth`);
  access token held in memory only, refreshed on expiry (`exp` claim
  minus a minute). Tokens never touch disk or settings sync.
- The embedded client id identifies as clickhousectl; an own client id
  is an open ask to ClickHouse and swaps in trivially later.

## Increment 2: sign-in UX

- The Cloud panel (ClickHouse-mark button) leads with "Sign in with
  ClickHouse Cloud": shows the user code prominently, opens the
  verification URL, polls with a cancel button, lands signed in with
  the account email shown.
- API-key entry remains, relabelled as the power credential: needed to
  wake services, provision passwords, and manage the org. Both can
  coexist; the panel says which is present and what each enables.
- Sign-out clears the Keychain refresh token and memory state.

## Increment 3: discovery over OAuth

- `platform/clickhouse_cloud.rs` gains Bearer variants of the org and
  service listers; the panel uses whichever credential exists,
  preferring the API key when both are present (it can also wake).
- The Start button on an idle service renders disabled under
  OAuth-only, with a tooltip naming the reason: waking needs an API
  key. No silent failure.
- Service metadata fetch widens to tier/size/replica fields for later
  surfaces; unknown fields stay tolerated as today.

## Increment 4: prefilled setup, completed

- Service prefill keeps the `nativesecure` port it already parses and
  writes it into the node's explicit native port (closes debt 4 of
  `docs/CLOUD-STRATEGY.md`; the field shipped in 10.4).
- With an API key present, "Provision password" calls
  `PATCH .../services/{id}/password` behind an explicit dialog that
  says it rotates the existing password, stores the result in the
  Keychain, and never displays it. Password paste stays as the
  fallback and the only path under OAuth-only.
- The read-only default gets one sentence of explanation in the form
  ("Cloud connections start read-only; unlock in the fleet view"),
  because it silently gates KILL QUERY and Tier-3 measurements today.

## Increment 5: the yellow border

- When the active connection is linked to a Cloud service, the editor
  area wears a 1px border in ClickHouse yellow (`#FFCC01`, the same
  value as the mark asset). Exactly when linked, never otherwise;
  linkage is the stored service id, not a URL heuristic.

## Acceptance

- Browser-only user: sign in, see services with live state, add a
  connection, paste one password, run a query. No endpoint copying.
- API-key user: same flow with zero pastes (provisioning dialog), and
  Start works on idle services.
- OAuth-only user sees an honest disabled Start, not a failure.
- The border appears on the Cloud connection and disappears when
  switching to a self-hosted one.
- No secret material in settings sync; sign-out leaves no token.

## Test notes

- Device-flow module: unit tests against a mocked auth host (code
  issue, pending, slow_down, success, refresh, expiry math).
- Prefill: unit test that a service with a nativesecure endpoint
  yields a node with the explicit port set.
- Border: state test on the linkage predicate.
