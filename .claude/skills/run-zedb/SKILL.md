---
name: run-zedb
description: Build, sign, and (re)launch the zeDB macOS app. Use whenever asked to run, launch, restart, or relaunch zeDB, or to verify a change in the real app. Handles the two traps: bare debug builds cannot read the protected Keychain, and `open` silently no-ops while an old instance is running.
---

# Run zeDB (signed macOS build)

zeDB is a GPUI desktop app. Two things make naive launching fail:

1. **Keychain**: connection passwords live in the user-presence
   protected Keychain, which needs the keychain entitlement. A bare
   `./target/debug/zedb` cannot read them; connecting silently fails
   with no Touch ID prompt. Anything touching the connect flow needs
   the signed bundle.
2. **Stale relaunch**: `open zeDB.app` is a no-op while an instance
   is already running, so after a rebuild the user keeps seeing the
   old binary. Always quit the running instance first.

## Steps

1. Quit any running instance and confirm it exited:

   ```bash
   osascript -e 'tell application "zeDB" to quit' 2>/dev/null
   sleep 2
   osascript -e 'tell application "System Events" to (name of processes) contains "zedb"'
   # must print: false
   ```

2. Build, sign, and launch (from the repo root; takes a couple of
   minutes when the binary changed, dominated by the gpui link):

   ```bash
   ./scripts/run-signed-macos.sh
   ```

   The script builds `zedb-app` (debug), assembles
   `target/macos/zeDB.app`, signs it with the newest Apple
   Development identity plus the entitlements and provisioning
   profile, and `open`s it. Run it in the background; it inherits
   any cargo target-dir lock, so it waits if tests are running.

3. Confirm the app is up:

   ```bash
   osascript -e 'tell application "System Events" to (name of processes) contains "zedb"'
   # must print: true
   ```

## Notes

- Rebuild-only, no launch: `./scripts/run-signed-macos.sh --no-launch`.
- A fresh signed build may one-time re-prompt for Keychain access;
  that prompt is on the user's screen, not in your output.
- Bare `cargo run -p zedb-app` is acceptable only for UI-only
  iteration with no connect flow, and say so when using it.
- If signing fails with "No valid Apple Development signing
  identity", the fix is in Xcode Settings, Accounts, Manage
  Certificates (user action); a missing provisioning profile is set
  via `ZEDB_PROVISIONING_PROFILE`.
