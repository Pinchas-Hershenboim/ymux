# YMUX 0.5.0 — what shipped since the last cut, and how to verify it

Baseline: `v0.4.5-beta.1` = `bcaa330` (2026-07-31). Updated 2026-08-20 after the Zellij merge.
Head at time of writing: `f98e079`.
**189 commits** — 151 at first writing plus the 38-commit Zellij chain.

This is the pre-release test plan. Nothing here has been run against a
live build unless the Status column says so — CI green means syntax, not
behaviour (Rule #14). Work top to bottom: area 1 gates everything else,
because every other test on an upgraded machine runs through the migrated
config dir.

The Zellij chain is **merged** (2026-08-20): all three worktrees
(`zellinj-tmux-windows` ⊂ `zellijj-rtl-support` ⊂ `zellijj-commands`) were a
linear chain, so one merge took all 38 commits. Head is **52 commits** ahead of
`origin/main`, 0 behind. Green after the merge: `tsc`, `cargo check`,
`cargo test` **450 passed / 0 failed**, `go vet` + `go test` 10/10 packages.

The merge broke the build on exactly one line — `parse_zellij_sessions` is new
on that branch and predates Phase 81.F, so it omitted `auto_name` from the
`TmuxSessionInfo` initializer. Patched as `auto_name: None`; see area 16.

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

## 5. RTL / bidi — 18 commits, rebuilt from the model up ⚠️ highest-churn area

The 3 commits in the earlier plan became **18**. This was not polish — the model
changed. Every fix below was measured on a real build, and several are
corrections of a fix from earlier the same day.

**What the measurements settled** (`0b40012`, same build, same session):

| pane class | correct mode | what `off` did |
|---|---|---|
| remote (SSH → Linux) | `auto_per_line` | reversed |
| local (Windows ConPTY) | `bidi_reorder` | reversed |

So the setting is now **split per pane class**, and `4426ad4` gives each profile
its own measured default rather than inheriting the old shared value.

| # | Test | Pass condition | Status |
|---|---|---|---|
| 5.1 | Hebrew in a **local Windows** pane, all three RTL modes | each mode is separately testable on an **open** pane — xterm swaps renderers live (`bc12388`). Default is `bidi_reorder` | 🔴 |
| 5.2 | Hebrew in a **remote SSH/Linux** pane, all three modes | default is `auto_per_line`; remote was never broken and must stay that way | 🔴 |
| 5.3 | Ordinary shell output with **one** Hebrew word (`ls`, a Hebrew filename, a note on a long path) | does **not** flip. The dominance vote is TUI-rows-only (`341f931`) — it reached ordinary output once already | 🔴 |
| 5.4 | A TUI status bar with three Hebrew letters | does not flip (`bc9665c`) | 🔴 |
| 5.5 | Direction follows the **pane class**, never what runs inside it (`aeed936`) | run a Hebrew TUI in a remote pane and a Latin one locally — neither changes the rule | 🔴 |
| 5.6 | **Claude** output in a local pane | letters correct **and** not pinned to the left edge (`322655e` normalises visual→logical on the way in) | 🔴 |
| 5.7 | Fire a Claude hook, then **detach and reattach** | the RTL signal survives; a reattach must not clear it (`eaab4bf`) | 🔴 |
| 5.8 | Change any unrelated setting, then reopen Settings | `terminal.rtl` **survives** — `settings_save` replaced the whole document from a stale client copy (`2a7463c`) | 🔴 |
| 5.9 | Toggle one RTL field | the other pane class's switch is **not** disabled — a partial profile write was wiping it (`05d89f2`) | 🔴 |
| 5.10 | **Migration**: settings written by 0.4.5 | each profile gets its measured mode, not the old shared one (`4426ad4`) | 🔴 |

> The two settings bugs (5.8, 5.9) are why RTL "kept regressing" — a correct fix
> was being silently discarded on save. Test them **before** re-reporting any
> RTL bug.

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

## 8. WSL → local migration, then WSL removed — 2 commits ⚠️ DATA-LOSS RISK

`e4f54dc` migrates first, on its own, **because this is the step that can cost a
user every workspace they have**. `Connection` is `#[serde(tag = "type")]` and
lives in `workspaces.json`: deleting the `Wsl` variant while any file in the
wild still contains `"type":"wsl"` makes serde reject the **whole file**, and
`load_from_disk` then refuses every later save.

`137c789` then removes WSL from the UI: the "WSL (tmux)" option, the `W` sidebar
badge, and the eight-step WSL group in the local-setup wizard (chain constant,
checkbox, status summary, 51-line UI block). `finalize_wsl_workspace` was
**converted**, not deleted — it becomes `finalize_local_workspace` and creates a
native Windows workspace with no WSL chain. The name/folder inputs went away
with the WSL block and were re-added as their own section.

| # | Test | Pass condition | Status |
|---|---|---|---|
| 8.1 | **Open a `workspaces.json` that still contains `"type":"wsl"`** | file loads, **every** workspace survives, saves still work. This is the data-loss case | 🔴 |
| 8.2 | A migrated WSL workspace | appears as **local**, opens, and its panes work | 🔴 |
| 8.3 | Open a pre-migration `wsl` workspace in the edit modal | shows as **local** — what `load_from_disk` turns it into — rather than falling through to an error | 🔴 |
| 8.4 | Create-workspace modal | no "WSL (tmux)" option anywhere | 🔴 |
| 8.5 | Sidebar | no `W` badge on any row | 🔴 |
| 8.6 | Local-setup wizard end to end | no WSL group; finalize creates a **native Windows** workspace, and the name/folder inputs are still there and still work | 🔴 |
| 8.7 | Restart after migrating | migration is not re-run, nothing duplicated | 🔴 |

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

## 16. Zellij — session persistence for native Windows panes — 8 commits ⭐ NEW

Native Windows panes were the only class with **no session persistence**, and
that gap is the entire reason WSL existed in this product — everything else WSL
supplied already had a native equivalent. Zellij closes it directly, which is
what makes area 8's removal safe.

Pinned to **zellij 0.44.3**, upstream's own winget package `Zellij.Zellij`
(deliberately *not* the third-party `arndawg.zellij-windows` fork), binary at
`%LOCALAPPDATA%\Zellij\zellij.exe`. Issue #5365 reports detach/reattach broken
on Windows; it does **not** reproduce on 0.44.3.

Verbs, each checked against `zellij 0.44.3 --help` on Windows, now in one
documented block instead of argv at three call sites (`7cbf097`):

| verb | effect |
|---|---|
| `attach -c <name>` | attach-or-create; **also resurrects an EXITED session** |
| `list-sessions -n` | the parsing form — `-s` drops age *and* the EXITED mark |
| `kill-session <name>` | stops a RUNNING session; the serialized copy survives |
| `delete-session <name>` | discards that copy — exited sessions only |

| # | Test | Pass condition | Status |
|---|---|---|---|
| 16.1 | **The gate:** native Windows pane → detach → reattach → type | typing works. This is the whole premise; if it fails, WSL should not have been removed | 🔴 |
| 16.2 | Fresh machine → new-workspace wizard | `InstallZellij` runs via winget, no user knowledge of multiplexers needed; a re-run is a no-op (`winget_already_ok`) | 🔴 |
| 16.3 | Open a pane | **no frame** — neither the border (`pane_frames false`) nor the two plugin rows above/below, which come from zellij's default layout and no config key touches (`bc3f013`) | 🔴 |
| 16.4 | Check the log on pane open | `resolve_zellij_config` says whether the config was **found or not** — "the setting does not work" and "the config was never found" used to look identical (`3709c53`) | 🔴 |
| 16.5 | Run the **portable** exe with `resources\` beside it | config is found next to the running binary, not only via `resource_dir()` | 🔴 |
| 16.6 | Delete `resources/ymux-zellij.kdl` and open a pane | supported state: zellij uses its own config, pane still works | 🔴 |
| 16.7 | Press `Ctrl+p n` in a pane | does **not** split the pane — the default keybinds were live and unlocked (`bc3f013`) | 🔴 |
| 16.8 | Kill a **running** session from the picker | it is gone, **not** resurrectable. The old code sent `delete-session` only when the kill *failed*, so on a live session it came back EXITED (`2b4c9ba`) | 🔴 |
| 16.9 | Close a pane, then look at the picker | its session is still listed with a resurrect badge — deliberate | 🔴 |
| 16.10 | An EXITED session in the picker → trash button | actually buried. Before `dfbe384` the list was **append-only** with no way to act on it | 🔴 |
| 16.11 | Reboot the machine, reopen ymux | sessions survive — the one thing zellij does that tmux cannot | 🔴 |
| 16.12 | Session name in the picker for a zellij session | falls back to the raw name. **Known gap:** zellij carries no `session-meta` join, so `auto_name` is `None` — see the P2 filed 2026-08-20 | 🔴 |
| 16.13 | `pane_kill_session` on a machine with **no** multiplexer installed | reports `multiplexer_missing`, not success. It used to return nothing and the frontend inferred success from "the invoke did not throw" | 🔴 |

---

## Merged in — 2026-08-20

| Branch | Commits | State |
|---|---|---|
| `claude/zellijj-commands-469a3e` | 38 | ✅ merged. Took the whole chain: `zellinj-tmux-windows` ⊂ `zellijj-rtl-support` ⊂ `zellijj-commands`. Both dormant worktrees removed, branches deleted |

Merge cost: 4 conflicts, 3 of them append-only docs. `Sidebar.tsx`, `App.tsx`
and `PaneView.tsx` all auto-merged clean despite both sides touching them.
One real break — `parse_zellij_sessions` missing `auto_name` — caught by
`cargo check`, not by a test.

## Zombie branches — superseded, safe to delete

| Branch | Commits | Why |
|---|---|---|
| `browser-dev-mode-tickets` | 13 | Phase 54 browser dev-mode. The work landed in main via the `browser-tickets-v2` rewrite |
| `design-pass-01` | 5 | Design tokens, welcome screen, command palette — all present in main |

---

## Test status roll-up

| | Count |
|---|---|
| Release-shape blockers | 5 |
| Feature areas | 16 |
| Individual checks | **100** (5 release-shape + 95 feature) |
| Verified live so far | 1 |
| Open FOLLOWUPS carried in | 40 (2× P0, 8× P1) |
| **Status** | ✅ all branches merged — **ready to build and test** |

Biggest areas by check count: **Zellij (13)**, **RTL (10)**, **workspace tree
(10)**. Those three plus the rename (6) and the WSL migration (7) are the ones
that can lose user data or break the upgrade path — do them first.
