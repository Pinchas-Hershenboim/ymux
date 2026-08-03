# Followups

Out-of-scope bugs found in passing. One line per item. Read at session start; surface P0/P1 to the user before starting new work.

Format:

```
- [ ] P<0|1|2|3> | <YYYY-MM-DD> | <file>:<line> | <one-line repro/symptom>
```

## Open


## Done
- [ ] P3 | 2026-08-03 | app/src-tauri/build.rs (resources) | A fresh git worktree can't `cargo build`: `app/src-tauri/resources/winmux-cli.exe` is gitignored, so the Tauri build script dies with "resource path doesn't exist". Has to be copied from the main checkout by hand. Either build it on demand or document the step.
