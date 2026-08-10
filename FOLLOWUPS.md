# Followups

Out-of-scope bugs found in passing. One line per item. Read at session start; surface P0/P1 to the user before starting new work.

Format:

```
- [ ] P<0|1|2|3> | <YYYY-MM-DD> | <file>:<line> | <one-line repro/symptom>
```

## Open

<!-- Found while fixing the macOS ports + upload bugs (2026-08-10). All confirmed
     by reading the code, none of them reproduced live. Out of scope for that fix. -->
- [ ] P1 | 2026-08-10 | app/src-tauri/src/provisioning.rs:278 | `dpapi_protect` on non-Windows returns `noprotect:<secret>` — remembered SSH passwords sit in plaintext at rest on macOS. Violates CLAUDE.md Rule #2. Fix = Keychain via `security add-generic-password` or a keyring crate.
- [ ] P1 | 2026-08-10 | app/src-tauri/mcp/src/main.rs:179 | MCP server hard-errors on macOS ("only Windows transport is implemented (named pipe)") — the MCP tool is unusable there, local or remote.
- [ ] P1 | 2026-08-10 | app/src-tauri/cli/src/main.rs:1171 | Local CLI has no Unix-socket transport, so every local hook/`winmux` call on macOS exits 2 unless `WINMUX_SOCKET_ADDR` is set by hand. The server side already binds one (`winmux_core::pipe_names()`) — mirroring it here is small, but CI builds no mac-native `winmux-cli` to ship it in.
- [ ] P2 | 2026-08-10 | app/src-tauri/src/lib.rs:1518 | `pick_default_shell()` is not cfg-gated and falls through to `"cmd.exe"` on every OS — creating a local workspace on macOS spawns a binary that doesn't exist.
- [ ] P2 | 2026-08-10 | app/src-tauri/src/local_wizard.rs:120 | `detect_local_shells()` pushes "Windows PowerShell" and "Command Prompt" with `available: true` regardless of host OS.
- [ ] P2 | 2026-08-10 | app/src-tauri/src/local_wizard.rs:251 | `builtin_defaults()` backslash-joins cwd suggestions (`{h}\Documents`) — shows `/Users/yossi\Documents` on macOS.
- [ ] P2 | 2026-08-10 | app/src-tauri/src/updater.rs | In-app updater is NSIS/MSI-only; no-op on macOS (already noted in PROGRESS.txt as a known gap).
- [ ] P3 | 2026-08-10 | app/src-tauri/src/settings.rs:1661 | No font enumeration on macOS/Linux (no `fc-list`/CoreText branch) — the font picker only ever offers the hardcoded baseline list.
- [ ] P3 | 2026-08-10 | app/src-tauri/src/local_setup.rs | The "local → new" smart-setup wizard is WSL/winget-only (~1200 lines) with no macOS equivalent; every step fails with ENOENT there. Not cfg-gated, so it's reachable from the UI.
- [ ] P3 | 2026-08-10 | app/src-tauri/src/insights_local.rs:283 | Docker hint text hardcodes `\\.\pipe\docker_engine` even on the non-Windows failure path.

## Done
