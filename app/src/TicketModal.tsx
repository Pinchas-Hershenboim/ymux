import { createResource, createSignal, onCleanup, onMount, Show } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { t } from "./i18n";
import { IconBug, IconClose, IconWarning } from "./icons";
import { loadProjectOverride, type ElementCapture } from "./browserDevMode";
import type { Ticket } from "./bindings/Ticket";
import type { ProjectResolution } from "./bindings/ProjectResolution";

// Finalizes a captured element into a ticket on disk.
//
// Opened by the `browser:ticket-captured` event (see App.tsx). The
// capture came from an untrusted page, so the element HTML is rendered
// as TEXT — never as markup — and the whole preview is collapsed by
// default: it can contain whatever the page was showing, which may be
// the user's own data.

interface Props {
  workspaceId: string;
  capture: ElementCapture;
  onClose: () => void;
  onSaved: (ticket: Ticket) => void;
}

export function TicketModal(p: Props) {
  const [description, setDescription] = createSignal("");
  const [saving, setSaving] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [showHtml, setShowHtml] = createSignal(false);
  // The snapshot is taken at right-click time, before this modal exists,
  // so the choice here is whether to KEEP it — not whether to take it.
  const [includeShot, setIncludeShot] = createSignal(true);

  // Resolve the destination BEFORE saving and show it. Writing into
  // someone's repo should never be a surprise, and when we cannot (a
  // remote workspace) the reason has to be visible, not silent.
  const [dest] = createResource(
    () => p.workspaceId,
    (ws) =>
      invoke<ProjectResolution>("tickets_resolve_project", {
        workspaceId: ws,
        projectOverride: loadProjectOverride(ws),
      }),
  );

  // Esc closes. Captured on the window so it wins over the page-level
  // shortcut handler, and stopped so it doesn't also reach it.
  onMount(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !saving()) {
        e.preventDefault();
        e.stopPropagation();
        p.onClose();
      }
    };
    window.addEventListener("keydown", onKey, true);
    onCleanup(() => window.removeEventListener("keydown", onKey, true));
  });

  const save = async () => {
    setSaving(true);
    setError(null);
    try {
      const created = await invoke<Ticket>("tickets_create", {
        workspaceId: p.workspaceId,
        projectOverride: loadProjectOverride(p.workspaceId),
        data: {
          url: p.capture.url,
          element: {
            xpath: p.capture.xpath,
            selector: p.capture.selector,
            html: p.capture.html,
            style: p.capture.style,
          },
          screenshot_data_url: includeShot() ? p.capture.shot : null,
          description: description(),
        },
      });
      p.onSaved(created);
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div class="ticket-modal-backdrop" onClick={() => !saving() && p.onClose()}>
      <div
        class="ticket-modal"
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
        aria-label={t("tickets.modal.title")}
      >
        <div class="ticket-modal-header">
          <span class="ticket-modal-title">
            <IconBug size={14} /> {t("tickets.modal.title")}
          </span>
          <button
            class="ticket-modal-x"
            onClick={p.onClose}
            title={t("common.close")}
            aria-label={t("common.close")}
          >
            <IconClose size={14} />
          </button>
        </div>

        <div class="ticket-modal-body">
          <div class="ticket-modal-field">
            <span class="ticket-modal-label">{t("tickets.modal.url")}</span>
            <div class="ticket-modal-value" title={p.capture.url}>
              {p.capture.url || "—"}
            </div>
          </div>

          <div class="ticket-modal-field">
            <span class="ticket-modal-label">{t("tickets.modal.selector")}</span>
            <code class="ticket-modal-code" title={p.capture.selector}>
              {p.capture.selector || "—"}
            </code>
          </div>

          <Show when={p.capture.shot}>
            {(shot) => (
              <div class="ticket-modal-field">
                <label class="ticket-modal-check">
                  <input
                    type="checkbox"
                    checked={includeShot()}
                    onChange={(e) => setIncludeShot(e.currentTarget.checked)}
                  />
                  {t("tickets.modal.shot.include")}
                </label>
                <Show when={includeShot()}>
                  {/* Rendered by the page, so it can be anything — it is
                      only ever shown as an image, never executed. */}
                  <img
                    class="ticket-modal-shot"
                    src={shot()}
                    alt={t("tickets.modal.shot.alt")}
                  />
                </Show>
              </div>
            )}
          </Show>

          <Show when={p.capture.html}>
            <div class="ticket-modal-field">
              <button
                class="ticket-modal-disclose"
                onClick={() => setShowHtml(!showHtml())}
                aria-expanded={showHtml()}
              >
                {showHtml()
                  ? t("tickets.modal.html.hide")
                  : t("tickets.modal.html.show", {
                      n: String(p.capture.html.length),
                    })}
              </button>
              {/* Text content, never innerHTML — this markup came from
                  an untrusted page. */}
              <Show when={showHtml()}>
                <pre class="ticket-modal-html">{p.capture.html}</pre>
              </Show>
            </div>
          </Show>

          <div class="ticket-modal-field">
            <label class="ticket-modal-label" for="ticket-modal-desc">
              {t("tickets.modal.description")}
            </label>
            <textarea
              id="ticket-modal-desc"
              class="ticket-modal-textarea"
              value={description()}
              placeholder={t("tickets.modal.descPlaceholder")}
              onInput={(e) => setDescription(e.currentTarget.value)}
              onKeyDown={(e) => {
                // Ctrl/Cmd+Enter saves; plain Enter stays a newline.
                if ((e.ctrlKey || e.metaKey) && e.key === "Enter") {
                  e.preventDefault();
                  if (description().trim() && !saving()) void save();
                }
                // Keep keystrokes out of the global shortcut handler.
                e.stopPropagation();
              }}
              rows={5}
              autofocus
            />
          </div>

          {/* Destination. Shown for every save, not just the fallback —
              the project association is the point of the ticket. */}
          <Show when={dest()}>
            {(d) => (
              <div class="ticket-modal-field">
                <span class="ticket-modal-label">
                  {t("tickets.modal.project")}
                </span>
                <Show
                  when={d().in_project}
                  fallback={
                    <div class="ticket-modal-fallback">
                      <IconWarning size={13} />
                      <div>
                        <div>{d().fallback_reason}</div>
                        <code class="ticket-modal-code">{d().tickets_dir}</code>
                      </div>
                    </div>
                  }
                >
                  <code class="ticket-modal-code" title={d().tickets_dir}>
                    {d().tickets_dir}
                  </code>
                </Show>
              </div>
            )}
          </Show>

          <Show when={error()}>
            <div class="ticket-modal-err" role="alert">
              {error()}
            </div>
          </Show>
        </div>

        <div class="ticket-modal-footer">
          <button
            class="ticket-modal-cancel"
            onClick={p.onClose}
            disabled={saving()}
          >
            {t("common.cancel")}
          </button>
          <button
            class="ticket-modal-save"
            disabled={saving() || !description().trim()}
            onClick={() => void save()}
          >
            {saving() ? t("common.saving") : t("tickets.modal.save")}
          </button>
        </div>
      </div>
    </div>
  );
}
