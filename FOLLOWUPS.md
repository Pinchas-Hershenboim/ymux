# Followups

Out-of-scope bugs found in passing. One line per item. Read at session start; surface P0/P1 to the user before starting new work.

Format:

```
- [ ] P<0|1|2|3> | <YYYY-MM-DD> | <file>:<line> | <one-line repro/symptom>
```

## Open

- [ ] P1 | 2026-08-03 | app/src-tauri/src/fonts.rs | Install has no matching UNINSTALL. A user who installs FiraCode Nerd Font (27 MB, 6 files) has no in-app way to remove it — they must hand-delete from %LOCALAPPDATA%\Microsoft\Windows\Fonts and HKCU. Symmetry matters here because we are the ones who put the files there.

- [ ] P2 | 2026-08-03 | app/src-tauri/src/fonts.rs:73 | Font catalog pins upstream tags+sha256; needs a periodic refresh check (JetBrainsMono v2.304, FiraCode 6.2, nerd-fonts v3.5.0, powerlevel10k-media master). An upstream re-release makes install fail with "checksum mismatch" — correct but opaque to the user.
- [ ] P2 | 2026-08-03 | app/src-tauri/src/fonts.rs:588 | MesloLGS NF assets are pinned to `master` on powerlevel10k-media, not a tag — the sha256 pin is what protects us, but a legit upstream update will break the entry until refreshed. Prefer a tagged source if one appears.
- [ ] P3 | 2026-08-03 | app/src-tauri/src/settings.rs:1662 | Mono/UI split is a name-substring heuristic; "Courier 10,12,15 (120)" bitmap .fon entries still reach the terminal picker and are known to measure badly (see terminalInstance.ts remeasureFont comment). Consider filtering bitmap .fon families out entirely.
- [ ] P3 | 2026-08-03 | app/src-tauri/src/settings.rs:1629 | Font enumeration shells out to PowerShell on every Settings open (~300ms). Fine at current call frequency; if the picker ever refreshes more often, read the registry via the winreg dep that fonts.rs now pulls in.

## Done
