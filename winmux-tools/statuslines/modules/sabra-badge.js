// winmux-tools statusline module: 🌵 sabra mode badge.
// Extracted verbatim from the sabra statusline.js monolith (lines 40-52).
// Reads the live mode from ~/.claude/sabra.state; honors SABRA_BADGE toggle.
// Contract: module.exports = (data, cfg) => string
const fs = require('fs');
const os = require('os');
const path = require('path');

const STATE = path.join(os.homedir(), '.claude', 'sabra.state');

module.exports = (data, cfg) => {
  if (cfg.SABRA_BADGE === '0') return '';
  try {
    const raw = fs.readFileSync(STATE, 'utf8').replace(/\s/g, '');
    if (!raw) return '';
    let color, mode = raw;
    if (raw === 'lite') color = '\x1b[36m';
    else if (raw === 'full') color = '\x1b[32m';
    else if (raw === 'ultra') color = '\x1b[1;31m';
    else { color = '\x1b[32m'; mode = 'full'; }
    return `${color}🌵 צבר·${mode}\x1b[0m`;
  } catch {
    return '';
  }
};
