/* @refresh reload */
// logger.ts must load BEFORE the console monkeypatch below — it captures the
// original console fns so logger output is never forwarded twice.
import "./logger";
import { render } from "solid-js/web";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
// Global stylesheets live at the entry point so BOTH the main <App> and the
// #4 pop-out window (which bypasses <App>) get xterm's CSS + our theme.
// Previously these were imported inside App.tsx, so a popout webview rendered
// unstyled — blank white screen, invisible terminal.
import "@xterm/xterm/css/xterm.css";
import "./App.css";
import App from "./App";
import { PopoutTerminal } from "./components/PopoutTerminal";

// Phase 8.E → unified logging: capture console.error / console.warn as a
// safety net for un-swept or third-party output. Forwarded fire-and-forget
// to `ui_log`, which writes debug.log AND the dev ring buffer (`ymux dev
// console-tail`). Swept code logs through createLogger() instead — it uses
// the original console fns captured in logger.ts, so nothing loops through
// here twice. Original console output is preserved.
{
  const origErr = console.error;
  const origWarn = console.warn;
  const fmt = (args: unknown[]): string =>
    args
      .map((a) => {
        if (typeof a === "string") return a;
        if (a instanceof Error) return `${a.name}: ${a.message}`;
        try {
          return JSON.stringify(a);
        } catch {
          return String(a);
        }
      })
      .join(" ");
  console.error = (...args: unknown[]) => {
    origErr(...(args as []));
    invoke("ui_log", {
      level: "error",
      tag: "CONSOLE",
      message: fmt(args),
    }).catch(() => {});
  };
  console.warn = (...args: unknown[]) => {
    origWarn(...(args as []));
    invoke("ui_log", {
      level: "warn",
      tag: "CONSOLE",
      message: fmt(args),
    }).catch(() => {});
  };
  window.addEventListener("error", (e) => {
    invoke("ui_log", {
      level: "error",
      tag: "CONSOLE",
      message: `unhandled: ${e.message} @ ${e.filename}:${e.lineno}`,
    }).catch(() => {});
  });
  window.addEventListener("unhandledrejection", (e) => {
    invoke("ui_log", {
      level: "error",
      tag: "CONSOLE",
      message: `unhandled rejection: ${String(e.reason)}`,
    }).catch(() => {});
  });
}

// Unshipped-fivefer (#4): pop-out terminal windows. Bail to a bare
// full-screen <PopoutTerminal> BEFORE mounting <App>, so none of the
// workspace/settings bootstrap runs in the popout webview.
//
// The session id comes from the window LABEL (`popout-<sid>`). The popout URL
// is a CLEAN `index.html` (no query/fragment) because Tauri's built-app asset
// protocol serves a blank page for any suffixed path — so the label, not the
// URL, carries the id.
let winLabel = "";
try {
  winLabel = getCurrentWindow().label;
} catch {
  // window metadata not ready — treat as the main window
}
const popoutSid = winLabel.startsWith("popout-")
  ? winLabel.slice("popout-".length)
  : null;

if (popoutSid) {
  render(
    () => <PopoutTerminal sessionId={popoutSid} />,
    document.getElementById("root") as HTMLElement,
  );
} else {
  render(() => <App />, document.getElementById("root") as HTMLElement);
}
