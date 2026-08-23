# ymux-server API reference

`ymux-server` (2.0.0) exposes a localhost HTTP+WS API behind the ymux tunnel,
bearer-token gated. Two audiences:

1. **Client-SDK surface** (mobile / web / contract tests) — generated OpenAPI +
   the WS frame contract. This is what `sdk/` wraps.
2. **Desktop-internal surface** — the Insights metrics API, consumed by the
   desktop Monitor's Rust client. Not part of the generated SDK.

## Discovery

| Endpoint | Auth | What |
|---|---|---|
| `GET /healthz` | none | liveness `{ok, version}` |
| `GET /api/version` | none | negotiation `{name, version, api_versions:[2], frame_version}` |
| `GET /api/openapi.json` | none | REST contract (generated from huma handlers) |
| `GET /api/asyncapi.json` | none | WS frame contract (AsyncAPI 2.6) |
| `GET /api/frames.schema.json` | none | WS frames as JSON-Schema 2020-12 (SDK source) |

## Client-SDK surface (bearer, generated into the SDKs)

Auth on `/api/v2/*` accepts **the shared desktop token OR a paired device's
long-term token** (from `pairing/redeem`), so a phone uses the same surface.

**Pairing** — `POST /api/pairing/redeem` (public; one-shot token in body is the
credential) → `{device_id, long_term_token, default_workspace_id}`.

**Files** — sandboxed to the server's files root (`$HOME` by default):
`GET files/list?path=&depth=1|2`, `GET files/read?path=&max_bytes=` (raw bytes +
`X-Ymux-Truncated`), `POST files/upload?path=` (multipart `file`),
`GET files/download?path=` (attachment), `DELETE files/delete?path=`.

**Logs** — per-client log tree + the `server` pseudo-client:
`GET logs/list`, `GET logs/read?client_id=&file=&tail=`,
`GET logs/stream?client_id=&file=` (SSE `event: line`).

**Workspace** — `GET /api/v2/workspace/list`,
`POST /api/v2/workspace/{id}/sessions` (`{kind}` → `{session_id, kind}`),
`GET /api/v2/workspace/{id}/session/{sid}`, plus the WS stream
`GET workspace/{id}/session/{sid}/subscribe?cursor=&client_id=&token=`
(frames in [CLIENTS.md](CLIENTS.md) + AsyncAPI).

> Desktop-only workspace admin (`POST /create`, `GET /{id}`, `DELETE /{id}`,
> `GET /{id}/sessions`) stays on raw handlers — outside the SDK spec.

Full schemas: the served `openapi.json` (REST) + `frames.schema.json` (WS).

## Desktop-internal surface (not in the SDK spec)

Insights metrics, served at both legacy paths and `/api/v2/insights/*`:
`current`, `history`, `analytics`, `claude-usage`, `hygiene[/kill]`,
`docker[/…]`, `processes`,
plus `/api/v2/logs/daemon`. Dynamic JSON (metric/docker/process maps) consumed by
the desktop over SSH; intentionally kept on raw stdlib handlers and out of the
generated OpenAPI (PHASE-77-DESIGN §6).

`GET analytics?since=&until=&points=` backs the desktop Monitor’s **Analytics**
tab. It rolls the whole `samples` / `disk_samples` / `docker_samples` window up
in SQL and returns the entire screen — `totals`, a bucketed `series`, and the
`by_period` / `by_disk` / `by_container` tables — in **one** response, because
every desktop fetch is a `curl` over the workspace SSH session (`--max-time 6`).
Prefer it over `history` for anything wider than a couple of hours: `history`
returns raw rows with `LIMIT 2000`, which at the 5s sample interval is the
*oldest* 2.8 hours of whatever range you ask for. All three query params are
clamped, never rejected (`since` to the 7-day retention window and to a 5-minute
floor, `points` to 20…400). Timestamps come back as unix seconds so the desktop
formats them in the viewer’s timezone, not the server’s.

`GET claude-usage?since=&until=` backs the **Claude** tab's cost panel. It walks
`~/.claude/projects/**/*.jsonl` — Claude Code's own transcripts — and returns
token counts rolled up by hour, by model+speed, by project and by session, plus
a main-loop/subagent split. It **counts tokens and never prices them**: the
price table lives in `app/src/claudePricing.ts`, in one place, so a rate change
is a one-file desktop edit rather than a server rebake plus a matching edit in
the Rust local mirror. Token counts are facts; prices are a table that goes
stale.

The scan is bounded by an mtime prune — a transcript's mtime is its last
append, so a file older than `since` cannot hold an in-window line and is never
opened. That is what keeps a 240 MB tree affordable inside a six-second `curl`.
The response reports `scanned_files` / `skipped_files` / `parse_errors` so an
empty answer can be told apart from a failed one, and cache writes come back
split 5m vs 1h because those are priced differently (1.25x vs 2x base input).

## Pairing (desktop-facing)

`/api/pairing/*` — the desktop Monitor issues QR + device tokens. The legacy
mobile Claude-chat HTTP surface (`/api/claude/*`, `/ws/claude/*`) was retired in
Phase 77 → **410 Gone**; clients use `/api/v2/workspace/*`.
