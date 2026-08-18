//! Enumerate and create git worktrees for a project-folder workspace.
//!
//! A "project folder" is a workspace flagged `is_project_root` whose
//! `cwd` is a git repo — usually a directory on a server. Every git call
//! here dispatches on that workspace's connection:
//!
//! | connection      | transport                                        |
//! |-----------------|--------------------------------------------------|
//! | `Local` / none  | `tokio::process::Command` with an arg array      |
//! | `Wsl`           | `local_setup::wsl_exec` (a `sh -ls` script)      |
//! | `Ssh`           | an exec channel on a LIVE handle to that host    |
//!
//! The SSH path reuses whatever session is already open to that host —
//! matched on user@host:port, not on a workspace id, because a repo
//! belongs to a host rather than to any one workspace, and a child
//! inherits a CLONE of its parent's connection, never a shared handle.
//! It never
//! dials a second connection just to list worktrees. No live handle
//! means an explicit "connect first" error, not a silent empty list: an
//! empty list would read as "this repo has no worktrees", which is a
//! different and much more misleading statement.
//!
//! Absolute Rule #3: the local path passes each argument separately. The
//! WSL and SSH paths are shell scripts by construction (that is the only
//! interface `wsl_exec` / `channel.exec` offer), so every interpolated
//! value goes through `winmux_core::shell_quote` — never raw concatenation.

use russh::client::Handle as SshHandle;
use russh::ChannelMsg;
use serde::Serialize;

use crate::local_setup;
use crate::{AppState, Connection, SshClient};
use winmux_core::{log_debug, shell_quote};

/// One entry from `git worktree list --porcelain`.
///
/// Not persisted — this is the shape of a scan result handed to the UI.
#[derive(Clone, Debug, PartialEq, Serialize, ts_rs::TS)]
#[ts(export, export_to = "../../src/bindings/")]
pub struct WorktreeEntry {
    /// Absolute path on the project folder's host.
    pub path: String,
    /// Short branch name (`refs/heads/` stripped). None when detached.
    pub branch: Option<String>,
    /// Commit the worktree is checked out at. Empty for a bare main entry.
    pub head: String,
    /// The first entry git reports is the main working tree.
    pub is_main: bool,
    pub is_detached: bool,
    pub is_locked: bool,
    /// git considers the entry removable (its directory is gone).
    pub is_prunable: bool,
}

/// Sanitize a branch name for use as a **path component**.
///
/// Extracted from `workspace_create_worktree`, which grew it inline, so
/// both the local-workspace path and the project-folder path enforce one
/// rule. Deliberately narrower than what git itself accepts — we own the
/// directory naming, so anything outside the allow-list becomes `-`.
///
/// Returns Err for names that could escape the target directory or that
/// git would reject outright.
pub(crate) fn sanitize_branch_name(branch: &str) -> Result<String, String> {
    let safe: String = branch
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '/' {
                c
            } else {
                '-'
            }
        })
        .collect();
    if safe.is_empty() || safe.starts_with('-') || safe.contains("..") {
        return Err("invalid branch name".to_string());
    }
    Ok(safe)
}

/// The directory component for a branch: `feature/x` must not create a
/// nested `feature/` directory, so slashes collapse to hyphens.
pub(crate) fn branch_dir_component(safe_branch: &str) -> String {
    safe_branch.replace('/', "-")
}

/// Default target for a new worktree: a sibling of the project directory,
/// `<parent>/<project-basename>-<branch>`.
///
/// `workspace_create_worktree` puts app-created worktrees under
/// `config_dir()/worktrees/`, which does not exist on a remote host — and
/// a sibling directory is the convention most people already use. Only a
/// suggestion; the caller shows it and lets the user edit it.
pub(crate) fn default_worktree_target(project_path: &str, safe_branch: &str) -> String {
    let dir_branch = branch_dir_component(safe_branch);
    let normalized = project_path.replace('\\', "/");
    let trimmed = normalized.trim_end_matches('/');
    let (parent, base) = match trimmed.rsplit_once('/') {
        Some((p, b)) => (p, b),
        // A bare relative name: keep it relative to whatever it was.
        None => return format!("{trimmed}-{dir_branch}"),
    };
    // A path directly under root ("/repo") leaves an empty parent.
    if parent.is_empty() {
        format!("/{base}-{dir_branch}")
    } else {
        format!("{parent}/{base}-{dir_branch}")
    }
}

/// Parse `git worktree list --porcelain`.
///
/// Format: one record per worktree, records separated by a blank line.
/// `worktree <path>` opens a record; `HEAD <sha>`, `branch <ref>`,
/// `detached`, `locked [reason]`, `prunable [reason]`, and `bare` follow.
/// Paths may contain spaces, so only the first token is split off.
///
/// Tolerant by design: unknown keys are ignored and a record without a
/// trailing blank line still lands. Garbage in means a short list out,
/// never a panic.
pub(crate) fn parse_worktree_porcelain(text: &str) -> Vec<WorktreeEntry> {
    let mut out: Vec<WorktreeEntry> = Vec::new();
    let mut cur: Option<WorktreeEntry> = None;
    for raw in text.lines() {
        let line = raw.trim_end_matches('\r');
        if line.trim().is_empty() {
            if let Some(e) = cur.take() {
                out.push(e);
            }
            continue;
        }
        let (key, rest) = match line.split_once(' ') {
            Some((k, r)) => (k, r),
            None => (line, ""),
        };
        match key {
            "worktree" => {
                if let Some(e) = cur.take() {
                    out.push(e);
                }
                cur = Some(WorktreeEntry {
                    path: rest.to_string(),
                    branch: None,
                    head: String::new(),
                    // Fixed up after the loop — git always lists the main
                    // working tree first.
                    is_main: false,
                    is_detached: false,
                    is_locked: false,
                    is_prunable: false,
                });
            }
            "HEAD" => {
                if let Some(e) = cur.as_mut() {
                    e.head = rest.to_string();
                }
            }
            "branch" => {
                if let Some(e) = cur.as_mut() {
                    e.branch = Some(rest.strip_prefix("refs/heads/").unwrap_or(rest).to_string());
                }
            }
            "detached" => {
                if let Some(e) = cur.as_mut() {
                    e.is_detached = true;
                }
            }
            "locked" => {
                if let Some(e) = cur.as_mut() {
                    e.is_locked = true;
                }
            }
            "prunable" => {
                if let Some(e) = cur.as_mut() {
                    e.is_prunable = true;
                }
            }
            _ => {}
        }
    }
    if let Some(e) = cur.take() {
        out.push(e);
    }
    if let Some(first) = out.first_mut() {
        first.is_main = true;
    }
    out
}

// ─── transports ────────────────────────────────────────────────────────

/// Run `git -C <cwd> <args…>` locally. Arg array, never a shell string.
async fn run_git_local(cwd: &str, args: &[&str]) -> Result<String, String> {
    let mut cmd = tokio::process::Command::new("git");
    cmd.arg("-C").arg(cwd).arg("--no-pager");
    for a in args {
        cmd.arg(a);
    }
    // Don't flash a console window out of the GUI process.
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let out = cmd.output().await.map_err(|e| format!("spawn git: {e}"))?;
    if !out.status.success() {
        return Err(git_error(&String::from_utf8_lossy(&out.stderr)));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Build the `sh` one-liner used by the WSL and SSH transports. Every
/// interpolated value is shell-quoted.
fn git_script(cwd: &str, args: &[&str]) -> String {
    let mut s = format!("git -C {} --no-pager", shell_quote(cwd));
    for a in args {
        s.push(' ');
        s.push_str(&shell_quote(a));
    }
    // stderr on stdout so a failure explains itself: neither transport
    // gives us a separate stderr stream we can rely on.
    s.push_str(" 2>&1");
    s
}

async fn run_git_wsl(distro: Option<&str>, cwd: &str, args: &[&str]) -> Result<String, String> {
    let (code, text) = local_setup::wsl_exec(distro, None, &git_script(cwd, args)).await?;
    if code != 0 {
        return Err(git_error(&text));
    }
    Ok(text)
}

/// One-shot exec channel on an existing handle, stdout captured.
/// Mirrors `claude_log::ssh_exec_capture` — each module keeps its own
/// copy so it stays self-contained.
async fn ssh_exec_capture(
    handle: &SshHandle<SshClient>,
    cmd: &str,
    timeout_secs: u64,
) -> Result<(i32, String), String> {
    let mut ch = handle
        .channel_open_session()
        .await
        .map_err(|e| format!("channel_open: {e}"))?;
    ch.exec(true, cmd.as_bytes())
        .await
        .map_err(|e| format!("exec: {e}"))?;
    let mut stdout = Vec::new();
    let mut code = 0i32;
    let timed_out = tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        async {
            while let Some(msg) = ch.wait().await {
                match msg {
                    ChannelMsg::Data { ref data } => stdout.extend_from_slice(data),
                    ChannelMsg::ExitStatus { exit_status } => code = exit_status as i32,
                    ChannelMsg::Eof | ChannelMsg::Close => break,
                    _ => {}
                }
            }
        },
    )
    .await
    .is_err();
    let _ = ch.close().await;
    if timed_out {
        return Err("git timed out on the remote host".to_string());
    }
    Ok((code, String::from_utf8_lossy(&stdout).to_string()))
}

/// Any live SSH handle already open to this host.
///
/// Matched on user@host:port rather than on a workspace id: a project
/// folder belongs to a HOST, not to a workspace, and once one pane
/// anywhere is connected to that server there is no reason to refuse to
/// list its worktrees. `key_path` is a credential, not an identity, so
/// it is not part of the match.
fn pick_ssh_handle_for_host(
    state: &AppState,
    host: &str,
    user: &str,
    port: u16,
) -> Option<std::sync::Arc<SshHandle<SshClient>>> {
    let sessions = state.core.sessions.lock().ok()?;
    sessions.values().find_map(|s| match s {
        crate::Session::Ssh(ssh)
            if ssh.host == host && ssh.user == user && ssh.port == port =>
        {
            Some(ssh.handle.clone())
        }
        _ => None,
    })
}

/// Trim git's stderr into something worth putting in front of a user.
/// Keeps the first non-empty line; git puts the actual reason there and
/// the rest is usually a usage dump.
fn git_error(stderr: &str) -> String {
    let first = stderr
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("git failed");
    format!("git: {first}")
}

/// Run a git subcommand on a workspace's host, whichever host that is.
///
/// Returns the resolved repo path alongside the output, so callers never
/// take one as a parameter — the old shape accepted both a folder id and
/// a path, which could disagree with each other.
pub(crate) async fn run_git_for_workspace(
    state: &AppState,
    workspace_id: &str,
    args: &[&str],
) -> Result<(String, String), String> {
    // Snapshot connection + cwd under a short lock, then drop it — the
    // git call below is I/O and must never hold the workspaces mutex.
    let (conn, cwd) = {
        let file = state
            .workspaces
            .lock()
            .map_err(|e| format!("workspaces lock poisoned: {e}"))?;
        let ws = file
            .workspaces
            .iter()
            .find(|w| w.id == workspace_id)
            .ok_or_else(|| "workspace not found".to_string())?;
        let cwd = ws
            .cwd
            .clone()
            .filter(|c| !c.trim().is_empty())
            .ok_or_else(|| "this workspace has no project directory".to_string())?;
        (ws.connection.clone(), cwd)
    };
    let out = run_git_over(state, &conn, &cwd, args).await?;
    Ok((out, cwd))
}

/// The transport dispatch itself, split out so the pin flow can validate
/// a path against a connection before any workspace exists.
pub(crate) async fn run_git_over(
    state: &AppState,
    conn: &Option<Connection>,
    cwd: &str,
    args: &[&str],
) -> Result<String, String> {
    match conn {
        None | Some(Connection::Local { .. }) => run_git_local(cwd, args).await,
        Some(Connection::Wsl { distro }) => run_git_wsl(distro.as_deref(), cwd, args).await,
        Some(Connection::Ssh { host, user, port, .. }) => {
            // No live session is an explicit error, never an empty list:
            // an empty list reads as "this repo has no worktrees", which
            // is a different and far more misleading claim.
            let handle = pick_ssh_handle_for_host(state, host, user, *port).ok_or_else(|| {
                format!("no live SSH session to {user}@{host} — connect a pane there first")
            })?;
            let (code, text) = ssh_exec_capture(&handle, &git_script(cwd, args), 20).await?;
            if code != 0 {
                return Err(git_error(&text));
            }
            Ok(text)
        }
    }
}

// ─── commands ──────────────────────────────────────────────────────────

/// List the worktrees of a project-folder workspace.
#[tauri::command]
pub(crate) async fn workspace_list_worktrees(
    state: tauri::State<'_, AppState>,
    workspace_id: String,
) -> Result<Vec<WorktreeEntry>, String> {
    let (text, _cwd) =
        run_git_for_workspace(&state, &workspace_id, &["worktree", "list", "--porcelain"]).await?;
    let list = parse_worktree_porcelain(&text);
    log_debug(
        "WORKTREE",
        &format!("listed {} worktree(s) for ws={workspace_id}", list.len()),
    );
    Ok(list)
}

/// Probe a path over a connection before any workspace exists — the pin
/// flow's validation step, so a path that isn't a repo (or a host nothing
/// is connected to) fails with git's own message BEFORE a dead row lands
/// in the sidebar.
#[tauri::command]
pub(crate) async fn git_probe_worktrees(
    state: tauri::State<'_, AppState>,
    path: String,
    connection: Option<Connection>,
) -> Result<Vec<WorktreeEntry>, String> {
    let path = path.trim().to_string();
    if path.is_empty() {
        return Err("project path is required".to_string());
    }
    let text = run_git_over(&state, &connection, &path, &["worktree", "list", "--porcelain"]).await?;
    Ok(parse_worktree_porcelain(&text))
}

/// Create a worktree for a project-folder workspace and return the
/// refreshed list.
///
/// Works on every transport, unlike `workspace_create_worktree`, which is
/// local-only and additionally re-anchors the whole workspace's cwd. This
/// one only touches the repo.
#[tauri::command]
pub(crate) async fn workspace_create_project_worktree(
    state: tauri::State<'_, AppState>,
    workspace_id: String,
    branch_name: String,
    base_branch: String,
    target_path: Option<String>,
) -> Result<Vec<WorktreeEntry>, String> {
    let branch_name = branch_name.trim().to_string();
    let safe_branch = sanitize_branch_name(&branch_name)?;
    let base_branch = base_branch.trim().to_string();
    if base_branch.is_empty() {
        return Err("base branch is required".to_string());
    }
    // Resolve the repo path from the workspace itself, so the default
    // target is a sibling of the REAL repo rather than of whatever path
    // a caller believed the repo to be.
    let (_probe, project_path) =
        run_git_for_workspace(&state, &workspace_id, &["rev-parse", "--is-inside-work-tree"])
            .await?;
    let target = match target_path.map(|t| t.trim().to_string()) {
        Some(t) if !t.is_empty() => t,
        _ => default_worktree_target(&project_path, &safe_branch),
    };
    run_git_for_workspace(
        &state,
        &workspace_id,
        &["worktree", "add", &target, "-b", &branch_name, &base_branch],
    )
    .await?;
    winmux_core::log_info(
        "WORKTREE",
        &format!("[worktree] created {target} for ws={workspace_id} branch={branch_name}"),
    );
    let (text, _) =
        run_git_for_workspace(&state, &workspace_id, &["worktree", "list", "--porcelain"]).await?;
    Ok(parse_worktree_porcelain(&text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_main_and_linked() {
        let text = "\
worktree /home/y/src/winmux
HEAD abc123def
branch refs/heads/main

worktree /home/y/src/winmux-feature-x
HEAD 999888777
branch refs/heads/feature/x
";
        let got = parse_worktree_porcelain(text);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].path, "/home/y/src/winmux");
        assert_eq!(got[0].branch.as_deref(), Some("main"));
        assert_eq!(got[0].head, "abc123def");
        assert!(got[0].is_main);
        assert_eq!(got[1].branch.as_deref(), Some("feature/x"));
        assert!(!got[1].is_main);
    }

    #[test]
    fn parses_detached_locked_prunable() {
        let text = "\
worktree /repo
HEAD aaa
branch refs/heads/main

worktree /repo-detached
HEAD bbb
detached

worktree /repo-locked
HEAD ccc
branch refs/heads/wip
locked held for review

worktree /repo-gone
HEAD ddd
branch refs/heads/old
prunable gitdir file points to non-existent location
";
        let got = parse_worktree_porcelain(text);
        assert_eq!(got.len(), 4);
        assert!(got[1].is_detached && got[1].branch.is_none());
        assert!(got[2].is_locked);
        assert!(got[3].is_prunable);
    }

    #[test]
    fn keeps_spaces_in_paths() {
        let text = "worktree /home/y/My Projects/win mux\nHEAD abc\nbranch refs/heads/main\n";
        let got = parse_worktree_porcelain(text);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].path, "/home/y/My Projects/win mux");
    }

    #[test]
    fn tolerates_missing_trailing_blank_line_and_unknown_keys() {
        let text = "worktree /repo\nHEAD abc\nbare\nsomethingnew value\nbranch refs/heads/main";
        let got = parse_worktree_porcelain(text);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].branch.as_deref(), Some("main"));
        assert!(got[0].is_main);
    }

    #[test]
    fn empty_and_truncated_input_yield_no_panic() {
        assert!(parse_worktree_porcelain("").is_empty());
        assert!(parse_worktree_porcelain("\n\n\n").is_empty());
        let truncated = parse_worktree_porcelain("worktree /repo\nHEA");
        assert_eq!(truncated.len(), 1);
        assert_eq!(truncated[0].head, "");
    }

    #[test]
    fn branch_sanitizing_rejects_escapes() {
        assert!(sanitize_branch_name("").is_err());
        assert!(sanitize_branch_name("-lead").is_err());
        assert!(sanitize_branch_name("a/../b").is_err());
        // Everything outside the allow-list collapses to '-', which can
        // itself produce a leading hyphen — still rejected.
        assert!(sanitize_branch_name("$evil").is_err());
        assert_eq!(sanitize_branch_name("feature/x").unwrap(), "feature/x");
        assert_eq!(sanitize_branch_name("fix bug;rm").unwrap(), "fix-bug-rm");
    }

    #[test]
    fn branch_dir_component_flattens_slashes() {
        assert_eq!(branch_dir_component("feature/x"), "feature-x");
        assert_eq!(branch_dir_component("main"), "main");
    }

    #[test]
    fn default_target_is_a_sibling() {
        assert_eq!(
            default_worktree_target("/home/y/src/winmux", "feature/x"),
            "/home/y/src/winmux-feature-x"
        );
        // Trailing slash must not produce an empty basename.
        assert_eq!(
            default_worktree_target("/home/y/src/winmux/", "wip"),
            "/home/y/src/winmux-wip"
        );
        assert_eq!(default_worktree_target("/repo", "wip"), "/repo-wip");
        assert_eq!(
            default_worktree_target("C:\\src\\winmux", "wip"),
            "C:/src/winmux-wip"
        );
    }

    #[test]
    fn git_script_quotes_every_interpolation() {
        let s = git_script("/home/y/my repo", &["worktree", "list", "--porcelain"]);
        assert!(s.contains("'/home/y/my repo'"));
        assert!(s.ends_with(" 2>&1"));
        // A path that tries to break out of the quotes stays inert.
        let evil = git_script("/tmp/x'; rm -rf /; echo '", &["status"]);
        assert!(!evil.contains("; rm -rf /; echo ;"));
        assert!(evil.contains("'\\''"));
    }

    #[test]
    fn git_error_keeps_the_first_real_line() {
        let e = git_error("\n\nfatal: not a git repository\nusage: git ...\n");
        assert_eq!(e, "git: fatal: not a git repository");
        assert_eq!(git_error("   "), "git: git failed");
    }
}
