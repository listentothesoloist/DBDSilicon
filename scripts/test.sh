#!/bin/bash
set -euo pipefail
PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
export PATH="$HOME/.cargo/bin:$PATH"
python3 "$PROJECT_ROOT/scripts/verify-localization.py"
cargo fmt --manifest-path "$PROJECT_ROOT/Cargo.toml" --all -- --check
cargo clippy --locked --manifest-path "$PROJECT_ROOT/Cargo.toml" --workspace --all-targets --all-features -- -D warnings
cargo test --locked --manifest-path "$PROJECT_ROOT/Cargo.toml" --workspace
cargo fmt --manifest-path "$PROJECT_ROOT/crates/steam-native/Cargo.toml" --all -- --check
cargo clippy --locked --manifest-path "$PROJECT_ROOT/crates/steam-native/Cargo.toml" --workspace --all-targets -- -D warnings
cargo test --locked --manifest-path "$PROJECT_ROOT/crates/steam-native/Cargo.toml" --workspace
xcodebuild -project "$PROJECT_ROOT/apps/macos/Obelisk.xcodeproj" -scheme Obelisk \
    -configuration Debug -derivedDataPath "$PROJECT_ROOT/build/xcode" \
    -destination 'platform=macOS' test
"$PROJECT_ROOT/scripts/verify-app-signature.sh" "$PROJECT_ROOT/build/xcode/Build/Products/Debug/Obelisk.app"
# Valve's Steam components exist once a Steam game's download or launch got
# them from Valve's CDN; this check reads the copy Obelisk keeps and changes
# nothing.
VERSION="$(/usr/bin/plutil -extract version raw -o - "$PROJECT_ROOT/crates/game-runtime/runtime-manifest.json")"
PAYLOAD="$HOME/Library/Caches/com.zagrios.obelisk/SteamRuntime/valve-$VERSION.zip"
if [[ -f "$PAYLOAD" ]]; then
    OBELISK_STEAM_BUNDLE="$PROJECT_ROOT/build/xcode/Build/Products/Debug/Obelisk.app" OBELISK_STEAM_PAYLOAD="$PAYLOAD" \
        cargo test --locked --manifest-path "$PROJECT_ROOT/Cargo.toml" -p game-runtime \
        downloaded_runtime_loads_without_steam_installation_access -- --ignored
else
    echo "Skipped the Steam components load check: launch a Steam game once so Obelisk downloads them."
fi
