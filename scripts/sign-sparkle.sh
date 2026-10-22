#!/bin/bash
set -euo pipefail
# Sparkle's helpers come ad hoc signed, and Xcode signs only the framework they
# sit in. Notarization wants every one signed with Obelisk's identity and the
# hardened runtime, innermost first, then the framework that seals them.
SPARKLE="$TARGET_BUILD_DIR/$FRAMEWORKS_FOLDER_PATH/Sparkle.framework"
IDENTITY="${EXPANDED_CODE_SIGN_IDENTITY:?A stable Apple signing identity is required}"
# Notarization needs a secure timestamp; Debug skips the round trip to Apple.
if [[ "${CONFIGURATION:-Debug}" == Release ]]; then TIMESTAMP=--timestamp; else TIMESTAMP=--timestamp=none; fi
sign() {
    /usr/bin/codesign --force --sign "$IDENTITY" "$TIMESTAMP" --options runtime "$@"
}
sign "$SPARKLE/Versions/B/XPCServices/Installer.xpc"
sign --preserve-metadata=entitlements "$SPARKLE/Versions/B/XPCServices/Downloader.xpc"
sign "$SPARKLE/Versions/B/Autoupdate"
sign "$SPARKLE/Versions/B/Updater.app"
sign "$SPARKLE"
