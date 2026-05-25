#!/usr/bin/env bash
# Build a distributable DMG from VoiceTyper.app.
#
# Run this AFTER make-app.sh. The resulting target/macos/VoiceTyper.dmg
# contains the app and an /Applications symlink.

set -euo pipefail

cd "$(dirname "$0")/.."

APP="target/macos/VoiceTyper.app"
DMG="target/macos/VoiceTyper.dmg"
STAGE="target/macos/dmg-stage"

if [[ ! -d "$APP" ]]; then
    echo "missing $APP — run bundle/make-app.sh first" >&2
    exit 1
fi

rm -rf "$STAGE" "$DMG"
mkdir -p "$STAGE"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"

hdiutil create \
    -volname "VoiceTyper" \
    -srcfolder "$STAGE" \
    -ov \
    -format UDZO \
    "$DMG"

echo "Built $DMG"
