#!/bin/bash
set -euo pipefail

DISTRIBUTION=false
if [[ "${1:-}" == --distribution ]]; then
    DISTRIBUTION=true
    shift
fi
if [[ $# -ne 1 ]]; then
    echo "Usage: $0 [--distribution] APP" >&2
    exit 2
fi

# An ad hoc signature can be valid, but its identity changes with the binary.
# Require an Apple-issued certificate as well as a valid bundle signature.
if ! /usr/bin/codesign --verify --deep --strict -R='anchor apple generic' "$1"; then
    echo "Obelisk needs a valid Apple certificate signature to preserve Keychain access across builds." >&2
    echo "Configure apps/macos/Config/Signing.local.xcconfig using the example beside it." >&2
    exit 1
fi

require_architectures() {
    local binary="$1"
    local expected="$2"
    local actual
    actual="$(/usr/bin/lipo -archs "$binary" | tr ' ' '\n' | LC_ALL=C sort | paste -sd ' ' -)"
    if [[ "$actual" != "$expected" ]]; then
        echo "Unexpected architectures for $binary: $actual; expected $expected." >&2
        exit 1
    fi
    /usr/bin/codesign --verify --strict -R='anchor apple generic' "$binary"
}

STEAM_HELPERS="$1/Contents/Helpers/SteamNative"
require_architectures "$STEAM_HELPERS/obelisk-steam-host" "arm64"
if [[ -e "$STEAM_HELPERS/obelisk-steam-sdk-arm64" ]]; then
    echo "Obelisk contains the removed ARM64 SDK diagnostic helper." >&2
    exit 1
fi
require_architectures "$STEAM_HELPERS/obelisk-steam-sdk-x86_64" "x86_64"
require_architectures "$STEAM_HELPERS/libobelisk_steam_discovery_adapter.dylib" "arm64 x86_64"
require_architectures "$1/Contents/Helpers/Obelisk Rosetta.app/Contents/MacOS/ObeliskRosetta" "x86_64"
# Valve's Steam components are downloaded with a Steam game, never shipped.
if [[ -e "$1/Contents/Resources/SteamRuntime" ]]; then
    echo "Obelisk contains Valve's Steam components, which it must download instead." >&2
    exit 1
fi

if [[ "$DISTRIBUTION" != true ]]; then
    exit 0
fi

# What Gatekeeper and notarization demand of every binary a stranger downloads:
# a Developer ID of one team, a secure timestamp, no debugger entitlement, and
# the hardened runtime on executables.
TEAM="$(/usr/bin/codesign -dv "$1" 2>&1 | sed -n 's/^TeamIdentifier=//p')"
if [[ -z "$TEAM" || "$TEAM" == "not set" ]]; then
    echo "Obelisk has no signing team." >&2
    exit 1
fi
DEVELOPER_ID="anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6] exists \
and certificate leaf[field.1.2.840.113635.100.6.1.13] exists and certificate leaf[subject.OU] = \"$TEAM\""
while IFS= read -r -d '' file; do
    kind="$(/usr/bin/file -b "$file")"
    [[ "$kind" == *Mach-O* ]] || continue
    if ! /usr/bin/codesign --verify --strict -R="$DEVELOPER_ID" "$file"; then
        echo "$file is not signed with team $TEAM's Developer ID." >&2
        exit 1
    fi
    details="$(/usr/bin/codesign -dvv "$file" 2>&1)"
    if ! grep -q '^Timestamp=' <<<"$details"; then
        echo "$file has no secure timestamp." >&2
        exit 1
    fi
    if [[ "$kind" == *executable* ]] && ! grep -q '^CodeDirectory.*flags=.*runtime' <<<"$details"; then
        echo "$file is not signed with the hardened runtime." >&2
        exit 1
    fi
    if /usr/bin/codesign -d --entitlements - --xml "$file" 2>/dev/null | grep -q get-task-allow; then
        echo "$file allows debugging (get-task-allow)." >&2
        exit 1
    fi
done < <(find "$1" -type f -print0)
