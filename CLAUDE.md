# CLAUDE.md

This file is read at the start of every Claude session working on ymux. Keep it small. Deep references live in `docs/`.

## Where to start

- `docs/ARCHITECTURE.md` — system map
- `docs/CONTRIBUTING.md` — recipes, style, commit conventions
- `docs/RELEASING.md` — version cut process
- `docs/DECISIONS.md` — **READ FIRST**: open threads + decisions log
- `docs/COMPETITIVE-SCAN.md` — survey of the 8 GitHub projects named `winmux`, ideas inventory, Secrets Vault design (pre-rename doc, kept verbatim: it is about *other people's* repos and is what motivated the move to YMUX)
- `docs/IDEAS-RANKING.md` — decision table for the ideas inventory (MUST / SHOULD / COULD)

## Session workflow (memory arch)

- **`git fetch` FIRST, every session — and again before every merge/push.** ymux is worked on from several machines and servers at once (Yossi's box, collaborators, cloud sessions), so `origin/main` moves while you work. Start with `git fetch origin && git status`; if `main` is behind, pull before touching anything. Re-fetch before any merge or push — a plan built on a stale tree is worse than no plan. This is not theoretical: on 2026-08-09 a collaborator's macOS port landed 5 commits on `origin/main` mid-session and was only noticed by accident at the end. Never assume the tree you started with is current.
- **PROGRESS.txt** — append after every significant change (timestamp, task, files, result). NEVER overwrite. Too big → rename `PROGRESS_OLD_<date>.txt`, start fresh.
- **FOLLOWUPS.md / BACKLOG.md** — read both at session start. Open P0/P1 in FOLLOWUPS → surface before new work. Out-of-scope bug found in passing → one line to FOLLOWUPS (P0-P3, file:line, repro). Out-of-scope idea / mock-stub debt → BACKLOG. Never silently leave broken state.
- **Past-work lookup order** — before re-investigating: 1) `PROGRESS.txt` + `PROGRESS_OLD_*` 2) `git log --all --oneline --grep=<keyword>` 3) memory search 4) `docs/*.md` + this file.
- **"Verified" = real run, not compile.** Build/type-check pass = syntax only. Say "compiles, untested" until run live.
- **Sync this file with code.** New port/service/endpoint/schema/deploy step → update the matching doc in the same commit.

## Decisions & open threads

When an idea or design question comes up:

1. If it's resolved in the same message, do it — no log entry needed.
2. If a decision is made but action is deferred, log it under **Decided** in `docs/DECISIONS.md` with the outcome and a deferral note.
3. If it stays open (user hasn't decided, blocked on input, flagged for later), log it under **Open** in `docs/DECISIONS.md` with options and current state.

When starting a new session, scan the **Open** section. Don't let threads die silently — if something's been pending a while, surface it.

## Pinned deps

- `tauri = "=2.10.3"` with `features = ["unstable"]` (app/src-tauri/Cargo.toml). The unstable feature gates `Window::add_child`, which Phase 53 uses to mount per-workspace browser webviews inside the main window. Bumping tauri requires verifying `Window::add_child`'s signature hasn't changed and the multi-webview shape still compiles. Run `cargo check --workspace` after any bump and smoke-test the workspace Browser window (sidebar 🌐 → open / hide via a modal / navigate / close).

## Off-limits paths

- `backup-phase23-*` folders — never touch
- Repo-root `.bat` / `.ps1` helper scripts the user maintains — never touch
- `release_notes.md` — do not commit
- `remote-manifest.json` timestamp churn — discard unless the SHA actually changed
- Linux CLI binary rebakes itself on release builds (CARGO_PKG_VERSION) — expected, commit as part of the release

## Release safety

- Never push a half-done release. If a step fails for a real reason, stop and report.
- Build through the Tauri CLI, never plain `cargo build --release` — see Rule #13.
- `app.exe` running on the user's machine causes `os error 32` during NSIS bundler cleanup — cosmetic; the binary + bundles produced fine. A running `app.exe` also blocks the link step outright (`failed to remove file … Access is denied`); ask Yossi to close it rather than retrying.
- v0.2.3+: updater uses native `ureq` + `rustls` (no more PowerShell).

## The winmux → YMUX rename (2026-08-18)

The app was `winmux` until 0.4.5. **A `winmux` you find in the code is almost
certainly load-bearing, not a leftover** — every one is a compat shim for an
existing install or an already-provisioned remote, and each is commented as
such. Do not "finish the rename" by deleting them; the removal is scheduled
(FOLLOWUPS P1, one release after 0.5.0) and has to happen as a set.

- **Still emits the legacy wire tag.** The handshake sends `WINMUX-CHALLENGE`
  on purpose: a pre-rename remote CLI does a literal prefix match. Both ends
  read *and mirror* either dialect (`CHALLENGE_TAG` in `crates/ymux-tunnel`,
  `challengeTag` in `server/internal/chat/chat_hookrpc.go`) — flip both together.
- **Migrations that run once, on upgrade:** `%APPDATA%\winmux` → `ymux`
  (`ymux-core::config_dir`), `~/.winmux` → `~/.ymux` (bootstrap + CLI), and
  the daemon's data dir. These stay long after the rest go.
- **`"winmux"` does not contain `"ymux"`.** Two substring checks broke on
  exactly that and were fixed; if you add another, match both spellings.
- History is deliberately NOT renamed: `PROGRESS.txt`, the Decided log in
  `docs/DECISIONS.md`, and `docs/COMPETITIVE-SCAN.md` (that one is about
  *other people's* repos named winmux — renaming it would make it a lie).

## CI (GitHub Actions)

- `ci-windows.yml` — cargo test + tsc + vite + go test on every push/PR to `main` (~5 min warm).
- `build-windows.yml` — installers + exe on `workflow_dispatch` or a `v*` tag; enforces Rule #13 by asserting the asset hash is embedded, and Rule #2's spirit by asserting the `$USERNAME` scrub landed. A tag gets MSI **and** NSIS (the published `manifest.json` advertises both); a `workflow_dispatch` gets NSIS only. Publishing stays manual (`docs/RELEASING.md`).
- `warm-rust-cache.yml` — **the reason release builds are ~7 min and not ~15.** GitHub scopes cache *reads* to the current branch plus the default branch, and `build-windows.yml` never runs on `main`, so a build dispatched from a fresh branch used to compile all ~740 lockfile packages from scratch (measured: 813s cold vs 420s warm, same workflow, same week). This job keeps that release-profile cache alive on `main` under `shared-key: windows-release`, on Cargo.lock changes and twice weekly. If release builds suddenly go slow again, check this workflow first.
- `build-macos-intel.yml` — the collaborator's macOS build, on `workflow_dispatch` or push to `macos-build`.
- Steps that shell out to Windows PowerShell need `shell: cmd`. The default `run:` shell is pwsh, which rewrites `PSModulePath` for its children, so the 5.1 instance `build:linux-cli` spawns loses `Get-FileHash`.
- `npm run build:linux-cli` must run before any cargo step on a fresh checkout — it stages the gitignored `ymux-cli.exe` the Tauri build script requires. It stages by **sha256, not mtime**: everything in `resources/` is pulled into the app crate with `include_bytes!`/`include_str!`, and Cargo's staleness check is mtime-based, so re-copying a byte-identical file used to force a full rebuild of the 9.4k-line lib.
- **A `.ps1` in this repo must keep non-ASCII out of code lines.** Windows PowerShell 5.1 reads BOM-less UTF-8 as ANSI, so an em-dash inside a string literal decodes to a smart quote that the tokenizer accepts as a string delimiter — the file then fails to parse with a cascade of errors pointing at innocent lines. Em-dashes in `#` comments are fine (already all over `build-linux-cli.ps1`); inside `"..."` they are a build-breaker.

## Communication

- User: Yossi (`yyhezkel@gmail.com`). Prefers Hebrew, terse, action-oriented replies.
- Phase numbering: stable in commit history. Sub-numbers (`23.J`) for follow-ups. No reuse.
- Commit format per `docs/CONTRIBUTING.md`.

## Absolute Rules — Do Not Violate

1. **Never log PTY input or output content.** Only metadata (pane ID, byte counts, error kinds). User shell content is private.
2. **Never store SSH passphrases or sudo passwords in plaintext at rest.** Use DPAPI (`CryptProtectData`) when persistence is necessary; otherwise keep in memory only.
3. **Never build shell commands by string concatenation.** Use `Command::new(...).arg(...)` arrays. The agent and provisioning paths are the only places this is enforced repeatedly — don't drift from it.
4. **No `unwrap()` or `expect()` in non-test Rust** outside the `main()` boot path. Use `?` or `.map_err(...)` and surface a clean error.
5. **No `any` in TypeScript.** Use `unknown` and narrow, or define a type. Tauri command return types are always explicit.
6. **All Tauri commands return `Result<_, String>`.** The frontend handles the error; don't `panic!`.
7. **Workspace persistence is atomic.** Write to `<file>.tmp` then `rename` to the target. Never partial writes to `workspaces.json` / `settings.json`.
8. **Never expose the tunnel HMAC token to logs.** Treat it like a password.
9. **The unified logger (`ymux_core::log_debug/info/warn/error(tag, msg)`) is user-visible** (lands in `%APPDATA%\ymux\debug.log`, format `[ts] [LEVEL] [TAG] msg`; threshold from Settings → Logs). Rust uses `log_*` with a component tag; frontend uses `createLogger(tag)` from `app/src/logger.ts` (never raw `console.*`); Go server uses `internal/logging.New("SRV:X")`; CLI hooks use `hook_log(level, msg)`. `dlog()`/`dlog_tag()` are legacy info-level shims — don't add new callers. `tracing::*` stays engineer-only (dev builds). Pick by audience.
10. **Don't push a half-done release.** If any step in RELEASING.md fails for a real reason (not the `os error 32` NSIS cleanup false-alarm), stop and report.
11. **Don't touch `backup-phase23-*/` or repo-root `.bat` / `.ps1` helper scripts.** Don't commit `release_notes.md`.
12. **`remote-manifest.json` timestamp churn is cosmetic.** Discard unless the embedded SHA actually changed.
13. **Never build the app with plain `cargo build --release`.** It links cleanly and produces a binary that loads `devUrl` (`localhost:1420`) at startup — every window is an `ERR_CONNECTION_REFUSED` page on any machine without a dev server, and the Rust log gives no hint (boot stops silently after `rpc server spawned`). `tauri-build` only embeds `frontendDist` when the build runs through the Tauri CLI. Use `npm run tauri build -- --no-bundle` from `app/`, or `app/scripts/build-release.ps1` for a real cut. To check a binary: the current `app/dist/assets/index-<hash>.js` filename must appear inside `app.exe` (`localhost:1420` appears in both kinds and proves nothing). Details in `docs/CONTRIBUTING.md` → "Building a runnable exe".
14. **A build that compiles is not a build that runs.** Launch it and confirm the UI comes up before saying "built" or "verified" — this is Rule #13's parent, and the reason that one shipped twice.
15. **Never push without fetching first, and never force-push a shared branch.** Several machines push to this repo. `git fetch origin` immediately before any merge or push; if `origin/main` moved, integrate and re-run the checks before pushing. Rewriting published history costs a collaborator their work.
16. **One session, one worktree — never two sessions in the same working copy.** Dispatched code tasks use `isolation: "worktree"`. The main checkout stays on `main` and clean, for reading and integration only. Commit before the session ends and remove the worktree by hand: git only auto-cleans worktrees that are *unchanged*, which is why 13 of them accumulated by 2026-08-03. Weekly: `git worktree prune` + delete branches with `ahead=0`. Two sessions sharing one tree is how commits land on the wrong branch.
