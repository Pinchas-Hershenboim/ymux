import { createMemo, createSignal, For, Show } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { t } from "./i18n";
import { createLogger } from "./logger";
import type { ClaudeCommand, FileEntry } from "./types";

// The Sessions view's input box. Split out of ClaudeSessionsView once it grew
// a command menu and a file picker — the view is about reading a transcript,
// and this is about composing one line of input for it.
//
// Everything here ends as plain text written into the PTY. There is no
// structured "send a command" or "attach a file" protocol with Claude Code,
// because none is needed: `/foo` and `@path` ARE its syntax. So the menus only
// help you type, and anything they produce you could have typed by hand.

const log = createLogger("SESSIONS");

/** Built-in slash commands.
 *
 *  A SNAPSHOT, and knowingly so. These belong to whatever Claude Code version
 *  is installed on the machine the pane is on, and nothing here can ask it —
 *  the CLI has no "list your commands" surface to query. So this list can
 *  drift, and the menu labels it as built-in to say where it came from.
 *  Custom commands do NOT live here: `claude_commands_list` reads those off
 *  disk, on whichever machine the session belongs to, and cannot rot.
 *
 *  Kept to the ones that make sense from a chat box. Anything that takes over
 *  the terminal UI (`/vim`, `/terminal-setup`) is deliberately absent: it
 *  would work, but the result is only visible in the pane. */
const BUILTIN_COMMANDS: ReadonlyArray<ClaudeCommand> = [
  { name: "clear", description: "Clear the conversation history" },
  { name: "compact", description: "Summarise the conversation so far" },
  { name: "context", description: "Show what is currently in context" },
  { name: "cost", description: "Token usage and cost for this session" },
  { name: "usage", description: "Plan limits and current usage" },
  { name: "model", description: "Show or switch the model" },
  { name: "status", description: "Version, account, model, connectivity" },
  { name: "doctor", description: "Check the installation for problems" },
  { name: "init", description: "Create a CLAUDE.md for this project" },
  { name: "memory", description: "Edit CLAUDE.md memory files" },
  { name: "review", description: "Review a pull request" },
  { name: "agents", description: "Manage subagents" },
  { name: "mcp", description: "Manage MCP servers" },
  { name: "hooks", description: "Manage hooks" },
  { name: "permissions", description: "Manage tool permissions" },
  { name: "todos", description: "List the current todos" },
  { name: "export", description: "Export the conversation" },
  { name: "help", description: "List every command the installed version has" },
];

/** How many rows either menu shows before it scrolls. */
const MENU_MAX = 8;

interface Props {
  workspaceId: string;
  /** Whether the pane runs over SSH — decides which machine both menus read. */
  isRemote: boolean;
  /** The session's project directory, where the file picker opens and what
   *  `@` paths are made relative to. */
  projectPath?: string;
  /** PTY to write into. Null disables everything. */
  ptySessionId: string | null;
  onError: (message: string) => void;
}

export function ClaudeComposer(p: Props) {
  const [draft, setDraft] = createSignal("");
  const [commands, setCommands] = createSignal<ClaudeCommand[]>([]);
  const [commandsLoaded, setCommandsLoaded] = createSignal(false);
  const [cursor, setCursor] = createSignal(0);

  const [pickerOpen, setPickerOpen] = createSignal(false);
  const [pickerDir, setPickerDir] = createSignal("");
  const [pickerEntries, setPickerEntries] = createSignal<FileEntry[]>([]);
  const [pickerBusy, setPickerBusy] = createSignal(false);

  let inputEl: HTMLTextAreaElement | undefined;

  const sepOf = () => (p.isRemote ? "/" : "\\");

  // ─── slash commands ─────────────────────────────────────────────────────

  const loadCommands = async () => {
    if (commandsLoaded()) return;
    setCommandsLoaded(true);
    try {
      const custom = await invoke<ClaudeCommand[]>("claude_commands_list", {
        workspaceId: p.isRemote ? p.workspaceId : null,
      });
      // Custom first: a user who wrote a command wants it above a built-in
      // that happens to sort earlier.
      setCommands([...(custom ?? []), ...BUILTIN_COMMANDS]);
    } catch (e) {
      log.error("command list failed", e);
      setCommands([...BUILTIN_COMMANDS]);
    }
  };

  /** The `/query` being typed, or null when the draft is not a command.
   *
   *  Only a draft that STARTS with `/` and has no space yet counts. Once there
   *  is a space the user is writing arguments, and a menu over their argument
   *  text would fight them for the Enter key. */
  const slashQuery = createMemo(() => {
    const d = draft();
    if (!d.startsWith("/")) return null;
    const rest = d.slice(1);
    if (/\s/.test(rest)) return null;
    return rest.toLowerCase();
  });

  const matches = createMemo(() => {
    const q = slashQuery();
    if (q === null) return [];
    const all = commands();
    if (!q) return all.slice(0, MENU_MAX * 3);
    return all
      .filter((c) => c.name.toLowerCase().includes(q))
      // A prefix match is what the user meant; a substring match is a
      // courtesy. Rank accordingly.
      .sort((a, b) => {
        const ap = a.name.toLowerCase().startsWith(q) ? 0 : 1;
        const bp = b.name.toLowerCase().startsWith(q) ? 0 : 1;
        return ap - bp || a.name.localeCompare(b.name);
      })
      .slice(0, MENU_MAX * 3);
  });

  const menuOpen = createMemo(() => slashQuery() !== null && matches().length > 0);

  const pickCommand = (c: ClaudeCommand) => {
    // A trailing space so arguments can follow immediately, and so the menu
    // closes (a space ends the command token by definition).
    setDraft(`/${c.name} `);
    setCursor(0);
    inputEl?.focus();
  };

  // ─── file picker ────────────────────────────────────────────────────────

  const listDir = async (dir: string) => {
    setPickerBusy(true);
    try {
      const entries = p.isRemote
        ? await invoke<FileEntry[]>("file_list_remote", {
            workspaceId: p.workspaceId,
            path: dir,
            showHidden: false,
          })
        : await invoke<FileEntry[]>("file_list_local", {
            path: dir,
            showHidden: false,
          });
      // Directories first, then files, each alphabetical — the order every
      // file list in this app already uses.
      const sorted = [...(entries ?? [])].sort(
        (a, b) =>
          Number(b.is_dir) - Number(a.is_dir) || a.name.localeCompare(b.name),
      );
      setPickerEntries(sorted);
      setPickerDir(dir);
    } catch (e) {
      log.error("file list failed", e);
      p.onError(String(e));
      setPickerOpen(false);
    } finally {
      setPickerBusy(false);
    }
  };

  const openPicker = async () => {
    if (pickerOpen()) {
      setPickerOpen(false);
      return;
    }
    setPickerOpen(true);
    if (pickerEntries().length && pickerDir()) return;
    let start = p.projectPath ?? "";
    if (!start) {
      try {
        start = p.isRemote
          ? await invoke<string>("file_home_remote", {
              workspaceId: p.workspaceId,
            })
          : await invoke<string>("file_home_local");
      } catch (e) {
        log.error("home lookup failed", e);
        p.onError(String(e));
        setPickerOpen(false);
        return;
      }
    }
    await listDir(start);
  };

  const parentOf = (dir: string) => {
    const s = sepOf();
    const trimmed = dir.endsWith(s) ? dir.slice(0, -1) : dir;
    const at = trimmed.lastIndexOf(s);
    if (at <= 0) return p.isRemote ? "/" : trimmed;
    return trimmed.slice(0, at);
  };

  const joinPath = (dir: string, name: string) => {
    const s = sepOf();
    return dir.endsWith(s) ? `${dir}${name}` : `${dir}${s}${name}`;
  };

  /** `@` references read best relative to where Claude was launched, which is
   *  the session's project directory. Anything outside it stays absolute —
   *  a wrong relative path is worse than a long one. */
  const referenceFor = (absolute: string) => {
    const root = p.projectPath;
    if (!root) return absolute;
    const s = sepOf();
    const rootNorm = root.endsWith(s) ? root.slice(0, -1) : root;
    if (!absolute.startsWith(rootNorm + s)) return absolute;
    return absolute.slice(rootNorm.length + 1);
  };

  const pickFile = (e: FileEntry) => {
    const full = joinPath(pickerDir(), e.name);
    if (e.is_dir) {
      void listDir(full);
      return;
    }
    const ref = referenceFor(full);
    const d = draft();
    const needsSpace = d.length > 0 && !d.endsWith(" ");
    setDraft(`${d}${needsSpace ? " " : ""}@${ref} `);
    setPickerOpen(false);
    inputEl?.focus();
  };

  // ─── send ───────────────────────────────────────────────────────────────

  const send = async () => {
    const text = draft().trim();
    const sid = p.ptySessionId;
    if (!text || !sid) return;
    try {
      // The same call PaneView makes on every keystroke. The trailing `\r`
      // submits, exactly as pressing Enter in the terminal would.
      await invoke("pty_write", { sessionId: sid, data: `${text}\r` });
      setDraft("");
      setPickerOpen(false);
    } catch (e) {
      log.error("pty_write failed", e);
      p.onError(String(e));
    }
  };

  const onKeyDown = (e: KeyboardEvent) => {
    if (menuOpen()) {
      const list = matches();
      if (e.key === "ArrowDown") {
        e.preventDefault();
        setCursor((i) => (i + 1) % list.length);
        return;
      }
      if (e.key === "ArrowUp") {
        e.preventDefault();
        setCursor((i) => (i - 1 + list.length) % list.length);
        return;
      }
      if (e.key === "Enter" || e.key === "Tab") {
        // Enter picks from the menu rather than sending. Sending a
        // half-typed command name would be the wrong guess almost every time.
        e.preventDefault();
        const chosen = list[Math.min(cursor(), list.length - 1)];
        if (chosen) pickCommand(chosen);
        return;
      }
      if (e.key === "Escape") {
        e.preventDefault();
        // Leave the text; just stop the menu claiming Enter.
        setDraft(draft() + " ");
        return;
      }
    }
    if (e.key === "Escape" && pickerOpen()) {
      e.preventDefault();
      setPickerOpen(false);
      return;
    }
    // Enter sends, Shift+Enter breaks the line — the convention every chat
    // surface uses, and the one the terminal's own paste path already honours.
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      void send();
    }
  };

  // ─── render ─────────────────────────────────────────────────────────────

  return (
    <div class="cs-composer-wrap">
      <Show when={pickerOpen()}>
        <div class="cs-menu cs-picker">
          <div class="cs-picker-head">
            <button
              class="cs-picker-up"
              onClick={() => void listDir(parentOf(pickerDir()))}
              title={t("cs.picker.up")}
            >
              ↑
            </button>
            <span class="cs-picker-path" dir="ltr" title={pickerDir()}>
              {pickerDir()}
            </span>
          </div>
          <Show
            when={!pickerBusy()}
            fallback={<p class="cs-menu-empty">{t("cs.loading")}</p>}
          >
            <Show
              when={pickerEntries().length}
              fallback={<p class="cs-menu-empty">{t("cs.picker.empty")}</p>}
            >
              <div class="cs-menu-list">
                <For each={pickerEntries()}>
                  {(e) => (
                    <button class="cs-menu-row" onClick={() => pickFile(e)}>
                      <span class="cs-menu-icon" aria-hidden="true">
                        {e.is_dir ? "▸" : "·"}
                      </span>
                      <span class="cs-menu-name" dir="ltr">
                        {e.name}
                      </span>
                    </button>
                  )}
                </For>
              </div>
            </Show>
          </Show>
        </div>
      </Show>

      <Show when={menuOpen()}>
        <div class="cs-menu">
          <div class="cs-menu-list">
            <For each={matches()}>
              {(c, i) => (
                <button
                  class="cs-menu-row"
                  classList={{ active: i() === cursor() }}
                  onMouseEnter={() => setCursor(i())}
                  onClick={() => pickCommand(c)}
                >
                  <span class="cs-menu-name" dir="ltr">
                    /{c.name}
                  </span>
                  <span class="cs-menu-desc" dir="auto">
                    {c.description}
                  </span>
                </button>
              )}
            </For>
          </div>
        </div>
      </Show>

      <form
        class="cs-composer"
        onSubmit={(e) => {
          e.preventDefault();
          void send();
        }}
      >
        <button
          type="button"
          class="cs-composer-plus"
          disabled={!p.ptySessionId}
          title={t("cs.picker.attach")}
          onClick={() => void openPicker()}
        >
          +
        </button>
        <textarea
          class="cs-composer-input"
          dir="auto"
          rows={1}
          ref={inputEl}
          placeholder={t("cs.composer.placeholder")}
          value={draft()}
          onFocus={() => void loadCommands()}
          onInput={(e) => {
            setDraft(e.currentTarget.value);
            setCursor(0);
          }}
          onKeyDown={onKeyDown}
        />
        <button
          class="cs-composer-send"
          type="submit"
          disabled={!p.ptySessionId || !draft().trim()}
          title={t("cs.composer.send")}
        >
          {t("cs.composer.send")}
        </button>
      </form>
    </div>
  );
}
