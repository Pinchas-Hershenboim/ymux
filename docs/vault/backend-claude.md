---
vault: backend-claude
covers:
  - app/src-tauri/src/claude_log.rs
  - app/src-tauri/src/claude_summary.rs
  - app/src-tauri/src/claude_usage.rs
  - app/src-tauri/src/insights_local.rs
  - app/src-tauri/src/claude_usage_local.rs
---

# Claude integration + local Insights

Four modules, ~2,100 lines (plus the uncovered `claude_usage_local.rs` mirror, Phase 84.E). Three of them read or drive the `claude` CLI **on the machine
that hosts the transcripts** — usually the remote, not the desktop. The fourth is the
local-machine half of the Insights panel.

## `claude_summary.rs` (375) — session auto-summary

Takes a Claude Code JSONL transcript from
`~/.claude/projects/<proj>/<session>.jsonl`, pipes the last N exchanges through
`claude -p "<prompt>"` **on the same machine that holds those transcripts**, and saves
the result as a ymux Note tagged `summary`.

Two entry points:

- **Manual** — Ctrl+Alt+B, the Summarize button in Settings → Claude, or the
  `claude_summarize` Tauri command.
- **Automatic** — a Claude Code Stop hook arriving via `feed.push`, when
  `settings.claude.auto_summarize_on_stop` is on. `rpc_server`'s dispatcher calls
  `summarize_session_for_pane` in the background. **Failures are logged, never fatal.**

## `claude_usage.rs` (397) — real subscription quota

`claude -p "/usage" --output-format json` returns the user's actual Pro/Max quota —
session %, weekly %, per-model %, reset times, and a "what's contributing" breakdown —
inside the envelope's `result` string.

The call is **free** (`total_cost_usd: 0`, `num_turns: 0`) but costs **~8 seconds** of
real round-trip. So: cached per workspace for **5 minutes**, fetched on demand or on a
slow auto-refresh. **Never fast-poll this.**

**Rule #1 applies hard here:** log the workspace id and the percentages, never the
`/usage` body — it names the user's subagents, skills, and MCP servers.

## `claude_log.rs` (600) — alive on purpose, unused on purpose

Backend for the ClaudeLog pane, which Phase 24.D removed from the frontend ("three
competing 'talk to claude' UIs felt fragmented"). Yossi asked to keep the backend for a
future unified view, so the three commands stay registered in `invoke_handler!` with
**no frontend caller**. The `#![allow(dead_code)]` at the top is what silences the
resulting warning cascade.

- `claude_log_sync(workspace_id, session_id?)` — SFTP-mirror new/changed files,
  mtime-gated, full-file fetch (no byte diffing).
- `claude_log_list(workspace_id)` — local directory scan + per-file summary.
- `claude_log_read(workspace_id, session_id)` — parse the local JSONL into a structured
  `ClaudeLogEntry` stream.

**Do not delete this as dead code.** It is deliberate, and the header says so.

## `insights_local.rs` (755) — Insights for Local workspaces

Speaks the **same JSON shape** as the remote `ymux-server` HTTP API, so
`InsightsWindow.tsx` shares its parsing code. The only routing decision is
remote-vs-local, and `addons.rs::insights_fetch` makes it transparently — the frontend
never chooses.

- CPU / memory / disks / network / processes come from `sysinfo` — cross-platform, no
  WMI plumbing.
- Docker on Windows via `bollard` over `\\.\pipe\docker_engine` (Docker Desktop). If
  Docker is not running it returns an **empty container list rather than an error**; the
  panel already renders a friendly "no docker" state.
- Log tag: `[INSIGHTS-LOCAL]`.
- **Two routes the remote daemon gained in Phase 84.C/E are answered here too, but not
  with the same data.** `/analytics` (the Monitor's Analytics tab) returns the literal
  `{"unavailable":"local"}`: the remote rolls up seven days of SQLite samples, while this
  module samples on demand and persists nothing, so there is no history to aggregate.
  It is a marker, not an error, so the panel can explain *why* instead of showing a raw
  "unsupported path". `/claude-usage` is delegated to `claude_usage_local.rs`, a Rust
  mirror of the Go `claudeusage.go` walk over `~/.claude/projects/**/*.jsonl` — the
  duplication is deliberate, since the alternative was SFTP-pulling hundreds of MB of
  transcripts to the desktop each time the tab opens. Both halves count tokens only;
  pricing lives in one place, `app/src/claudePricing.ts`.

## Related, elsewhere

- `AppState.claude_paths` (in `lib.rs`) caches the absolute path to the `claude` binary
  per `<workspace_id>:<scope>`, where scope is `ssh` or `local`. Detection runs on first
  chat-send and sticks for the session. It exists because **SSH execs do not source
  `~/.bashrc`**, so a `claude` that is only on the user's interactive PATH is otherwise
  invisible.
- `pane_list_claude_sessions`, `list_claude_sessions_local`, `peek_claude_jsonl`
  (`CLAUDE_PEEK_BYTES = 256 KB`) and `claude_project_dir_prefix` live in `lib.rs` — the
  session picker reads only the tail of a transcript rather than the whole file.
- The hook verbs that feed all of this (`stop`, `user-prompt-submit`, …) are handled in
  `rpc_server.rs`; see `backend-rpc.md`.

## Invariants

- **Rule #1** — transcript *content* never reaches `debug.log`. Summaries are written to
  notes (a user-visible store), which is a different thing from logging.
- Everything here is best-effort: a missing `claude` binary, a Docker daemon that is
  down, or an unparseable transcript degrades the feature, never the app.
- The local and remote Insights payloads must stay shape-compatible. Changing one
  without the other silently breaks the panel for half the workspaces.

## Read the source when

You need the `/usage` JSON envelope's exact fields, the summary prompt text, or the
Insights payload schema. The remote half is in `server-go.md`; the panel that consumes
it is in `frontend-panes.md`.
