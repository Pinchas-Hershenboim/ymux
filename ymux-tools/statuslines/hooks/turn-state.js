#!/usr/bin/env node
// ymux-tools turn-state hook — powers the session Ticker (issue #4).
//
//   turn-state.js start        (UserPromptSubmit) — stamp the turn start
//   turn-state.js end          (Stop)             — fold turn duration into mean
//   turn-state.js session-end  (SessionEnd)       — drop the session's state
//
// State is per-session, keyed by session_id, at
// ~/.claude/ymux-tools/turn-state/<sid>.json = { turn_started_at, sum_ms, count }.
// Never blocks the prompt/turn (always exit 0). Reads Claude's hook JSON on stdin.
const fs = require('fs');
const os = require('os');
const path = require('path');

const DIR = path.join(os.homedir(), '.claude', 'ymux-tools', 'turn-state');
const MIN_TURN_MS = 2000; // exclude trivial/text-only turns from the average

const phase = process.argv[2] || '';

let input = '';
try { input = fs.readFileSync(0, 'utf8'); } catch {}
let data = {};
try { data = JSON.parse(input); } catch {}

const sid = data.session_id;
if (!sid) process.exit(0);

const file = path.join(DIR, `${sid}.json`);

const load = () => {
  try { return JSON.parse(fs.readFileSync(file, 'utf8')); } catch { return {}; }
};
const save = (o) => {
  try {
    fs.mkdirSync(DIR, { recursive: true });
    const tmp = `${file}.tmp`;
    fs.writeFileSync(tmp, JSON.stringify(o));
    fs.renameSync(tmp, file); // atomic
  } catch {}
};

if (phase === 'session-end') {
  try { fs.unlinkSync(file); } catch {}
  process.exit(0);
}

const st = load();
st.sum_ms = st.sum_ms || 0;
st.count = st.count || 0;
const now = Date.now();

if (phase === 'start') {
  st.turn_started_at = now;
} else if (phase === 'end') {
  if (st.turn_started_at) {
    const dur = now - st.turn_started_at;
    if (dur >= MIN_TURN_MS) {
      st.sum_ms += dur;
      st.count += 1;
    }
    st.turn_started_at = null;
  }
}

save(st);
process.exit(0);
