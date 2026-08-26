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

/** The remote tick is slower because it does more: the mirror under
 *  `claude-logs/` is a SNAPSHOT, so re-reading it alone shows a frozen
 *  transcript no matter how often you ask. Each remote tick re-syncs the open
 *  session first — one file over SFTP, not the whole tree — and that is an SSH
 *  round-trip, so it does not belong on a 2.5s loop. */
const POLL_REMOTE_MS = 6000;

/** One thing to draw. A tool call and the result answering it are ONE card,
 *  paired by `tool_id`, because splitting them reads as two unrelated events. */
type Rendered =
  | { kind: "msg"; entry: ClaudeLogEntry }
  | { kind: "tool"; call: ClaudeLogEntry; result?: ClaudeLogEntry }
  | { kind: "meta"; entry: ClaudeLogEntry };

interface Props {
  workspaceId: string;
  /** Whether the active workspace runs over SSH.
   *
   *  This decides which source can be resumed, and it is not cosmetic. A
   *  `local` session id names a conversation in `~/.claude/projects` on THIS
   *  machine; a pane on an SSH workspace runs `claude` on the server, where
   *  that id does not exist and the Windows project path does not either. The
   *  first cut offered Resume regardless, so on an SSH-only setup the button
   *  reconnected the pane, ran `cd 'C:\…' && claude --resume <unknown-id>`,
   *  and left a bare shell — after which everything typed went nowhere.
   *  Nothing in the failure was visible: the SSH connect itself succeeded. */
  workspaceIsRemote: boolean;
  /** Pane whose PTY the composer writes into — the terminal running `claude`.
   *  Null when the workspace has no pane focused, which disables the composer
   *  rather than guessing at a target. */
  activePaneId: string | null;
  /** Launch `claude --resume <sessionId>` in the active pane, from the
   *  session's own project directory. App owns `connectPane`; this view only
   *  asks. Resolves once the connect attempt is done, so the composer is not
   *  enabled against a pane that has not been pointed at this session yet. */
  onResume: (sessionId: string, projectPath?: string) => Promise<void>;
}

export function ClaudeSessionsView(p: Props) {
  // Open on the source this workspace can actually act on. Defaulting to
  // "local" on an SSH workspace showed a list where nothing was resumable.
  const [source, setSource] = createSignal<Source>(
    p.workspaceIsRemote ? "remote" : "local",
  );
  const [sessions, setSessions] = createSignal<ClaudeLogSummary[]>([]);
  const [selected, setSelected] = createSignal<string | null>(null);
  const [entries, setEntries] = createSignal<ClaudeLogEntry[]>([]);
  const [loading, setLoading] = createSignal(false);
  const [syncing, setSyncing] = createSignal(false);
  const [query, setQuery] = createSignal("");
  const [draft, setDraft] = createSignal("");
  const [error, setError] = createSignal<string | null>(null);
  const [ptyTick, setPtyTick] = createSignal(0);
  const [expanded, setExpanded] = createSignal<ReadonlySet<string>>(new Set());
  // Which session this view actually resumed into the pane. Writing is only
  // offered for THIS one — see the note on `writable` below.
  const [attached, setAttached] = createSignal<string | null>(null);

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
    const remote = source() === "remote";
    void fetchEntries(sid);
    pollTimer = window.setInterval(() => {
      // Re-mirror the open session before re-reading it. Without this the
      // remote tab reads a snapshot taken at the last manual Sync, so a live
      // conversation looked frozen even when the agent was answering.
      const tick = remote
        ? invoke<ClaudeSyncResult>("claude_log_sync", {
            workspaceId: p.workspaceId,
            sessionId: sid,
          })
            .then(() => fetchEntries(sid, true))
            // A dropped SSH handle is expected — the pane may be between
            // connections. Fall back to the mirror instead of blanking.
            .catch(() => fetchEntries(sid, true))
        : fetchEntries(sid, true);
      void tick;
      void fetchSessions();
    }, remote ? POLL_REMOTE_MS : POLL_MS);
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

  /** Pair each tool call with its result, so a card can show both.
   *
   *  A `tool_result` whose `tool_use` is missing still gets drawn — a
   *  transcript that begins mid-turn is normal after a `--resume`, and
   *  silently dropping the output would look like the tool did nothing. */
  const rendered = createMemo<Rendered[]>(() => {
    const es = entries();
    const resultFor = new Map<string, ClaudeLogEntry>();
    const hasCall = new Set<string>();
    for (const e of es) {
      if (!e.tool_id) continue;
      if (e.entry_type === "tool_result") resultFor.set(e.tool_id, e);
      if (e.entry_type === "tool_use") hasCall.add(e.tool_id);
    }
    const out: Rendered[] = [];
    for (const e of es) {
      switch (e.entry_type) {
        case "user":
        case "assistant":
          out.push({ kind: "msg", entry: e });
          break;
        case "tool_use":
          out.push({
            kind: "tool",
            call: e,
            result: e.tool_id ? resultFor.get(e.tool_id) : undefined,
          });
          break;
        case "tool_result":
          // Drawn inside its call's card whenever that call is present.
          if (!e.tool_id || !hasCall.has(e.tool_id)) {
            out.push({ kind: "tool", call: e });
          }
          break;
        default:
          out.push({ kind: "meta", entry: e });
      }
    }
    return out;
  });

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

  /** Writing is offered ONLY for the session this view resumed into the pane.
   *
   *  The first cut sent into whatever pane happened to be focused, which meant
   *  opening an old transcript and typing put the text into a DIFFERENT
   *  conversation — the one the pane was already running — and nothing
   *  appeared in the transcript being read. It looked like the composer was
   *  broken. It was worse than broken: it was writing somewhere invisible.
   *  So the two are bound explicitly, and anything else offers Resume. */
  const writable = createMemo(
    () => !!ptySession() && !!selected() && attached() === selected(),
  );

  /** Whether the open session can be resumed in this pane at all.
   *
   *  A session id only means something on the machine that holds its
   *  transcript. `local` ids live in `~/.claude/projects` here; `remote` ids
   *  live on the workspace's server. Offering Resume across that line runs a
   *  command that cannot succeed and leaves a bare shell behind, which is
   *  exactly what it did before this guard existed. */
  const resumable = createMemo(() =>
    source() === "remote" ? p.workspaceIsRemote : !p.workspaceIsRemote,
  );

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

  const toggleExpanded = (id: string) => {
    const next = new Set(expanded());
    if (next.has(id)) next.delete(id);
    else next.add(id);
    setExpanded(next);
  };

  const [resuming, setResuming] = createSignal(false);

  const resumeHere = async () => {
    const sid = selected();
    // The button is already hidden when this is false; checked again here so
    // the rule lives with the action and not only with its affordance.
    if (!sid || resuming() || !resumable()) return;
    setResuming(true);
    try {
      await p.onResume(sid, current()?.project_path ?? undefined);
      setAttached(sid);
    } catch (e) {
      log.error("resume failed", e);
      setError(String(e));
    } finally {
      setResuming(false);
    }
  };

  const send = async () => {
    const text = draft().trim();
    const sid = ptySession();
    if (!text || !sid || !writable()) return;
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

  // ─── pieces ─────────────────────────────────────────────────────────────

  const message = (entry: ClaudeLogEntry) => (
    <div
      class="cs-row"
      classList={{
        "cs-row-user": entry.entry_type === "user",
        "cs-row-assistant": entry.entry_type !== "user",
      }}
    >
      <div
        class="cs-bubble"
        classList={{
          "cs-bubble-user": entry.entry_type === "user",
          "cs-bubble-assistant": entry.entry_type !== "user",
        }}
        dir="auto"
      >
        {entry.text}
      </div>
    </div>
  );

  /** A tool call as one card: name + one-line summary always, the full input
   *  and output behind a click. Collapsed by default — a transcript is mostly
   *  tool traffic, and expanding it all by default buries the conversation. */
  const toolCard = (call: ClaudeLogEntry, result?: ClaudeLogEntry) => {
    const id = call.tool_id ?? `line-${call.line_no}`;
    const isOpen = () => expanded().has(id);
    const hasBody = () => !!call.tool_input || !!result?.text || !!call.text;
    return (
      <div class="cs-tool-card" classList={{ open: isOpen() }}>
        <button
          class="cs-tool-head"
          onClick={() => hasBody() && toggleExpanded(id)}
          disabled={!hasBody()}
        >
          <span class="cs-tool-caret" aria-hidden="true">
            {isOpen() ? "▾" : "▸"}
          </span>
          <span class="cs-tool-name">
            {call.tool_name ?? t("cs.tool.result")}
          </span>
          <span class="cs-tool-summary" dir="auto">
            {call.tool_name ? call.text : ""}
          </span>
        </button>
        <Show when={isOpen()}>
          <div class="cs-tool-body">
            <Show when={call.tool_input}>
              <div class="cs-tool-section">
                <span class="cs-tool-label">{t("cs.tool.in")}</span>
                <pre class="cs-tool-pre" dir="ltr">
                  {call.tool_input}
                </pre>
              </div>
            </Show>
            <Show when={result?.text ?? (call.tool_name ? "" : call.text)}>
              <div class="cs-tool-section">
                <span class="cs-tool-label">{t("cs.tool.out")}</span>
                <pre class="cs-tool-pre" dir="ltr">
                  {result?.text ?? call.text}
                </pre>
              </div>
            </Show>
          </div>
        </Show>
      </div>
    );
  };

  // ─── render ─────────────────────────────────────────────────────────────

  // dir="rtl" is set on the view rather than inherited from the app shell:
  // the UI language can stay English while the conversations are Hebrew, and
  // it is the conversation this surface exists to show. Every rule in the
  // stylesheet uses logical properties, so the whole layout mirrors from here
  // — session column, bubble sides, composer — while each bubble keeps its
  // own dir="auto" for its actual content.
  return (
    <div class="cs-root" dir="rtl">
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
                  classList={{
                    active: s.session_id === selected(),
                    attached: s.session_id === attached(),
                  }}
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
              <For each={rendered()}>
                {(r) =>
                  r.kind === "msg" ? (
                    message(r.entry)
                  ) : r.kind === "tool" ? (
                    toolCard(r.call, r.result)
                  ) : (
                    <div class="cs-meta-line" dir="auto">
                      {r.entry.text}
                    </div>
                  )
                }
              </For>
            </Show>
          </div>
        </Show>

        <Show when={error()}>
          <div class="cs-error" dir="auto">
            {error()}
          </div>
        </Show>

        {/* The composer never writes blind. Until this view has resumed the
            open session into the pane, there is no way to know the pane is
            running the conversation on screen — so it offers Resume instead
            of a text box that would land somewhere invisible. */}
        <Show
          when={writable()}
          fallback={
            <Show when={selected()}>
              <div class="cs-attach" dir="auto">
                <span class="cs-attach-text">
                  {!resumable()
                    ? source() === "local"
                      ? t("cs.attach.wrongMachine.local")
                      : t("cs.attach.wrongMachine.remote")
                    : ptySession()
                      ? t("cs.attach.hint")
                      : t("cs.composer.nopane")}
                </span>
                <Show when={resumable() && ptySession()}>
                  <button
                    class="cs-attach-btn"
                    disabled={resuming()}
                    onClick={() => void resumeHere()}
                  >
                    {resuming() ? t("cs.loading") : t("cs.attach.resume")}
                  </button>
                </Show>
              </div>
            </Show>
          }
        >
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
              placeholder={t("cs.composer.placeholder")}
              value={draft()}
              onInput={(e) => setDraft(e.currentTarget.value)}
              onKeyDown={onComposerKey}
            />
            <button
              class="cs-composer-send"
              type="submit"
              disabled={!draft().trim()}
              title={t("cs.composer.send")}
            >
              {t("cs.composer.send")}
            </button>
          </form>
        </Show>
      </section>
    </div>
  );
}
