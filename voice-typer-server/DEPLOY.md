# Deploying voice-typer-server

Reach: Cloudflare Tunnel → axum binary on `127.0.0.1:8787` on a Linux VPS. SQLite
lives in `/var/lib/voice-typer-server/`. The Deepgram API key never leaves the
server.

## 1. Build the binary

On the VPS (or cross-compile and scp):

```bash
git clone <repo> /opt/voice-typer-server-src
cd /opt/voice-typer-server-src/voice-typer-server
(cd spa && npm ci && npm run build)
cargo build --release
sudo install -d /opt/voice-typer-server /opt/voice-typer-server/spa
sudo install -m 0755 target/release/voice-typer-server /opt/voice-typer-server/
sudo cp -r spa/dist /opt/voice-typer-server/spa/dist
sudo cp -r migrations /opt/voice-typer-server/migrations
```

## 2. User + data dir

```bash
sudo useradd --system --no-create-home --shell /usr/sbin/nologin voice-typer
sudo install -d -o voice-typer -g voice-typer -m 0750 /var/lib/voice-typer-server
sudo install -d -o root -g voice-typer -m 0750 /etc/voice-typer-server
sudo cp deploy/env.example /etc/voice-typer-server/env
sudo chmod 0640 /etc/voice-typer-server/env
sudo chown root:voice-typer /etc/voice-typer-server/env
# edit /etc/voice-typer-server/env, set DEEPGRAM_API_KEY and ADMIN_BOOTSTRAP_*
```

## 3. systemd unit

```bash
sudo cp deploy/voice-typer-server.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now voice-typer-server
sudo journalctl -u voice-typer-server -f
```

The bootstrap admin token is printed once on first start; grab it from
`journalctl` and save it.

## 4. Cloudflare Tunnel

The axum server only binds `127.0.0.1`, so the only public path in is through the
tunnel. Install cloudflared on the same VPS, then:

```bash
cloudflared tunnel login                              # one-time, browser-based
cloudflared tunnel create voice-typer                 # writes ~/.cloudflared/<UUID>.json
cloudflared tunnel route dns voice-typer voice.your-domain.com
sudo install -d /etc/cloudflared
sudo cp cloudflared.example.yml /etc/cloudflared/config.yml
sudo $EDITOR /etc/cloudflared/config.yml               # fill UUID + hostname
sudo cp ~/.cloudflared/<UUID>.json /etc/cloudflared/   # credentials file
sudo cloudflared service install
sudo systemctl enable --now cloudflared
```

Visit `https://voice.your-domain.com` — Cloudflare terminates TLS, forwards to
the local axum server.

## 5. First admin login

1. `https://voice.your-domain.com/login`, sign in with `ADMIN_BOOTSTRAP_*`.
2. Go to `/admin`, mint invites.
3. **Rotate your admin password** — currently there is no UI for this; bypass:
   ```bash
   sudo systemctl stop voice-typer-server
   sudo -u voice-typer sqlite3 /var/lib/voice-typer-server/voice-typer.db \
     "DELETE FROM users WHERE email='you@example.com';"
   sudo $EDITOR /etc/voice-typer-server/env   # set new ADMIN_BOOTSTRAP_PASSWORD
   sudo systemctl start voice-typer-server
   ```
   The service re-bootstraps admin with the new password on next start.

## 6. Updates

```bash
cd /opt/voice-typer-server-src
git pull
(cd voice-typer-server/spa && npm ci && npm run build)
cargo build --release --manifest-path voice-typer-server/Cargo.toml
sudo systemctl stop voice-typer-server
sudo install -m 0755 voice-typer-server/target/release/voice-typer-server /opt/voice-typer-server/
sudo cp -r voice-typer-server/spa/dist /opt/voice-typer-server/spa/dist
sudo cp -r voice-typer-server/migrations /opt/voice-typer-server/
sudo systemctl start voice-typer-server
```

Migrations run on startup (`sqlx::migrate!`), so no manual step.

## Backup

Just the SQLite file: `/var/lib/voice-typer-server/voice-typer.db`.

```bash
sudo sqlite3 /var/lib/voice-typer-server/voice-typer.db ".backup '/var/backups/vt-$(date +%F).db'"
```

(WAL journal mode is on; the `.backup` command captures a consistent snapshot
without stopping the service.)
