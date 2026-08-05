# Shipping TODO

What is left before a `zeDB.app` from CI runs cleanly on someone else's Mac.
The release pipeline itself already exists: pushing a `v*` tag runs
`.github/workflows/release.yaml`, which builds `scripts/bundle-macos.sh` and
attaches `zeDB-<version>-macos.zip` to a GitHub Release. Signing and
notarization secrets are configured, so releases are Developer ID signed,
notarized, and stapled; the icon (section 1) is the main thing still missing.

## 1. App icon

- [x] Ten PNGs in `packaging/macos/icons/zeDB.iconset/`, `zeDB.icns`
      generated and committed; the bundle script wires it in via
      `CFBundleIconFile` automatically. Regenerate with
      `iconutil -c icns packaging/macos/icons/zeDB.iconset -o packaging/macos/icons/zeDB.icns`
      whenever the artwork changes.

## 2. Developer ID certificate

- [x] Create a **Developer ID Application** certificate for team `M8Y82YQ4GF`:
      Xcode > Settings > Accounts > Manage Certificates > + . Free with the
      existing paid membership.
- [x] Export it (with private key) from Keychain Access as a
      password-protected `.p12`.
- [x] Add repo secrets:
      - `MACOS_CERTIFICATE_P12` = `base64 -i cert.p12 | pbcopy`
      - `MACOS_CERTIFICATE_PASSWORD` = the export password

## 3. Notarization credentials

- [x] Create an app-specific password at appleid.apple.com (Sign-In and
      Security > App-Specific Passwords).
- [x] Add repo secrets:
      - `NOTARY_APPLE_ID` = your Apple ID email
      - `NOTARY_PASSWORD` = the app-specific password
      - `NOTARY_TEAM_ID` = `M8Y82YQ4GF`

## 4. Entitlements decision (protected keychain)

Done. `packaging/macos/zeDB_DeveloperID.provisionprofile` (Developer ID
profile for `dev.zedb.app`, keychain access group `M8Y82YQ4GF.*`, expires
2044) is committed, and `bundle-macos.sh` embeds it and applies
`zeDB.entitlements` automatically whenever signing with a real identity.
Identity-signed builds therefore keep the protected-keychain path in
`zedb-core::secrets`; ad-hoc dev bundles still skip the restricted
entitlements so they can launch unprofiled.

- [x] Create a **Developer ID provisioning profile** for `dev.zedb.app` with
      the keychain access group, commit it to `packaging/macos/`, and embed
      it from `bundle-macos.sh`. Verified locally: signed bundle launches
      with the entitlements applied.

## 5. First release

- [ ] `git tag v0.1.0 && git push origin v0.1.0`
- [ ] Check the Release page artifact; verify on a second Mac that the app
      opens without Gatekeeper complaints:
      `spctl -a -vv /Applications/zeDB.app` should say `accepted` and
      `source=Notarized Developer ID`.

## Later / nice to have

- [x] Ship a DMG alongside the zip, signed and notarized
      (`scripts/make-dmg.sh` + the release workflow).
- [x] DMG window polish: `make-dmg.sh` uses `create-dmg` (falls back to a
      plain DMG when absent) with the designed background. Dark variant is
      wired in; both variants and their SVG sources live in
      `packaging/macos/` (a DMG carries exactly one background, no
      light/dark switching). To swap variants, rebuild the tiff:
      `tiffutil -cathidpicheck dmg-background-light.png dmg-background-light@2x.png -out dmg-background.tiff`.
- [x] Hand-rolled auto-update (`zedb-app/src/updates.rs`): the app checks the
      GitHub Releases feed at startup; the title-bar pill downloads the
      release zip, verifies the new bundle is signed by our team, swaps it
      in place, and relaunches on click. Bare `cargo run` builds fall back
      to opening the release page. Needs the repo public (or
      `ZEDB_GITHUB_TOKEN`) to see releases and download assets.
- [x] `LSMinimumSystemVersion` set intentionally to 14.0: releases are
      arm64-only, macOS 14 is the oldest Apple-supported release, and
      nothing older is tested. `MACOSX_DEPLOYMENT_TARGET` in
      `bundle-macos.sh` keeps the compiled binary in lockstep; revisit
      when Apple drops macOS 14 support.
