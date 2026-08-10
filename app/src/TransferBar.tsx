import { createSignal, For, Show } from "solid-js";
import { t } from "./i18n";
import {
  IconUpload,
  IconDownload,
  IconClose,
  IconChevronUp,
  IconChevronDown,
  IconWarning,
  IconCheck,
  IconBan,
} from "./icons";
import {
  transferList,
  cancelTransfer,
  dismissTransfer,
  fmtBytes,
  fmtSpeed,
  fmtEta,
  percent,
  type Transfer,
} from "./transferStore";

// Phase 81.C: progress strip for in-flight SFTP transfers.
//
// Renders at the bottom of the File Manager. One transfer shows as a
// single row; several collapse behind a count the user can expand. The
// component is presentational — every piece of state comes from
// transferStore, which the App-root listener feeds.

function directionIcon(t: Transfer) {
  return t.direction === "upload" ? <IconUpload size={13} /> : <IconDownload size={13} />;
}

function TransferRow(props: { tx: Transfer }) {
  const tx = () => props.tx;
  const pct = () => percent(tx());
  const active = () => tx().state === "active" || tx().state === "canceling";

  return (
    <div class={`tx-row tx-row-${tx().state}`}>
      <span class="tx-dir" aria-hidden="true">
        {directionIcon(tx())}
      </span>
      <span class="tx-name" title={tx().path || tx().name}>
        {tx().name}
      </span>

      <div
        class={`tx-track ${pct() === null && active() ? "tx-track-indeterminate" : ""}`}
        role="progressbar"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={pct() ?? undefined}
        aria-label={tx().name}
      >
        <div class="tx-fill" style={{ width: `${pct() ?? 100}%` }} />
      </div>

      <span class="tx-meta">
        <Show when={tx().state === "active"}>
          <Show when={pct() !== null} fallback={<>{fmtBytes(tx().done)}</>}>
            {pct()}% · {fmtBytes(tx().done)} / {fmtBytes(tx().total)}
          </Show>
          <Show when={fmtSpeed(tx().speedBps)}>
            {" · "}
            {fmtSpeed(tx().speedBps)}
          </Show>
          <Show when={fmtEta(tx())}>
            {" · "}
            {fmtEta(tx())}
          </Show>
        </Show>
        <Show when={tx().state === "canceling"}>{t("fm.transfer.canceling")}</Show>
        <Show when={tx().state === "done"}>
          <IconCheck size={12} /> {fmtBytes(tx().done)}
        </Show>
        <Show when={tx().state === "canceled"}>
          <IconBan size={12} /> {t("fm.transfer.canceled")}
        </Show>
        <Show when={tx().state === "error"}>
          <span class="tx-error" title={tx().error}>
            <IconWarning size={12} /> {tx().error ?? t("fm.transfer.failed")}
          </span>
        </Show>
      </span>

      {/* Cancel while it runs, dismiss once it stopped. A finished-clean
          row clears itself, so the button only matters for errors. */}
      <Show
        when={tx().state === "active"}
        fallback={
          <Show when={tx().state === "error"}>
            <button
              class="tx-btn"
              title={t("fm.transfer.dismiss")}
              aria-label={t("fm.transfer.dismiss")}
              onClick={() => dismissTransfer(tx().id)}
            >
              <IconClose size={12} />
            </button>
          </Show>
        }
      >
        <button
          class="tx-btn"
          title={t("fm.transfer.cancel")}
          aria-label={t("fm.transfer.cancel")}
          onClick={() => void cancelTransfer(tx().id)}
        >
          <IconClose size={12} />
        </button>
      </Show>
    </div>
  );
}

export function TransferBar() {
  const [expanded, setExpanded] = createSignal(false);
  const rows = () => transferList();
  const multi = () => rows().length > 1;

  return (
    <Show when={rows().length > 0}>
      <div class="tx-bar">
        <Show when={multi()}>
          <button
            class="tx-summary"
            onClick={() => setExpanded((v) => !v)}
            aria-expanded={expanded()}
          >
            {expanded() ? <IconChevronDown size={12} /> : <IconChevronUp size={12} />}
            {t("fm.transfer.count", { n: rows().length })}
          </button>
        </Show>
        {/* A single transfer always shows its row; a batch shows the
            newest one collapsed so the bar still reports movement. */}
        <Show
          when={!multi() || expanded()}
          fallback={<TransferRow tx={rows()[rows().length - 1]} />}
        >
          <div class="tx-rows">
            <For each={rows()}>{(tx) => <TransferRow tx={tx} />}</For>
          </div>
        </Show>
      </div>
    </Show>
  );
}
