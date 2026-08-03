# Followups

Out-of-scope bugs found in passing. One line per item. Read at session start; surface P0/P1 to the user before starting new work.

Format:

```
- [ ] P<0|1|2|3> | <YYYY-MM-DD> | <file>:<line> | <one-line repro/symptom>
```

## Open

- [ ] P3 | 2026-08-03 | app/src-tauri/src/lib.rs:1604 | bidi_filter state is keyed by pane, so an SSH channel's stdout and stderr still share one escape-sequence state machine (the UTF-8 + OSC halves were split by the emit_data fix). A partial CSI/OSC on one stream can be completed by the other's bytes. Deliberate: a synthetic per-stream key would miss the per-pane smart_bidi toggle and silently leave stderr unfiltered. Only reachable via ExtendedData, rare on a PTY channel.
- [ ] P2 | 2026-08-03 | app/src-tauri/src/local_setup.rs:881 | WSL elevation detection is an English substring match (`low.contains("elevat") || "0x80070005"`) -> on localized Windows a UAC-declined `wsl --install` is misreported as a generic StepFailed. Also no ERROR_SUCCESS_REBOOT_REQUIRED (3010) / reboot-pending concept in inspect_wsl (:378-473).

## Done

- [x] P1 | 2026-08-03 | app/src-tauri/src/lib.rs:1565 | emit_data strict-UTF8 decoder: if `leftover` starts with an invalid byte, `valid_up_to==0` returns WITHOUT draining -> that pane goes permanently silent and the buffer grows unbounded (no cap, no error branch). Found while tracing the local-PowerShell Hebrew bug. FIXED 2026-08-03 — decoder moved to `pty_decode::Utf8Stream`, which branches on `Utf8Error::error_len()`: a truncated tail is held (Hebrew/emoji splits still survive), genuinely invalid bytes become U+FFFD and are drained, so a stall is structurally impossible and leftover is capped at 3 bytes by construction. 14 unit tests.
- [x] P2 | 2026-08-03 | app/src-tauri/src/local_setup.rs:1096 | local-setup failures log only `err.user_message()`, dropping `stderr` -> the UI red card shows the real reason but the log never does, so a user-reported install failure is undiagnosable after the fact. FIXED 2026-08-03 by 6753759 — failure logs now append StepFailed stderr.
