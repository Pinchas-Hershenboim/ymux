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

const KEY = "ymux.fm.paths.v1";

export type FmPaths = { local?: string; remote?: string };
type Store = Record<string, FmPaths>;

// Shape gates, applied on READ. Nothing here is a security boundary against a
// user browsing their own disk — the file manager does that by design. What
// they stop is a path this app never produced being replayed UNATTENDED at
// mount: `\\host\share` would make Windows open an outbound SMB handshake
// (and leak a NetNTLM challenge) before the user clicked anything. A local
// path must be host-absolute, a remote path POSIX-absolute; anything else
// is dropped and the column opens at $HOME. Reaching a UNC share still works
// — it just takes an explicit navigation click, which is where it belongs.
//
// macOS port: a local path is drive-absolute (`C:\…`, Windows) OR
// POSIX-absolute (`/Users/…`, mac/Linux). The leading-`//` exclusion is what
// keeps the UNC rule above intact — `\\host\share` and `//host/share` are the
// same replay, and neither is a path this app ever hands out.
const LOCAL_OK = /^(?:[A-Za-z]:[\\/]|\/(?![\\/]))/;
const REMOTE_OK = /^\//;

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
      if (typeof e.local === "string" && LOCAL_OK.test(e.local)) entry.local = e.local;
      if (typeof e.remote === "string" && REMOTE_OK.test(e.remote)) entry.remote = e.remote;
      if (entry.local || entry.remote) out[wsId] = entry;
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
    // Quota / private mode — the file manager just opens at $HOME next time.
  }
}

/** Last-known local/remote directories for a workspace ({} if none). */
export function loadFmPaths(workspaceId: string): FmPaths {
  if (!workspaceId) return {};
  return read()[workspaceId] ?? {};
}

/**
 * Remember the directories a workspace's file manager is showing.
 *
 * MERGE, not replace: an empty/absent field means "no opinion", never "forget
 * it". The two columns resolve at different times — local is a local syscall,
 * remote is an SSH round-trip — so a naive replace would persist
 * `{local, remote: ""}` in the gap and wipe a perfectly good remote path
 * (and a workspace with no SSH would wipe it on every open).
 */
export function saveFmPaths(workspaceId: string, paths: FmPaths): void {
  if (!workspaceId) return;
  const s = read();
  const prev = s[workspaceId] ?? {};
  const entry: FmPaths = { ...prev };
  if (paths.local) entry.local = paths.local;
  if (paths.remote) entry.remote = paths.remote;
  if (prev.local === entry.local && prev.remote === entry.remote) return;
  if (!entry.local && !entry.remote) return;
  s[workspaceId] = entry;
  write(s);
}

/**
 * Drop every entry whose workspace no longer exists.
 *
 * No TTL here (unlike sessionRestore.ts): the live workspace list is an exact
 * answer, so nothing has to be guessed from age. Remote paths carry real
 * context about a host's layout — they shouldn't outlive the workspace that
 * justified storing them.
 */
export function pruneFmPaths(liveWorkspaceIds: Set<string>): void {
  const s = read();
  let changed = false;
  for (const wsId of Object.keys(s)) {
    if (!liveWorkspaceIds.has(wsId)) {
      delete s[wsId];
      changed = true;
    }
  }
  if (changed) write(s);
}
