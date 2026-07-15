//! Phase 80: local smart setup — detection + install engine for the
//! "local → new" wizard flow, plus the shared hidden-console process
//! helpers every wsl.exe / winget / npm invocation in the app goes
//! through (nothing in src/ set CREATE_NO_WINDOW before this module;
//! spawning wsl.exe from a GUI app without it flashes a console).
//!
//! Everything here follows Rule #3: argv arrays only. Values interpolated
//! into POSIX scripts run inside a distro go through
//! `winmux_core::shell_quote` exclusively.

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
