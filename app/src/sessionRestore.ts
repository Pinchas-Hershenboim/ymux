// Phase 80: session restore — remember which tmux session each SSH pane was attached
// to, so the next app start can re-attach it instead of showing [Connect].
//
// Rationale for localStorage (not settings.json / workspaces.json): this is
// per-MACHINE, high-churn session state — the same class as the floating-
// window rects and the sidebar width, which DECISIONS.md already sends here
// deliberately to keep Rule #7's atomic-write surface small. workspaces.json
// stays the layout's source of truth; this map is a disposable hint. Losing
// it costs one click, never data.
//
// The stored name is the REAL session name the backend reports through
// `pane_persistence_list` after a successful connect — not a frontend
// re-derivation of `sanitize_tmux_session_name`. A pane connected through the
// tmux picker (or with a title-derived name, Phase 23.I) keeps whatever name
// it actually has, and the Rust naming rules stay in one place.
//
// Non-tmux panes are deliberately NOT remembered: a plain SSH shell leaves
// nothing alive on the server, so reconnecting it would produce an empty
// prompt rather than a restored session. Those keep their [Connect] button.

const KEY = "winmux.paneSessions.v1";

// Entries older than this are dropped on read. A tmux session the user
// hasn't touched in two weeks is almost certainly gone (server reboot,
// manual kill) — pruning keeps the map from growing forever on a machine
// whose panes come and go.
const MAX_AGE_MS = 14 * 24 * 60 * 60 * 1000;

type Entry = { tmux: string; ts: number };
type Store = Record<string, Entry>;

function read(): Store {
  try {
    const raw = localStorage.getItem(KEY);
    if (!raw) return {};
    const parsed = JSON.parse(raw) as unknown;
    if (!parsed || typeof parsed !== "object") return {};
    const now = Date.now();
    const out: Store = {};
    for (const [paneId, v] of Object.entries(parsed as Record<string, unknown>)) {
      if (!v || typeof v !== "object") continue;
      const e = v as Partial<Entry>;
      if (typeof e.tmux !== "string" || !e.tmux) continue;
      if (typeof e.ts !== "number") continue;
      if (now - e.ts > MAX_AGE_MS) continue;
      out[paneId] = { tmux: e.tmux, ts: e.ts };
    }
    return out;
  } catch {
    // Corrupt / unavailable storage — behave as if nothing was remembered.
    return {};
  }
}

function write(s: Store): void {
  try {
    localStorage.setItem(KEY, JSON.stringify(s));
  } catch {
    // Quota or private-mode failure — restore silently degrades to [Connect].
  }
}

/** Record that `paneId` is attached to tmux session `tmuxName`. */
export function rememberPaneSession(paneId: string, tmuxName: string): void {
  if (!paneId || !tmuxName) return;
  const s = read();
  s[paneId] = { tmux: tmuxName, ts: Date.now() };
  write(s);
}

/** Drop the hint for `paneId` (pane closed, session killed, or gone stale). */
export function forgetPaneSession(paneId: string): void {
  const s = read();
  if (!(paneId in s)) return;
  delete s[paneId];
  write(s);
}

/** The tmux session `paneId` was last attached to, if still remembered. */
export function getPaneSession(paneId: string): string | null {
  return read()[paneId]?.tmux ?? null;
}

/** Every remembered pane → tmux mapping. Diagnostics only. */
export function allPaneSessions(): Record<string, string> {
  const out: Record<string, string> = {};
  for (const [paneId, e] of Object.entries(read())) out[paneId] = e.tmux;
  return out;
}

/** Drop every hint whose pane no longer exists in any workspace layout. */
export function prunePaneSessions(livePaneIds: Set<string>): void {
  const s = read();
  let changed = false;
  for (const paneId of Object.keys(s)) {
    if (!livePaneIds.has(paneId)) {
      delete s[paneId];
      changed = true;
    }
  }
  if (changed) write(s);
}
