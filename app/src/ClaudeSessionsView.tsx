import {
  createEffect,
  createMemo,
  createSignal,
  For,
  onCleanup,
  Show,
} from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { t } from "./i18n";
import { createLogger } from "./logger";
import { sessionIdForPane } from "./terminalInstance";
import { fmtBytes, fmtSpan } from "./insightsFmt";
import type {
  ClaudeLogEntry,
  ClaudeLogSummary,
  ClaudeSyncResult,
} from "./types";

// The unified Claude view Phase 24.D parked. It is deliberately NOT a pane
// kind: the objection that killed ClaudeChat + ClaudeLog was that three
// competing "talk to claude" UIs felt fragmented, and a fourth pane alongside
// Terminal / Browser / FileManager would have been exactly that again. This
// is a second *surface* over the one channel that already exists — the
// terminal running `claude`. Reading comes from the JSONL transcript Claude
// itself writes; writing goes through `pty_write` into that same PTY. Nothing
// here can drift from what the terminal shows, because there is no second
// conversation for it to drift from.

const log = createLogger("SESSIONS");

/** Where transcripts are read from. `local` is `~/.claude/projects` on this
 *  machine; `remote` is the SFTP mirror `claude_log_sync` maintains under
 *  `%APPDATA%\ymux\claude-logs\<workspace_id>\`. */
type Source = "local" | "remote";

/** How often an open transcript is re-read, in ms. The file grows while the
 *  agent is answering and there is no change event to listen for — Claude
 *  appends to the JSONL directly. Cheap: a few hundred KB parsed in Rust. */
const POLL_MS = 2500;

interface Props {
  workspaceId: string;
  /** Pane whose PTY the composer writes into — the terminal running `claude`.
   *  Null when the workspace has no pane focused, which disables the composer
   *  rather than guessing at a target. */
  activePaneId: string | null;
}

export function ClaudeSessionsView(p: Props) {
  const [source, setSource] = createSignal<Source>("local");
  const [sessions, setSessions] = createSignal<ClaudeLogSummary[]>([]);
  const [selected, setSelected] = createSignal<string | null>(null);
  const [entries, setEntries] = createSignal<ClaudeLogEntry[]>([]);
  const [loading, setLoading] = createSignal(false);
  const [syncing, setSyncing] = createSignal(false);
  const [query, setQuery] = createSignal("");
  const [draft, setDraft] = createSignal("");
  const [error, setError] = createSignal<string | null>(null);
  const [ptyTick, setPtyTick] = createSignal(0);

  let transcriptEl: HTMLDivElement | undefined;
  let pollTimer: number | undefined;

  // Drives `ptySession` — see the note there for why a timer and not a signal.
  const ptyPoll = window.setInterval(() => setPtyTick((n) => n + 1), 1000);
  onCleanup(() => clearInterval(ptyPoll));

  // ─── data ───────────────────────────────────────────────────────────────

  const fetchSessions = async () => {
    try {
      const list =
        source() === "local"
          ? await invoke<ClaudeLogSummary[]>("claude_log_list_local")
          : await invoke<ClaudeLogSummary[]>("claude_log_list", {
              workspaceId: p.workspaceId,
            });
      setSessions(list ?? []);
      setError(null);
    } catch (e) {
      log.error("session list failed", e);
      setError(String(e));
    }
  };

  const fetchEntries = async (sid: string, quiet = false) => {
    if (!quiet) setLoading(true);
    try {
      const list =
        source() === "local"
          ? await invoke<ClaudeLogEntry[]>("claude_log_read_local", {
              sessionId: sid,
            })
          : await invoke<ClaudeLogEntry[]>("claude_log_read", {
              workspaceId: p.workspaceId,
              sessionId: sid,
            });
      const next = list ?? [];
      // Only touch the signal when the transcript actually grew. A poll that
      // replaced an identical array would re-render every bubble and fight
      // the user's scroll position.
      if (next.length !== entries().length) {
        setEntries(next);
        queueMicrotask(scrollToEnd);
      }
      setError(null);
    } catch (e) {
      log.error("transcript read failed", e);
      setError(String(e));
    } finally {
      if (!quiet) setLoading(false);
    }
  };

  const syncRemote = async () => {
    setSyncing(true);
    try {
      const r = await invoke<ClaudeSyncResult>("claude_log_sync", {
        workspaceId: p.workspaceId,
      });
      log.info(
        `synced=${r.synced} skipped=${r.skipped} errors=${r.errors.length}`,
      );
      if (r.errors.length) setError(r.errors[0]);
      await fetchSessions();
    } catch (e) {
      log.error("sync failed", e);
      setError(String(e));
    } finally {
      setSyncing(false);
    }
  };

  // ─── effects ────────────────────────────────────────────────────────────

  // Switching source invalidates both the list and whatever was open: a
  // session id from the mirror does not necessarily exist locally.
  createEffect(() => {
    source();
    setSelected(null);
    setEntries([]);
    void fetchSessions();
  });

  createEffect(() => {
    const sid = selected();
    if (pollTimer !== undefined) {
      clearInterval(pollTimer);
      pollTimer = undefined;
    }
    if (!sid) {
      setEntries([]);
      return;
    }
    void fetchEntries(sid);
    pollTimer = window.setInterval(() => {
      void fetchEntries(sid, true);
      void fetchSessions();
    }, POLL_MS);
  });

  onCleanup(() => {
    if (pollTimer !== undefined) clearInterval(pollTimer);
  });

  // ─── derived ────────────────────────────────────────────────────────────

  const filtered = createMemo(() => {
    const q = query().trim().toLowerCase();
    if (!q) return sessions();
    return sessions().filter(
      (s) =>
        s.session_id.toLowerCase().includes(q) ||
        (s.first_user ?? "").toLowerCase().includes(q) ||
        (s.project_path ?? "").toLowerCase().includes(q),
    );
  });

  const current = createMemo(
    () => sessions().find((s) => s.session_id === selected()) ?? null,
  );

  /** The composer is only honest when there is a PTY to write into.
   *
   *  `g_terminals` is a plain module set rather than a signal, so a pane
   *  connecting while this view is already open would not invalidate this on
   *  its own and the composer would stay greyed out until something else
   *  re-rendered. The tick re-reads it; the lookup is a walk over a handful
   *  of panes, so the cost is nil. */
  const ptySession = createMemo(() => {
    ptyTick();
    return p.activePaneId ? sessionIdForPane(p.activePaneId) : null;
  });

  const age = (epochSecs: number) =>
    epochSecs
      ? fmtSpan(Math.max(0, Math.floor(Date.now() / 1000) - epochSecs))
      : "—";

  /** First user turn, trimmed to a line — the closest thing a Claude session
   *  has to a title. Falls back to the id so a row is never blank. */
  const titleOf = (s: ClaudeLogSummary) => {
    const first = (s.first_user ?? "").replace(/\s+/g, " ").trim();
    return first || s.session_id;
  };

  /** Leaf of the project path — the working directory name, which is what
   *  distinguishes two sessions with similar opening lines. */
  const projectOf = (s: ClaudeLogSummary) => {
    const path = s.project_path ?? "";
    if (!path) return "";
    const parts = path.split(/[\\/]/).filter(Boolean);
    return parts[parts.length - 1] ?? path;
  };

  // ─── actions ────────────────────────────────────────────────────────────

  function scrollToEnd() {
    if (transcriptEl) transcriptEl.scrollTop = transcriptEl.scrollHeight;
  }

  const send = async () => {
    const text = draft().trim();
    const sid = ptySession();
    if (!text || !sid) return;
    try {
      // The same call PaneView makes on every keystroke. The trailing `\r`
      // submits, exactly as pressing Enter in the terminal would.
      await invoke("pty_write", { sessionId: sid, data: `${text}\r` });
      setDraft("");
    } catch (e) {
      log.error("pty_write failed", e);
      setError(String(e));
    }
  };

  const onComposerKey = (e: KeyboardEvent) => {
    // Enter sends, Shift+Enter breaks the line — the convention every chat
    // surface uses, and the one the terminal's own paste path already honours.
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      void send();
    }
  };

  // ─── bubbles ────────────────────────────────────────────────────────────

  const bubble = (entry: ClaudeLogEntry) => {
    switch (entry.entry_type) {
      case "user":
        return (
          <div class="cs-row cs-row-user">
            <div class="cs-bubble cs-bubble-user" dir="auto">
              {entry.text}
            </div>
          </div>
        );
      case "assistant":
        return (
          <div class="cs-row cs-row-assistant">
            <div class="cs-bubble cs-bubble-assistant" dir="auto">
              {entry.text}
            </div>
          </div>
        );
      case "tool_use":
        return (
          <div class="cs-tool" dir="auto">
            <span class="cs-tool-name">{entry.tool_name ?? "?"}</span>
            <Show when={entry.text}>
              <span class="cs-tool-args">{entry.text}</span>
            </Show>
          </div>
        );
      case "tool_result":
        return (
          <div class="cs-tool cs-tool-result" dir="auto">
            <span class="cs-tool-args">{entry.text}</span>
          </div>
        );
      case "system":
      case "summary":
        return (
          <div class="cs-meta-line" dir="auto">
            {entry.text}
          </div>
        );
      default:
        return null;
    }
  };

  // ─── render ─────────────────────────────────────────────────────────────

  return (
    <div class="cs-root">
      <aside class="cs-sessions">
        <div class="cs-source">
          <button
            class="cs-source-btn"
            classList={{ active: source() === "local" }}
            onClick={() => setSource("local")}
          >
            {t("cs.source.local")}
          </button>
          <button
            class="cs-source-btn"
            classList={{ active: source() === "remote" }}
            onClick={() => setSource("remote")}
          >
            {t("cs.source.remote")}
          </button>
        </div>

        <input
          class="cs-search"
          type="text"
          dir="auto"
          placeholder={t("cs.search")}
          value={query()}
          onInput={(e) => setQuery(e.currentTarget.value)}
        />

        <Show when={source() === "remote"}>
          <button
            class="cs-sync"
            disabled={syncing()}
            onClick={() => void syncRemote()}
          >
            {syncing() ? t("cs.syncing") : t("cs.sync")}
          </button>
        </Show>

        <div class="cs-list">
          <Show
            when={filtered().length > 0}
            fallback={
              <p class="cs-empty" dir="auto">
                {source() === "local"
                  ? t("cs.empty.local")
                  : t("cs.empty.remote")}
              </p>
            }
          >
            <For each={filtered()}>
              {(s) => (
                <button
                  class="cs-item"
                  classList={{ active: s.session_id === selected() }}
                  onClick={() => setSelected(s.session_id)}
                  title={s.project_path ?? s.session_id}
                >
                  <span class="cs-item-title" dir="auto">
                    {titleOf(s)}
                  </span>
                  <span class="cs-item-meta">
                    <Show when={projectOf(s)}>
                      <span class="cs-item-project">{projectOf(s)}</span>
                    </Show>
                    <span>
                      {t("cs.messages", { n: s.message_count })}
                    </span>
                    <span>{age(s.local_mtime)}</span>
                  </span>
                </button>
              )}
            </For>
          </Show>
        </div>
      </aside>

      <section class="cs-conversation">
        <Show
          when={selected()}
          fallback={
            <div class="cs-blank" dir="auto">
              <p>{t("cs.empty.pick")}</p>
            </div>
          }
        >
          <header class="cs-conv-header">
            <span class="cs-conv-title" dir="auto">
              {current() ? titleOf(current()!) : selected()}
            </span>
            <Show when={current()}>
              <span class="cs-conv-meta">
                {fmtBytes(current()!.file_size)} ·{" "}
                {t("cs.messages", { n: current()!.message_count })}
              </span>
            </Show>
          </header>

          <div class="cs-transcript" ref={transcriptEl}>
            <Show
              when={!loading()}
              fallback={<p class="cs-empty">{t("cs.loading")}</p>}
            >
              <For each={entries()}>{(e) => bubble(e)}</For>
            </Show>
          </div>
        </Show>

        <Show when={error()}>
          <div class="cs-error" dir="auto">
            {error()}
          </div>
        </Show>

        <form
          class="cs-composer"
          onSubmit={(e) => {
            e.preventDefault();
            void send();
          }}
        >
          <textarea
            class="cs-composer-input"
            dir="auto"
            rows={1}
            placeholder={
              ptySession()
                ? t("cs.composer.placeholder")
                : t("cs.composer.nopane")
            }
            disabled={!ptySession()}
            value={draft()}
            onInput={(e) => setDraft(e.currentTarget.value)}
            onKeyDown={onComposerKey}
          />
          <button
            class="cs-composer-send"
            type="submit"
            disabled={!ptySession() || !draft().trim()}
            title={t("cs.composer.send")}
          >
            {t("cs.composer.send")}
          </button>
        </form>
      </section>
    </div>
  );
}
