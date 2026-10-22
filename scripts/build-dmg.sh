#!/bin/bash
set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUTPUT_DIRECTORY="$PROJECT_ROOT/build/dist"
BUILD_APPLICATION=true
NOTARIZE=true
# Created once with: xcrun notarytool store-credentials obelisk-notary --apple-id ... --team-id ...
NOTARY_PROFILE="${OBELISK_NOTARY_PROFILE:-obelisk-notary}"
# An App Store Connect API key (the .p8 file, its ID and issuer) replaces the profile, as in CI.
if [[ -n "${OBELISK_NOTARY_KEY:-}" ]]; then
    NOTARY_AUTH=(--key "$OBELISK_NOTARY_KEY" --key-id "${OBELISK_NOTARY_KEY_ID:?}" \
        --issuer "${OBELISK_NOTARY_ISSUER:?}")
else
    NOTARY_AUTH=(--keychain-profile "$NOTARY_PROFILE")
fi

while [[ $# -gt 0 ]]; do
    case "$1" in
        --skip-build) BUILD_APPLICATION=false; shift ;;
        --no-notarize) NOTARIZE=false; shift ;;
        --output-dir)
            if [[ $# -lt 2 || -z "$2" || "$2" == --* ]]; then
                echo "--output-dir requires a directory." >&2
                exit 2
            fi
            OUTPUT_DIRECTORY="$2"
            shift 2
            ;;
        --help|-h)
            echo "Usage: $0 [--skip-build] [--no-notarize] [--output-dir DIRECTORY]"
            echo "Build the Release application and a signed, notarized drag-and-drop DMG for its architecture."
            echo "Notarization uses the notarytool keychain profile \$OBELISK_NOTARY_PROFILE (default obelisk-notary),"
            echo "or the API key \$OBELISK_NOTARY_KEY with \$OBELISK_NOTARY_KEY_ID and \$OBELISK_NOTARY_ISSUER."
            exit 0
            ;;
        *) echo "Unknown argument: $1" >&2; exit 2 ;;
    esac
done

if [[ "$(uname -s)" != Darwin ]]; then
    echo "Create the DMG on macOS with Xcode installed." >&2
    exit 1
fi

if [[ "$BUILD_APPLICATION" == true ]]; then
    CONFIGURATION=Release "$PROJECT_ROOT/scripts/build.sh"
fi

APP="$PROJECT_ROOT/build/xcode/Build/Products/Release/Obelisk.app"
INFO="$APP/Contents/Info.plist"
LAYOUT="$PROJECT_ROOT/packaging/macos/FinderLayout.dsstore"
if [[ ! -f "$INFO" || ! -f "$LAYOUT" ]]; then
    echo "Missing Release app or Finder layout. Run without --skip-build." >&2
    exit 1
fi

VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$INFO")"
EXECUTABLE="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$INFO")"
ARCHITECTURES="$(/usr/bin/lipo -archs "$APP/Contents/MacOS/$EXECUTABLE" | tr ' ' '-')"
case "$VERSION-$ARCHITECTURES" in
    *[!a-zA-Z0-9._+-]*) echo "Invalid version or architecture in DMG filename." >&2; exit 1 ;;
esac
"$PROJECT_ROOT/scripts/verify-app-signature.sh" --distribution "$APP"
IDENTITY="$(/usr/bin/codesign -dvv "$APP" 2>&1 | sed -n 's/^Authority=//p' | head -n 1)"

mkdir -p "$OUTPUT_DIRECTORY" "$PROJECT_ROOT/build/dmg"
OUTPUT_DIRECTORY="$(cd "$OUTPUT_DIRECTORY" && pwd)"
STAGING="$(mktemp -d "$PROJECT_ROOT/build/dmg/staging.XXXXXX")"
# This directory is created by this invocation and contains only generated files.
trap 'rm -rf "$STAGING"' EXIT
mkdir "$STAGING/content"
/usr/bin/ditto "$APP" "$STAGING/content/Obelisk.app"
ln -s /Applications "$STAGING/content/Applications"
cp "$LAYOUT" "$STAGING/content/.DS_Store"

NAME="Obelisk-$VERSION-$ARCHITECTURES.dmg"
/usr/bin/hdiutil create -volname "Installer Obelisk" -fs HFS+ \
    -srcfolder "$STAGING/content" -format UDZO \
    -imagekey zlib-level=9 "$STAGING/$NAME"
/usr/bin/hdiutil verify "$STAGING/$NAME"
/usr/bin/codesign --force --sign "$IDENTITY" --timestamp "$STAGING/$NAME"
if [[ "$NOTARIZE" == true ]]; then
    RESULT="$STAGING/notarization.json"
    /usr/bin/xcrun notarytool submit "$STAGING/$NAME" "${NOTARY_AUTH[@]}" \
        --wait --output-format json > "$RESULT"
    STATUS="$(/usr/bin/plutil -extract status raw -o - "$RESULT")"
    if [[ "$STATUS" != Accepted ]]; then
        SUBMISSION="$(/usr/bin/plutil -extract id raw -o - "$RESULT")"
        echo "Notarization ended with status $STATUS. Apple's log:" >&2
        /usr/bin/xcrun notarytool log "$SUBMISSION" "${NOTARY_AUTH[@]}" >&2 || true
        exit 1
    fi
    /usr/bin/xcrun stapler staple "$STAGING/$NAME"
    /usr/sbin/spctl --assess --type open --context context:primary-signature --verbose "$STAGING/$NAME"
fi
mv -f "$STAGING/$NAME" "$OUTPUT_DIRECTORY/$NAME"
(
    cd "$OUTPUT_DIRECTORY"
    /usr/bin/shasum -a 256 "$NAME" > "$STAGING/$NAME.sha256"
)
mv -f "$STAGING/$NAME.sha256" "$OUTPUT_DIRECTORY/$NAME.sha256"

echo "DMG: $OUTPUT_DIRECTORY/$NAME"
echo "SHA-256: $OUTPUT_DIRECTORY/$NAME.sha256"
if [[ "$NOTARIZE" != true ]]; then
    echo "This DMG is not notarized: Gatekeeper blocks it on other Macs."
fi
