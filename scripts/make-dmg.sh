#!/bin/bash
# Package target/macos/zeDB.app (built by bundle-macos.sh) into a compressed
# drag-to-Applications DMG. Signs the image with ZEDB_CODESIGN_IDENTITY when
# set; notarization/stapling of the DMG happens in the release workflow.
#
# Usage: make-dmg.sh [output.dmg]

set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
app="$root/target/macos/zeDB.app"
out=${1:-zeDB.dmg}

if [[ ! -d "$app" ]]; then
    echo "error: $app not found; run scripts/bundle-macos.sh first" >&2
    exit 1
fi

staging=$(mktemp -d "${TMPDIR:-/tmp}/zedb-dmg.XXXXXX")
trap 'rm -rf "$staging"' EXIT

cp -R "$app" "$staging/"
ln -s /Applications "$staging/Applications"

hdiutil create -volname "zeDB" -srcfolder "$staging" -ov -format UDZO "$out"

if [[ -n ${ZEDB_CODESIGN_IDENTITY:-} && ${ZEDB_CODESIGN_IDENTITY} != - ]]; then
    echo "Signing DMG with identity: $ZEDB_CODESIGN_IDENTITY"
    codesign --force --sign "$ZEDB_CODESIGN_IDENTITY" --timestamp "$out"
    codesign --verify --verbose=2 "$out"
fi

echo "DMG ready: $out"
