# Shipping TODO

What is left before a `zeDB.app` from CI runs cleanly on someone else's Mac.
The release pipeline itself already exists: pushing a `v*` tag runs
`.github/workflows/release.yaml`, which builds `scripts/bundle-macos.sh` and
attaches `zeDB-<version>-macos.zip` to a GitHub Release. Signing and
notarization secrets are configured, so releases are Developer ID signed,
notarized, and stapled; the icon (section 1) is the main thing still missing.

## 1. App icon

- [ ] Produce the ten PNGs listed in `packaging/macos/icons/REQUIRED.md`
      and drop them in `packaging/macos/icons/zeDB.iconset/`.
- [ ] Generate the icns and commit it:
      `iconutil -c icns packaging/macos/icons/zeDB.iconset -o packaging/macos/icons/zeDB.icns`
      (the bundle script picks it up automatically and sets `CFBundleIconFile`).
- [ ] Delete `REQUIRED.md`.

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

`packaging/macos/zeDB.entitlements` carries restricted entitlements
(`com.apple.application-identifier`, `keychain-access-groups`) that require an
embedded provisioning profile. Release builds currently sign **without** them,
so stored passwords use the legacy keychain path (`secrets.rs` falls back on
`ERR_SEC_MISSING_ENTITLEMENT`). Either:

- [ ] Accept that for now (nothing to do), or
- [ ] Create a **Developer ID provisioning profile** for `dev.zedb.app` with
      the keychain access group, commit it to `packaging/macos/`, and update
      `bundle-macos.sh` to embed it and pass
      `ZEDB_SIGN_ENTITLEMENTS=packaging/macos/zeDB.entitlements`.

## 5. First release

- [ ] `git tag v0.1.0 && git push origin v0.1.0`
- [ ] Check the Release page artifact; verify on a second Mac that the app
      opens without Gatekeeper complaints:
      `spctl -a -vv /Applications/zeDB.app` should say `accepted` and
      `source=Notarized Developer ID`.

## Later / nice to have

- [ ] Ship a DMG instead of (or alongside) the zip, and sign + notarize the
      DMG itself.
- [ ] Sparkle or a hand-rolled update check for in-app updates.
- [ ] Bump `LSMinimumSystemVersion` intentionally (currently 13.0).
