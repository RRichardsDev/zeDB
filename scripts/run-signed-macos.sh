#!/bin/zsh

set -euo pipefail

zedb_root=${0:A:h:h}
zedb_app="$zedb_root/target/macos/zeDB.app"
zedb_contents="$zedb_app/Contents"
zedb_identity=${ZEDB_CODESIGN_IDENTITY:-}
zedb_profile=${ZEDB_PROVISIONING_PROFILE:-/Users/rhodririchards/Downloads/zeDB_Mac_Development.provisionprofile}
zedb_entitlements="$zedb_root/packaging/macos/zeDB.entitlements"

select_newest_identity() {
    local identity_list cert_dir cert_file cert_sha cert_expiry cert_epoch
    local newest_identity=""
    local newest_epoch=0

    identity_list=$(security find-identity -v -p codesigning)
    cert_dir=$(mktemp -d "${TMPDIR:-/tmp}/zedb-certificates.XXXXXX")
    trap "rm -rf '$cert_dir'" EXIT

    security find-certificate -a -c "Apple Development" -p |
        awk -v directory="$cert_dir" '
            /-----BEGIN CERTIFICATE-----/ {
                count += 1
                output = sprintf("%s/certificate-%d.pem", directory, count)
            }
            output != "" { print > output }
        '

    for cert_file in "$cert_dir"/*.pem; do
        [[ -f "$cert_file" ]] || continue
        cert_sha=$(openssl x509 -in "$cert_file" -noout -fingerprint -sha1 |
            sed 's/^.*=//; s/://g')
        [[ "$identity_list" == *"$cert_sha"* ]] || continue

        cert_expiry=$(openssl x509 -in "$cert_file" -noout -enddate |
            sed 's/^notAfter=//')
        cert_epoch=$(date -j -f "%b %e %T %Y %Z" "$cert_expiry" +%s)
        if (( cert_epoch > newest_epoch )); then
            newest_epoch=$cert_epoch
            newest_identity=$cert_sha
        fi
    done

    if [[ -z "$newest_identity" ]]; then
        print -u2 "No valid Apple Development signing identity was found."
        print -u2 "Create one in Xcode Settings, Accounts, Manage Certificates."
        return 1
    fi

    print -r -- "$newest_identity"
}

if [[ -z "$zedb_identity" ]]; then
    zedb_identity=$(select_newest_identity)
fi

if [[ ! -f "$zedb_profile" ]]; then
    print -u2 "Provisioning profile not found: $zedb_profile"
    print -u2 "Set ZEDB_PROVISIONING_PROFILE to the downloaded Mac App Development profile."
    exit 1
fi

print "Building zeDB..."
export ZEDB_BUILD_COMMIT=$(git -C "$zedb_root" rev-parse HEAD)
export ZEDB_BUILD_NUMBER=$(git -C "$zedb_root" rev-list --count HEAD)
cargo build --manifest-path "$zedb_root/Cargo.toml" -p zedb-app

mkdir -p "$zedb_contents/MacOS" "$zedb_contents/Resources"
install -m 755 "$zedb_root/target/debug/zedb" "$zedb_contents/MacOS/zedb"
install -m 644 "$zedb_root/packaging/macos/Info.plist" "$zedb_contents/Info.plist"
install -m 644 "$zedb_profile" "$zedb_contents/embedded.provisionprofile"

print "Signing with identity $zedb_identity..."
codesign \
    --force \
    --sign "$zedb_identity" \
    --timestamp=none \
    --entitlements "$zedb_entitlements" \
    "$zedb_app"

codesign --verify --deep --strict --verbose=2 "$zedb_app"
print "Signed app: $zedb_app"

if [[ ${1:-} != "--no-launch" ]]; then
    open "$zedb_app"
fi
