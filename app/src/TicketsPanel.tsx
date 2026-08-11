import { createEffect, createSignal, For, Show } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { t } from "./i18n";
import { IconBug, IconCheck, IconRefresh, IconTrash, IconFolder } from "./icons";
import { PanelSurface } from "./PanelSurface";
import { createLogger } from "./logger";
import type { Surface } from "./panels";
import type { Ticket } from "./bindings/Ticket";

const log = createLogger("TICKETS");

// Workspace-scoped ticket list. Everything shown here was captured by
// right-clicking an element in the workspace Browser with Dev Mode on.
//
// Rides the shared PanelSurface lifecycle (drawer → float → fullscreen)
// like Notifications / Files / Monitor, so it gets that chrome for free
// instead of hand-rolling a fourth variant.

interface Props {
  surface: Surface;
  workspaceId?: string;
  workspaceName?: string;
  onClose: () => void;
  onDrawer: () => void;
  onFloat: () => void;
  onFullscreen: () => void;
}

type Filter = "open" | "resolved" | "all";

export function TicketsPanel(p: Props) {
  const [items, setItems] = createSignal<Ticket[]>([]);
  const [filter, setFilter] = createSignal<Filter>("open");
  const [selectedId, setSelectedId] = createSignal<string | null>(null);
  const [loading, setLoading] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [copied, setCopied] = createSignal(false);

  const load = async () => {
    const ws = p.workspaceId;
    if (!ws) {
      setItems([]);
      return;
    }
    setLoading(true);
    setError(null);
    try {
      setItems(await invoke<Ticket[]>("tickets_list", { workspaceId: ws }));
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  };

  // Reload on workspace change and whenever the panel (re)opens, so a
  // ticket created while it was closed shows up without a manual refresh.
  createEffect(() => {
    void p.workspaceId;
    if (p.surface === "closed") return;
    void load();
  });

  const visible = () => {
    const f = filter();
    return f === "all" ? items() : items().filter((tk) => tk.status === f);
  };

  const openCount = () => items().filter((tk) => tk.status === "open").length;

  const toggleResolved = async (tk: Ticket) => {
    const next = tk.status === "resolved" ? "open" : "resolved";
    try {
      await invoke("tickets_update", {
        workspaceId: tk.workspace_id,
        id: tk.id,
        status: next,
      });
      await load();
    } catch (e) {
      setError(String(e));
    }
  };

  const deleteOne = async (tk: Ticket) => {
    try {
      await invoke("tickets_delete", { workspaceId: tk.workspace_id, id: tk.id });
      if (selectedId() === tk.id) setSelectedId(null);
      await load();
    } catch (e) {
      setError(String(e));
    }
  };

  const asMarkdown = (tk: Ticket): string => {
    let style = "{}";
    try {
      style = JSON.stringify(tk.element.style, null, 2);
    } catch {
      /* keep "{}" */
    }
    return [
      `# ${tk.id}`,
      "",
      `- Status: **${tk.status}**`,
      `- Created: ${tk.created}`,
      `- URL: ${tk.url}`,
      "",
      "## Description",
      "",
      tk.description || "_(none)_",
      "",
      "## Element",
      "",
      `- Selector: \`${tk.element.selector}\``,
      `- XPath: \`${tk.element.xpath}\``,
      "",
      "```html",
      tk.element.html,
      "```",
      "",
      "## Computed style",
      "",
      "```json",
      style,
      "```",
      "",
    ].join("\n");
  };

  // Clipboard rather than a file dialog: the ticket JSON already lives
  // on disk (see "reveal folder"), and markdown-on-the-clipboard is what
  // actually gets pasted into an issue tracker or a Claude prompt.
  const copyMarkdown = async (tk: Ticket) => {
    try {
      await navigator.clipboard.writeText(asMarkdown(tk));
      setCopied(true);
      setTimeout(() => setCopied(false), 1800);
    } catch (e) {
      setError(String(e));
    }
  };

  const revealFolder = async () => {
    const ws = p.workspaceId;
    if (!ws) return;
    try {
      const dir = await invoke<string>("tickets_dir_path", { workspaceId: ws });
      await revealItemInDir(dir);
    } catch (e) {
      log.warn("reveal tickets dir failed", e);
      setError(String(e));
    }
  };

  const shortDate = (iso: string) => iso.replace("T", " ").replace("Z", "");

  return (
    <PanelSurface
      surface={p.surface}
      icon={<IconBug size={14} />}
      title={
        p.workspaceName
          ? t("tickets.panel.titleWs", { ws: p.workspaceName })
          : t("tickets.panel.title")
      }
      drawerStorageKey="winmux.tickets-drawer-width"
      drawerDefaultWidth={420}
      drawerMinWidth={320}
      floatStorageKey={`winmux.tickets-float.${p.workspaceId ?? "none"}`}
      floatDefault={{ x: 160, y: 100, w: 720, h: 560 }}
      floatMinW={420}
      floatMinH={320}
      onClose={p.onClose}
      onDrawer={p.onDrawer}
      onFloat={p.onFloat}
      onFullscreen={p.onFullscreen}
      headerActions={() => (
        <>
          <button
            class="tk-hdr-btn"
            onClick={() => void load()}
            title={t("tickets.panel.refresh")}
          >
            <IconRefresh size={14} />
          </button>
          <button
            class="tk-hdr-btn"
            onClick={() => void revealFolder()}
            title={t("tickets.panel.reveal")}
          >
            <IconFolder size={14} />
          </button>
        </>
      )}
      body={() => (
        <div class="tk-panel">
          <div class="tk-filters" role="tablist">
            <For each={["open", "resolved", "all"] as Filter[]}>
              {(f) => (
                <button
                  role="tab"
                  aria-selected={filter() === f}
                  class={`tk-filter ${filter() === f ? "active" : ""}`}
                  onClick={() => setFilter(f)}
                >
                  {t(`tickets.filter.${f}`)}
                  <Show when={f === "open" && openCount() > 0}>
                    <span class="tk-filter-count">{openCount()}</span>
                  </Show>
                </button>
              )}
            </For>
          </div>

          <Show when={error()}>
            <div class="tk-err" role="alert">
              {error()}
            </div>
          </Show>

          <Show
            when={visible().length > 0}
            fallback={
              <div class="tk-empty">
                <IconBug size={22} />
                <p>{loading() ? t("common.loading") : t("tickets.panel.empty")}</p>
                <Show when={!loading()}>
                  <p class="tk-empty-hint">{t("tickets.panel.emptyHint")}</p>
                </Show>
              </div>
            }
          >
            <ul class="tk-list">
              <For each={visible()}>
                {(tk) => (
                  <li
                    class={`tk-item ${selectedId() === tk.id ? "sel" : ""} ${tk.status}`}
                  >
                    <button
                      class="tk-item-main"
                      onClick={() =>
                        setSelectedId(selectedId() === tk.id ? null : tk.id)
                      }
                      aria-expanded={selectedId() === tk.id}
                    >
                      <span class="tk-item-desc">
                        {tk.description || t("tickets.panel.noDesc")}
                      </span>
                      <code class="tk-item-sel">{tk.element.selector}</code>
                      <span class="tk-item-meta">{shortDate(tk.created)}</span>
                    </button>
                    <div class="tk-item-actions">
                      <button
                        class="tk-act"
                        onClick={() => void toggleResolved(tk)}
                        title={t(
                          tk.status === "resolved"
                            ? "tickets.action.reopen"
                            : "tickets.action.resolve",
                        )}
                      >
                        <IconCheck size={13} />
                      </button>
                      <button
                        class="tk-act danger"
                        onClick={() => void deleteOne(tk)}
                        title={t("tickets.action.delete")}
                      >
                        <IconTrash size={13} />
                      </button>
                    </div>

                    <Show when={selectedId() === tk.id}>
                      <div class="tk-detail">
                        <div class="tk-detail-row">
                          <span class="tk-detail-k">{t("tickets.modal.url")}</span>
                          <span class="tk-detail-v">{tk.url}</span>
                        </div>
                        <div class="tk-detail-row">
                          <span class="tk-detail-k">XPath</span>
                          <code class="tk-detail-v">{tk.element.xpath}</code>
                        </div>
                        {/* Text, never innerHTML — untrusted page markup. */}
                        <pre class="tk-detail-html">{tk.element.html}</pre>
                        <button
                          class="tk-copy"
                          onClick={() => void copyMarkdown(tk)}
                        >
                          {copied()
                            ? t("tickets.action.copied")
                            : t("tickets.action.copyMd")}
                        </button>
                      </div>
                    </Show>
                  </li>
                )}
              </For>
            </ul>
          </Show>
        </div>
      )}
    />
  );
}
