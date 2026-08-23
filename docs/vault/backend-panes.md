---
vault: backend-panes
covers:
  - app/src-tauri/src/diff_pane.rs
  - app/src-tauri/src/file_manager.rs
  - app/src-tauri/src/workspace_browser.rs
  - app/src-tauri/src/worktrees.rs
  - app/src-tauri/src/workspaces_merge.rs
  - app/src-tauri/src/notes.rs
  - app/src-tauri/src/tickets.rs
  - app/src-tauri/src/skills.rs
  - app/src-tauri/src/stt.rs
  - app/src-tauri/src/fonts.rs
  - app/src-tauri/src/tray.rs
  - app/src-tauri/src/dev.rs
---

# Non-terminal panes and app features

Twelve modules, ~8,300 lines. A pane in ymux is not necessarily a shell — Diff, File
Manager, and Browser are panes too, and they share the layout tree with terminals.

## Panes

**`diff_pane.rs` (322)** — one background tokio task per Diff pane, polling `git diff`
every `POLL_INTERVAL_MS` and emitting `diff-pane-updated` when the output hash changes.
Duplicate suppression uses a cheap fnv-style `u64` rather than a string compare, so an
unchanged large diff costs a read and zero allocations. Each tick re-reads the
workspace's cwd + `DiffSource` under a **short** lock — the cwd can change (worktree
re-anchor) and later polls must see it. The task self-terminates when the workspace is
gone or its layout no longer contains the pane id. Handles live in
`CoreState.diff_pane_watchers`.

**`file_manager.rs` (1,741)** — dual-column file manager: lists, transfers, mutations on
both sides. Local ops use `std::fs`; remote ops piggy-back on the workspace's
already-authenticated SSH session and open **a fresh SFTP channel per call**. Sessions
are deliberately *not* cached: a new SFTP subsystem on an existing handle is cheap, and
caching would mean chasing teardown semantics when the terminal pane disconnects.

**`workspace_browser.rs` (506)** — **at most one child Webview per workspace**, attached
to the main window via `Window::add_child` (this is what pins `tauri = "=2.10.3"` with
`features = ["unstable"]`). `workspace_browser_show(workspace_id, url, x, y, w, h)`
creates or reveals it. All browser webviews share the **process-default WebView2
environment**; a per-workspace `--user-data-dir` forces a separate environment per
workspace and WebView2 does not support multiple environments in one process — that
surfaced as `0x8007139F`. Creation is serialized by `AppState.browser_create_lock` for
the same reason. Runtime-only, never persisted; `workspace_delete` calls
`cleanup_workspace_sessions` to remove `browser-sessions/<workspace_id>/`.

## Git

**`worktrees.rs` (638)** — enumerate and create git worktrees for a workspace flagged
`is_project_root` whose cwd is a repo. Every git call dispatches on that workspace's
connection:

| connection | transport |
|---|---|
| `Local` / none | `tokio::process::Command`, arg array |
| `Wsl` | `local_setup::wsl_exec` (an `sh -ls` script) |
| `Ssh` | an exec channel on a **live** handle to that host |

The SSH path matches on `user@host:port`, **not on workspace id** — a repo is reachable
from any session to the same host.

**`workspaces_merge.rs` (415)** — the three-way merge `save_to_disk` calls. A save is not
"dump what I have": it re-reads the file and, if it changed since we last touched it,
applies only *our* delta onto *theirs*. Needed because a stable build and a dev build
share `%APPDATA%\ymux` unless someone sets `YMUX_CONFIG_DIR`, and the pre-rename
`%APPDATA%\winmux` + `WINMUX_CONFIG_DIR` are still honoured — so an old and a new binary
can land on the same directory from either side of the rename. `reconcile(ours, base,
theirs)` is a **pure function with unit tests**, which is the point: an idle app never
saves, so the interesting path cannot be reached by launching one and waiting.

## Capture and content

**`notes.rs` (341)** — `%APPDATA%\ymux\notes.json`, atomic write + poison gate, emits
`notes:changed`. Exposed as **both** Tauri commands and JSON-RPC methods, so the CLI on
a remote pane can drop a note through the tunnel.

**`tickets.rs` (1,867)** — dev-mode element capture. Right-clicking in a workspace
browser with Dev Mode on records xpath + CSS selector + bounded HTML + computed-style
summary + optional screenshot + a description. Tickets belong to a **project**, not a
workspace — the point is handing one to Claude Code inside the right repo — so they are
written to `<project>/.ymux-tickets/` when the project is reachable from this machine,
and fall back to `<config_dir>/tickets/<workspace_id>/` while still recording the project
path, so nothing is orphaned.

**`skills.rs` (288)** — installs a skill folder (`SKILL.md` + scripts) from the local
registry at `config_dir()/ymux-tools/skills/<name>/` onto a workspace's
`~/.claude/skills/<name>/`: filesystem copy for local, SFTP over the existing session for
remote. Populating the source registry is out of scope for this module.

**`stt.rs` (243)** — **local-endpoint** speech-to-text only. The Web Speech API backend
is entirely frontend (`window.SpeechRecognition` ships in WebView2). This posts audio to
a user-configured HTTP endpoint using a multipart shape that mirrors OpenAI's
`/v1/audio/transcriptions`, so whisper.cpp's server, faster-whisper-server, and
OpenAI-compatible proxies all work with no per-server adapter.

## Shell chrome

**`fonts.rs` (880)** — download a curated font, verify it, register it **per-user**:
files land in `%LOCALAPPDATA%\Microsoft\Windows\Fonts` and register under HKCU, which
needs no elevation on Windows 10 1809+. `settings::list_system_fonts` reads that same
hive, so an install is visible in the picker immediately. Exists because flagging
unavailable families with ⚠️ was only half an answer — the user still had to go find a
`.ttf`.

**`tray.rs` (91)** — tray icon, quick menu, taskbar badge. **Best-effort**: a failed
build logs and carries on. `TRAY_ACTIVE` gates close-to-tray so a failed tray can never
trap the user with a hidden window and no way back.

**`dev.rs` (174)** — shared structures and pure helpers behind `ymux dev`: state-snapshot
building, the console ring buffer (`CONSOLE_MAX = 200`, fed by frontend
`console.error`/`warn` captures, exposed as `AppState.console_buffer`), and log /
bug-report file IO. The commands and RPC handlers themselves live in `lib.rs` and
`rpc_server.rs`.

## Invariants

- **Rule #7** — `notes.json`, ticket files, and every other config write are tmp +
  fsync + rename.
- **Rule #1** — the file manager and diff pane see user content; log paths and byte
  counts, never bodies.
- A watcher task must self-terminate when its pane leaves the layout. `diff_pane` is the
  reference implementation; `port_watcher_tasks` in `CoreState` follows the same shape.
- One Webview per workspace, one WebView2 environment per process. Do not add a
  `--user-data-dir`.

## Read the source when

You need a file-manager operation's exact error mapping, the ticket JSON schema, or the
curated font list. `docs/CONFIG.md` covers the user-visible settings these expose.
