# Followups

Out-of-scope bugs found in passing. One line per item. Read at session start; surface P0/P1 to the user before starting new work.

Format:

```
- [ ] P<0|1|2|3> | <YYYY-MM-DD> | <file>:<line> | <one-line repro/symptom>
```

## Open

- [ ] P1 | 2026-08-03 | app/src-tauri/src/lib.rs:1565 | emit_data strict-UTF8 decoder: if `leftover` starts with an invalid byte, `valid_up_to==0` returns WITHOUT draining -> that pane goes permanently silent and the buffer grows unbounded (no cap, no error branch). Found while tracing the local-PowerShell Hebrew bug.
- [ ] P2 | 2026-08-03 | app/src-tauri/src/local_setup.rs:881 | WSL elevation detection is an English substring match (`low.contains("elevat") || "0x80070005"`) -> on localized Windows a UAC-declined `wsl --install` is misreported as a generic StepFailed. Also no ERROR_SUCCESS_REBOOT_REQUIRED (3010) / reboot-pending concept in inspect_wsl (:378-473).
- [ ] P2 | 2026-08-03 | app/src-tauri/src/local_setup.rs:1096 | local-setup failures log only `err.user_message()`, dropping `stderr` -> the UI red card shows the real reason but the log never does, so a user-reported install failure is undiagnosable after the fact.

## Done
