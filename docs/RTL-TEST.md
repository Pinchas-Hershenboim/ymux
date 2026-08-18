# RTL per-line direction — test matrix (v0.4.4, Approach C)

ymux gives every **visible** terminal row an explicit `dir` computed from its
text by `detectDirection()` (`app/src/textDirection.ts`), replacing xterm.js's
`dir="auto"` ("first strong directional character wins"), which mis-rendered a
mixed Hebrew+Latin line that happened to start with Latin.

**Rule (Yossi):** a line with **any** Hebrew/Arabic char → **RTL** (mixed OR
pure); a **pure-Latin** line → **LTR**; digits / symbols / whitespace only →
**LTR** (safe default).

Only affects the `auto_per_line` RTL mode (the default). Gated by
**Settings → Terminal → "Auto-direction per line"** (default ON).

## Unit tests

`app/src/textDirection.test.ts` — 47 cases (`node:test`), of which 10 cover
`nextTuiOwnsBidi`. Run:

```
cd app && node --experimental-strip-types --test src/textDirection.test.ts
```

## Detection matrix

| Input | Expected | Why |
|-------|----------|-----|
| `1. Hello world` | **LTR** | pure Latin |
| `1. שלום עולם` | **RTL** | pure Hebrew |
| `1. שלום world` | **RTL** | mixed → RTL |
| `/opt/wa/.shared.env` | **LTR** | pure ASCII path |
| `שרת רץ על port 4200` | **RTL** | mixed Hebrew + latin/digits |
| `` (empty) | **LTR** | safe default |
| `12345` | **LTR** | digits only |
| `→ ← ↑ ↓` | **LTR** | arrows/symbols only |
| `$ ls -la /home` | **LTR** | shell prompt + command |
| `git commit -m 'תיקון'` | **RTL** | Latin command with Hebrew arg |
| `ERROR: קובץ לא נמצא` | **RTL** | Latin word then Hebrew |
| `مرحبا بالعالم` | **RTL** | pure Arabic |
| `run مرحبا now` | **RTL** | mixed Arabic + Latin |
| `הפורט 4200 פתוח` | **RTL** | Hebrew wrapping a number |

Within an RTL line, embedded Latin runs (paths, `port 4200`) keep their natural
LTR order via the browser's BiDi algorithm — the row's paragraph direction is
RTL, the runs are not reversed.

## Visual smoke test (real-world)

1. RTL mode ON (Hebrew UI). Connect a shell; `printf` a numbered list where one
   item is a pure ASCII path and the others start with Hebrew → the path line
   sits LTR, the Hebrew/mixed lines sit RTL, list markers align on the right.
2. `echo "שרת רץ על port 4200"` → whole line RTL; "port 4200" reads L-to-R
   inside it.
3. `cat` a source file (pure Latin) → unchanged LTR, no flips.
4. Toggle **Settings → Terminal → Auto-direction per line = OFF** → every row
   renders LTR (classic terminal). Toggle back ON → per-line detection resumes
   live (no reconnect).

## TUI-owns-bidi smoke (Claude Code visual-order RTL)

Covers `tuiOwnsBidi` / `nextTuiOwnsBidi`. **Run this whole section after any
merge that touches `terminalInstance.ts` or `textDirection.ts`** — the feature
was silently lost in merge `bcaa330` (2026-07-31) and stayed gone for 18 days
with a green test suite, because the merge deleted the code and its tests
together. See `docs/DECISIONS.md`, the TUI-owns-bidi entry.

1. Start Claude Code in a pane and print Hebrew → renders correctly: no
   reversed letters, no left-aligned scramble, no clipped glyphs.
2. `%APPDATA%\winmux\debug.log` shows `[TERM] tui-owns-bidi on pane=<id>` at
   Claude start. **If this line never appears, the feature is inert** — for
   tmux panes that is expected to be the open question (see follow-up 3 in
   DECISIONS); for a local pane it is a bug.
3. Type a Hebrew sentence ending in `?` into Claude's input box → the `?` stays
   at the **end** of the line, not the start. This is the exact A/B symptom
   from the round-6 diagnosis and the fastest single check.
4. Exit Claude → `tui-owns-bidi off` in the log → Hebrew shell scrollback flips
   back to per-line RTL.
5. **Repeat steps 1–4 twice in the same pane.** This is what catches the
   latch-ON failure: if the off-signal never arrives, every later shell screen
   renders forced-LTR with no recovery short of a new pane.
6. Devtools console: no xterm parser errors. (Round 6 saw 519 `FSI U+2068`
   errors when the pipeline double-bidi'd.)
7. Repeat **inside tmux over SSH**, not only locally — tmux swallows OSC 0/2
   titles by default, so this is the case that decides follow-up 3.
8. Restart the app with session restore on, reattaching a pane whose Claude is
   already running (it will not re-emit its title) → check whether the state
   engages. New interaction; the feature predates session restore.

Accepted costs, **not** failures (`DECISIONS.md`, 2026-07-17 — display
correctness wins): the caret sits one cell forward when typing Hebrew to
Claude, and mouse selection on mixed Hebrew/English lines starts a few cells
off.

## Performance

- Only **visible** rows carry DOM nodes, so scrollback size (up to millions of
  lines) is irrelevant — the pass touches ~24–50 rows max.
- Row mutations are coalesced to **one pass per animation frame**
  (`requestAnimationFrame`).
- A per-row text cache (`WeakMap<Element,string>`) skips any row whose text is
  unchanged since the last pass.

## Cursor interaction (PARKED "RTL caret", 2026-06-26)

`isCurrentLineRtl()` (the Left/Right arrow-mirroring gate) now uses the **same**
`detectDirection()` rule, so the caret/arrow behaviour matches the visual
direction on mixed lines. Candidate fix for the parked caret item — **verify
live** before marking it resolved.
