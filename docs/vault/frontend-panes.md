---
vault: frontend-panes
covers:
  - app/src/BrowserWindow.tsx
  - app/src/BrowserPane.tsx
  - app/src/browserDevMode.ts
  - app/src/FileManagerWindow.tsx
  - app/src/FileManagerPane.tsx
  - app/src/FileEditor.tsx
  - app/src/fmPaths.ts
  - app/src/MarkdownViewer.tsx
  - app/src/mdViewerStore.ts
  - app/src/DiffPane.tsx
  - app/src/InsightsWindow.tsx
  - app/src/InsightsAnalytics.tsx
  - app/src/InsightsClaudeCost.tsx
  - app/src/insightsReport.ts
  - app/src/claudePricing.ts
  - app/src/insightsFmt.ts
  - app/src/clipboardText.ts
  - app/src/HygienePanel.tsx
  - app/src/PortsWindow.tsx
  - app/src/TicketsPanel.tsx
  - app/src/TicketModal.tsx
  - app/src/FeedPanel.tsx
  - app/src/NotificationCenter.tsx
  - app/src/AddonsWindow.tsx
  - app/src/AddonsTab.tsx
  - app/src/YmuxToolsTab.tsx
  - app/src/ClaudeUsageIndicator.tsx
  - app/src/claudeUsageFmt.ts
  - app/src/TransferBar.tsx
  - app/src/transferStore.ts
  - app/src/MobilePairing.tsx
  - app/src/HelpPane.tsx
  - app/src/components/PopoutTerminal.tsx
---

# Panes, windows, and panels

Everything the user opens that is not a terminal and not a wizard. Most of these ride
the shared `PanelSurface` lifecycle from `frontend-shell.md` — drawer → float →
fullscreen — so they get that chrome for free instead of hand-rolling a variant.

## Browser

**`BrowserWindow.tsx` (792)** — the workspace-level Browser floating window. The actual
page is a **native child Webview** the Rust side mounts
(`workspace_browser.rs`); this component owns the toolbar, tabs, port+path entry,
navigation, and the window geometry. At most one per workspace. Phase 82.F added a
DevTools button beside the Dev Mode toggle that invokes `workspace_browser_open_devtools`
— on macOS it replaces the Safari → Develop → machine → webview walk, on Windows it is the
only way in, since F12 is not wired up for a child webview (and App's own F12 blocker
stays). Only this webview is inspectable; the why is in `build-glue.md`.

**`browserDevMode.ts` (448)** — right-click an element in the workspace browser to
capture it as a ticket. Kept out of `BrowserWindow.tsx` on purpose: with Dev Mode in its
own module, the browser component gains one signal, one toolbar button, and one
re-inject effect, and nothing about tabs or navigation changes.

**`BrowserPane.tsx` (546)** — the pre-Phase-53 in-pane Browser. **Not imported by
`LayoutView` any more**; kept as reference for its in-pane Webview wiring. Do not wire
it back up without reading the WebView2 single-environment constraint in
`backend-panes.md`.

## Files

**`FileManagerWindow.tsx` (151)** wraps **`FileManagerPane.tsx` (1,645)** — the dual
column local + remote SFTP manager — in the same drag/resize chrome as BrowserWindow.
Pure HTML, no native Webview, so the only persistence concern is geometry.

**`fmPaths.ts` (112)** remembers the last directory each column showed, per workspace, in
localStorage — so re-opening lands where the user left off instead of snapping to
`$HOME`. localStorage for the same reason as `sessionRestore.ts`: per-machine state that
changes on every navigation click, where loss costs one click.

**`FileEditor.tsx` (552)** — a modal with a monospace `<textarea>`, Save / Cancel /
Reload, and an unsaved-changes guard. **Syntax highlighting is deliberately out of
scope**: this is "view the file, fix a typo, save", not a code editor.

**`MarkdownViewer.tsx` (118)** + **`mdViewerStore.ts` (31)** — double-clicking a `.md`
in the File Manager opens it here instead of the OS app. **Security:** `html: false`
makes markdown-it drop raw HTML at parse time, and DOMPurify scrubs the rendered output
as a second layer.

**`TransferBar.tsx` (149)** + **`transferStore.ts` (207)** — in-flight SFTP transfers.
A module signal rather than prop-threading, because transfers start from several places
(the File Manager, terminal drag-drop, OSC 8 link downloads) and one listener at the App
root feeds them all into a single list.

## Monitoring

**`InsightsWindow.tsx` (586)** — pull-based server monitor. Fetches the live snapshot
through the `insights_fetch` Tauri command, which curls `127.0.0.1:7879` over the
workspace SSH session — **or serves it from `insights_local.rs` for a local workspace;
the routing is transparent to this component.** No mock data: if the daemon is not
installed or not running, the panel says so. It takes a `local` prop (App passes
`connection.type === "local"`) only so the copy-the-commands blocks can name the right
paths. Phase 84.C added a sixth tab, **Analytics** (`InsightsAnalytics.tsx`): what the
server has *been* doing, from the 7-day SQLite history the daemon had kept since Phase
68 that nothing ever read. One `/analytics` fetch (not `/history`, which is
`LIMIT 2000` oldest-first and answers a 7-day question with the wrong 2.8 hours) gives
stat tiles, a hand-rolled SVG sparkline with a `<details>` table twin, and by-period /
by-disk / by-container rollups — no charting library, no poll, loads on tab open and
Refresh. Amber is spent only on a disk past 85% or a container under 95% uptime; every
other bar is accent, because a bar encodes magnitude. A local workspace gets
`{"unavailable":"local"}` and an explanation; a 404 from an old daemon becomes
"reinstall from Add-ons". Phase 84.D's **Copy for Claude** flattens exactly what is on
screen to fixed-width text via the pure, tested `insightsReport.ts` (own one-decimal
`pct()`, absolute local-time stamps, states what is *not* included). Phase 84.E put
`InsightsClaudeCost.tsx` under the Claude tab's quota bars: `/claude-usage` tokens by
hour / model+speed / project / session, priced **on the desktop only** from
`claudePricing.ts` (API list price against a subscription — an estimate, and the UI says
so three times; unknown models cost 0 and are flagged, mixed rows show a "~" blended
rate). `fmtBytes`/`fmtBps` moved to `insightsFmt.ts` so the new tabs share them.

**`HygienePanel.tsx` (159)** — the Monitor's Cleanup tab. Surfaces the two server-side
leaks Yossi hit (duplicate ymux port-watchers, orphaned claude sessions) from the
daemon's `/hygiene` endpoint, and reaps the safe ones via `/hygiene/kill`. Phase 86: a
port-watcher row also carries `orphan` (ppid=1, its SSH channel gone), rendered like a
duplicate and reaped by the same button — "Kill duplicates & orphans".

**`PortsWindow.tsx` (267)** — detect-only plus click-to-forward. The remote watcher
reports a LISTEN port → a row appears with **[Forward]** → the backend opens the tunnel
(with a TCP sanity probe first, so a dead bind never reaches the browser) → the row
flips to **[Open] [Stop]**. Stop tears the tunnel down; the row reverts to detected-only,
or disappears when `port.closed` fires.

**`MobilePairing.tsx` (428)** — the Monitor's Mobile tab. Drives the nginx-proxy install
and the daemon's pairing endpoints via the `mobile_pairing_*` commands. Host and port are
used **only** to render the URL card.

## Agent surface

**`NotificationCenter.tsx` (170)** — unifies the two notification streams (OSC 9/99/777
from terminals, RPC/agent notifications from Claude hooks) into one filterable,
read-aware timeline. The item list and read set live in `App.tsx`; this component is
presentational and owns only the active filter. Each item carries its originating pane
when known, so a click lands on the exact pane.

**`FeedPanel.tsx` (191)** — the allow/deny cards. The feed mixes cards from every
workspace and session, so each card resolves its owning workspace (by `pane_id` →
layout, falling back to `workspace_id`) and can be filtered or grouped by it. Kind /
subkind / state codes are translated, falling back to the raw code for any value without
a key.

**`TicketsPanel.tsx` (366)** + **`TicketModal.tsx` (354)** — workspace-scoped ticket
list, and the dialog that finalizes a captured element into a ticket on disk. **The
capture came from an untrusted page**, so the element HTML is rendered as **text, never
as markup**, and the preview is collapsed by default.

**`ClaudeUsageIndicator.tsx` (181)** + **`claudeUsageFmt.ts` (120)** — the always-visible
subscription-usage chip. With room it shows session · week · top model; narrow, it
collapses to the single most-critical metric with the rest in the tooltip, one per line,
reset times converted to the viewer's **local** timezone.

## Per-workspace management

**`AddonsWindow.tsx` (81)** wraps **`AddonsTab.tsx` (124)** — add-ons live on the remote,
so they are managed per workspace, opened from the workspace's right-click menu and from
the Insights monitor's install prompt. **`YmuxToolsTab.tsx` (126)** is the same shape for
skills. Both are self-contained specifically so they do not bloat `SettingsModal`.

**`DiffPane.tsx` (341)** — on mount it tells the backend the persisted source (or
`Working`), which restarts the per-pane watcher task; the watcher emits
`diff-pane-updated` and this filters by `pane_id` and re-renders.

**`HelpPane.tsx` (96)** — renders bundled markdown (currently ssh-key-setup) keyed by
topic and UI language, with a Copy button on every fenced block.

**`components/PopoutTerminal.tsx` (136)** — the pop-out terminal window. Ctrl+wheel font
zoom applies to popouts only (the grid stays Settings-driven); all open popouts share one
zoom level, synced via the `popout:zoom` event and persisted in localStorage.

## Invariants

- **Rule #5** — no `any`.
- Content from a page, a remote file, or a transcript is **untrusted**: render as text,
  or sanitize. `TicketModal` and `MarkdownViewer` are the two reference implementations.
- A panel that can float must go through `PanelSurface`, not its own chrome.
- Insights payloads must stay shape-compatible between the remote daemon and
  `insights_local.rs` — this panel parses one shape.

## Read the source when

You need a component's exact props, the Insights JSON field names, or the ticket schema.
The backends are in `backend-panes.md` and `backend-remote.md`; the daemon is in
`server-go.md`.
