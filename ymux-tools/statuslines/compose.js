#!/usr/bin/env node
// ymux-tools statusline COMPOSER.
//
// Claude Code allows exactly one `statusLine` command, so this single script
// runs the enabled modules (from ./config, in order) and joins their non-empty
// output with two spaces. This is the decomposition of the old sabra
// statusline.js monolith: each concern is now a module under ./modules/.
//
// Config precedence (later wins): ~/.claude/sabra.conf (legacy toggles the
// ctx/badge modules still read) then ./config (ymux-tools module list).
const fs = require('fs');
const os = require('os');
const path = require('path');

const HERE = __dirname;
const SABRA_CONF = path.join(os.homedir(), '.claude', 'sabra.conf');
const CONF = path.join(HERE, 'config');

const cfg = { MODULES: 'ctx-meter,model-dir,sabra-badge,turn-ticker' };
for (const f of [SABRA_CONF, CONF]) {
  try {
    for (const line of fs.readFileSync(f, 'utf8').split(/\r?\n/)) {
      const m = line.match(/^\s*([A-Z_]+)=(.*)$/);
      if (m) cfg[m[1]] = m[2].trim();
    }
  } catch {}
}

let input = '';
try { input = fs.readFileSync(0, 'utf8'); } catch {}
let data = {};
try { data = JSON.parse(input); } catch {}

const parts = [];
for (const name of (cfg.MODULES || '').split(',').map((s) => s.trim()).filter(Boolean)) {
  try {
    const mod = require(path.join(HERE, 'modules', name + '.js'));
    const out = mod(data, cfg);
    if (out) parts.push(out);
  } catch {}
}

process.stdout.write(parts.join('  ') + '\n');
