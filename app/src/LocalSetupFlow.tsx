import { createSignal, For, Show, onMount, onCleanup, createMemo } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { t } from "./i18n";
import { isWindows } from "./platform";
import { IconClose, IconCheck, IconCircle } from "./icons";
import type { ProvisioningError, StepProgress, RunHandle } from "./provisioningTypes";

// Phase 80 (unified setup wizard): "local → new" — the smart local
// (Windows) setup flow. Detects what's installed (git/node/Claude Code/
// codex/gemini/WSL+tmux), offers to install what's missing (winget /
// official installers / npm -g), installs ymux hooks for local Claude
// Code, and optionally provisions a PERSISTENT local environment: tmux
// inside WSL + the ymux Linux CLI, then creates a `wsl` workspace whose
// panes survive app restarts (NOT Windows restarts — the copy says so).
//
// macOS port: same three screens, branched on `isWindows()`. On mac the
// inspect's `winget` slot carries Homebrew, the hooks checkbox is hidden
// (the winmux CLI isn't bundled for mac yet), and the WSL group becomes a
// two-step local tmux chain (`MAC_CHAIN`) that ends in a plain local
// workspace. No username / elevation / reboot UI — preflight never asks
// for elevation there. The Windows path is untouched.
//
// Backend contract (src-tauri/src/local_setup.rs): commands
// `local_setup_inspect` / `local_setup_start`, events
// `local-setup:progress` (same StepProgress payload as provisioning) and
// `local-setup:complete`. The payload types are ts-rs generated bindings;
// all other contract knowledge (step names, event names) stays here.

import type { ToolStatus } from "./bindings/ToolStatus";
import type { LocalSetupInspect } from "./bindings/LocalSetupInspect";
import type { LocalSetupResult } from "./bindings/LocalSetupResult";
import type { WslPreflight } from "./bindings/WslPreflight";

interface Props {
  onCreated: (workspaceId: string) => void;
  onClose: () => void;
}

// The tool steps, in execution order (same kinds on both platforms).
// `needsWinget` rows are disabled (with a hint) when the package manager
// itself is missing — winget on Windows (LTSC/Server), Homebrew on mac
// (the inspect reports brew in the `winget` slot there); Claude Code
// deliberately does NOT depend on it — its official installer is a
// standalone script on both platforms.
const TOOL_ROWS: {
  step: string;
  tool: keyof Pick<LocalSetupInspect, "git" | "node" | "claude" | "codex" | "gemini">;
  needsWinget: boolean;
  needsNode: boolean;
}[] = [
  { step: "InstallGit", tool: "git", needsWinget: true, needsNode: false },
  { step: "InstallNodejs", tool: "node", needsWinget: true, needsNode: false },
  { step: "InstallClaudeCode", tool: "claude", needsWinget: false, needsNode: false },
  { step: "InstallCodex", tool: "codex", needsWinget: false, needsNode: true },
  { step: "InstallGemini", tool: "gemini", needsWinget: false, needsNode: true },
];

// The WSL persistence chain, in execution order. The flow sends only the
// steps the inspect says are actually needed; the backend additionally
// skips no-op steps, so this is belt-and-suspenders.
const WSL_CHAIN = [
  "InstallWsl",
  "CreateWslUser",
  "EnsureDistroReady",
  "InstallTmuxInWsl",
  "DeployYmuxCliToWsl",
  "DeployTmuxConfToWsl",
  // Before the hooks step, which registers ymux's hooks with Claude:
  // installing hooks for an agent that is not there produced a green run
  // and then `claude: command not found` in the first pane.
  "InstallClaudeCodeInWsl",
  "InstallHooksInWsl",
] as const;

// The macOS persistence chain: tmux via Homebrew + the winmux tmux.conf,
// then a plain local workspace. Same skip rule as WSL_CHAIN — the flow
// only sends what the inspect says is missing.
const MAC_CHAIN = ["InstallTmuxLocal", "DeployTmuxConfLocal"] as const;

export function LocalSetupFlow(p: Props) {
  type FlowStep = "inspect" | "execute" | "done";
  const [step, setStep] = createSignal<FlowStep>("inspect");
  // Resolved once before first render (platform.ts) — a plain boolean is
  // enough, nothing here needs to react to it.
  const mac = !isWindows();

  const [inspecting, setInspecting] = createSignal(true);
  const [inspectErr, setInspectErr] = createSignal<string | null>(null);
  const [inspect, setInspect] = createSignal<LocalSetupInspect | null>(null);
  // Whether enabling the WSL features will need a UAC prompt, known
  // before the run instead of discovered by failing halfway through.
  const [preflight, setPreflight] = createSignal<WslPreflight | null>(null);
  const [elevationAcked, setElevationAcked] = createSignal(false);
  const [confirmElevation, setConfirmElevation] = createSignal(false);
  const [restarting, setRestarting] = createSignal(false);
  const [restartErr, setRestartErr] = createSignal<string | null>(null);

  // Which tool steps the user keeps checked. Seeded from the inspect
  // result (missing → checked).
  const [checkedTools, setCheckedTools] = createSignal<Set<string>>(new Set());
  const [installLocalHooks, setInstallLocalHooks] = createSignal(true);
  // The WSL persistent-environment group (tmux-backed panes).
  const [wslGroup, setWslGroup] = createSignal(true);
  const [wslUsername, setWslUsername] = createSignal("");
  const [workspaceName, setWorkspaceName] = createSignal(mac ? "tmux" : "WSL");
  const [workspaceCwd, setWorkspaceCwd] = createSignal("");

  // Execute-step state (mirrors ProvisionNewServerFlow).
  const [runId, setRunId] = createSignal<string | null>(null);
  const [plannedSteps, setPlannedSteps] = createSignal<string[]>([]);
  const [stepStates, setStepStates] = createSignal<Record<number, StepProgress>>({});
  const [result, setResult] = createSignal<LocalSetupResult | null>(null);

  let unlisten: UnlistenFn | null = null;
  let unlistenComplete: UnlistenFn | null = null;

  onMount(async () => {
    // Inspect runs concurrently with listener registration — the
    // listeners only need to be live before local_setup_start (a user
    // click), not before the read-only inspect.
    void runInspect();
    unlisten = await listen<StepProgress>("local-setup:progress", (e) => {
      setStepStates((prev) => ({ ...prev, [e.payload.step_index]: e.payload }));
    });
    unlistenComplete = await listen<LocalSetupResult>("local-setup:complete", (e) => {
      setResult(e.payload);
      setStep("done");
    });
  });
  onCleanup(() => {
    if (unlisten) unlisten();
    if (unlistenComplete) unlistenComplete();
  });

  const runInspect = async () => {
    setInspecting(true);
    setInspectErr(null);
    try {
      const r = await invoke<LocalSetupInspect>("local_setup_inspect", {
        distro: null,
      });
      setInspect(r);
      // Read-only; a failure here must not block the wizard, it only
      // costs us the up-front UAC warning.
      try {
        setPreflight(await invoke<WslPreflight>("local_setup_preflight"));
      } catch {
        setPreflight(null);
      }
      // Pre-check every missing tool (that its dependencies allow).
      const pre = new Set<string>();
      for (const row of TOOL_ROWS) {
        if (!r[row.tool].present && !(row.needsWinget && !r.winget.present)) {
          pre.add(row.step);
        }
      }
      setCheckedTools(pre);
      // Hooks only make sense when Claude Code exists or is being installed.
      // On mac the checkbox is hidden (no bundled CLI yet) — never send it.
      setInstallLocalHooks(!mac && (r.claude.present || pre.has("InstallClaudeCode")));
      // mac: tmux comes from Homebrew, so without brew AND without tmux
      // the chain is guaranteed to fail — start it unchecked (and the
      // group checkbox is disabled below) instead of failing mid-run.
      if (mac && !r.winget.present && !r.mac?.tmux.present) setWslGroup(false);
      if (!wslUsername().trim() && r.wsl.default_distro) {
        // Existing distro → an existing user likely exists too; the
        // backend only creates one when OOBE never ran.
        setWslUsername("");
      }
    } catch (e) {
      setInspectErr(String(e));
    } finally {
      setInspecting(false);
    }
  };

  const toggleTool = (stepId: string) => {
    const next = new Set(checkedTools());
    if (next.has(stepId)) next.delete(stepId);
    else next.add(stepId);
    setCheckedTools(next);
  };

  // Build the effective step list: checked tools (in canonical order) +
  // local hooks + only the *needed* parts of the WSL chain.
  const buildSteps = (): string[] => {
    const r = inspect();
    const steps: string[] = [];
    for (const row of TOOL_ROWS) {
      if (checkedTools().has(row.step)) steps.push(row.step);
    }
    if (installLocalHooks()) steps.push("InstallLocalHooks");
    if (wslGroup() && r && mac) {
      for (const s of MAC_CHAIN) {
        if (s === "InstallTmuxLocal" && r.mac?.tmux.present) continue;
        if (s === "DeployTmuxConfLocal" && r.mac?.tmux_conf_ok) continue;
        steps.push(s);
      }
    } else if (wslGroup() && r) {
      for (const s of WSL_CHAIN) {
        if (s === "InstallWsl" && r.wsl.wsl_ready && r.wsl.distros.length > 0) continue;
        if (s === "InstallTmuxInWsl" && r.wsl.tmux_installed === true) continue;
        if (s === "DeployYmuxCliToWsl" && r.wsl.ymux_cli_state === "ok") continue;
        if (s === "DeployTmuxConfToWsl" && r.wsl.tmux_conf_ok === true) continue;
        // claude_inside is `command -v claude` inside the distro, not the
        // presence of ~/.claude — that directory is created by the hooks
        // step itself, so the old probe would have skipped this forever.
        if (s === "InstallClaudeCodeInWsl" && r.wsl.claude_inside === true) continue;
        steps.push(s);
      }
    }
    return steps;
  };

  // mac: a machine that already has tmux + conf leaves the step list empty,
  // yet the checked group still asks for the workspace — let that run go
  // through (the backend then only creates the workspace).
  const canStart = () => buildSteps().length > 0 || (mac && wslGroup());

  const startRun = async () => {
    const steps = buildSteps();
    if (!canStart()) return;
    // Installing the WSL platform needs admin. Say so — with the reboot
    // caveat — before a UAC dialog appears out of nowhere mid-run.
    if (
      steps.includes("InstallWsl") &&
      preflight()?.needs_elevation &&
      !elevationAcked()
    ) {
      setConfirmElevation(true);
      return;
    }
    setPlannedSteps(steps);
    setStepStates({});
    setResult(null);
    setStep("execute");
    try {
      const handle = await invoke<RunHandle>("local_setup_start", {
        input: {
          steps,
          distro: mac ? null : (inspect()?.wsl.default_distro ?? null),
          wsl_username: mac ? null : (wslUsername().trim() || null),
          workspace_name: workspaceName().trim() || null,
          create_workspace: wslGroup(),
          workspace_cwd: workspaceCwd().trim() || null,
        },
      });
      setRunId(handle.run_id);
    } catch (e) {
      setStepStates({
        0: {
          run_id: "",
          step_index: 0,
          step_kind: "spawn",
          state: "failed",
          log_chunk: "",
          message: String(e),
          timestamp_iso: new Date().toISOString(),
        },
      });
    }
  };

  const stateBadge = (s?: StepProgress) => {
    if (!s) return { cls: "pending", icon: IconCircle };
    if (s.state === "done") return { cls: "ok", icon: IconCheck };
    if (s.state === "failed") return { cls: "err", icon: IconClose };
    if (s.state === "running") return { cls: "running", icon: null };
    // Without this the backend's "skipped" fell through to "pending" —
    // an abandoned WSL chain looked like it simply hadn't started yet.
    if (s.state === "skipped") return { cls: "skipped", icon: IconClose };
    return { cls: "pending", icon: IconCircle };
  };

  const toolStatusLine = (ts: ToolStatus): string =>
    ts.present
      ? `${t("localSetup.status.detected")}${ts.version ? ` · ${ts.version}` : ""}`
      : t("localSetup.status.missing");

  const wslStatusSummary = createMemo(() => {
    const r = inspect();
    if (!r) return "";
    if (mac) {
      const m = r.mac;
      if (!m || !m.tmux.present) return t("localSetup.mac.tmux.none");
      const ver = m.tmux.version ? ` ${m.tmux.version}` : "";
      return `tmux ✓${ver} · ${m.tmux_conf_ok ? "conf ✓" : "conf ✗"}`;
    }
    if (!r.wsl.wsl_ready || r.wsl.distros.length === 0) return t("localSetup.wsl.none");
    const d = r.wsl.default_distro ?? r.wsl.distros[0];
    const tmux = r.wsl.tmux_installed ? "tmux ✓" : "tmux ✗";
    const cli = r.wsl.ymux_cli_state === "ok" ? "ymux ✓" : "ymux ✗";
    return `${d} · ${tmux} · ${cli}`;
  });

  return (
    <>
      <p class="provisioning-substep">
        {step() === "inspect" && t("localSetup.step.inspect")}
        {step() === "execute" && t("provisioning.step.execute")}
        {step() === "done" && t("provisioning.step.done")}
      </p>

      {/* Step 1: inspect + choose */}
      <Show when={step() === "inspect"}>
        <Show when={inspecting()}>
          <p class="settings-hint">{t("localSetup.inspecting")}</p>
        </Show>
        <Show when={inspectErr()}>
          <div class="wizard-test-result err">
            <div class="wizard-test-line"><IconClose size={14} /> {inspectErr()}</div>
          </div>
        </Show>
        <Show when={!inspecting() && inspect()}>
          {(r) => (
            <>
              <p class="settings-hint">{t("localSetup.hint")}</p>

              <h4 class="provisioning-h4">{t("localSetup.tools.title")}</h4>
              <Show when={!r().winget.present}>
                <p class="settings-hint">
                  {t(mac ? "localSetup.mac.brew_missing" : "localSetup.hint.winget_missing")}
                </p>
              </Show>
              <div class="provisioning-steps">
                <For each={TOOL_ROWS}>
                  {(row) => {
                    const ts = () => r()[row.tool];
                    const blocked = () =>
                      (row.needsWinget && !r().winget.present) ||
                      (row.needsNode &&
                        !r().node.present &&
                        !checkedTools().has("InstallNodejs"));
                    return (
                      <label class="provisioning-step-row">
                        <input
                          type="checkbox"
                          checked={ts().present || checkedTools().has(row.step)}
                          disabled={ts().present || blocked()}
                          onChange={() => toggleTool(row.step)}
                        />
                        <span>
                          {t(`localSetup.step.${row.step}`)}
                          <span class="provisioning-mode-hint"> — {toolStatusLine(ts())}</span>
                        </span>
                      </label>
                    );
                  }}
                </For>
                {/* mac: the winmux CLI isn't bundled there yet, so hooks
                    can't be installed — say so instead of a dead checkbox. */}
                <Show
                  when={!mac}
                  fallback={
                    <p class="settings-hint">{t("localSetup.mac.hooks_unavailable")}</p>
                  }
                >
                  <label class="provisioning-step-row">
                    <input
                      type="checkbox"
                      checked={installLocalHooks()}
                      disabled={!r().claude.present && !checkedTools().has("InstallClaudeCode")}
                      onChange={() => setInstallLocalHooks(!installLocalHooks())}
                    />
                    <span>
                      {t("localSetup.step.InstallLocalHooks")}
                      <span class="provisioning-mode-hint">
                        {" — "}
                        {r().local_hooks_version
                          ? `${t("localSetup.status.detected")} · v${r().local_hooks_version}`
                          : t("localSetup.status.missing")}
                      </span>
                    </span>
                  </label>
                </Show>
              </div>

              <h4 class="provisioning-h4">
                {t(mac ? "localSetup.mac.tmux.title" : "localSetup.wsl.title")}
              </h4>
              <label class="provisioning-step-row">
                <input
                  type="checkbox"
                  checked={wslGroup()}
                  disabled={mac && !r().winget.present && !r().mac?.tmux.present}
                  onChange={() => setWslGroup(!wslGroup())}
                />
                <span>
                  {t(mac ? "localSetup.mac.tmux.group" : "localSetup.wsl.group")}
                  <span class="provisioning-mode-hint"> — {wslStatusSummary()}</span>
                </span>
              </label>
              <p class="settings-hint">
                {t(mac ? "localSetup.mac.persistence_scope" : "localSetup.persistence_scope")}
              </p>
              <Show when={wslGroup()}>
                {/* Stated up front: without an elevated token the WSL
                    feature enable cannot happen silently. */}
                <Show when={preflight()?.needs_elevation}>
                  <div class="prov-error-card">
                    <div class="prov-error-title">
                      {t("localSetup.preflight.admin.title")}
                    </div>
                    <p class="prov-error-body">{t("localSetup.preflight.admin.body")}</p>
                  </div>
                </Show>
                <Show when={!mac && (!r().wsl.wsl_ready || r().wsl.distros.length === 0)}>
                  <p class="settings-hint">{t("localSetup.hint.uac")}</p>
                  <label>
                    <span>{t("localSetup.field.wsl_username")}</span>
                    <input
                      value={wslUsername()}
                      onInput={(e) => setWslUsername(e.currentTarget.value)}
                      placeholder={t("localSetup.field.wsl_username.placeholder")}
                    />
                  </label>
                </Show>
                <label>
                  <span>{t("provisioning.field.workspace_name")}</span>
                  <input
                    value={workspaceName()}
                    onInput={(e) => setWorkspaceName(e.currentTarget.value)}
                  />
                </label>
                <label>
                  <span>{t("ws.create.cwd.label")}</span>
                  <input
                    value={workspaceCwd()}
                    onInput={(e) => setWorkspaceCwd(e.currentTarget.value)}
                    placeholder={t("ws.create.cwd.placeholder")}
                  />
                </label>
              </Show>

              {/* The "this needs admin, continue?" gate. Replaces the
                  action row so the run cannot start behind it. */}
              <Show
                when={confirmElevation()}
                fallback={
                  <div class="modal-buttons">
                    <button onClick={p.onClose}>{t("common.cancel")}</button>
                    <button
                      class="primary"
                      disabled={!canStart()}
                      onClick={() => void startRun()}
                    >
                      {t("localSetup.btn.install")}
                    </button>
                  </div>
                }
              >
                <div class="prov-error-card">
                  <div class="prov-error-title">
                    {t("localSetup.preflight.confirm.title")}
                  </div>
                  <p class="prov-error-body">{t("localSetup.preflight.confirm.body")}</p>
                </div>
                <div class="modal-buttons">
                  <button onClick={() => setConfirmElevation(false)}>
                    {t("common.cancel")}
                  </button>
                  <button
                    class="primary"
                    onClick={() => {
                      setElevationAcked(true);
                      setConfirmElevation(false);
                      void startRun();
                    }}
                  >
                    {t("localSetup.preflight.confirm.btn")}
                  </button>
                </div>
              </Show>
            </>
          )}
        </Show>
      </Show>

      {/* Step 2: execute — step cards, same CSS as provisioning */}
      <Show when={step() === "execute"}>
        <p class="settings-hint">
          {t("provisioning.run.label", {
            id: runId() ?? t("provisioning.run.starting"),
            host: "localhost",
          })}
        </p>
        <div class="provisioning-step-list">
          <For each={plannedSteps()}>
            {(stepId, idx) => {
              const s = createMemo(() => stepStates()[idx()]);
              const b = createMemo(() => stateBadge(s()));
              return (
                <div class={`provisioning-step-card state-${b().cls}`}>
                  <div class="provisioning-step-head">
                    <span class={`provisioning-step-icon ${b().cls}`}>
                      {(() => {
                        const I = b().icon;
                        return I ? <I size={14} /> : "…";
                      })()}
                    </span>
                    <span class="provisioning-step-label">
                      {t(`localSetup.step.${stepId}`)}
                    </span>
                  </div>
                  {/* 3010: the features are staged but dormant. Retrying
                      is pointless, so this card offers a restart instead. */}
                  <Show when={s()?.error?.kind === "RebootRequired"}>
                    {(() => {
                      const rb = s()!.error as Extract<
                        ProvisioningError,
                        { kind: "RebootRequired" }
                      >;
                      return (
                        <div class="prov-error-card">
                          <div class="prov-error-title">
                            {t("localSetup.error.reboot_required")}
                          </div>
                          <p class="prov-error-body">{rb.details.hint}</p>
                          <Show when={restartErr()}>
                            <p class="prov-error-hint">{restartErr()}</p>
                          </Show>
                          {/* Rebooting can destroy unsaved work in other
                              apps, so it takes two explicit clicks and
                              doing nothing is always a valid answer. */}
                          <p class="prov-error-hint">
                            {t("localSetup.hint.restart_later")}
                          </p>
                          <div class="modal-buttons">
                            <Show
                              when={restarting()}
                              fallback={
                                <button onClick={() => setRestarting(true)}>
                                  {t("localSetup.btn.restart_now")}
                                </button>
                              }
                            >
                              <button onClick={() => setRestarting(false)}>
                                {t("common.cancel")}
                              </button>
                              <button
                                class="danger"
                                onClick={() => {
                                  setRestartErr(null);
                                  invoke("restart_windows").catch((e) =>
                                    setRestartErr(String(e))
                                  );
                                }}
                              >
                                {t("localSetup.btn.restart_confirm")}
                              </button>
                            </Show>
                          </div>
                        </div>
                      );
                    })()}
                  </Show>
                  {/* ElevationRequired gets an instruction card (UAC
                      declined / WSL feature-enable needs a reboot). */}
                  <Show
                    when={s()?.error?.kind === "ElevationRequired"}
                    fallback={
                      <Show
                        when={s()?.error?.kind === "StepFailed"}
                        fallback={
                          <Show when={s()?.message || s()?.log_chunk}>
                            <pre class="provisioning-step-log">
                              {s()?.message ? `${s()?.message}\n` : ""}
                              {s()?.log_chunk ?? ""}
                            </pre>
                          </Show>
                        }
                      >
                        {(() => {
                          const sf = s()!.error as Extract<
                            ProvisioningError,
                            { kind: "StepFailed" }
                          >;
                          return (
                            <div class="prov-step-failed">
                              <div class="prov-step-failed-head">
                                {t("prov.error.stepFailed.title", { step: sf.details.step })}
                                <span class="prov-step-exit">exit {sf.details.exit_code}</span>
                              </div>
                              <pre class="prov-step-stderr">{sf.details.stderr}</pre>
                            </div>
                          );
                        })()}
                      </Show>
                    }
                  >
                    {(() => {
                      const er = s()!.error as Extract<
                        ProvisioningError,
                        { kind: "ElevationRequired" }
                      >;
                      return (
                        <div class="prov-error-card">
                          <div class="prov-error-title">
                            {t("localSetup.error.elevation_required")}
                          </div>
                          <p class="prov-error-body">{er.details.hint}</p>
                          <p class="prov-error-hint">{t("localSetup.hint.wsl_reboot")}</p>
                        </div>
                      );
                    })()}
                  </Show>
                </div>
              );
            }}
          </For>
        </div>
        <div class="modal-buttons">
          <button onClick={() => setStep("done")}>{t("provisioning.btn.mark_done")}</button>
        </div>
      </Show>

      {/* Step 3: done */}
      <Show when={step() === "done"}>
        {/* A failed WSL chain skips workspace creation silently; saying
            "finished" there is what made a broken install look clean. */}
        <Show
          when={(result()?.failed_steps.length ?? 0) > 0}
          fallback={<p>{t("localSetup.done.message")}</p>}
        >
          <div class="prov-error-card">
            <div class="prov-error-title">{t("localSetup.done.failed.title")}</div>
            <p class="prov-error-body">
              {t("localSetup.done.failed.body", {
                steps: result()!
                  .failed_steps.map((s) => t(`localSetup.step.${s}`))
                  .join(", "),
              })}
            </p>
            <Show when={result()!.skipped_steps.length > 0}>
              <p class="prov-error-hint">
                {t("localSetup.done.failed.skipped", {
                  steps: result()!
                    .skipped_steps.map((s) => t(`localSetup.step.${s}`))
                    .join(", "),
                })}
              </p>
            </Show>
            <Show when={!result()!.wsl_chain_ok}>
              <p class="prov-error-hint">
                {t(
                  mac
                    ? "localSetup.mac.done.no_workspace"
                    : "localSetup.done.failed.no_workspace"
                )}
              </p>
            </Show>
          </div>
        </Show>
        <Show when={result()?.workspace_id}>
          <div class="wizard-test-result ok">
            <div class="wizard-test-line">
              {t("provisioning.done.workspace_created", {
                name: result()!.workspace_name ?? "",
              })}
            </div>
          </div>
        </Show>
        <p class="settings-hint">
          {t(mac ? "localSetup.mac.persistence_scope" : "localSetup.persistence_scope")}
        </p>
        <div class="modal-buttons">
          <Show
            when={result()?.workspace_id}
            fallback={
              <button class="primary" onClick={p.onClose}>{t("common.close")}</button>
            }
          >
            <button
              class="primary"
              onClick={() => {
                const id = result()!.workspace_id!;
                p.onCreated(id);
                p.onClose();
              }}
            >
              {t("provisioning.done.btn.open_now")}
            </button>
          </Show>
        </div>
      </Show>
    </>
  );
}
