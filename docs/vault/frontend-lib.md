---
vault: frontend-lib
covers:
  - app/src/terminalInstance.ts
  - app/src/types.ts
  - app/src/settings.ts
  - app/src/claudePricing.ts
  - app/src/insightsFmt.ts
  - app/src/insightsReport.ts
  - app/src/insightsCommands.ts
  - app/src/clipboardText.ts
  - app/src/textDirection.ts
  - app/src/bidi.ts
  - app/src/copyBidi.ts
  - app/src/mouseRtl.ts
  - app/src/sessionRestore.ts
  - app/src/logger.ts
  - app/src/shortcuts.ts
  - app/src/stt.ts
  - app/src/platform.ts
  - app/src/download.ts
  - app/src/fontProbe.ts
  - app/src/i18n/index.ts
---

# Frontend library modules

The non-component half of `app/src/`. Two things dominate: the terminal wrapper, and
**RTL** — four separate modules exist because Hebrew broke in four different places.

## `terminalInstance.ts` (1,867) — the xterm.js wrapper

`class TerminalInstance` owns one xterm `Terminal`, its `FitAddon`, the optional
`WebglAddon`, and the DOM container. Module-scope globals cache font family/size, theme,
and the Ctrl+C-copies-selection flag so new panes construct with the current values;
`setTerminalTheme` / `setTerminalFont` / `setRtlProfiles` push changes to live panes.

Things it does that are easy to get wrong:

- **`applyRowDirections` / `ensureDirObserver`** — a `MutationObserver` coalesces a burst
  of cell mutations into **one** `applyDir()` per animation frame, and a `WeakMap` cache
  skips any row whose text is unchanged. Without both, per-line direction is a
  per-mutation DOM write.
- **`fitAndResize`** is rAF-throttled — the `ResizeObserver` fires per pixel during a
  divider drag, and every call sends a SIGWINCH down the SSH channel. tmux cannot keep up
  and the renderer thrashes.
- **WebGL glyph atlas is flushed on resize.** Without that the GPU canvas keeps painting
  the previous viewport's grid metrics — visible as lines that do not reflow.
- **Link handling** — OSC 8 hyperlinks and a plain-text `[file]` link provider (Claude
  Code prints produced files as plain text, not OSC 8). Each has a one-shot diagnostic
  flag, metadata only per Rule #1, so "the regex never matched" is distinguishable from
  "the click path is broken". `workspaceId` is set on connect so a `file://` click can
  SFTP-download from the right remote.
- **`writeData` buffers and flushes**, and the custom right-click menu lives here too.

### RTL profiles — read this before touching direction

`RtlProfileSettings` mirrors `RtlProfile` in `settings.rs`: `rtlMode`
(`auto_per_line | bidi_reorder | off`), `autoDirection`, `mirrorArrowsRtl`,
`tuiOwnsBidi`, and `directionPolicy`.

`directionPolicy` is the field to understand:

- **`any_rtl`** — any Hebrew/Arabic on the row takes it RTL. What every version before
  2026-08-19 shipped, and what remote panes are **known** to render correctly.
- **`tui_dominance`** — the `RTL_DOMINANCE` vote (in `textDirection.ts`): Hebrew wins
  unless massively outnumbered by Latin, which stops a TUI status bar from mirroring its
  own layout.

**It is keyed on the pane class, never on what is running inside the pane.** The vote
first shipped gated on `tuiOwnsBidi`, and because the OSC title propagates over SSH — that
is how Claude Code is detected at all — it fired on remote panes and broke them. Yossi's
instruction afterwards was a total separation between local and remote, so a change aimed
at local panes cannot reach remote ones. A per-profile field is that separation, and
`remote_direction_policy_is_the_pre_2026_08_19_rule` in `settings.rs` plus the parity
tests in `textDirection.test.ts` enforce it. The same reasoning is why the four knobs
stopped being scalar globals.

## The four RTL modules

**`textDirection.ts` (379)** — per-line direction. xterm's DOM renderer with `dir="auto"`
uses "first strong directional character wins", which mis-renders a mixed line that
happens to *start* with Latin: `2. /opt/wa/.shared.env - הערה` laid out LTR because the
first strong char is Latin, though the line is mostly Hebrew. Yossi's rule instead: a
line containing **any** Hebrew/Arabic is RTL. `RTL_DOMINANCE` is the `tui_dominance`
refinement on top.

**`bidi.ts` (71)** — the `bidi_reorder` path (bidi-js, no type defs). Exports the escape
matcher so the visual→logical pass protects escapes **exactly** the way this file does —
one definition of "what an escape looks like".

**`copyBidi.ts` (185)** — visual→logical for text on its way to the **clipboard**.
Measured on Yossi's machine, 2026-08-20: plain PowerShell renders reversed on screen but
pastes correctly, while Claude Code renders correctly and pastes reversed — exactly
inverted, because the two panes hold opposite orders in the buffer.

**`mouseRtl.ts` (82)** — coordinate transform for RTL rows. xterm's `SelectionService`
maps `clientX` → buffer column assuming LTR. With `dir="rtl"` on a row the browser paints
it mirrored, so a click on what the user sees as cell 5 lands on cell `cols - 5 - 1`.
Selection and click positioning both land on the wrong side without this.

## Typed mirrors

**`types.ts` (542)** — the data-model types are **generated from the Rust structs by
ts-rs** and re-exported here so `from "./types"` keeps working. Regenerate after a Rust
struct change with `cd app/src-tauri && cargo test`. **Do not hand-edit
`src/bindings/*.ts`.** Note ts-rs renders `Option<T>` as `T | null` — a required,
nullable key, not `T?` — so helpers such as `effectiveIdentity` widen their params to
`T | null | undefined`. The hand-written helpers here (`paneCaps`, `profileFor`,
`describeConnection`, `isLocalConn`, `isRemoteEffective`, `collectPanes`, `findPane`)
are what components use to reason about a pane.

**`settings.ts` (761)** — the typed settings mirror plus load/save and the CSS-variable
apply. `src-tauri/src/settings.rs` owns the canonical schema; this follows it. Also
carries the font-catalog bindings: `fontCatalog` (each item now reporting whether it is
`installed`, read from the font directory on every call rather than from any record of
past installs), `fontInstall`, and `fontUninstall`.

## The Insights pure modules

All four are **DOM-free and framework-free on purpose** — no Solid, no i18n, no
clipboard — which is what makes them testable as plain node tests. Callers pass in
already-translated strings.

- **`claudePricing.ts` (246)** — the one place token counts become dollars. Both the Go
  server and the Rust local mirror deliberately count and do not price, so a rate change
  is a one-file edit here instead of a server rebake plus a matching Rust edit. Carries
  `PRICING_AS_OF` and per-model `intro` rates with an `until` timestamp; past that
  moment the table falls back to the standard rate on its own. **Nothing re-checks the
  rest of the table** — re-read it against published pricing (via the `claude-api`
  skill, never from memory) whenever a model ships or a rate moves.
- **`insightsReport.ts` (427)** — the wire shape of `GET /analytics`, plus the one thing
  you can do with it outside the panel: flatten it to a plain-text report to paste into
  Claude, an email or an incident ticket. The column alignment is exactly the kind of
  thing that stays quietly wrong forever if nothing asserts it, so
  `insightsReport.test.ts` does.
- **`insightsFmt.ts` (33)** — `fmtBytes` / `fmtBps` / `fmtSpan`, lifted out of
  `InsightsWindow.tsx` once a second panel needed them.
- **`insightsCommands.ts` (122)** — the copy-the-commands blocks; takes whether the
  workspace is local so it can name the right paths.
- **`clipboardText.ts` (31)** — one clipboard write with a fallback. Note what is NOT
  covered by any test: whether `navigator.clipboard.writeText` is actually granted
  inside WebView2 for these panels.

## Small modules

- **`logger.ts` (77)** — `createLogger(tag)`. Lines reach both devtools and the single
  local `debug.log` via the `ui_log` command, tagged `[UI:TAG]`. Level filtering is
  **double-gated**: skip the IPC below the threshold here (cheap), and the backend filters
  again — the backend is authoritative, so a popout window that never loads settings still
  behaves. **Import this before the console monkeypatch.** Rule #9.
- **`i18n/index.ts` (86)** — dictionaries statically imported (~30 KB total, no async
  loader). Active language and direction are two signals, so `t(key)` and the document
  `dir` react together. A missing key returns the key itself.
- **`platform.ts` (50)** — host OS resolved **once** from Rust (`host_platform`,
  `std::env::consts::OS`). Exists because two Windows-only assumptions were baked in as
  literals and both broke on mac: local paths joined with a hardcoded `\`, and drag-drop
  positions divided by `devicePixelRatio` (WebView2 reports physical pixels, wry's macOS
  backend reports logical points).
- **`sessionRestore.ts` (102)** — remembers which tmux session each SSH pane was attached
  to, so the next start re-attaches instead of showing [Connect]. **localStorage on
  purpose**: per-machine, high-churn session state, the same class as window rects and
  sidebar width. Losing it costs one click, never data, and it keeps Rule #7's
  atomic-write surface small.
- **`shortcuts.ts` (200)** — parses accelerators like `Ctrl+Shift+C` from
  `settings.shortcuts.<name>` once on settings load, and exposes
  `matches(event, accelerator)`. Same vocabulary in the hand-editable JSON and the
  click-to-record picker.
- **`stt.ts` (262)** — one recorder interface over two backends: `webspeech` uses
  `window.SpeechRecognition` directly (WebView2 ships it, but Chrome streams to Google's
  servers behind the scenes — which is exactly why the Local option exists), and `local`
  records with MediaRecorder and POSTs through `stt_transcribe_local`.
- **`download.ts` (55)**, **`fontProbe.ts` (121)** — OSC 8 / file-link downloads, and
  probing whether a font family is actually installed.

## Invariants

- **Rule #5** — no `any`. `XtermInternals` in `terminalInstance.ts` is the pattern: a
  minimal typed view into a private API rather than a cast.
- **Rule #9** — `createLogger`, never `console.*`.
- **Rule #1** — the diagnostic flags around links log *that* something matched, never the
  matched text.
- Local and remote RTL behaviour are separated by profile and must stay that way.
- `src/bindings/` is generated. Edit the Rust struct.

## Read the source when

You need an xterm addon's exact wiring, the full RTL decision table, or a specific
accelerator's parse rules. All four RTL modules have unit tests
(`textDirection.test.ts`, `bidi.test.ts`, `copyBidi.test.ts`, `mouseRtl.test.ts`) —
those tests are the specification and are deliberately not covered by this vault file.
