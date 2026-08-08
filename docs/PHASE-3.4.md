# Phase 3.4: identity and settings sync

Optional GitHub sign-in for a user profile, and settings sync over
plain git. Identity is OAuth; the sync transport is a git repo, so the
BYO philosophy survives and no forge is locked out.

## Shape

- **Sign-in is optional and gates almost nothing.** Signed out, zeDB
  behaves exactly as today. Signed in, you get your avatar and name in
  the app and a one-click path to a sync repo. That's all identity is
  for; there is no account, no backend, no hosted anything.
- **Device flow, minimal scope.** GitHub's OAuth device flow: the app
  shows a short code, opens github.com/login/device, polls for the
  token. Initial scope is `read:user` only. The token lives in the
  macOS Keychain next to the connection passwords.
- **The sync offer arrives at the right moment.** On the first visit
  to settings while signed in (or immediately after signing in), a
  card offers settings sync with two paths:
  - One-click: create a private `zedb-settings` repo on the user's
    GitHub and wire it up. This needs the `repo` scope, requested
    incrementally at that moment with an explanation, never at
    sign-in.
  - Bring your own: paste any git URL (any host, no OAuth involved),
    reusing the migration-repo clone plumbing.
- **What syncs**: preferences, connections (never passwords; the
  Keychain does not sync), custom agents, always-allow lists. Pull on
  launch and window refocus, commit-and-push on change, last-writer-
  wins by timestamp. The repo is readable: the sync history is a git
  log of your own settings.
- **Provider abstraction.** A small trait (device-flow start/poll,
  profile fetch, repo create) with GitHub first. GitLab's device flow
  (17.x) is a near-clone later; Gitea/Forgejo via PKCE where needed.
  Bitbucket has no device flow; its users take the paste-a-URL path
  and lose nothing.

## Milestones

- **M0: device flow + profile.** `zedb-core` (or a small `zedb-auth`
  module) implements GitHub device flow against a registered OAuth
  app; token in Keychain; fetch login/name/avatar; settings view
  shows the signed-in identity with a sign-out button. Signed-out
  state unchanged everywhere.
- **M1: settings repo, BYO path.** A settings-sync section in
  preferences: paste a git URL, zeDB clones it (existing plumbing),
  writes the sync payload, commits and pushes with the user's git.
  Pull on launch/refocus; last-writer-wins merge; a quiet status line
  (synced N minutes ago / conflict resolved / push failed).
- **M2: one-click bootstrap.** Signed-in users get "create a private
  zedb-settings repo for me": incremental `repo` scope request,
  create via API, clone over https with the OAuth token as the git
  credential. The first-settings-visit offer card lands here.
- **M3: hygiene and exits.** Sign-out revokes locally and keeps sync
  working if the repo remains reachable by other means; disabling
  sync leaves the repo intact; a "sync now" button; redaction check
  in CI asserting the payload never contains password fields.

## Risks

- Scope optics: a database tool asking for `repo` reads badly if
  asked too early; the incremental request at the moment of repo
  creation, with copy explaining exactly why, is the mitigation.
- OAuth app registration is a standing artifact (client id baked into
  the app); device flow means no secret ships in the binary.
- Sync conflicts are inherently low-stakes (settings, not data);
  last-writer-wins with the losing version preserved in git history
  is deliberately simple.
- Connections sync includes endpoints, which some users treat as
  sensitive; sync is opt-in and the payload's contents are documented
  in the offer card.

## Done when

A fresh laptop signs in, accepts the sync offer, and inherits the
other machine's preferences, connections (sans passwords), and custom
agents within one launch; signing out and disabling sync returns both
machines to fully local behavior.
