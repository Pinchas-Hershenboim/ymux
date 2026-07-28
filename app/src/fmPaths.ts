// Phase 80.1: file manager — remember the last directory each column was showing, per
// workspace, so re-opening the file manager lands where the user left off
// instead of snapping back to $HOME.
//
// localStorage for the same reason as sessionRestore.ts / the floating-window
// rects: this is per-MACHINE UI state (a Windows path on THIS box, a remote
// path on THIS workspace's host), it changes on every navigation click, and
// losing it costs one click. workspaces.json stays the layout's source of
// truth and keeps its atomic-write surface small (Rule #7).
//
// Keyed by workspace id: the same pane in two workspaces browses two different
// hosts, and the local column usually tracks whatever project that workspace
// is about.

const KEY = "winmux.fm.paths.v1";

export type FmPaths = { local?: string; remote?: string };
type Store = Record<string, FmPaths>;

function read(): Store {
  try {
    const raw = localStorage.getItem(KEY);
    if (!raw) return {};
    const parsed = JSON.parse(raw) as unknown;
    if (!parsed || typeof parsed !== "object") return {};
    const out: Store = {};
    for (const [wsId, v] of Object.entries(parsed as Record<string, unknown>)) {
      if (!v || typeof v !== "object") continue;
      const e = v as Partial<FmPaths>;
      const entry: FmPaths = {};
      if (typeof e.local === "string" && e.local) entry.local = e.local;
      if (typeof e.remote === "string" && e.remote) entry.remote = e.remote;
      if (entry.local || entry.remote) out[wsId] = entry;
    }
    return out;
  } catch {
    // Corrupt / unavailable storage — behave as if nothing was remembered.
    return {};
  }
}

/** Last-known local/remote directories for a workspace ({} if none). */
export function loadFmPaths(workspaceId: string): FmPaths {
  if (!workspaceId) return {};
  return read()[workspaceId] ?? {};
}

/** Remember the directories a workspace's file manager is showing. */
export function saveFmPaths(workspaceId: string, paths: FmPaths): void {
  if (!workspaceId) return;
  const entry: FmPaths = {};
  if (paths.local) entry.local = paths.local;
  if (paths.remote) entry.remote = paths.remote;
  const s = read();
  const prev = s[workspaceId];
  if (prev && prev.local === entry.local && prev.remote === entry.remote) return;
  if (!entry.local && !entry.remote) delete s[workspaceId];
  else s[workspaceId] = entry;
  try {
    localStorage.setItem(KEY, JSON.stringify(s));
  } catch {
    // Quota / private mode — the file manager just opens at $HOME next time.
  }
}
