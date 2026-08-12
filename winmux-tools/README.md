# winmux-tools

A modular home for Claude Code add-ons that winmux can install per-workspace.
Split in two:

## `statuslines/`

Modular Claude Code status-line components. Claude allows exactly one
`statusLine` command, so [`compose.js`](statuslines/compose.js) is the single
entry point: it reads [`config`](statuslines/config), runs the enabled modules
from `modules/` in order, and joins their output. This decomposes the old
monolithic `~/.claude/sabra/statusline.js` (which mixed a context meter + the
🌵 badge) into independent, toggleable pieces.

Modules (`(data, cfg) => string`, `data` = Claude's status-line JSON):

| module | shows |
|---|---|
| `ctx-meter` | context-window usage bar (legacy `SABRA_CTX*` toggles) |
| `model-dir` | model name + shortened cwd |
| `sabra-badge` | 🌵 sabra mode from `~/.claude/sabra.state` (`SABRA_BADGE`) |
| `turn-ticker` | **session Ticker** — current turn elapsed + rolling avg (`WINMUX_TICKER`) |

### Session Ticker (GitHub issue #4)

Shows `⏱ 3:10 · avg 40s` — how long the current turn has run vs. the session's
average past turn. An honest "running this long, usually takes that long"
signal, not a fabricated ETA. Turn boundaries come from
[`hooks/turn-state.js`](statuslines/hooks/turn-state.js), wired to Claude's
lifecycle hooks (it fires in every permission mode, unlike `pre-tool-use`):

```
UserPromptSubmit  →  turn-state.js start        (stamp turn start)
Stop              →  turn-state.js end          (fold duration into the mean)
SessionEnd        →  turn-state.js session-end  (drop the session's state)
```

State lives at `~/.claude/winmux-tools/turn-state/<session_id>.json`. Turns
under 2s (text-only) are excluded from the average.

The same turn signals also feed a winmux **chrome ticker** (a per-pane label in
the app), via winmux's own `UserPromptSubmit`/`Stop` hooks — see the backend.

## `skills/`

A registry the user drops their own skills into; winmux installs them
per-workspace onto each environment's `~/.claude/skills/` (local, or remote
over SFTP), modeled on winmux's existing Add-on manager.
