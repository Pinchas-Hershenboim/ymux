#!/usr/bin/env node
// winmux-tools statuslines installer (local prototype of the Half B statusLine
// deploy). Copies this statuslines/ folder into ~/.claude/winmux-tools/ and
// wires ~/.claude/settings.json to use it:
//   - statusLine.command -> node <dest>/compose.js
//   - UserPromptSubmit  -> node <dest>/hooks/turn-state.js start
//   - Stop              -> node <dest>/hooks/turn-state.js end
//   - SessionEnd        -> node <dest>/hooks/turn-state.js session-end
//
// MERGE, never clobber: existing hooks (sabra-prompt.js, winmux-cli, etc.) are
// preserved — we only ADD our own entries and only replace a prior winmux-tools
// one. Your old statusLine is backed up in the printed diff and can be restored
// from the timestamped settings.json.bak.
//
// DRY-RUN by default — prints what would change. Pass --apply to write (a .bak
// is made first). Pass --uninstall to remove our entries.
const fs = require('fs');
const os = require('os');
const path = require('path');

const APPLY = process.argv.includes('--apply');
const UNINSTALL = process.argv.includes('--uninstall');

const CLAUDE = path.join(os.homedir(), '.claude');
const SETTINGS = path.join(CLAUDE, 'settings.json');
const SRC = __dirname; // winmux-tools/statuslines
const DEST = path.join(CLAUDE, 'winmux-tools', 'statuslines');
const isOurs = (s) => typeof s === 'string' && s.includes('winmux-tools');
const q = (p) => `node "${p}"`;

const composeCmd = q(path.join(DEST, 'compose.js'));
const turnHook = (phase) => q(path.join(DEST, 'hooks', 'turn-state.js')) + ' ' + phase;

function copyDir(src, dest) {
  fs.mkdirSync(dest, { recursive: true });
  for (const e of fs.readdirSync(src, { withFileTypes: true })) {
    if (e.name === 'install.js') continue;
    const s = path.join(src, e.name), d = path.join(dest, e.name);
    if (e.isDirectory()) copyDir(s, d);
    else fs.copyFileSync(s, d);
  }
}

// Ensure a hook entry for `event` running `command` exists exactly once (ours).
function wireHook(hooks, event, command) {
  const arr = (hooks[event] = hooks[event] || []);
  for (const group of arr) {
    (group.hooks || []).forEach((h) => { if (isOurs(h.command)) h._drop = true; });
    group.hooks = (group.hooks || []).filter((h) => !h._drop);
  }
  // prune now-empty groups we emptied
  hooks[event] = arr.filter((g) => (g.hooks || []).length > 0);
  if (!UNINSTALL) {
    hooks[event].push({ hooks: [{ type: 'command', command }] });
  }
}

function main() {
  let settings = {};
  try { settings = JSON.parse(fs.readFileSync(SETTINGS, 'utf8')); } catch {}

  const before = JSON.stringify(settings.statusLine || null);
  const priorStatus = settings.statusLine;

  if (UNINSTALL) {
    if (isOurs(priorStatus && priorStatus.command)) delete settings.statusLine;
  } else {
    if (priorStatus && !isOurs(priorStatus.command)) {
      console.log('NOTE previous statusLine (kept in the .bak):', priorStatus.command);
    }
    settings.statusLine = { type: 'command', command: composeCmd, padding: 0 };
  }

  settings.hooks = settings.hooks || {};
  wireHook(settings.hooks, 'UserPromptSubmit', turnHook('start'));
  wireHook(settings.hooks, 'Stop', turnHook('end'));
  wireHook(settings.hooks, 'SessionEnd', turnHook('session-end'));

  console.log(`\n${APPLY ? 'APPLYING' : 'DRY-RUN (pass --apply to write)'}${UNINSTALL ? ' [uninstall]' : ''}`);
  console.log('  copy   :', SRC, '->', DEST);
  console.log('  statusLine:', before, '->', JSON.stringify(settings.statusLine || null));
  console.log('  hooks  : UserPromptSubmit/Stop/SessionEnd += winmux-tools turn-state');

  if (!APPLY) { console.log('\n(no changes written)'); return; }

  if (!UNINSTALL) copyDir(SRC, DEST);
  try {
    if (fs.existsSync(SETTINGS)) {
      fs.copyFileSync(SETTINGS, `${SETTINGS}.bak-${Date.now()}`);
    }
  } catch {}
  const tmp = `${SETTINGS}.tmp`;
  fs.writeFileSync(tmp, JSON.stringify(settings, null, 2));
  fs.renameSync(tmp, SETTINGS); // atomic
  console.log('\nwritten. restart Claude Code (or open a new session) to pick up the statusline.');
}

main();
