# ymux-server deployment

The desktop installs the daemon for you (Monitor → install). This documents what
that automation does, for operators who want to run it standalone.

## Placement

- Binary: `~/.ymux/bin/ymux-server` (SFTP-uploaded by the desktop; the right
  arch is picked from the two embedded `ymux-server-linux-{x64,arm64}`).
- A `~/.ymux/bin/ymux-insights` symlink → `ymux-server` is kept so any
  pre-2.x tooling / version probe still resolves.
- Data dir: `~/.ymux/insights/` (token, `*.db`, `logs/`, `insights.log`).

## systemd (user unit)

The installer writes `~/.config/systemd/user/ymux-server.service`:

```ini
[Unit]
Description=ymux server daemon
After=network.target
[Service]
ExecStart=%h/.ymux/bin/ymux-server serve
Restart=on-failure
RestartSec=5
[Install]
WantedBy=default.target
```

Then `systemctl --user daemon-reload && systemctl --user enable --now ymux-server`.
Any old `ymux-insights` unit is disabled + removed first (its data dir is
unchanged, so nothing is lost). Without systemd it falls back to a `nohup … &`
launch (optionally wrapped in `sg docker` when the user was just added to the
docker group).

On SIGINT/SIGTERM the daemon drains in-flight HTTP requests (5s deadline) before
exiting, so a `systemctl restart` won't cut off a metrics/file request.

## Networking

- The daemon binds **127.0.0.1** only. Remote access is via the ymux SSH
  tunnel (and the optional nginx-proxy add-on for the mobile/split-QR path).
- Everything except `/healthz`, `/api/version`, and the spec endpoints requires
  the bearer token (`~/.ymux/insights/token`).

## Health

`GET /healthz` → `{ok, version, uptime_seconds}` (unauthenticated). `GET /api/version`
advertises `{api_versions, frame_version}` for client negotiation.

## Logs

`~/.ymux/insights/insights.log`, rotated at 1 MB, plus a 7-day janitor. The
per-client Logs API tree lives under `~/.ymux/insights/logs/`.
