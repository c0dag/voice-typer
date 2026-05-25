# Building Voice Typer for macOS

The macOS build shares 100% of the cross-platform code (audio capture via
`cpal`, config, logging, proxy HTTP/WebSocket) and swaps in macOS-specific
modules for the system-integration bits:

| Module | Windows | macOS |
|---|---|---|
| `hotkey` | `WH_KEYBOARD_LL` / `WH_MOUSE_LL` | `CGEventTap` |
| `paste`  | `SendInput` Ctrl+V | `CGEvent` Cmd+V |
| `mute_key` | `SendInput` virtual keys | `CGEvent` keyboard events |
| `popup`  | Win32 layered window | (stub — no UI feedback yet) |
| `overlay`| Win32 layered window | (stub — no live transcript yet) |
| `settings` | Win32 dialog | (stub — opens config.json in editor) |
| `tray`   | `tray-icon` (cross-platform) | same |

Both `popup` and `overlay` are currently no-op stubs on macOS. The app works
headless: hold the hotkey, Deepgram transcribes via your proxy, the text is
pasted at the cursor. Visual feedback for the recording state is a known gap;
fill in via AppKit `NSPanel` once you're iterating on a Mac.

## Prerequisites

- macOS 12+ (Monterey or later)
- Xcode Command Line Tools: `xcode-select --install`
- Rust: `rustup target add aarch64-apple-darwin x86_64-apple-darwin`

## One-shot build

```bash
cd voice-typer-rs
bundle/make-app.sh           # builds for host arch (typically arm64)
bundle/make-app.sh universal # builds a universal2 (arm64 + x86_64) binary
```

Output: `target/macos/VoiceTyper.app`. Open it with `open target/macos/VoiceTyper.app`.

## First launch

1. macOS will prompt for **Microphone** access — required for `cpal`.
2. Hold the configured hotkey (default `f9`). macOS will then prompt for
   **Input Monitoring** (Settings → Privacy & Security → Input Monitoring).
   On macOS 12 you may also need **Accessibility** to allow `CGEvent` synthesis.
3. After approval, restart the app. The push-to-talk should now work end-to-end.

## Config file

Lives at `~/Library/Application Support/VoiceTyper/config.json`. Fields are the
same as the Windows version; relevant ones for first setup:

```jsonc
{
  "proxy_url": "https://voice.your-domain.com",
  "proxy_token": "paste-the-token-from-the-spa-dashboard",
  "hotkey": "f9",
  "deepgram_model": "nova-3",
  "language": "multi",
  "device_name": ""
}
```

There is no settings UI on macOS yet. Edit the JSON directly; the app re-reads
it on every launch.

## Distribution (sharing the .app outside your own Mac)

For a personal build, **adhoc signing is enough**:

```bash
codesign --force --sign - --options runtime \
    --entitlements bundle/entitlements.plist \
    target/macos/VoiceTyper.app
```

The `bundle/make-app.sh` script does this automatically.

To ship to other Macs, you need **Developer ID signing + notarization**:

```bash
# 1. Set your Developer ID in the env
export CODESIGN_IDENTITY="Developer ID Application: Your Name (TEAMID)"

# 2. Build + sign
bundle/make-app.sh universal

# 3. Notarize (requires an Apple Developer account)
xcrun notarytool submit target/macos/VoiceTyper.dmg \
    --apple-id you@example.com \
    --team-id TEAMID \
    --password "app-specific-password" \
    --wait

# 4. Staple the ticket
xcrun stapler staple target/macos/VoiceTyper.dmg
```

Then `bundle/make-dmg.sh` produces a notarized `target/macos/VoiceTyper.dmg`
that other users can open and drag to /Applications.

## What's still TODO

These are the parts that need iteration on a real Mac:

1. **`popup` UI** — pick an approach (tao window with custom NSView, raw
   AppKit via objc2, or a small SwiftUI side-loaded helper). The state machine
   is already wired via `popup::set_state_async()`.
2. **`overlay` UI** — same considerations, plus an `NSTextField` showing the
   live Deepgram interim results.
3. **`settings` UI** — for now, settings::spawn opens config.json in the
   default editor via `open`. A proper AppKit settings panel can come later.
4. **CGEventTap retry/permission UX** — the current code retries every 5s until
   the user grants Input Monitoring. A foreground notification ("Voice Typer
   needs Input Monitoring — open Settings?") would be friendlier.

The cross-platform parts (audio, proxy HTTP/WS, config persistence, hotkey
*parsing*) are fully shared, so any improvements there benefit both platforms.
