---
vault: backend-core
covers:
  - app/src-tauri/src/lib.rs
  - app/src-tauri/src/main.rs
---

# Backend core — `lib.rs`

**Windows are built programmatically, not in `tauri.conf.json`** — its `windows` array is
empty on purpose. `main` is built in `.setup()` with `.devtools(false)`; `popout_pane`
builds `popout-<sid>` terminal windows and `workspace_browser::browser_popout_open`
builds `browser-popout-<ws>`. All three share the same three non-negotiables: the
builder call must be in an **`async`** command (on Windows `WebviewWindowBuilder`
deadlocks from a synchronous one — the shell appears, the webview stays blank white), the
URL must be a **clean `index.html`** with the id carried by the window LABEL (the built
app's asset protocol serves a blank page for any suffixed path), and lifecycle is wired
through `on_window_event` on `Destroyed`, never `CloseRequested`. Each label prefix needs
its own file in `capabilities/`; the globs are prefix-anchored, so `browser-popout-*` is
not covered by `popout-*`.

`teardown_workspace_runtime` is the single place a workspace's runtime state dies: the
Browser child Webview, its pop-out OS window (`close_popout_window` — otherwise the
window outlives the workspace), the browser session dir, the bootstrap verdict, and the
reverse-tunnel state.

**12,809 lines, and about 1,700 of them are `#[cfg(test)]` at the bottom.** It is the
"everything else" module: app state, the workspace data model and its persistence, the
PTY and SSH spawn paths, the multiplexer (zellij / tmux) plumbing, and ~70 Tauri
commands. `main.rs` is 6 lines — the Windows-subsystem flag and `app_lib::run()`. Never
put logic there.

## Shape of the file, in order

| Lines | What |
|---|---|
| 1–40 | `mod` declarations for the 34 sibling modules; `use ymux_tunnel as tunnel` |
| 40–280 | `AppState`, `AgentRunState`, `FeedItem`/`FeedStore`, `NotificationItem`, `LoadState` |
| 305–830 | `WorkspacesFile`, config paths, `machine_id`, `save_to_disk`/`load_from_disk`, migrations |
| 1190–1570 | Pure layout-tree walkers (`close_pane_in`, `set_split_ratio_in`, …) |
| 1570–2220 | Shell detection, UTF-8 arg/env quoting, Smart Connect script builder, `emit_data` |
| 2224–3100 | `spawn_local_pty`, the zellij verb wrappers, tmux attach script, `spawn_wsl_pty` |
| 3100–4600 | SSH: key offer/generate, `connect_and_authenticate`, `spawn_ssh`, `TunnelLease` |
| 4600–5000 | Port watcher, workspace↔ssh-handle lookup |
| 4986–7270 | Workspace/group/layout Tauri commands |
| 7274–9500 | Connect path: `workspace_ensure_connected`, `pane_connect`, session listing/labels/owners |
| 9798–10180 | `pane_kill_session`, `pane_disconnect`, pty write/resize, feed, `doctor`, `popout_pane` |
| 10185–10760 | `run()` — the Tauri builder |
| 10759+ | Unit tests, one `mod` per concern |

## Key types

- **`AppState`** ([lib.rs:145](../../app/src-tauri/src/lib.rs)) — the single managed Tauri
  state, `Clone` because every field is an `Arc<Mutex<…>>` and the RPC server task needs
  its own handle. It **wraps** `ymux_core::CoreState` at `state.core`: `sessions`,
  `pane_sessions`, `forwards`, `port_watchers`, `detected_ports`, `port_watcher_tasks`,
  `diff_pane_watchers`. Everything else — `workspaces`, `load_state`, `notifications`,
  `pane_status`, `agent_runs`, `feed`, `notes`, `settings`, `recent_paths`,
  `console_buffer`, `claude_paths`, `bidi_filters`, `workspace_browsers`,
  `browser_create_lock`, `bootstrap_guard`, `tunnel_registry` — is app-shell concern and
  lives on the outer struct. **Reach russh state through `state.core.<field>`.**
- **`Session` / `LocalSession` / `SshSession` / `SshCmd`** — defined in
  `ymux-core`, re-exported here so `crate::Session` still resolves. See `crates.md`.
- **`Connection`, `LayoutNode`, `Workspace`** — `ymux-types`. `LayoutNode::Pane` carries
  its own optional `connection`, so one workspace's leaves can target different hosts.
- **`LoadState`** — `Loaded | Failed`. A poison flag: if `load_from_disk` hit a real
  read/parse error, `persist` refuses to write, because saving in-memory state over a
  file we failed to understand destroys the user's workspaces.
- **`PaneAgentState` / `AgentRunState` / `PaneAgentSnapshot`** — per-pane Claude state,
  in `AppState.agent_runs`. `apply_hook(subkind, notification_type)` is the transition
  table and it is the **single owner** of the state machine; the frontend only paints
  what it is handed. `NEEDS_INPUT_NOTIFICATIONS` and `RESUMED_NOTIFICATIONS` list the
  `notification_type` values that mean "blocked on the user" and "unblocked". A `stop`
  arriving after a notification still wins, an unmapped notification changes nothing,
  and a long turn does not keep resetting its own clock — all of that is pinned by unit
  tests in the same file. Transitions reach the UI as the **`pane:agent-run`** event via
  `emit_agent_run_event`, which carries `(started, avg, state, since, seq)`; `seq` bumps
  only on an applied transition, so a no-op skips the emit. In-memory and
  session-scoped — never persisted.
- **`workspace_set_tabs_mode`** — flips `Workspace.tabs_mode` and emits
  `workspaces:changed`. The layout tree is not touched; see `crates.md` for why this is
  a flag and not a `LayoutNode` variant.

## Persistence — the part to get right

`%APPDATA%\ymux\workspaces.json`, via `save_to_disk` ([lib.rs:514](../../app/src-tauri/src/lib.rs)).

1. Serialize to pretty JSON.
2. **Three-way merge before writing.** `LAST_KNOWN` (a `static Mutex<Option<String>>`)
   holds the file text as this process last read or wrote it. `save_to_disk` re-reads
   the file and hands `(ours, base, theirs)` to `workspaces_merge::reconcile`. Reason:
   a stable build and a dev build share `%APPDATA%` unless someone sets
   `WINMUX_CONFIG_DIR`, and a plain dump is last-write-wins across the whole document —
   the older binary silently drops every field its structs don't know.
3. Write `workspaces.<pid>.tmp`, `write_all`, **`sync_all`**, then `rename`. Rule #7.
4. `remember_file_text` updates the merge base.
5. Log line records `N workspaces: R root / N-R nested / P repo` — the tree *shape*,
   not just a count, because two pinned folders once lost `parent_id` with nothing in
   the log to bracket when.

`load_from_disk` repairs on the way in and each repair is logged: WSL→Local connection
rewrite (`migrate_wsl_workspaces`), `backfill_sort_orders`, `normalize_parents`,
`migrate_legacy_project_folders`. Other files in the same dir, each with the same
tmp+rename discipline: `machine-id` (stable per-install id, deliberately **not** in
settings.json so "Reset all settings" can't change this machine's identity),
tmux labels, session owners.

## Spawning a shell

`pane_connect` ([lib.rs:7419](../../app/src-tauri/src/lib.rs)) is the front door and takes
a wide argument list because every connection mode funnels through it: `persistent`,
`mode` (`default | tmux | plain | cmd | claude`), `cwd_override`, `cmd`, `claude_args`,
`tmux_session_name`, plus the credential arguments.

- Connection resolution prefers **the pane's own** `connection`, falling back to the
  workspace's. That is what stops an SSH workspace from quietly spawning a local shell
  in a pane that was split off a FileManager or Browser pane.
- **Local** → `spawn_local_pty`: ConPTY pair via `portable_pty`, shell from
  `pick_default_shell`, a reader thread that emits `pty:data` and cleans itself out of
  the session maps on child exit. `persist_session: Option<String>` picks the
  multiplexer by `cfg`: **zellij on Windows, tmux on macOS/Unix**. This is the one place
  the platforms genuinely differ, and it is deliberate (CLAUDE.md § Platforms).
- **SSH** → `connect_and_authenticate` then `spawn_ssh`: auth chain is ssh-agent
  (OpenSSH + Pageant, each wrapped in `catch_unwind` to absorb upstream panics) →
  explicit key file (optional passphrase) → default `~/.ssh/id_*` → password. Then
  best-effort bootstrap, `tcpip_forward(0)` for the reverse tunnel, env file via
  `ymux-tunnel`, shell channel with `set_env` for the `YMUX_*` vars, `request_pty`,
  `request_shell`, channel-pump task.
- `emit_data` ([lib.rs:1960](../../app/src-tauri/src/lib.rs)) is UTF-8 **boundary-safe** —
  it buffers a partial multibyte sequence rather than emitting a broken string. Do not
  "simplify" it.

## Multiplexer wrappers

Zellij verbs are built as argument vectors (`zellij_args_list`,
`zellij_args_delete_force`, `zellij_args_write_chars`) and run through
`zellij_try`/`zellij_run`, which classify spawn errors into a `ZellijOutcome` rather
than bubbling an `io::Error`. Never build these by string concatenation (Rule #3), and
check `docs/ZELLIJ.md` for what our pinned 0.44.3 binary actually supports before adding
a verb — zellij.dev documents a different version.

tmux is the SSH-side equivalent: `TMUX_LIST_FORMAT` + the `<<<YMUX_META>>>` marker frame
the listing output so `parse_tmux_sessions` can read it back unambiguously.
`session-meta` labels cross the wire **hex-encoded** (`hex_utf8`) so Hebrew/RTL labels
never meet shell quoting.

## Invariants

- **Rule #7** — every config write is tmp + fsync + rename. No exceptions in this file.
- **Rule #6** — every `#[tauri::command]` returns `Result<_, String>`; no `panic!`.
- **Rule #4** — no `unwrap`/`expect` outside tests and the `run()` boot path. The
  `state.workspaces.lock().unwrap()` calls are the known exception and predate the rule.
- **Rule #1** — PTY bytes are never logged. `log_debug` lines carry byte counts and
  pane ids only.
- `persist` gates on `LoadState::Loaded`. Anything that writes workspaces must go
  through it.

## Gotchas

- `run()` installs a **panic hook before anything else** and forces `RUST_BACKTRACE=1`,
  then calls `ymux_core::flush_log()` from inside the hook — log writes are queued, and
  a panic on its way to an abort loses them otherwise. This exists because a Hebrew-title
  crash (Phase 23.I) was a `STATUS_STACK_BUFFER_OVERRUN` with no Rust trace at all.
- **`.devtools(false)` on the main window and on every popout is mandatory, not
  cosmetic.** Phase 82.E turned on tauri's `devtools` feature so the workspace Browser
  child webview can be inspected (see `build-glue.md` and `backend-panes.md`), and
  `tauri-runtime-wry` reads that setting as `devtools.unwrap_or(true)` — the feature
  flips the default to *inspectable* for **every** webview in the process. These two
  windows render live PTY output, so an inspector on them is a Rule #1 leak. The opt-outs
  in `run()` and in `popout_pane` are the only thing standing between the feature and
  that. Do not "clean them up".
- `WEBVIEW2_USER_DATA_FOLDER` is set at the very top of `run()`, before any webview
  exists, to **one app-wide** profile dir. Per-workspace profiles reintroduce
  `0x8007139F` — WebView2 allows one environment per process. Windows-only by
  construction; WKWebView and WebKitGTK ignore the variable.
- `AGENT_RUN_MIN_TURN_MS = 2000` mirrors a gate in
  `ymux-tools/statuslines/hooks/turn-state.js`. Change one, change both.
- `FEED_MAX_ITEMS` here is `#[allow(dead_code)]` documentation — `rpc_server.rs` has
  its own copy.

## Read the source when

You need the exact `invoke_handler` list, the body of a specific command, the precise
russh channel-message match in `spawn_ssh`, or the Smart Connect script text. This file
tells you where those live; it does not reproduce them.

Design rationale lives in `docs/ARCHITECTURE.md`; the wire formats in
`docs/PROTOCOLS.md`; zellij's real CLI surface in `docs/ZELLIJ.md`.
