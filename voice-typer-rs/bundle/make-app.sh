#!/usr/bin/env bash
# Build a VoiceTyper.app bundle from the release binary.
#
# Run this on macOS. It expects `cargo build --release` (or universal-build
# below) to have produced target/{aarch64,x86_64,...}/release/VoiceTyper.
#
# Usage:
#   bundle/make-app.sh                  # uses target/release (host arch)
#   bundle/make-app.sh universal        # builds a universal2 (arm64 + x86_64) binary
#
# Output:   target/macos/VoiceTyper.app
# Codesign: adhoc by default. Set CODESIGN_IDENTITY="Developer ID Application: …"
#           for a real signature.

set -euo pipefail

cd "$(dirname "$0")/.."
MODE="${1:-host}"

if [[ "$MODE" == "universal" ]]; then
    rustup target add aarch64-apple-darwin x86_64-apple-darwin >/dev/null
    cargo build --release --target aarch64-apple-darwin
    cargo build --release --target x86_64-apple-darwin
    mkdir -p target/macos/universal
    lipo -create -output target/macos/universal/VoiceTyper \
        target/aarch64-apple-darwin/release/VoiceTyper \
        target/x86_64-apple-darwin/release/VoiceTyper
    SRC_BIN="target/macos/universal/VoiceTyper"
else
    cargo build --release
    SRC_BIN="target/release/VoiceTyper"
fi

APP="target/macos/VoiceTyper.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

cp "$SRC_BIN" "$APP/Contents/MacOS/VoiceTyper"
cp bundle/Info.plist "$APP/Contents/Info.plist"

# Convert logo.png → icon.icns if iconutil is available and we don't have one yet.
if [[ ! -f bundle/icon.icns ]]; then
    if command -v sips >/dev/null && command -v iconutil >/dev/null; then
        ICONSET="$(mktemp -d)/icon.iconset"
        mkdir -p "$ICONSET"
        for sz in 16 32 64 128 256 512 1024; do
            sips -z "$sz" "$sz" assets/logo.png --out "$ICONSET/icon_${sz}x${sz}.png" >/dev/null
            half=$((sz / 2))
            if [[ $half -ge 16 ]]; then
                cp "$ICONSET/icon_${sz}x${sz}.png" "$ICONSET/icon_${half}x${half}@2x.png"
            fi
        done
        iconutil -c icns "$ICONSET" -o bundle/icon.icns
    else
        # Fallback: copy logo.png and rely on Finder rendering. The .icns lookup
        # in Info.plist will then fail but the app still runs.
        cp assets/logo.png bundle/icon.icns 2>/dev/null || true
    fi
fi
[[ -f bundle/icon.icns ]] && cp bundle/icon.icns "$APP/Contents/Resources/icon.icns"

# Codesign (adhoc by default).
IDENTITY="${CODESIGN_IDENTITY:--}"
codesign --force --sign "$IDENTITY" \
    --options runtime \
    --entitlements bundle/entitlements.plist \
    "$APP"

echo
echo "Built $APP"
echo "Quick run:    open $APP"
echo "First launch will prompt for Microphone + Input Monitoring (and on older"
echo "macOS, Accessibility). Approve them in System Settings → Privacy & Security."
