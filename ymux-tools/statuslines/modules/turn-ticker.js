// ymux-tools statusline module: session Ticker (GitHub issue #4).
// Shows the CURRENT TURN's elapsed time + the session's rolling average of
// past turns, e.g. "⏱ 3:10 · avg 40s". This is an honest signal ("running
// this long, usually takes that long") — NOT a fabricated ETA prediction.
//
// Turn boundaries can't come from Claude's `cost.total_duration_ms` (that's
// whole-session incl. idle), so they're written by the turn-state hook
// (../hooks/turn-state.js) into ~/.claude/ymux-tools/turn-state/<sid>.json.
// Toggle with YMUX_TICKER=0.
// Contract: module.exports = (data, cfg) => string
const fs = require('fs');
const os = require('os');
const path = require('path');

const STATE_DIR = path.join(os.homedir(), '.claude', 'ymux-tools', 'turn-state');

const fmtClock = (ms) => {
  const s = Math.max(0, Math.floor(ms / 1000));
  const m = Math.floor(s / 60);
  return `${m}:${String(s % 60).padStart(2, '0')}`;
};
const fmtAvg = (ms) => {
  const s = Math.round(ms / 1000);
  return s < 60 ? `${s}s` : fmtClock(ms);
};

module.exports = (data, cfg) => {
  if (cfg.YMUX_TICKER === '0') return '';
  const sid = data.session_id;
  if (!sid) return '';

  let st;
  try {
    st = JSON.parse(fs.readFileSync(path.join(STATE_DIR, `${sid}.json`), 'utf8'));
  } catch {
    return '';
  }

  const parts = [];
  if (st.turn_started_at) parts.push(`⏱ ${fmtClock(Date.now() - st.turn_started_at)}`);
  if (st.count > 0) parts.push(`avg ${fmtAvg(st.sum_ms / st.count)}`);
  if (!parts.length) return '';

  return `\x1b[35m${parts.join(' · ')}\x1b[0m`;
};
