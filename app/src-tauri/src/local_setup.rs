//! Phase 80: local smart setup — detection + install engine for the
//! "local → new" wizard flow, plus the shared hidden-console process
//! helpers every wsl.exe / winget / npm invocation in the app goes
//! through (nothing in src/ set CREATE_NO_WINDOW before this module;
//! spawning wsl.exe from a GUI app without it flashes a console).
//!
//! The engine mirrors provisioning.rs deliberately: same StepProgress
//! payload (so the wizard reuses the step-card UI verbatim), same
//! keep-going-on-failure per-step semantics, events
//! `local-setup:progress` / `local-setup:complete`.
//!
//! Everything here follows Rule #3: argv arrays only. Values interpolated
//! into POSIX scripts run inside a distro go through
//! `winmux_core::shell_quote` exclusively.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use tauri::{AppHandle, Emitter, State};
use winmux_core::shell_quote;

use crate::provisioning::{ProvisioningError, RunHandle, StepProgress};
use crate::{dlog, AppState};

/// CREATE_NO_WINDOW — suppress the console window a GUI-subsystem parent
/// would otherwise flash when spawning a console-subsystem child.
#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Build a hidden-console tokio Command. ALL wsl.exe / winget / npm /
/// probe invocations go through this (or `wsl_cmd`).
pub(crate) fn hidden_cmd(program: &str) -> tokio::process::Command {
    let mut c = tokio::process::Command::new(program);
    #[cfg(target_os = "windows")]
    {
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c.stdin(std::process::Stdio::null());
    c.stdout(std::process::Stdio::piped());
    c.stderr(std::process::Stdio::piped());
    c
}

/// wsl.exe with UTF-8 output forced and a hidden console. wsl.exe emits
/// UTF-16LE by default; WSL_UTF8=1 (supported since WSL 0.64) makes the
/// management-command output parseable. Output parsing must still strip
/// stray NULs for very old inbox WSL builds that ignore the variable.
pub(crate) fn wsl_cmd() -> tokio::process::Command {
    let mut c = hidden_cmd("wsl.exe");
    c.env("WSL_UTF8", "1");
    c
}

/// Strip UTF-16 leftovers (NULs) + CRs from wsl.exe output. See wsl_cmd.
pub(crate) fn clean_wsl_output(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .chars()
        .filter(|c| *c != '\0' && *c != '\r')
        .collect()
}

/// Run a script inside a distro via `wsl.exe [-d <distro>] [-u <user>]
/// -- sh -lc <script>` and return (exit_code, merged stdout+stderr).
/// `script` must be a static string or built exclusively with
/// `winmux_core::shell_quote` for interpolated values (same discipline
/// as the remote tmux path).
pub(crate) async fn wsl_exec(
    distro: Option<&str>,
    user: Option<&str>,
    script: &str,
) -> Result<(i32, String), String> {
    let mut c = wsl_cmd();
    if let Some(d) = distro {
        if !d.is_empty() {
            c.arg("-d").arg(d);
        }
    }
    if let Some(u) = user {
        c.arg("-u").arg(u);
    }
    c.arg("--").arg("sh").arg("-lc").arg(script);
    let out = c.output().await.map_err(|e| format!("wsl.exe spawn: {e}"))?;
    let mut text = clean_wsl_output(&out.stdout);
    let err = clean_wsl_output(&out.stderr);
    if !err.is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&err);
    }
    Ok((out.status.code().unwrap_or(-1), text))
}

/// Run any hidden command to completion with a wall-clock timeout,
/// returning (exit_code, merged output). A timeout surfaces as Err —
/// per-step handling turns it into a failed step, never a hang.
async fn run_capture(
    mut c: tokio::process::Command,
    what: &str,
    timeout_secs: u64,
) -> Result<(i32, String), String> {
    let fut = async {
        let out = c.output().await.map_err(|e| format!("{what}: spawn: {e}"))?;
        let mut text = clean_wsl_output(&out.stdout);
        let err = clean_wsl_output(&out.stderr);
        if !err.is_empty() {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&err);
        }
        Ok::<(i32, String), String>((out.status.code().unwrap_or(-1), text))
    };
    tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), fut)
        .await
        .map_err(|_| format!("{what}: timed out after {timeout_secs}s"))?
}

// ─── Detection ───────────────────────────────────────────────────────

#[derive(Clone, Serialize, ts_rs::TS)]
#[ts(export, export_to = "../../src/bindings/")]
pub(crate) struct ToolStatus {
    pub present: bool,
    pub version: Option<String>,
    pub path: Option<String>,
}

impl ToolStatus {
    fn missing() -> Self {
        Self {
            present: false,
            version: None,
            path: None,
        }
    }
}

#[derive(Clone, Serialize, ts_rs::TS)]
#[ts(export, export_to = "../../src/bindings/")]
pub(crate) struct WslInspect {
    pub wsl_present: bool,
    pub wsl_ready: bool,
    pub distros: Vec<String>,
    pub default_distro: Option<String>,
    pub tmux_installed: Option<bool>,
    /// "ok" | "stale" | "missing" — winmux CLI inside the distro vs the
    /// embedded manifest sha.
    pub winmux_cli_state: Option<String>,
    pub tmux_conf_ok: Option<bool>,
    pub claude_inside: Option<bool>,
    pub hooks_version_inside: Option<String>,
}

#[derive(Clone, Serialize, ts_rs::TS)]
#[ts(export, export_to = "../../src/bindings/")]
pub(crate) struct LocalSetupInspect {
    pub winget: ToolStatus,
    pub git: ToolStatus,
    pub node: ToolStatus,
    pub npm: ToolStatus,
    pub claude: ToolStatus,
    pub codex: ToolStatus,
    pub gemini: ToolStatus,
    pub wsl: WslInspect,
    pub local_hooks_version: Option<String>,
    pub local_claude_dir: bool,
}

/// Probe one tool: PATH lookup over the candidate exe names (PATHEXT-
/// aware via local_wizard::which), then optional canonical fallback
/// paths, then a best-effort `--version` with a short timeout. Never
/// fails — a broken tool just reads as missing/version-less.
async fn probe_tool(candidates: &[&str], canonical: &[PathBuf]) -> ToolStatus {
    let mut found: Option<PathBuf> = None;
    for c in candidates {
        if let Some(p) = crate::local_wizard::which(c) {
            found = Some(p);
            break;
        }
    }
    if found.is_none() {
        for p in canonical {
            if p.is_file() {
                found = Some(p.clone());
                break;
            }
        }
    }
    let Some(path) = found else {
        return ToolStatus::missing();
    };
    let mut vc = hidden_cmd(&path.to_string_lossy());
    vc.arg("--version");
    let version = run_capture(vc, "version probe", 5)
        .await
        .ok()
    .and_then(|(code, out)| {
        if code == 0 {
            out.lines().next().map(|l| l.trim().to_string())
        } else {
            None
        }
    });
    ToolStatus {
        present: true,
        version,
        path: Some(path.to_string_lossy().to_string()),
    }
}

fn program_files() -> PathBuf {
    PathBuf::from(std::env::var("ProgramFiles").unwrap_or_else(|_| "C:\\Program Files".into()))
}

fn user_profile() -> PathBuf {
    PathBuf::from(std::env::var("USERPROFILE").unwrap_or_else(|_| "C:\\".into()))
}

/// Canonical install locations probed AFTER PATH — a tool winget just
/// installed isn't on our process's (stale) PATH.
fn canonical_git() -> Vec<PathBuf> {
    vec![program_files().join("Git").join("cmd").join("git.exe")]
}
fn canonical_node() -> Vec<PathBuf> {
    vec![program_files().join("nodejs").join("node.exe")]
}
fn canonical_npm() -> Vec<PathBuf> {
    vec![program_files().join("nodejs").join("npm.cmd")]
}
fn canonical_claude() -> Vec<PathBuf> {
    // The official install.ps1 target (verified against
    // code.claude.com/docs/en/setup).
    vec![user_profile()
        .join(".local")
        .join("bin")
        .join("claude.exe")]
}

/// Parse `wsl -l -v` — the default distro carries a `*` marker (stable
/// across Windows display languages, unlike `wsl --status` text).
fn parse_wsl_list_verbose(text: &str) -> (Vec<String>, Option<String>) {
    let mut distros = Vec::new();
    let mut default = None;
    for (i, line) in text.lines().enumerate() {
        if i == 0 {
            continue; // header
        }
        let starred = line.trim_start().starts_with('*');
        let name = line
            .trim_start()
            .trim_start_matches('*')
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_string();
        if name.is_empty() || name.to_ascii_lowercase().starts_with("docker-desktop") {
            continue;
        }
        if starred {
            default = Some(name.clone());
        }
        distros.push(name);
    }
    (distros, default)
}

async fn inspect_wsl(distro_override: Option<&str>) -> WslInspect {
    let mut w = WslInspect {
        wsl_present: crate::local_wizard::which("wsl.exe").is_some(),
        wsl_ready: false,
        distros: Vec::new(),
        default_distro: None,
        tmux_installed: None,
        winmux_cli_state: None,
        tmux_conf_ok: None,
        claude_inside: None,
        hooks_version_inside: None,
    };
    if !w.wsl_present {
        return w;
    }
    // `wsl --status` exits 0 with non-empty output when the platform is
    // installed (same semantics local_wizard::wsl_available pinned).
    let mut status_cmd = wsl_cmd();
    status_cmd.arg("--status");
    if let Ok((code, out)) = run_capture(status_cmd, "wsl --status", 15).await {
        w.wsl_ready = code == 0 && !out.trim().is_empty();
    }
    if !w.wsl_ready {
        return w;
    }
    if let Ok((code, out)) = run_capture(
        {
            let mut c = wsl_cmd();
            c.arg("-l").arg("-v");
            c
        },
        "wsl -l -v",
        15,
    )
    .await
    {
        if code == 0 {
            let (distros, default) = parse_wsl_list_verbose(&out);
            w.distros = distros;
            w.default_distro = default;
        }
    }
    if w.distros.is_empty() {
        return w;
    }
    let target = distro_override
        .map(|s| s.to_string())
        .or_else(|| w.default_distro.clone());
    // One batched probe — a single wsl.exe spawn instead of five keeps
    // cold-distro latency tolerable. Machine-readable lines, parsed here.
    let script = r#"
echo "TMUX $(command -v tmux >/dev/null 2>&1 && echo yes || echo no)"
if [ -f "$HOME/.winmux/bin/winmux-linux-x64" ]; then echo "CLI $(sha256sum "$HOME/.winmux/bin/winmux-linux-x64" | cut -d' ' -f1)"; else echo "CLI missing"; fi
if [ -f "$HOME/.winmux/tmux.conf" ]; then echo "CONF $(sha256sum "$HOME/.winmux/tmux.conf" | cut -d' ' -f1)"; else echo "CONF missing"; fi
echo "CLAUDE $([ -d "$HOME/.claude" ] && echo yes || echo no)"
echo "HOOKSV $(sed -n 's/.*"hooks_version"[: ]*"\([^"]*\)".*/\1/p' "$HOME/.claude/settings.json" 2>/dev/null | head -1)"
"#;
    let probe = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        wsl_exec(target.as_deref(), None, script),
    )
    .await;
    let Ok(Ok((0, out))) = probe else {
        return w;
    };
    let manifest = crate::remote_bootstrap::embedded_manifest().ok();
    let cli_sha = manifest
        .as_ref()
        .and_then(|m| m.get("x86_64-linux").map(|e| e.sha256.clone()));
    let conf_sha = manifest
        .as_ref()
        .and_then(|m| m.get("tmux-conf").map(|e| e.sha256.clone()));
    for line in out.lines() {
        let line = line.trim();
        if let Some(v) = line.strip_prefix("TMUX ") {
            w.tmux_installed = Some(v == "yes");
        } else if let Some(v) = line.strip_prefix("CLI ") {
            w.winmux_cli_state = Some(if v == "missing" {
                "missing".into()
            } else if cli_sha.as_deref() == Some(v) {
                "ok".into()
            } else {
                "stale".into()
            });
        } else if let Some(v) = line.strip_prefix("CONF ") {
            w.tmux_conf_ok = Some(v != "missing" && conf_sha.as_deref() == Some(v));
        } else if let Some(v) = line.strip_prefix("CLAUDE ") {
            w.claude_inside = Some(v == "yes");
        } else if let Some(v) = line.strip_prefix("HOOKSV ") {
            if !v.is_empty() {
                w.hooks_version_inside = Some(v.to_string());
            }
        }
    }
    w
}

fn local_hooks_version() -> Option<String> {
    let p = user_profile().join(".claude").join("settings.json");
    let text = std::fs::read_to_string(p).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    v.get("winmux_meta")?
        .get("hooks_version")?
        .as_str()
        .map(|s| s.to_string())
}

/// Detect everything the "local → new" wizard offers to install. Never
/// fails as a whole — each probe degrades independently.
#[tauri::command]
pub(crate) async fn local_setup_inspect(distro: Option<String>) -> Result<LocalSetupInspect, String> {
    let winget = probe_tool(&["winget.exe"], &[]).await;
    let git = probe_tool(&["git.exe"], &canonical_git()).await;
    let node = probe_tool(&["node.exe"], &canonical_node()).await;
    let npm = probe_tool(&["npm.cmd", "npm.exe"], &canonical_npm()).await;
    let claude = probe_tool(&["claude.exe", "claude.cmd"], &canonical_claude()).await;
    let codex = probe_tool(&["codex.cmd", "codex.exe"], &[]).await;
    let gemini = probe_tool(&["gemini.cmd", "gemini.exe"], &[]).await;
    let wsl = inspect_wsl(distro.as_deref()).await;
    Ok(LocalSetupInspect {
        winget,
        git,
        node,
        npm,
        claude,
        codex,
        gemini,
        wsl,
        local_hooks_version: local_hooks_version(),
        local_claude_dir: user_profile().join(".claude").is_dir(),
    })
}

// ─── Install engine ──────────────────────────────────────────────────

#[derive(Clone, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../src/bindings/")]
pub(crate) struct LocalSetupInput {
    /// LocalStepKind names, in execution order (the wizard sends only
    /// the needed ones; unknown names fail their step, not the run).
    pub steps: Vec<String>,
    #[serde(default)]
    pub distro: Option<String>,
    #[serde(default)]
    pub wsl_username: Option<String>,
    #[serde(default)]
    pub workspace_name: Option<String>,
    #[serde(default)]
    pub create_workspace: bool,
    #[serde(default)]
    pub workspace_cwd: Option<String>,
}

#[derive(Clone, Serialize, ts_rs::TS)]
#[ts(export, export_to = "../../src/bindings/")]
pub(crate) struct LocalSetupResult {
    pub run_id: String,
    pub workspace_id: Option<String>,
    pub workspace_name: Option<String>,
}

static RUN_COUNTER: AtomicU64 = AtomicU64::new(0);
fn new_run_id() -> String {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let n = RUN_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("local_{t:x}_{n:x}")
}

fn emit_step(
    app: &AppHandle,
    run_id: &str,
    idx: usize,
    kind: &str,
    state: &'static str,
    log_chunk: String,
    message: Option<String>,
    error: Option<ProvisioningError>,
) {
    let _ = app.emit(
        "local-setup:progress",
        StepProgress {
            run_id: run_id.to_string(),
            step_index: idx,
            step_kind: kind.to_string(),
            state,
            log_chunk,
            message,
            error,
            timestamp_iso: crate::provisioning::iso_now(),
        },
    );
}

/// Spawn a local-setup run. Emits `local-setup:progress` per step and a
/// final `local-setup:complete` (carrying the created workspace id when
/// the WSL chain succeeded and `create_workspace` was requested).
#[tauri::command]
pub(crate) async fn local_setup_start(
    state: State<'_, AppState>,
    app: AppHandle,
    input: LocalSetupInput,
) -> Result<RunHandle, String> {
    let run_id = new_run_id();
    let run_id_clone = run_id.clone();
    let state_for_task: AppState = (*state).clone();
    tauri::async_runtime::spawn(async move {
        run_local_setup(app, state_for_task, run_id_clone, input).await;
    });
    Ok(RunHandle { run_id })
}

/// Sanitize a Linux username: lowercase, [a-z0-9-], must start with a
/// letter, max 32 chars. Falls back to "winmux" when nothing survives.
fn sanitize_linux_username(raw: &str) -> String {
    let mut out = String::new();
    for c in raw.to_lowercase().chars() {
        if out.is_empty() {
            if c.is_ascii_lowercase() {
                out.push(c);
            }
        } else if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' {
            out.push(c);
        }
        if out.len() >= 32 {
            break;
        }
    }
    if out.is_empty() {
        "winmux".into()
    } else {
        out
    }
}

/// Pipe embedded bytes into a file inside the distro (default user's
/// home), then sha-verify + atomically rename into place. stdin-pipe is
/// the transport — `\\wsl$` UNC is flaky on cold distros and wrong for
/// non-default users.
async fn wsl_pipe_to_file(
    distro: Option<&str>,
    bytes: &[u8],
    dest_rel: &str, // path under $HOME, pre-quoted here
    sha256: &str,
    mode: &str,
) -> Result<String, String> {
    use tokio::io::AsyncWriteExt;
    let tmp_name = format!("{dest_rel}.winmux-upload.tmp");
    // Stage 1: cat stdin into the temp file. The script only embeds
    // shell_quote()d values (Rule #3 discipline).
    let cat_script = format!(
        "mkdir -p \"$(dirname \"$HOME\"/{d})\" && cat > \"$HOME\"/{t}",
        d = shell_quote(dest_rel),
        t = shell_quote(&tmp_name)
    );
    let mut c = wsl_cmd();
    if let Some(d) = distro {
        if !d.is_empty() {
            c.arg("-d").arg(d);
        }
    }
    c.arg("--").arg("sh").arg("-c").arg(&cat_script);
    c.stdin(std::process::Stdio::piped());
    let mut child = c.spawn().map_err(|e| format!("wsl.exe spawn: {e}"))?;
    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| "wsl stdin unavailable".to_string())?;
        stdin
            .write_all(bytes)
            .await
            .map_err(|e| format!("stdin write: {e}"))?;
        stdin.shutdown().await.map_err(|e| format!("stdin close: {e}"))?;
    }
    let out = tokio::time::timeout(std::time::Duration::from_secs(300), child.wait_with_output())
        .await
        .map_err(|_| "upload timed out".to_string())?
        .map_err(|e| format!("wsl wait: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "upload failed: {}",
            clean_wsl_output(&out.stderr)
        ));
    }
    // Stage 2: verify + chmod + atomic rename.
    let fin_script = format!(
        "S=$(sha256sum \"$HOME\"/{t} | cut -d' ' -f1); \
         if [ \"$S\" = {sha} ]; then chmod {mode} \"$HOME\"/{t} && mv -f \"$HOME\"/{t} \"$HOME\"/{d} && echo OK; \
         else echo \"SHA MISMATCH got $S\"; rm -f \"$HOME\"/{t}; exit 1; fi",
        t = shell_quote(&tmp_name),
        d = shell_quote(dest_rel),
        sha = shell_quote(sha256),
        mode = mode
    );
    let (code, out) = wsl_exec(distro, None, &fin_script).await?;
    if code != 0 {
        return Err(format!("finalize failed: {out}"));
    }
    Ok(out)
}

/// True when winget's exit code means "nothing to do" rather than a
/// real failure (0x8A15002B: no applicable installer/upgrade found —
/// typically "already installed").
fn winget_already_ok(code: i32) -> bool {
    code == 0x8A15_002Bu32 as i32
}

async fn winget_install(id: &str) -> Result<(i32, String), String> {
    let mut c = hidden_cmd("winget");
    c.args([
        "install",
        "--id",
        id,
        "-e",
        "--source",
        "winget",
        "--accept-source-agreements",
        "--accept-package-agreements",
        "--disable-interactivity",
    ]);
    run_capture(c, "winget install", 900).await
}

async fn npm_install_global(pkg: &str) -> Result<(i32, String), String> {
    let npm = probe_tool(&["npm.cmd", "npm.exe"], &canonical_npm()).await;
    let Some(path) = npm.path else {
        return Err("npm not found — enable the Node.js step first".into());
    };
    let mut c = hidden_cmd(&path);
    c.args(["install", "-g", pkg]);
    run_capture(c, "npm install -g", 600).await
}

/// Resolve the bundled Windows CLI (winmux-cli.exe). Installed builds
/// carry it under the Tauri resource dir; dev builds fall back to the
/// path the build script stages it at.
fn resolve_winmux_cli(app: &AppHandle) -> Option<PathBuf> {
    use tauri::Manager;
    let candidates = app
        .path()
        .resource_dir()
        .ok()
        .into_iter()
        .flat_map(|r| [r.join("resources").join("winmux-cli.exe"), r.join("winmux-cli.exe")])
        .collect::<Vec<_>>();
    for c in candidates {
        if c.is_file() {
            return Some(c);
        }
    }
    None
}

async fn run_local_setup(app: AppHandle, state: AppState, run_id: String, input: LocalSetupInput) {
    // The distro the WSL-chain steps target. A fresh `wsl --install`
    // registers Ubuntu; an existing setup passes the inspect's default.
    let mut distro: Option<String> = input.distro.clone().filter(|s| !s.is_empty());
    // WSL-chain health — decides whether we finalize a workspace.
    let mut chain_failed = false;
    let mut ran_wsl_chain = false;

    for (idx, step) in input.steps.iter().enumerate() {
        let kind = step.as_str();
        emit_step(&app, &run_id, idx, kind, "running", String::new(), None, None);
        let is_wsl_chain = matches!(
            kind,
            "InstallWsl"
                | "CreateWslUser"
                | "EnsureDistroReady"
                | "InstallTmuxInWsl"
                | "DeployWinmuxCliToWsl"
                | "DeployTmuxConfToWsl"
        );
        if is_wsl_chain {
            ran_wsl_chain = true;
            // A broken chain makes every later chain step meaningless —
            // skip them explicitly instead of cascading noise.
            if chain_failed {
                emit_step(
                    &app,
                    &run_id,
                    idx,
                    kind,
                    "skipped",
                    String::new(),
                    Some("skipped — an earlier WSL step failed".into()),
                    None,
                );
                continue;
            }
        }
        let result: Result<String, ProvisioningError> = match kind {
            "InstallGit" => match winget_install("Git.Git").await {
                Ok((code, out)) if code == 0 || winget_already_ok(code) => Ok(out),
                Ok((code, out)) => Err(ProvisioningError::StepFailed {
                    step: kind.into(),
                    exit_code: code,
                    stderr: out,
                }),
                Err(e) => Err(ProvisioningError::Generic(e)),
            },
            "InstallNodejs" => match winget_install("OpenJS.NodeJS.LTS").await {
                Ok((code, out)) if code == 0 || winget_already_ok(code) => Ok(out),
                Ok((code, out)) => Err(ProvisioningError::StepFailed {
                    step: kind.into(),
                    exit_code: code,
                    stderr: out,
                }),
                Err(e) => Err(ProvisioningError::Generic(e)),
            },
            "InstallClaudeCode" => {
                // Official native installer (verified against
                // code.claude.com/docs/en/setup) — no admin, auto-updates,
                // lands at %USERPROFILE%\.local\bin\claude.exe. Static
                // command string: Rule #3 satisfied.
                let mut c = hidden_cmd("powershell");
                c.args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-Command",
                    "irm https://claude.ai/install.ps1 | iex",
                ]);
                match run_capture(c, "claude install", 600).await {
                    Ok((0, out)) => {
                        let installed = canonical_claude().iter().any(|p| p.is_file())
                            || crate::local_wizard::which("claude.exe").is_some();
                        if installed {
                            Ok(out)
                        } else {
                            Err(ProvisioningError::StepFailed {
                                step: kind.into(),
                                exit_code: 0,
                                stderr: format!(
                                    "installer exited 0 but claude.exe was not found\n{out}"
                                ),
                            })
                        }
                    }
                    Ok((code, out)) => Err(ProvisioningError::StepFailed {
                        step: kind.into(),
                        exit_code: code,
                        stderr: out,
                    }),
                    Err(e) => Err(ProvisioningError::Generic(e)),
                }
            }
            "InstallCodex" => match npm_install_global("@openai/codex").await {
                Ok((0, out)) => Ok(out),
                Ok((code, out)) => Err(ProvisioningError::StepFailed {
                    step: kind.into(),
                    exit_code: code,
                    stderr: out,
                }),
                Err(e) => Err(ProvisioningError::Generic(e)),
            },
            "InstallGemini" => match npm_install_global("@google/gemini-cli@latest").await {
                Ok((0, out)) => Ok(out),
                Ok((code, out)) => Err(ProvisioningError::StepFailed {
                    step: kind.into(),
                    exit_code: code,
                    stderr: out,
                }),
                Err(e) => Err(ProvisioningError::Generic(e)),
            },
            "InstallLocalHooks" => match resolve_winmux_cli(&app) {
                Some(cli) => {
                    let mut c = hidden_cmd(&cli.to_string_lossy());
                    c.args(["setup-hooks", "--agent", "claude", "--source", "bundled"]);
                    match run_capture(c, "setup-hooks", 120).await {
                        Ok((0, out)) => Ok(out),
                        Ok((code, out)) => Err(ProvisioningError::StepFailed {
                            step: kind.into(),
                            exit_code: code,
                            stderr: out,
                        }),
                        Err(e) => Err(ProvisioningError::Generic(e)),
                    }
                }
                None => Err(ProvisioningError::Generic(
                    "bundled winmux-cli.exe not found (dev build without staged resources?)"
                        .into(),
                )),
            },
            "InstallWsl" => {
                let d = distro.clone().unwrap_or_else(|| "Ubuntu".into());
                let mut c = wsl_cmd();
                c.args(["--install", "--no-launch", "-d", &d]);
                match run_capture(c, "wsl --install", 1800).await {
                    Ok((0, out)) => {
                        distro = Some(d);
                        Ok(out)
                    }
                    Ok((code, out)) => {
                        let low = out.to_lowercase();
                        // Modern WSL self-elevates via UAC; this branch is
                        // the fallback for a declined UAC or a machine
                        // that needs the Windows feature enable + reboot.
                        if low.contains("elevat") || low.contains("0x80070005") {
                            Err(ProvisioningError::ElevationRequired {
                                step: kind.into(),
                                hint:
                                    "Run `wsl --install` from an elevated terminal, reboot if prompted, then re-run this step."
                                        .into(),
                            })
                        } else {
                            Err(ProvisioningError::StepFailed {
                                step: kind.into(),
                                exit_code: code,
                                stderr: out,
                            })
                        }
                    }
                    Err(e) => Err(ProvisioningError::Generic(e)),
                }
            }
            "CreateWslUser" => {
                // Bypass Ubuntu's interactive OOBE entirely: create the
                // uid-1000 user as root + set it as the wsl.conf default,
                // then terminate the distro so the default applies.
                let user = sanitize_linux_username(
                    input
                        .wsl_username
                        .as_deref()
                        .filter(|s| !s.trim().is_empty())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| {
                            std::env::var("USERNAME").unwrap_or_else(|_| "winmux".into())
                        })
                        .as_str(),
                );
                let script = format!(
                    "U={u}; \
                     if getent passwd 1000 >/dev/null 2>&1; then U=\"$(id -nu 1000)\"; echo \"EXISTS $U\"; \
                     else useradd -m -u 1000 -s /bin/bash \"$U\" && (usermod -aG sudo \"$U\" 2>/dev/null || usermod -aG wheel \"$U\" 2>/dev/null || true) && echo \"CREATED $U\"; fi; \
                     if grep -q '^\\[user\\]' /etc/wsl.conf 2>/dev/null; then echo 'wsl.conf [user] already set'; \
                     else printf '\\n[user]\\ndefault=%s\\n' \"$U\" >> /etc/wsl.conf; echo 'wsl.conf updated'; fi",
                    u = shell_quote(&user)
                );
                match tokio::time::timeout(
                    std::time::Duration::from_secs(180),
                    wsl_exec(distro.as_deref(), Some("root"), &script),
                )
                .await
                {
                    Ok(Ok((0, out))) => {
                        // Apply the default-user change.
                        if let Some(d) = distro.as_deref() {
                            let mut c = wsl_cmd();
                            c.args(["--terminate", d]);
                            let _ = run_capture(c, "wsl --terminate", 30).await;
                        }
                        Ok(out)
                    }
                    Ok(Ok((code, out))) => Err(ProvisioningError::StepFailed {
                        step: kind.into(),
                        exit_code: code,
                        stderr: out,
                    }),
                    Ok(Err(e)) => Err(ProvisioningError::Generic(e)),
                    Err(_) => Err(ProvisioningError::Generic(
                        "CreateWslUser timed out".into(),
                    )),
                }
            }
            "EnsureDistroReady" => {
                match tokio::time::timeout(
                    std::time::Duration::from_secs(180),
                    wsl_exec(distro.as_deref(), None, "echo READY"),
                )
                .await
                {
                    Ok(Ok((0, out))) if out.contains("READY") => Ok(out),
                    Ok(Ok((code, out))) => Err(ProvisioningError::StepFailed {
                        step: kind.into(),
                        exit_code: code,
                        stderr: out,
                    }),
                    Ok(Err(e)) => Err(ProvisioningError::Generic(e)),
                    Err(_) => Err(ProvisioningError::Generic(
                        "distro did not become ready within 180s (first boot can be slow — retry)"
                            .into(),
                    )),
                }
            }
            "InstallTmuxInWsl" => {
                // `-u root` sidesteps sudo/password entirely (WSL allows
                // it by design) — no SudoRequired equivalent needed.
                let script = "if command -v tmux >/dev/null 2>&1; then echo 'tmux already installed'; \
                              elif command -v apt-get >/dev/null 2>&1; then apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y tmux; \
                              elif command -v dnf >/dev/null 2>&1; then dnf -y install tmux; \
                              elif command -v yum >/dev/null 2>&1; then yum -y install tmux; \
                              elif command -v apk >/dev/null 2>&1; then apk add tmux; \
                              else echo 'no known package manager'; exit 1; fi";
                match tokio::time::timeout(
                    std::time::Duration::from_secs(600),
                    wsl_exec(distro.as_deref(), Some("root"), script),
                )
                .await
                {
                    Ok(Ok((0, out))) => Ok(out),
                    Ok(Ok((code, out))) => Err(ProvisioningError::StepFailed {
                        step: kind.into(),
                        exit_code: code,
                        stderr: out,
                    }),
                    Ok(Err(e)) => Err(ProvisioningError::Generic(e)),
                    Err(_) => Err(ProvisioningError::Generic(
                        "tmux install timed out (slow mirror?) — retry".into(),
                    )),
                }
            }
            "DeployWinmuxCliToWsl" => {
                if std::env::consts::ARCH != "x86_64" {
                    Err(ProvisioningError::Generic(format!(
                        "unsupported host arch {} — only x86_64 WSL deploys are bundled",
                        std::env::consts::ARCH
                    )))
                } else {
                    let deploy = async {
                        let manifest = crate::remote_bootstrap::embedded_manifest()?;
                        let entry = manifest
                            .get("x86_64-linux")
                            .ok_or_else(|| "manifest missing x86_64-linux".to_string())?;
                        let bytes = crate::remote_bootstrap::embedded_payload(&entry.path)?;
                        let mut log = wsl_pipe_to_file(
                            distro.as_deref(),
                            &bytes,
                            ".winmux/bin/winmux-linux-x64",
                            &entry.sha256,
                            "0755",
                        )
                        .await?;
                        let (code, out) = wsl_exec(
                            distro.as_deref(),
                            None,
                            "ln -sf \"$HOME/.winmux/bin/winmux-linux-x64\" \"$HOME/.winmux/bin/winmux\" && echo SYMLINK-OK",
                        )
                        .await?;
                        if code != 0 {
                            return Err(format!("symlink failed: {out}"));
                        }
                        log.push('\n');
                        log.push_str(&out);
                        // PATH rc snippet — the same idempotent script the
                        // remote bootstrap runs; safe as a single argv.
                        let (code, out) = wsl_exec(
                            distro.as_deref(),
                            None,
                            crate::remote_bootstrap::PATH_RC_SNIPPET,
                        )
                        .await?;
                        if code != 0 {
                            return Err(format!("PATH setup failed: {out}"));
                        }
                        log.push('\n');
                        log.push_str(&out);
                        Ok::<String, String>(log)
                    };
                    match deploy.await {
                        Ok(out) => Ok(out),
                        Err(e) => Err(ProvisioningError::Generic(e)),
                    }
                }
            }
            "DeployTmuxConfToWsl" => {
                let deploy = async {
                    let manifest = crate::remote_bootstrap::embedded_manifest()?;
                    let entry = manifest
                        .get("tmux-conf")
                        .ok_or_else(|| "manifest missing tmux-conf".to_string())?;
                    let bytes = crate::remote_bootstrap::embedded_payload(&entry.path)?;
                    wsl_pipe_to_file(
                        distro.as_deref(),
                        &bytes,
                        ".winmux/tmux.conf",
                        &entry.sha256,
                        "0644",
                    )
                    .await
                };
                match deploy.await {
                    Ok(out) => Ok(out),
                    Err(e) => Err(ProvisioningError::Generic(e)),
                }
            }
            "InstallHooksInWsl" => {
                // Best-effort, mirroring the remote bootstrap's posture —
                // a distro without ~/.claude just reports and moves on.
                let script = "\"$HOME/.winmux/bin/winmux\" setup-hooks --agent claude --source bundled 2>&1 || true";
                match tokio::time::timeout(
                    std::time::Duration::from_secs(120),
                    wsl_exec(distro.as_deref(), None, script),
                )
                .await
                {
                    Ok(Ok((_code, out))) => Ok(out),
                    Ok(Err(e)) => Err(ProvisioningError::Generic(e)),
                    Err(_) => Err(ProvisioningError::Generic("hooks install timed out".into())),
                }
            }
            other => Err(ProvisioningError::Generic(format!(
                "unknown local step {other:?}"
            ))),
        };
        match result {
            Ok(log) => {
                emit_step(&app, &run_id, idx, kind, "done", log, None, None);
            }
            Err(err) => {
                if is_wsl_chain {
                    chain_failed = true;
                }
                let msg = err.user_message();
                dlog(&format!("local-setup[{run_id}] step {kind} failed: {msg}"));
                emit_step(
                    &app,
                    &run_id,
                    idx,
                    kind,
                    "failed",
                    String::new(),
                    Some(msg),
                    Some(err),
                );
            }
        }
    }

    // Finalize: create the WSL workspace when the chain came out clean.
    let mut result = LocalSetupResult {
        run_id: run_id.clone(),
        workspace_id: None,
        workspace_name: None,
    };
    if input.create_workspace && ran_wsl_chain && !chain_failed {
        match finalize_wsl_workspace(
            &state,
            &app,
            distro,
            input.workspace_name.clone(),
            input.workspace_cwd.clone(),
        ) {
            Ok((id, name)) => {
                result.workspace_id = Some(id);
                result.workspace_name = Some(name);
            }
            Err(e) => dlog(&format!("local-setup[{run_id}] finalize failed: {e}")),
        }
    }
    let _ = app.emit("local-setup:complete", result);
}

/// Create a fresh workspace with a single terminal pane on
/// Connection::Wsl — the local twin of provisioning::finalize_workspace
/// branch 2.
fn finalize_wsl_workspace(
    state: &AppState,
    app: &AppHandle,
    distro: Option<String>,
    workspace_name: Option<String>,
    cwd: Option<String>,
) -> Result<(String, String), String> {
    use crate::{new_pane_id, new_workspace_id, persist, Connection, LayoutNode, PaneKind, Workspace};

    let display_name = workspace_name
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| distro.clone().unwrap_or_else(|| "WSL".into()));
    let conn = Connection::Wsl { distro };
    let pane_id = new_pane_id();
    let layout = LayoutNode::Pane {
        pane_id,
        pane_kind: PaneKind::Terminal,
        connection: Some(conn.clone()),
        browser: None,
        title: None,
        annotation: None,
        color: None,
        emoji: None,
        help_topic: None,
        diff_source: None,
        smart_bidi: None,
    };
    let ws = Workspace {
        id: new_workspace_id(),
        name: display_name.clone(),
        color: Some(crate::provisioning::workspace_color_for_host(&display_name)),
        emoji: None,
        cwd: cwd.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
        connection: Some(conn),
        layout: Some(layout),
        setup_command: None,
        teardown_command: None,
        env: Vec::new(),
        auto_port_forward: false,
        last_active_at: 0,
        git_worktree: None,
        claude_separate_account: false,
        group_id: None,
        sort_order: None,
    };
    let id_out = ws.id.clone();
    {
        let mut file = state.workspaces.lock().unwrap();
        file.active_workspace_id = Some(id_out.clone());
        file.workspaces.push(ws);
    }
    persist(state)?;
    let _ = app.emit("workspaces:changed", ());
    Ok((id_out, display_name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_username_sanitized() {
        assert_eq!(sanitize_linux_username("Yossi Hezkel"), "yossihezkel");
        assert_eq!(sanitize_linux_username("123abc"), "abc");
        assert_eq!(sanitize_linux_username("!!!"), "winmux");
        assert_eq!(sanitize_linux_username("a-b-c"), "a-b-c");
    }

    #[test]
    fn wsl_list_verbose_parses_default_marker() {
        let text = "  NAME            STATE           VERSION\n* Ubuntu          Running         2\n  docker-desktop  Stopped         2\n  Debian          Stopped         2\n";
        let (distros, default) = parse_wsl_list_verbose(text);
        assert_eq!(distros, vec!["Ubuntu".to_string(), "Debian".to_string()]);
        assert_eq!(default.as_deref(), Some("Ubuntu"));
    }

    #[test]
    fn winget_no_upgrade_code_is_ok() {
        assert!(winget_already_ok(0x8A15_002Bu32 as i32));
        assert!(!winget_already_ok(1));
    }
}
