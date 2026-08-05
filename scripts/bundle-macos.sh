#!/bin/bash
# Build a release zeDB.app bundle at target/macos/zeDB.app.
#
# Signing is controlled by ZEDB_CODESIGN_IDENTITY:
#   unset        -> ad-hoc signature (runs locally, not distributable)
#   "<identity>" -> sign with that keychain identity; set
#                   ZEDB_HARDENED_RUNTIME=1 for a notarization-ready signature
#
# ZEDB_SIGN_ENTITLEMENTS may point at an entitlements plist to embed. Leave it
# unset for Developer ID builds unless a matching Developer ID provisioning
# profile is also embedded: packaging/macos/zeDB.entitlements holds restricted
# keys that make an app without a profile fail to launch.
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

sign_args=(--force --sign "$identity")
if [[ ${ZEDB_HARDENED_RUNTIME:-0} == 1 ]]; then
    sign_args+=(--options runtime --timestamp)
fi
if [[ -n ${ZEDB_SIGN_ENTITLEMENTS:-} ]]; then
    sign_args+=(--entitlements "$ZEDB_SIGN_ENTITLEMENTS")
fi

echo "Signing with identity: $identity"
codesign "${sign_args[@]}" "$app"
codesign --verify --strict --verbose=2 "$app"

echo "Bundle ready: $app"
