# YMUX 0.5.0 — what shipped since the last cut, and how to verify it

Baseline: `v0.4.5-beta.1` = `bcaa330` (2026-07-31).
Head at time of writing: `f98e079`.
**151 commits** (85 user-facing: 42 `feat`, 34 `fix`, 1 `perf`, 8 `refactor`).

This is the pre-release test plan. Nothing here has been run against a
live build unless the Status column says so — CI green means syntax, not
behaviour (Rule #14). Work top to bottom: area 1 gates everything else,
because every other test on an upgraded machine runs through the migrated
config dir.

**Legend** — Status: `⛔ blocker` · `🔴 untested, has a written repro` ·
`🟠 untested, no repro written` · `🟢 verified live`

---

## 0. Release-shape blockers (not features — read before testing)

| # | Item | Status |
|---|---|---|
| 0.1 | **Must be cut as 0.5.0, never 0.4.6.** `identifier` moved `com.winmux.app` → `com.ymux.app`, and that is the MSI/NSIS upgrade key. Windows treats 0.5.0 as a *different product*: it installs **side by side**, old winmux stays in Programs and Features with its own Start Menu entry, and the 0.4.5 updater will not offer it in place. No shim can fix this. | ⛔ FOLLOWUPS P0 |
| 0.2 | **Release notes must order the steps:** install YMUX → launch once and confirm workspaces are there (first launch renames `%APPDATA%\winmux` → `ymux`) → *then* uninstall old winmux. Uninstalling first is harmless; **running both at once is not** (see 11.2). | ⛔ blocks publish |
| 0.3 | **`manifest.json` is still on `0.4.5-beta.1`** and still points at `github.com/yyhezkel/winmux/...`. Per RELEASING.md step 5 this is edited *last* — until then existing installs stay on 0.4.5, which is the safe default. | ⛔ blocks publish |
| 0.4 | **Rename shims stay in for this release**, retired one release *after* 0.5.0, as a set: `WINMUX-CHALLENGE` wire tag, `%APPDATA%` + `~/.winmux` migrations, dual env-var spellings, legacy named pipe. Do not "finish the rename" during testing. | ⛔ FOLLOWUPS P1 |
| 0.5 | **tmux sessions vanish server-side after an abrupt client loss** (laptop unplugged). Confirmed by Yossi, `tmux ls` empty *before* his reboot. Every `tmux kill-session` path in the repo was read and excluded — **no mechanism found**. Not caused by this release; decide whether it ships as a known issue. | ⛔ FOLLOWUPS P0, unexplained |

---

## 1. The winmux → YMUX rename + upgrade migration — 1 commit, enormous surface

`90626bb refactor(rename): winmux → YMUX across the repo, with upgrade shims`

Every step below exercises a shim that **only exists on the upgrade path** —
a clean install proves nothing. Run these in order on a machine that already
has winmux 0.4.5 installed and used.

| # | Test | Pass condition | Status |
|---|---|---|---|
| 1.1 | Launch with an existing `%APPDATA%\winmux` present | log line `config dir migrated … (winmux -> ymux rename)`; workspaces, settings and known-hosts all survived | 🔴 |
| 1.2 | Connect to a server provisioned by the **old** build | `bootstrap: migrated remote …` + a working pane. Exercises `~/.winmux`→`~/.ymux` **and** the legacy handshake tag together | 🔴 |
| 1.3 | Fire a Claude hook in that pane | permission card appears **once**. Two cards per tool call = `is_ymux_entry` duplicated instead of replaced the old hook | 🔴 |
| 1.4 | Add-ons → Insights on a host with the old daemon | detect reports the existing install; after Update, `systemctl --user list-units \| grep -c mux` is **1**, not 2 | 🔴 |
| 1.5 | `ymux list-workspaces` from a shell, then the same from a pre-rename `winmux-cli.exe` if one is on PATH | both work — proves the legacy pipe listener | 🔴 |
| 1.6 | Bundle identity | installer is `ymux_0.5.0_*`, Start Menu entry says YMUX, and old winmux entry is **still there** (expected — see 0.1) | 🔴 |

---

## 2. Workspaces are a tree; a project folder IS a workspace — 15 commits ⚠️ BREAKING

`e2d0f17 feat(workspaces)!` is the breaking one: `parent_id` tree replaced the
`ProjectFolder` type outright. Also `4138d33` `74472be` `568c905` `94353d6`
`df9213d` `cbef36e` `d470f50` `f1f5197` `876d17f` `b6f32a4` `7739618`
`191fc75` `0ecdf99` `8700032`.

This is the largest behaviour change in the release and it rewrites
`workspaces.json`. Test it before anything that would be annoying to lose.

| # | Test | Pass condition | Status |
|---|---|---|---|
| 2.1 | Right-click SSH workspace → pin | SFTP browser lists **real** remote dirs | 🔴 |
| 2.2 | Pick a repo | child row nests under it with a folder glyph | 🔴 |
| 2.3 | Select it → open a pane | wizard shows the folder read-only, TMUX/regular, session list contains **only that folder's** sessions; after connect `pwd` is the folder (exercises SSH-cwd injection) | 🔴 |
| 2.4 | Expand worktrees → open one | nests under the folder, `pwd` is the worktree, repo root does **not** appear as a stub | 🔴 |
| 2.5 | `+` → new worktree | its workspace opens **and survives** (the cmux#5032 trap) | 🔴 |
| 2.6 | Delete the parent | confirm dialog **names** the descendants and the live count; all go; directories on the host untouched | 🔴 |
| 2.7 | Disconnected host / non-repo path | clear error, no stuck spinner; git's own message, nothing created | 🔴 |
| 2.8 | Restart | nesting + collapse state persist | 🔴 |
| 2.9 | **Migration**: open a `workspaces.json` still carrying `project_folders` | pins reappear as child workspaces, worktrees still work | 🔴 |
| 2.10 | A folder that becomes a repo later (`7739618`) | there is a way back — re-detect works | 🟠 |

---

## 3. Sidebar redesign — 9 commits

`e0ecdc8` one colour channel / one status column / 26px rows · `5dc8e3f` masthead
scales with the rail · `a0ce0d0` wordmark **is** the collapse control, notif badge
dropped · `f6f588f` pin name alignment · `b16537d` CSS lifted to `sidebar.css` ·
`97c8365` `28a8789` `22bed20` `9052d27` fixes.

| # | Test | Pass condition | Status |
|---|---|---|---|
| 3.1 | Click the wordmark | collapses/expands the rail — there is no separate collapse button any more | 🟠 |
| 3.2 | Collapse and expand at several widths | masthead stays one centred row and scales; nothing clips | 🟠 |
| 3.3 | Hebrew workspace names | name alignment pinned to the rail, **not** to the name's own script | 🟠 |
| 3.4 | Cold start with an unreachable host (`22bed20`) | no stale red error that only clears on manual refresh | 🟠 |
| 3.5 | Deep tree, rapid expand/collapse (`28a8789`) | no crash — the recursion guard was shared mutable state across renders | 🟠 |
| 3.6 | Any sidebar crash | lands in `debug.log` (`97c8365`) | 🟠 |

---

## 4. Tickets + Browser Dev Mode — 12 commits

`873e060` Dev Mode toggle + right-click element capture · `3f9b36e` `ymux-ticket:`
navigation bridge · `3025f5e` app-local store + 4 Tauri commands · `59cdae3` capture
modal · `7f7445b` tickets panel · `c66703c` derive project + store inside it ·
`12cf043` **Store enum — tickets land on the machine the agent runs on** ·
`e0c32d6` UI for host / override / copy-for-later · `76ec575` element screenshot via
SVG foreignObject · `95c0f31` remote-command layer · `8478e8f` `6612554` fixes.

| # | Test | Pass condition | Status |
|---|---|---|---|
| 4.1 | Sidebar 🌐 → open workspace browser → Dev Mode on → right-click an element | capture modal opens with the element identified | 🟠 |
| 4.2 | Save the ticket | lands on the machine the **agent** runs on, not the desktop (this is the v2 fix) | 🟠 |
| 4.3 | Element screenshot | the SVG foreignObject render actually produces an image | 🟠 |
| 4.4 | Ticket with empty `cwd` (`6612554`) | project derived from the tmux pane instead | 🟠 |
| 4.5 | Host override / copy-for-later paths (`e0c32d6`) | each transport produces a readable ticket | 🟠 |
| 4.6 | Tickets panel → filter / detail / export | works in drawer, float and fullscreen | 🟠 |

---

## 5. RTL / bidi — 3 commits, regression-prone

`1643fbf` restore `tuiOwnsBidi` (silently lost in merge `bcaa330` — **18 days gone**) ·
`ac71bfb` TUI-owns-bidi **off by default**, its premise expired and it broke local panes ·
`fd1af3d` resolve bidi base direction **per line** instead of forcing LTR.

This area has already regressed once through a silent merge loss. Test both
pane classes — the settings are split per class (local Windows vs Linux).

| # | Test | Pass condition | Status |
|---|---|---|---|
| 5.1 | Hebrew output in a **local Windows** pane | reads correctly; TUI-owns-bidi is off by default here | 🟠 |
| 5.2 | Hebrew output in a **Linux/SSH** pane | reads correctly | 🟠 |
| 5.3 | Mixed Hebrew/English lines | base direction resolved per line, not forced LTR | 🟠 |
| 5.4 | A TUI with a status bar containing a few Hebrew letters | status bar does **not** flip | 🟠 |
| 5.5 | Settings → RTL | the per-pane-class split is visible and each side takes effect | 🟠 |

> ⚠️ 22 further RTL commits sit unmerged on `claude/zellinj-tmux-windows-3a5afe`
> (bidi-override, pane-class direction rule, TUI-row dominance vote). **Excluded
> from 0.5.0 by decision.** If RTL testing surfaces bugs here, check that branch
> before writing new fixes — it may already be solved there.

---

## 6. File Manager — streaming uploads + unified transfer UI — 7 commits

`0112b91` 81.A stream uploads + unified progress/cancel · `3ae896f` 81.B transferStore
+ root listener · `b9f875f` 81.C `<TransferBar>` · `d9c7a0d` 81.D mount it ·
`b2dc708` 81.E **stop shipping picked files through the IPC bridge** · `7412a38`
`4f58002` refactors.

| # | Test | Pass condition | Status |
|---|---|---|---|
| 6.1 | Upload a **large** file (≥ 100 MB) | streams; progress strip moves; the IPC bridge is not carrying the bytes | 🟠 |
| 6.2 | Cancel mid-transfer | actually stops, no orphaned partial on the remote | 🟠 |
| 6.3 | Several transfers at once | TransferBar shows them all, per-item progress | 🟠 |
| 6.4 | Download direction | same three checks | 🟠 |

---

## 7. Fonts — install without admin — 5 commits

`e646383` install missing fonts **per-user, no admin** · `2dd0f3b` free-text family
field with a real availability probe · `fca8b54` flag not-installed + read HKCU ·
`400c3bd` use the family name the font declares · `73505cc` de-dupe the picker.

| # | Test | Pass condition | Status |
|---|---|---|---|
| 7.1 | Settings → font picker | no duplicates, no noise; not-installed fonts flagged | 🟠 |
| 7.2 | Install a Nerd Font as a **non-admin** user | succeeds, no UAC prompt, terminal picks it up | 🟠 |
| 7.3 | Type a family name by hand | availability probe answers correctly | 🟠 |
| 7.4 | A font whose file name ≠ declared family | picker shows the **declared** family | 🟠 |
| 7.5 | **Known gap:** there is no uninstall | 27 MB / 6 files must be hand-deleted from `%LOCALAPPDATA%\Microsoft\Windows\Fonts` + HKCU | 🔴 FOLLOWUPS P1 — ship as known issue? |

---

## 8. WSL Smart Install — 8 commits

`65d6fdc` real install path: preflight, UAC consent, reboot · `f3907fc` **install Claude
Code inside WSL — the chain never did** · `adfb237` capability layer + tmux session
restore for WSL workspaces · `9bd44bc` feed `wsl_exec` scripts on stdin (argv lost every
shell variable) · `fc87078` CreateWslUser reported success without creating a user ·
`fd1f537` capture elevated output · `6753759` stop reporting failure as success ·
`5673686` `reset-local-setup.ps1` to re-test the wizard.

Four of these are "it silently lied about succeeding" fixes, so the thing to
verify is the **failure** paths, not just the happy one. `reset-local-setup.ps1`
exists to make this repeatable.

| # | Test | Pass condition | Status |
|---|---|---|---|
| 8.1 | Wizard on a machine with **no** WSL | preflight → UAC consent → install → reboot prompt, all honest | 🟠 |
| 8.2 | After reboot, resume | user actually created (`fc87078`), Claude Code actually installed inside WSL (`f3907fc`) | 🟠 |
| 8.3 | Force a failure (deny UAC) | reported as a **failure**, with the elevated output shown (`6753759` + `fd1f537`) | 🟠 |
| 8.4 | WSL workspace, close and reopen | tmux session restored (`adfb237`) | 🟠 |
| 8.5 | Env vars inside a WSL setup script | survive (`9bd44bc` — argv used to eat them) | 🟠 |

> ⚠️ The unmerged `zellinj` branch **removes WSL from the UI entirely** and migrates
> WSL workspaces to local. Excluded from 0.5.0 — so WSL still ships here and must
> still work.

---

## 9. ymux-tools: statuslines, Ticker, skills registry — 6 commits

`2297285` modular statuslines + session Ticker (issue #4) · `f204555` chrome Ticker
backend + frontend · `bc505d2` skills registry backend · `bbb0fa0` skills registry UI
(Settings panel) · `63af376` statusline installer · `bf51952` inject `YMUX_PANE_ID`
into local panes.

| # | Test | Pass condition | Status |
|---|---|---|---|
| 9.1 | Install a statusline | appears in a real Claude pane | 🟠 |
| 9.2 | Chrome Ticker | turn timing shows and updates | 🟠 |
| 9.3 | Settings → skills registry | lists, installs, reflects state | 🟠 |
| 9.4 | A **local** (non-SSH) pane | `YMUX_PANE_ID` is set (`bf51952`) — statuslines depend on it | 🟠 |

---

## 10. Notifications + hooks — 4 commits

`1c6f712` quiet-mode routing — sound+blink only for meaningful hooks · `1fd4826` stop
flooding the feed with auto-approve audits; wire Stop pulse · `6ca9efc` honor an explicit
Stop-sound opt-in even when ymux is focused · `21eea83` silent-ack observability hooks
instead of printing raw RPC payload.

| # | Test | Pass condition | Status |
|---|---|---|---|
| 10.1 | Claude runs with auto-approve on | feed is **not** flooded with audit entries | 🟠 |
| 10.2 | Claude finishes a turn (Stop) | sidebar row pulses amber — needs a real pane + this build | 🔴 FOLLOWUPS P1 |
| 10.3 | Stop-sound opt-in, ymux focused | sound still fires (`6ca9efc`) | 🟠 |
| 10.4 | Quiet mode on | sound+blink only for meaningful hooks, not every event | 🟠 |
| 10.5 | `session-start` / `notification` hooks | silent-ack, nothing printed to hook-debug.log | 🟠 |

> The **pane-border** pulse (OSC 9/99/777 from Claude Code's bell) is a separate
> path and was **not** touched. If that's the "blink" you mean, 10.2 won't cover it.

---

## 11. Resilience + persistence — 5 commits

`6af56ca` **survive a full network drop — per-pane reconnect, resilient SFTP** ·
`0711d18` workspace save is a **three-way merge**, not a whole-file overwrite ·
`842d357` a UTF-8 BOM bricked `workspaces.json` and disabled persistence ·
`06914a6` dedup the connect-time bootstrap + make CLI skew visible ·
`38ef9a4` insights usage effect re-triggered itself on failure, flooding IPC.

| # | Test | Pass condition | Status |
|---|---|---|---|
| 11.1 | **4 panes on one workspace, single full network drop** | all four retry and reattach. Before the fix three were abandoned without a single attempt. No JS test runner in this repo — this needs a real build and a real drop | 🔴 FOLLOWUPS P1 |
| 11.2 | Two ymux builds sharing `%APPDATA%\ymux` | the three-way merge (`0711d18`) means the older build no longer strips `parent_id` / `is_project_root` on save. Was P1, now P2 — confirm | 🔴 |
| 11.3 | `workspaces.json` with a UTF-8 BOM | loads; persistence not disabled | 🟠 |
| 11.4 | 3–4 panes opened at once against a stale-CLI server | **exactly one** SFTP stream in `debug.log`, one `bootstrap: COMPLETE — upload verified`, no `.tmp` left in `~/.ymux/bin`, skew banner visible | 🔴 FOLLOWUPS P1 |
| 11.5 | Insights usage failing | no IPC flood (`38ef9a4`) | 🟠 |

---

## 12. Terminal / PTY — 3 commits

`1bcb584` force local Windows shells to UTF-8 (Hebrew was mojibake) · `fe9b840` stop a
bad byte from silencing a pane forever · `82b6c0c` paste was silently denied by WebView2 —
read the clipboard host-side.

| # | Test | Pass condition | Status |
|---|---|---|---|
| 12.1 | Hebrew in a local Windows pane | no mojibake | 🟠 |
| 12.2 | Cat a binary file into a pane | pane recovers; not silenced forever | 🟠 |
| 12.3 | Ctrl+V into a terminal | pastes (was silently denied by WebView2) | 🟠 |

---

## 13. Pane header overflow menu — 2 commits

`6ee80f6` overflow menu for pane header buttons · `eafd907` menu was clipped invisible;
fitter ignored padding.

| # | Test | Pass condition | Status |
|---|---|---|---|
| 13.1 | Split to 6–8 panes, shrink the window | buttons move into the chevron menu **and the menu opens** | 🔴 |
| 13.2 | Same, with Hebrew UI | same | 🔴 |

---

## 14. Session auto-naming (PR #6, merged into this cut) — 3 commits

On a session's **first real prompt**, the `UserPromptSubmit` hook derives
`"<two words> · <YYYY-MM-DD HH:MM>"` into `session-meta.json` as `auto_name`,
and returns it as the display title from then on — it beats `claude_title`,
which Claude rewrites as it goes. Display precedence: `label > auto_name >
claude_title > raw name`.

No hook spec change: this rides the existing v1.3.0 registration, so
**machines already set up do not need to re-run `setup-hooks`.**

| # | Test | Pass condition | Status |
|---|---|---|---|
| 14.1 | New session, first prompt | tmux picker shows `<two words> · <date time>` | 🔴 FOLLOWUPS P1 |
| 14.2 | Keep prompting in the same session | name **does not** change | 🔴 |
| 14.3 | Set a manual label | label wins over auto_name | 🔴 |
| 14.4 | New Claude session in the **same** tmux key | name re-derived | 🔴 |
| 14.5 | Server with an old CLI | fields absent, picker falls back to raw name, nothing breaks | 🔴 |

---

## 15. Build pipeline — 4 commits, not user-facing

`6234dac perf(build)`: stop building the app lib three times, warm the CI cache from
main. Measured on CI: **warm 9m24s → 7m06s**; cold unchanged (15m37s → 16m05s).

| # | Test | Pass condition | Status |
|---|---|---|---|
| 15.1 | Local release build via `build-release.ps1` | both bundles produced | 🟢 **verified** — 6m25s, `ymux_0.5.0_x64_en-US.msi` + `ymux_0.5.0_x64-setup.exe` |
| 15.2 | Rule #13 — current `dist/assets/index-<hash>.js` appears inside `app.exe` | not yet checked | 🔴 |
| 15.3 | Developer-path scrub — `grep -aoc $env:USERNAME app.exe` is 0 | not yet checked | 🔴 |
| 15.4 | Rule #14 — launch the built exe, UI comes up | not yet checked | 🔴 |

---

## What is deliberately NOT in 0.5.0

| Branch | Commits | Contents |
|---|---|---|
| `claude/zellinj-tmux-windows-3a5afe` | 22 | Zellij persistent sessions for native Windows panes · **WSL removed from the UI** + migration to local · 9 further RTL fixes |
| `browser-dev-mode-tickets` | 13 | Phase 54 browser dev-mode — **superseded**, the work landed in main via the `browser-tickets-v2` rewrite. Zombie branch |
| `design-pass-01` | 5 | Design tokens, welcome screen, command palette — **superseded**, all present in main. Zombie branch |

---

## Test status roll-up

| | Count |
|---|---|
| Release-shape blockers | 5 |
| Feature areas | 15 |
| Individual checks | 71 |
| Verified live so far | 1 |
| Open FOLLOWUPS carried in | 38 (2× P0, 8× P1) |
