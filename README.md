# Voice Typer

Push-to-talk voice dictation for Windows and macOS, with a hosted website and proxy.

Hold a key, speak, release, and your words paste at the cursor in any app. The
desktop app is a tiny native binary; the server holds the upstream speech-to-text
API key and bills access with Stripe subscriptions (per-plan monthly minutes).

## Layout

- **[`voice-typer-rs/`](voice-typer-rs/)** — the desktop app (Rust). Global
  push-to-talk hotkey, floating status badge, optional real-time streaming, and
  auto-paste. Windows and macOS. The macOS `.app` is built by GitHub Actions
  (`.github/workflows/macos.yml`). On macOS the hotkey uses Carbon
  `RegisterEventHotKey` (no Input Monitoring permission); the only permissions
  it needs are Microphone and, for auto-paste, Accessibility.
- **[`voice-typer-server/`](voice-typer-server/)** — the website + proxy server
  (Rust + axum, SQLite). Landing page, accounts, Stripe Checkout subscriptions,
  the per-user token system, and the speech-to-text proxy. The upstream API key
  never leaves the server. Serves `voice.codag.site`.

## How it fits together

The app authenticates to the server with a per-user token. The server proxies
audio upstream with its own API key, logs usage, and enforces each plan's
monthly minute quota. Subscriptions are created via Stripe Checkout and kept in
sync through Stripe webhooks.

## Local development (server)

Copy `voice-typer-server/.env.example` to `.env`, fill in the values, then:

```
cd voice-typer-server
cargo run --release
```

Secrets (`.env`), the SQLite database (`data/`), and built binaries
(`downloads/`, `target/`) are git-ignored.
