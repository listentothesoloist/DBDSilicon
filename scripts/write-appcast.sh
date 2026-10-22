#!/bin/bash
set -euo pipefail
# Writes the Sparkle feed of one release: its DMG, signed with the EdDSA key
# whose public half is SUPublicEDKey in apps/macos/Config/Info.plist, and its
# notes. The key comes from $SPARKLE_ED_PRIVATE_KEY, as in CI, or else from
# the login Keychain, where generate_keys --account zagrios keeps it.
if [[ $# -ne 4 ]]; then
    echo "Usage: $0 DMG DMG_URL NOTES_HTML OUTPUT" >&2
    exit 2
fi
DMG="$1" URL="$2" NOTES="$3" OUTPUT="$4"
PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# The tools of the Sparkle that apps/macos/project.yml pins.
SPARKLE_VERSION=2.10.0
SPARKLE_SHA256=c2bf58aa8387266ac179357b1415d6f2635f044da8be41042af32425dae6da0c
TOOLS="$PROJECT_ROOT/build/sparkle-$SPARKLE_VERSION"
if [[ ! -x "$TOOLS/bin/sign_update" ]]; then
    mkdir -p "$TOOLS"
    curl -fsSL --retry 3 -o "$TOOLS.tar.xz.partial" \
        "https://github.com/sparkle-project/Sparkle/releases/download/$SPARKLE_VERSION/Sparkle-$SPARKLE_VERSION.tar.xz"
    if [[ "$(shasum -a 256 "$TOOLS.tar.xz.partial" | cut -d' ' -f1)" != "$SPARKLE_SHA256" ]]; then
        echo "Sparkle $SPARKLE_VERSION checksum mismatch." >&2
        exit 1
    fi
    tar -xJf "$TOOLS.tar.xz.partial" -C "$TOOLS" ./bin/sign_update
    rm "$TOOLS.tar.xz.partial"
fi

# The versions the app inside the DMG declares, which Sparkle compares.
MOUNT="$(mktemp -d)"
/usr/bin/hdiutil attach -nobrowse -readonly -noautoopen -mountpoint "$MOUNT" "$DMG" > /dev/null
INFO="$MOUNT/Obelisk.app/Contents/Info.plist"
VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' "$INFO")"
SHORT_VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$INFO")"
MINIMUM_SYSTEM="$(/usr/libexec/PlistBuddy -c 'Print :LSMinimumSystemVersion' "$INFO")"
/usr/bin/hdiutil detach -quiet "$MOUNT"
rmdir "$MOUNT"

if [[ -n "${SPARKLE_ED_PRIVATE_KEY:-}" ]]; then
    SIGNATURE="$(printf '%s' "$SPARKLE_ED_PRIVATE_KEY" | "$TOOLS/bin/sign_update" --ed-key-file - -p "$DMG")"
else
    SIGNATURE="$("$TOOLS/bin/sign_update" --account zagrios -p "$DMG")"
fi
LENGTH="$(stat -f %z "$DMG")"

{
    cat <<EOF
<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle">
  <channel>
    <title>Obelisk</title>
    <item>
      <title>Obelisk $SHORT_VERSION</title>
      <pubDate>$(LC_ALL=C date -u '+%a, %d %b %Y %H:%M:%S +0000')</pubDate>
      <sparkle:version>$VERSION</sparkle:version>
      <sparkle:shortVersionString>$SHORT_VERSION</sparkle:shortVersionString>
      <sparkle:minimumSystemVersion>$MINIMUM_SYSTEM</sparkle:minimumSystemVersion>
EOF
    if [[ -s "$NOTES" ]]; then
        # Sparkle shows the notes as HTML; a CDATA section cannot hold its own end.
        printf '      <description><![CDATA[%s]]></description>\n' "$(sed 's/]]>/]]]]><![CDATA[>/g' "$NOTES")"
    fi
    cat <<EOF
      <enclosure url="$URL" length="$LENGTH" type="application/octet-stream" sparkle:edSignature="$SIGNATURE"/>
    </item>
  </channel>
</rss>
EOF
} > "$OUTPUT"
