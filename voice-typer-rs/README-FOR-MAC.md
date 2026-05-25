# Building Voice Typer on macOS (one-shot, for a friend)

Thanks for the help! This builds a `VoiceTyper.app` bundle for macOS. It takes
~5–10 minutes the first time (mostly downloading Rust crates).

## Prereqs

You need any Mac with **macOS 12 Monterey or newer**. Both Apple Silicon and
Intel work.

1. **Xcode Command Line Tools** (provides clang, the linker, and the macOS
   SDK). If you don't have them yet, this pops up a GUI installer:
   ```sh
   xcode-select --install
   ```

2. **Rust** (a small one-time installer, user-level — no sudo needed):
   ```sh
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
   source "$HOME/.cargo/env"
   ```

## Build

In a terminal, `cd` into this folder (the one with `Cargo.toml` in it), then:

```sh
# Make the bundle scripts executable (the ZIP loses the +x bit)
chmod +x bundle/make-app.sh bundle/make-dmg.sh

# Add the two Mac targets so we can produce a Universal2 binary (arm64 + x86_64)
rustup target add aarch64-apple-darwin x86_64-apple-darwin

# Build + bundle + adhoc-sign. Output: target/macos/VoiceTyper.app
bundle/make-app.sh universal
```

That's it. If everything compiles, you'll see something like:

```
...
Built target/macos/VoiceTyper.app
Quick run:    open target/macos/VoiceTyper.app
```

**If you only want a build for your own Mac's architecture** (faster, no
need to add the second target), drop the `universal` argument:

```sh
bundle/make-app.sh
```

## Send back

Zip the `.app` so it survives email/Slack/Drive:

```sh
cd target/macos
ditto -c -k --keepParent VoiceTyper.app VoiceTyper.app.zip
```

Send back `target/macos/VoiceTyper.app.zip`. That's the only file we need.

(Optionally, `bundle/make-dmg.sh` produces a `target/macos/VoiceTyper.dmg`
with the standard "drag to /Applications" layout, if you'd rather send a DMG.)

## If something goes wrong

The most likely failure modes:

- **`error: linker 'cc' not found`** → Xcode CLT didn't install fully. Run
  `xcode-select -p` to see if it's installed. Reinstall with
  `sudo xcode-select --install`.
- **A compile error in `src/platform_mac/*.rs`** → my Mac-specific code was
  written without on-device verification. If you hit something like "method
  not found on `CGEventTap*`" or "`CGEventFlags::MASK_X` is private", capture
  the error and send it back — that's expected and we'll patch it remotely.
- **`bundle/make-app.sh: Permission denied`** → you forgot the
  `chmod +x bundle/make-app.sh` step.

You do NOT need an Apple Developer ID or notarization for this build — it's
ad-hoc signed, which lets us install and run it on our own Macs (with a
right-click → Open the first time to bypass Gatekeeper).
