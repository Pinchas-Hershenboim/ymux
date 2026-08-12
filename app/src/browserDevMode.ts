// Browser Dev Mode — right-click an element in the workspace browser to
// capture it as a ticket.
//
// Everything specific to Dev Mode lives here rather than in
// BrowserWindow.tsx, so the browser component keeps its existing shape:
// it gains one signal, one toolbar button and one re-inject effect, and
// nothing about tabs / port+path / navigation changes.
//
// How the capture gets back to the app
// ------------------------------------
// The workspace browser is a NATIVE child webview pointed at an external
// (tunnelled) page. It has no Tauri IPC — deliberately, since injecting
// it would expose the app's command surface to whatever service the user
// happens to be viewing. Tauri 2.10's WebviewBuilder has no `on_ipc`
// either. The one channel that exists is navigation, so the injected
// script hands its payload over as a URL:
//
//     location.href = "winmux-ticket:<base64url of JSON>"
//
// `workspace_browser.rs::handle_ticket_navigation` decodes it, emits
// `browser:ticket-captured`, and cancels the navigation. Any other URL
// is passed straight through, unchanged.
//
// The script is injected on demand (Dev Mode on) and removed again on
// Dev Mode off — normal browsing never carries any of it.

import { invoke } from "@tauri-apps/api/core";
import { createSignal } from "solid-js";
import { createLogger } from "./logger";

const log = createLogger("TICKETS");

/** Per-workspace Dev Mode flag. Mirrors how the browser already keeps
 *  geometry / port / path / tabs — localStorage, not the Workspace
 *  schema, so enabling Dev Mode needs no workspaces.json migration. */
const DEVMODE_KEY = (workspaceId: string) =>
  `winmux.workspace-browser-devmode.${workspaceId}`;

export function loadDevMode(workspaceId: string): boolean {
  try {
    return localStorage.getItem(DEVMODE_KEY(workspaceId)) === "1";
  } catch {
    return false;
  }
}

export function saveDevMode(workspaceId: string, on: boolean): void {
  try {
    localStorage.setItem(DEVMODE_KEY(workspaceId), on ? "1" : "0");
  } catch {
    /* private mode / quota — Dev Mode just won't persist */
  }
}

/** Per-workspace project override for tickets. Empty/absent means "let
 *  the backend derive it" — which is the normal case. Kept here (not on
 *  the Workspace schema) for the same reason as the Dev Mode flag: no
 *  workspaces.json migration for a per-machine preference. The backend
 *  still owns the resolution ladder; this is only the top rung. */
const PROJECT_OVERRIDE_KEY = (workspaceId: string) =>
  `winmux.tickets-project.${workspaceId}`;

export function loadProjectOverride(workspaceId: string): string | null {
  try {
    const v = localStorage.getItem(PROJECT_OVERRIDE_KEY(workspaceId));
    return v && v.trim() ? v : null;
  } catch {
    return null;
  }
}

export function saveProjectOverride(workspaceId: string, path: string | null): void {
  try {
    if (path && path.trim()) {
      localStorage.setItem(PROJECT_OVERRIDE_KEY(workspaceId), path.trim());
    } else {
      localStorage.removeItem(PROJECT_OVERRIDE_KEY(workspaceId));
    }
  } catch {
    /* private mode / quota */
  }
}

/** Shape the injected script produces, and therefore the shape the Rust
 *  bridge round-trips to `browser:ticket-captured`. */
export interface ElementCapture {
  url: string;
  xpath: string;
  selector: string;
  html: string;
  style: Record<string, unknown>;
}

/** Payload of the `browser:ticket-captured` Tauri event. */
export interface TicketCaptureEvent {
  workspace_id: string;
  capture: ElementCapture;
}

/** Narrow an unknown event payload into an ElementCapture. The capture
 *  crosses a JSON boundary from an untrusted page, so nothing is assumed
 *  (Rule #5 — narrow, never `any`). */
export function parseCapture(value: unknown): ElementCapture | null {
  if (typeof value !== "object" || value === null) return null;
  const o = value as Record<string, unknown>;
  const str = (k: string): string => (typeof o[k] === "string" ? (o[k] as string) : "");
  const style =
    typeof o.style === "object" && o.style !== null
      ? (o.style as Record<string, unknown>)
      : {};
  // A capture with no selector AND no xpath tells us nothing about which
  // element was hit — treat it as malformed rather than opening an empty
  // ticket (the Phase 54 failure mode).
  const selector = str("selector");
  const xpath = str("xpath");
  if (!selector && !xpath) return null;
  return { url: str("url"), xpath, selector, html: str("html"), style };
}

/** The capture awaiting a description, or null when no ticket modal is
 *  open. Module-level so App.tsx can fold it into `anyModalOpen()` (the
 *  native browser webview paints above HTML and must be hidden while the
 *  modal is up) without the browser component having to route it. */
const [pendingCapture, setPendingCapture] = createSignal<PendingCapture | null>(null);
export { pendingCapture, setPendingCapture };

export interface PendingCapture {
  workspaceId: string;
  capture: ElementCapture;
}

/** Cap on captured markup. Keeps the sentinel URL small and bounds how
 *  much page content can land in a ticket file. */
const HTML_MAX = 4096;

/** The injected inspect script.
 *
 *  Self-contained ES5 (the target page can be anything, including one
 *  with a strict parser or an old framework). Idempotent: re-running it
 *  tears down a previous instance first. Everything it adds is reachable
 *  from `window.__winmuxTicket.stop()`.
 */
export function inspectScript(): string {
  return `
(function(){
  if (window.__winmuxTicket) { try { window.__winmuxTicket.stop(); } catch(e){} }
  var HTML_MAX = ${HTML_MAX};
  var hi = document.createElement('div');
  hi.setAttribute('data-winmux-ticket-ui','1');
  hi.style.cssText = 'position:fixed;z-index:2147483647;pointer-events:none;'
    + 'border:2px solid #e0567a;background:rgba(224,86,122,0.12);'
    + 'border-radius:2px;display:none;transition:none;';
  var tip = document.createElement('div');
  tip.setAttribute('data-winmux-ticket-ui','1');
  tip.style.cssText = 'position:fixed;z-index:2147483647;pointer-events:none;'
    + 'background:#e0567a;color:#fff;font:11px/1.4 ui-monospace,Menlo,Consolas,monospace;'
    + 'padding:1px 5px;border-radius:2px;display:none;max-width:60vw;'
    + 'overflow:hidden;text-overflow:ellipsis;white-space:nowrap;';
  function mount(){
    if (document.body) { document.body.appendChild(hi); document.body.appendChild(tip); }
  }
  mount();

  function cssEscape(s){ return String(s).replace(/[^a-zA-Z0-9_-]/g, '\\\\$&'); }

  function xpathOf(el){
    var parts = [];
    while (el && el.nodeType === 1 && parts.length < 64) {
      var i = 0, sib = el.previousSibling;
      while (sib) {
        if (sib.nodeType === 1 && sib.nodeName === el.nodeName) i++;
        sib = sib.previousSibling;
      }
      parts.unshift(el.nodeName.toLowerCase() + '[' + (i + 1) + ']');
      el = el.parentNode;
    }
    return '/' + parts.join('/');
  }

  // Shortest selector that still resolves to this exact element:
  // #id when unique, else a tag.class chain walked up until unique.
  function selectorOf(el){
    if (!el || el.nodeType !== 1) return '';
    if (el.id) {
      var byId = '#' + cssEscape(el.id);
      try { if (document.querySelectorAll(byId).length === 1) return byId; } catch(e){}
    }
    var parts = [], node = el;
    while (node && node.nodeType === 1 && parts.length < 8) {
      var seg = node.nodeName.toLowerCase();
      if (node.id) { seg = '#' + cssEscape(node.id); parts.unshift(seg); break; }
      var cls = (typeof node.className === 'string' ? node.className : '').trim();
      if (cls) {
        var list = cls.split(/\\s+/).slice(0, 3).map(cssEscape);
        if (list.length) seg += '.' + list.join('.');
      }
      var parent = node.parentNode;
      if (parent && parent.nodeType === 1) {
        var same = 0, idx = 0, k;
        for (k = 0; k < parent.children.length; k++) {
          if (parent.children[k].nodeName === node.nodeName) {
            same++;
            if (parent.children[k] === node) idx = same;
          }
        }
        if (same > 1) seg += ':nth-of-type(' + idx + ')';
      }
      parts.unshift(seg);
      var candidate = parts.join(' > ');
      try { if (document.querySelectorAll(candidate).length === 1) return candidate; } catch(e){}
      node = parent;
    }
    return parts.join(' > ');
  }

  function b64url(str){
    var bytes = new TextEncoder().encode(str);
    var bin = '';
    for (var i = 0; i < bytes.length; i++) bin += String.fromCharCode(bytes[i]);
    return btoa(bin).replace(/\\+/g, '-').replace(/\\//g, '_').replace(/=+$/, '');
  }

  function targetFrom(e){
    var el = e.target;
    if (!el || el.nodeType !== 1 || el.getAttribute('data-winmux-ticket-ui')) {
      el = document.elementFromPoint(e.clientX, e.clientY);
    }
    return (el && el.nodeType === 1) ? el : null;
  }

  function onMove(e){
    var el = targetFrom(e);
    if (!el) { hi.style.display = 'none'; tip.style.display = 'none'; return; }
    var r = el.getBoundingClientRect();
    hi.style.display = 'block';
    hi.style.left = r.left + 'px';
    hi.style.top = r.top + 'px';
    hi.style.width = r.width + 'px';
    hi.style.height = r.height + 'px';
    tip.style.display = 'block';
    tip.textContent = selectorOf(el);
    tip.style.left = r.left + 'px';
    tip.style.top = (r.top > 18 ? r.top - 18 : r.bottom + 2) + 'px';
  }

  function onCtx(e){
    var el = targetFrom(e);
    if (!el) return;
    e.preventDefault();
    e.stopPropagation();
    var r = el.getBoundingClientRect();
    var cs = window.getComputedStyle(el);
    var payload = {
      url: location.href,
      xpath: xpathOf(el),
      selector: selectorOf(el),
      html: (el.outerHTML || '').slice(0, HTML_MAX),
      style: {
        color: cs.color,
        background: cs.backgroundColor,
        font: cs.font || (cs.fontSize + ' ' + cs.fontFamily),
        display: cs.display,
        bbox: { x: r.left, y: r.top, w: r.width, h: r.height }
      }
    };
    try {
      location.href = 'winmux-ticket:' + b64url(JSON.stringify(payload));
    } catch (err) { /* nothing we can surface from in here */ }
  }

  document.addEventListener('mousemove', onMove, true);
  document.addEventListener('contextmenu', onCtx, true);

  window.__winmuxTicket = { stop: function(){
    document.removeEventListener('mousemove', onMove, true);
    document.removeEventListener('contextmenu', onCtx, true);
    if (hi.parentNode) hi.parentNode.removeChild(hi);
    if (tip.parentNode) tip.parentNode.removeChild(tip);
    window.__winmuxTicket = null;
  }};
})();
`;
}

/** Removes everything `inspectScript` added. Safe to run when nothing
 *  is installed. */
export function teardownScript(): string {
  return "if (window.__winmuxTicket) { try { window.__winmuxTicket.stop(); } catch(e){} }";
}

/** Inject (or remove) the inspect script in a workspace's browser
 *  webview. Never throws — a browser that isn't open yet just has no
 *  webview to talk to, which is not an error worth surfacing. */
export async function applyDevMode(workspaceId: string, on: boolean): Promise<void> {
  try {
    await invoke("workspace_browser_eval", {
      workspaceId,
      js: on ? inspectScript() : teardownScript(),
    });
  } catch (e) {
    log.debug("devmode eval skipped (no webview yet?)", e);
  }
}
