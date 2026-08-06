#!/bin/bash
# Build a release zeDB.app bundle at target/macos/zeDB.app.
#
# Signing is controlled by ZEDB_CODESIGN_IDENTITY:
#   unset        -> ad-hoc signature (runs locally, not distributable)
#   "<identity>" -> sign with that keychain identity; set
#                   ZEDB_HARDENED_RUNTIME=1 for a notarization-ready signature
#
# When signing with a real identity, the Developer ID provisioning profile in
# packaging/macos is embedded and the restricted entitlements from
# packaging/macos/zeDB.entitlements are applied, keeping the protected-keychain
# path in zedb-core::secrets working. Ad-hoc builds skip both: those restricted
# keys make an app fail to launch unless a matching profile backs them.
# ZEDB_SIGN_ENTITLEMENTS overrides the entitlements file if set.
#
# The dev loop (Apple Development cert + provisioning profile + launch) stays
# in scripts/run-signed-macos.sh; this script is for shippable artifacts.

set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
app="$root/target/macos/zeDB.app"
contents="$app/Contents"
identity=${ZEDB_CODESIGN_IDENTITY:--}

version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)
build_number=$(cd "$root" && git rev-list --count HEAD)

echo "Building zeDB $version ($build_number) release binary..."
# Keep the compiled deployment target in lockstep with LSMinimumSystemVersion
# in packaging/macos/Info.plist.
export ZEDB_BUILD_COMMIT=$(git -C "$root" rev-parse HEAD)
export ZEDB_BUILD_NUMBER=$build_number
export MACOSX_DEPLOYMENT_TARGET=14.0
cargo build --manifest-path "$root/Cargo.toml" --release -p zedb-app

rm -rf "$app"
mkdir -p "$contents/MacOS" "$contents/Resources"
install -m 755 "$root/target/release/zedb-app" "$contents/MacOS/zedb"
install -m 644 "$root/packaging/macos/Info.plist" "$contents/Info.plist"

/usr/libexec/PlistBuddy \
    -c "Set :CFBundleShortVersionString $version" \
    -c "Set :CFBundleVersion $build_number" \
    "$contents/Info.plist"

icns="$root/packaging/macos/icons/zeDB.icns"
if [[ -f "$icns" ]]; then
    install -m 644 "$icns" "$contents/Resources/zeDB.icns"
    /usr/libexec/PlistBuddy -c "Add :CFBundleIconFile string zeDB" \
        "$contents/Info.plist" 2>/dev/null ||
        /usr/libexec/PlistBuddy -c "Set :CFBundleIconFile zeDB" \
            "$contents/Info.plist"
else
    echo "warning: packaging/macos/icons/zeDB.icns not found; bundling without an icon" >&2
fi

profile="$root/packaging/macos/zeDB_DeveloperID.provisionprofile"
entitlements=${ZEDB_SIGN_ENTITLEMENTS:-}
if [[ $identity != - && -f "$profile" ]]; then
    install -m 644 "$profile" "$contents/embedded.provisionprofile"
    entitlements=${entitlements:-$root/packaging/macos/zeDB.entitlements}
fi

sign_args=(--force --sign "$identity")
if [[ ${ZEDB_HARDENED_RUNTIME:-0} == 1 ]]; then
    sign_args+=(--options runtime --timestamp)
fi
if [[ -n $entitlements ]]; then
    sign_args+=(--entitlements "$entitlements")
fi

echo "Signing with identity: $identity"
codesign "${sign_args[@]}" "$app"
codesign --verify --strict --verbose=2 "$app"

echo "Bundle ready: $app"
