// winmux-tools statusline module: model name + shortened cwd.
// Extracted verbatim from the sabra statusline.js monolith (the base `left`).
// Contract: module.exports = (data, cfg) => string   (data = Claude's statusLine JSON)
const os = require('os');
const path = require('path');

module.exports = (data) => {
  const model = (data.model && data.model.display_name) || '?';
  const dir = (data.workspace && data.workspace.current_dir) || data.cwd || '';
  const home = os.homedir();
  let shortDir = dir;
  if (dir && dir.startsWith(home)) shortDir = '~' + dir.slice(home.length);
  if (shortDir.length > 24) shortDir = '.../' + path.basename(dir);
  return `${model} · ${shortDir}`;
};
