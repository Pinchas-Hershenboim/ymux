// Unified frontend logger. Every module logs through `createLogger(tag)`
// instead of raw `console.*`; lines reach BOTH the devtools console and the
// single local debug.log (via the `ui_log` Tauri command) tagged `[UI:TAG]`.
//
// Level filtering is double-gated: we skip the IPC below the threshold here
// (cheap), and the backend filters again through the same global — the
// backend is authoritative, so popout windows that never load settings still
// behave correctly with the default.
//
// IMPORTANT: this module must be imported before the console monkeypatch in
// index.tsx — it captures the ORIGINAL console fns so logger output is never
// forwarded twice.
import { invoke } from "@tauri-apps/api/core";

export type LogLevel = "debug" | "info" | "warn" | "error";

const LEVEL_ORDER: Record<LogLevel, number> = {
  debug: 0,
  info: 1,
  warn: 2,
  error: 3,
};

// Captured before index.tsx monkeypatches console.error/warn to forward
// stray (un-swept / third-party) output to ui_log.
const orig = {
  debug: console.debug.bind(console),
  info: console.info.bind(console),
  warn: console.warn.bind(console),
  error: console.error.bind(console),
};

let currentLevel: LogLevel = "info";

/** Called from App.tsx when settings load / change. */
export function setLoggerLevel(level: LogLevel): void {
  currentLevel = level;
}

/** Compact, metadata-only rendering of an attached error/value. Never dump
 *  whole objects — keep debug.log free of payload content (CLAUDE.md Rule 1
 *  spirit: log what happened, not what flowed through). */
function describe(err: unknown): string {
  if (err === undefined) return "";
  if (err instanceof Error) return ` — ${err.name}: ${err.message}`;
  if (typeof err === "string") return ` — ${err}`;
  try {
    return ` — ${JSON.stringify(err)}`;
  } catch {
    return ` — ${String(err)}`;
  }
}

function emit(tag: string, level: LogLevel, msg: string, err?: unknown): void {
  const text = `${msg}${describe(err)}`;
  // Always visible in devtools regardless of the persisted threshold —
  // the threshold governs what lands in debug.log.
  orig[level](`[${tag}] ${text}`);
  if (LEVEL_ORDER[level] < LEVEL_ORDER[currentLevel]) return;
  invoke("ui_log", { level, tag, message: text }).catch(() => {});
}

export interface Logger {
  debug(msg: string, err?: unknown): void;
  info(msg: string, err?: unknown): void;
  warn(msg: string, err?: unknown): void;
  error(msg: string, err?: unknown): void;
}

export function createLogger(tag: string): Logger {
  return {
    debug: (msg, err?) => emit(tag, "debug", msg, err),
    info: (msg, err?) => emit(tag, "info", msg, err),
    warn: (msg, err?) => emit(tag, "warn", msg, err),
    error: (msg, err?) => emit(tag, "error", msg, err),
  };
}
