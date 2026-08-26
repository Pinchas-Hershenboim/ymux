---
vault: backend-claude
covers:
  - app/src-tauri/src/claude_log.rs
  - app/src-tauri/src/claude_summary.rs
  - app/src-tauri/src/claude_usage.rs
  - app/src-tauri/src/claude_usage_local.rs
  - app/src-tauri/src/insights_local.rs
---

# Claude integration + local Insights

Five modules, ~2,650 lines. Three of them read or drive the `claude` CLI **on the machine
that hosts the transcripts** — usually the remote, not the desktop. The other two are the
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

## `claude_usage_local.rs` (549) — token history, local half

Phase 84.E. A deliberate mirror of `server/internal/insights/claudeusage.go`: same scan,
same JSON field names, same clamping — the pattern `insights_local` already set for
`/current`, with `insights_fetch` routing remote-vs-local so the frontend never branches.

**Two implementations of one aggregation is real duplication, and it is the cheaper
option.** `~/.claude/projects` runs to 240 MB across ~170 transcripts on a working box;
the alternative — SFTP-mirroring the remote tree to the desktop and parsing it once, in
Rust — would pull hundreds of megabytes over the wire every time the tab opens. The cost
of the choice is the one that setup always has: **the two can drift apart silently**, so
compare their output on the same window when you touch either.

Counts tokens, does not price them — the table is `app/src/claudePricing.ts`, in one
place, so a rate change is a one-file edit and not a server rebake plus a Rust edit.
Cache writes are split 5-minute vs 1-hour because a 1-hour write costs 2x base input
against a 5-minute write's 1.25x, and collapsing them understates a long session.

**Rule #1 by construction:** it reads `message.model`, `message.usage`, the timestamp,
the session id and the cwd. It never reads message content, and it logs only counts.

## `claude_log.rs` (871) — the transcript reader behind the Sessions view

Backend for the ClaudeLog pane Phase 24.D removed from the frontend ("three competing
'talk to claude' UIs felt fragmented"). Yossi asked to keep the backend for a future
unified view, and that view now exists: `ClaudeSessionsView.tsx` is its first caller,
so these commands are no longer the dead-but-registered set this page used to describe.
`#![allow(dead_code)]` stays at the top — several response-type fields are still only
ever read by serde.

Two pairs of readers, differing only in where the JSONL comes from.

**Mirrored — a remote workspace.** Claude runs on the server, so its transcripts live
there too. `claude_log_sync` SFTP-copies them down and the two readers work off that
copy under `%APPDATA%\ymux\claude-logs\<workspace_id>\`.

- `claude_log_sync(workspace_id, session_id?)` — mtime-gated, full-file fetch (no byte
  diffing). Needs a live SSH handle and errors cleanly when there is none.
- `claude_log_list(workspace_id)` — directory scan of the mirror + per-file summary.
- `claude_log_read(workspace_id, session_id)` — parse one mirrored JSONL into a
  structured `ClaudeLogEntry` stream.

**Direct — this machine.** A local workspace has no server to mirror from, so the
mirror stays empty for it and `claude_log_list` answered `[]` forever. These two read
`~/.claude/projects` in place instead, sharing `summarize_jsonl` and `entries_from_json`
with the mirrored path so the two sources can never render differently.

- `claude_log_list_local()` — walks exactly one project level, because that is how deep
  Claude Code nests, and summarises each `<session>.jsonl`. A missing projects dir is an
  empty list rather than an error: Claude Code has simply never run here, and an empty
  list says that more usefully than a failure.
- `claude_log_read_local(session_id)` — same parse. It rejects any id that is not a bare
  filename stem, because the id is joined into a path and a `..` would otherwise read an
  arbitrary file.

### One line is not one entry

`entries_from_json` returns a `Vec`, and that shape is the whole reason a tool call can
be drawn as a card. A `tool_use` almost never arrives as its own transcript line — it is
one block inside `message.content`, next to the assistant's prose. While the parser
returned a single entry per line, `extract_text` had to flatten the call into the literal
string `[Tool: Bash]` inside the message body: there was no separate event to draw, and
the input and output were gone before the frontend saw anything. Expanding blocks into
their own entries fixes it at the source. `line_no` is therefore **not unique** across
the returned stream.

A call carries `tool_id` (`tool_use.id`) and its answer carries the same id
(`tool_result.tool_use_id`), which is how the frontend pairs them into one card.
`pretty_tool_input` unwraps a single-string input to its raw value rather than
JSON-escaping it — a Bash `command` or an Edit `new_string` is what the reader came for,
and `\n`-escaping a shell script makes it unreadable for no gain. Both sides are capped
at `TOOL_BODY_MAX` (4,000 chars) and say how much they cut, because a reader who cannot
tell truncation from a tool that returned nothing goes looking for a bug. Without the cap
a session with a few `cat` results re-serialises megabytes on every 2.5s poll.

`extract_text` still exists and still flattens, because `summarize_jsonl` wants exactly
that for the one-line session-list preview.

**Do not delete this as dead code.** The mirrored half predates its caller by a release
and the header says why.

## `insights_local.rs` (743) — Insights for Local workspaces

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

Two paths are answered without touching `sysinfo`. `/analytics` returns the literal
marker `{"unavailable":"local"}` — the Analytics tab rolls up the 7-day metric history
the remote daemon keeps in SQLite, and a local workspace has no daemon and no store, so
there is nothing to aggregate. A marker rather than an error string is what lets the
panel explain itself instead of surfacing a raw "unsupported path". `/claude-usage`
delegates to `claude_usage_local::route`.

## `claude_usage_local.rs` (550) — the local half of `/claude-usage`

A deliberate mirror of `server/internal/insights/claudeusage.go`: same scan, same JSON
field names, same clamping, so `insights_fetch` can route remote-vs-local and the
frontend never branches. **Two implementations of one aggregation is real duplication,
and it is the cheaper option** — `~/.claude/projects` is routinely 240 MB across 170
transcripts, so the alternative (SFTP-mirror the remote tree and parse it once, here)
would pull hundreds of megabytes over the wire every time the tab opens.

Same rule as the Go side: **it counts tokens and does not price them.** The price table
is `app/src/claudePricing.ts`, in one place. Cache writes stay split 5-minute vs 1-hour
because a 1-hour write costs 2x base input against a 5-minute write's 1.25x, and
collapsing them understates a long session.

Rule #1 is why the parser reads only `message.model`, `message.usage`, the timestamp, the
session id and the cwd — never message *content* — and logs nothing but counts.

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
