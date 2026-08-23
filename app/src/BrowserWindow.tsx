import { createEffect, createSignal, Show } from "solid-js";
import type { Workspace } from "./types";
import { t } from "./i18n";
import {
  clampToViewport,
  makeWindowControls,
  ResizeHandles,
  type Geometry,
} from "./floatingWindow";
import { IconGlobe, IconClose, IconExternalLink } from "./icons";
import {
  BrowserChromeBody,
  createBrowserChrome,
  CHROME_PORTBAR_H,
  CHROME_TABS_H,
  type DetectedPort,
  type ForwardInfo,
} from "./BrowserChrome";

// Phase 53 → 60 → 62.C: workspace-level Browser floating window.
//
// Phase 62.C reframed the browser as what it actually is: a window onto
// the services running ON THE REMOTE SERVER, reached through the SSH
// tunnel. The free-form URL bar is gone. Instead the user picks one of
// the ports the remote port-watcher has detected, optionally types a
// path, and the window forwards that remote port on demand and points
// the native child Webview at http://127.0.0.1:<local_tunnel_port>/<path>
// (127.0.0.1, not localhost — see item F note in go()). External
// browsing is intentionally not offered here.
//
// The native child Webview (managed by `workspace_browser` on the Rust
// side) paints above the SLOT and only the slot — the chrome around it
// (header + port bar + bottom strip + resize handles) stays HTML and
// clickable. When no URL is loaded yet the Webview is hidden so the
// in-slot empty-state / hint shows through.
//
// Phase 85.C: this file is now only the FLOATING SHELL — header drag,
// resize handles, persisted geometry, and the pop-out button. Everything
// inside (tabs, port bar, Go, Dev Mode, the Web Inspector, the native
// webview lifecycle) moved to `BrowserChrome.tsx`, which the popped-out
// OS window renders too. See the header comment there for the seam.

interface Props {
  open: boolean;
  /** The active workspace — its id keys the Webview + persistence. */
  workspace: Workspace | null;
  onClose: () => void;
  /** Pop the Browser out into its own OS window. */
  onPopOut: () => void;
  /** Lets the window re-call show() on modal-close transitions. */
  anyModalOpen: () => boolean;
  /** Remote ports detected on this workspace's server (live). */
  detectedPorts: DetectedPort[];
  /** Forwards already open for this workspace (remote→local mapping). */
  forwards: ForwardInfo[];
  /** Ensure the remote port-watcher is running + refresh the snapshot. */
  onEnsurePorts: (workspaceId: string) => void;
  /** Open (or reuse) a forward for a remote port; resolves to the local
   *  tunnel port the Webview should hit. */
  onStartForward: (remotePort: number) => Promise<number>;
}

const DEFAULT_GEOMETRY: Geometry = { x: 120, y: 80, w: 1100, h: 700 };
const MIN_W = 480;
const MIN_H = 320;
/** Header (32) + tab bar (26) + port bar (32). Must match the CSS
 *  heights. Beta.3: tab bar added between the header and the port bar. */
const CHROME_HEADER_H = 32;
const CHROME_TOP_PX = CHROME_HEADER_H + CHROME_TABS_H + CHROME_PORTBAR_H;
/** Bottom strip that keeps the resize grip clear of the Webview. */
const CHROME_BOTTOM_PX = 16;
/** Horizontal inset so the native Webview clears the left/right resize
 *  handles (native content paints above HTML). Matches .fw-resize width. */
const CHROME_SIDE_PX = 6;

const GEOM_KEY = (id: string) => `ymux.workspace-browser-geometry.${id}`;

function loadGeometry(workspaceId: string): Geometry {
  // Phase 64 (N): clamp to the viewport (stored OR default) for small
  // screens — the window must stay fully on-screen.
  try {
    const raw = localStorage.getItem(GEOM_KEY(workspaceId));
    if (raw) {
      const parsed: unknown = JSON.parse(raw);
      if (
        parsed &&
        typeof parsed === "object" &&
        typeof (parsed as Geometry).x === "number" &&
        typeof (parsed as Geometry).y === "number" &&
        typeof (parsed as Geometry).w === "number" &&
        typeof (parsed as Geometry).h === "number"
      ) {
        return clampToViewport(parsed as Geometry, MIN_W, MIN_H);
      }
    }
  } catch {
    // Corrupt entry — fall through to default.
  }
  return clampToViewport(DEFAULT_GEOMETRY, MIN_W, MIN_H);
}

function saveGeometry(workspaceId: string, g: Geometry): void {
  try {
    localStorage.setItem(GEOM_KEY(workspaceId), JSON.stringify(g));
  } catch {
    // Quota or private mode — ignore.
  }
}

/** The rect the native Webview should occupy = window minus chrome. */
function slotRect(g: Geometry): Geometry {
  return {
    x: g.x + CHROME_SIDE_PX,
    y: g.y + CHROME_TOP_PX,
    w: Math.max(1, g.w - 2 * CHROME_SIDE_PX),
    h: Math.max(1, g.h - CHROME_TOP_PX - CHROME_BOTTOM_PX),
  };
}

export function BrowserWindow(p: Props) {
  const [geom, setGeom] = createSignal<Geometry>(DEFAULT_GEOMETRY);

  // Reload persisted geometry on a real workspace change. The rest of
  // the per-workspace reload (tabs, port, path) lives in the chrome.
  let lastWsId: string | null = null;
  createEffect(() => {
    const id = p.workspace?.id;
    if (!id || id === lastWsId) return;
    lastWsId = id;
    setGeom(loadGeometry(id));
  });

  // Persist geometry whenever it changes (keyed write — cheap).
  createEffect(() => {
    const id = p.workspace?.id;
    if (!id) return;
    saveGeometry(id, geom());
  });

  // Mounted unconditionally, OUTSIDE the `<Show>` below: the chrome owns
  // the native-webview lifecycle, and its falling-edge close effect is
  // what hides the webview when this window closes. Inside the `<Show>`
  // it would unmount before it could fire.
  const chrome = createBrowserChrome({
    get open() {
      return p.open;
    },
    get workspace() {
      return p.workspace;
    },
    anyModalOpen: () => p.anyModalOpen(),
    get detectedPorts() {
      return p.detectedPorts;
    },
    get forwards() {
      return p.forwards;
    },
    onEnsurePorts: (id) => p.onEnsurePorts(id),
    onStartForward: (port) => p.onStartForward(port),
    slotRect: () => slotRect(geom()),
    windowLabel: "main",
  });

  // Phase 62 (item 2): shared header-drag + 8-way resize. Both header
  // buttons are drag-guarded, or a click on them would start a drag.
  const { onDragStart, onResizeStart } = makeWindowControls({
    geom,
    setGeom,
    minW: MIN_W,
    minH: MIN_H,
    closeGuardSelector: ".browser-window-x, .browser-window-popout",
  });

  return (
    <Show when={p.open && p.workspace}>
      <div
        class="browser-window"
        style={{
          left: `${geom().x}px`,
          top: `${geom().y}px`,
          width: `${geom().w}px`,
          height: `${geom().h}px`,
        }}
      >
        {/* Phase 62.A (item A): close button last → inline-END corner
            (right in LTR, left in RTL). */}
        <div class="browser-window-header" onMouseDown={onDragStart}>
          <span class="browser-window-title">
            <IconGlobe size={14} />{" "}
            {t("browser.window.title", { workspace: p.workspace!.name })}
          </span>
          <button
            class="browser-window-popout"
            onClick={() => p.onPopOut()}
            title={t("browser.popout.open")}
            aria-label={t("browser.popout.open")}
          >
            <IconExternalLink size={14} />
          </button>
          <button
            class="browser-window-x"
            onClick={p.onClose}
            title={t("common.close")}
            aria-label={t("common.close")}
          >
            <IconClose size={14} />
          </button>
        </div>
        <BrowserChromeBody api={chrome} />
        <div class="browser-window-bottom" />
        {/* Phase 62 (item 2): 4 edges + 4 corners. */}
        <ResizeHandles onStart={onResizeStart} />
      </div>
    </Show>
  );
}
