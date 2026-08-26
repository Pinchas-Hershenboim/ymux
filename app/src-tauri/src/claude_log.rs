//! Phase 24.A: the Claude Code transcript reader.
//!
//! Phase 24.D rolled back the ClaudeChat + ClaudeLog FE panes
//! ("three competing 'talk to claude' UIs felt fragmented"), but
//! Yossi explicitly asked to keep this backend module alive for a
//! future unified-view rebuild. That rebuild has landed:
//! `ClaudeSessionsView.tsx` is the caller these commands spent a
//! release waiting for. `#![allow(dead_code)]` stays at the top —
//! several response-type fields are still only ever read by serde.
//!
//! Mirrored — for a REMOTE workspace, whose transcripts live on the
//! server Claude runs on:
//!   - claude_log_sync(workspace_id, session_id?) — SFTP-mirror new/
//!     changed files (mtime-gated, full-file fetch — no byte diffing)
//!   - claude_log_list(workspace_id) — pure local directory scan of
//!     the mirror + per-file summary
//!   - claude_log_read(workspace_id, session_id) — parses one mirrored
//!     jsonl into a structured ClaudeLogEntry stream. One line expands
//!     to zero or more entries: each content block becomes its own,
//!     so a tool call is an event with an id, an input and an output,
//!     rather than the string `[Tool: Bash]` inside a message body.
//!
//! Direct — for THIS machine, which has no server to mirror from and
//! so read as permanently empty through the commands above:
//!   - claude_log_list_local() — same summary over ~/.claude/projects
//!   - claude_log_read_local(session_id) — same parse, same helpers
//!
//! No background SSH reconnects — if there's no live handle, sync
//! errors cleanly and the user connects a terminal pane first.

#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::Arc;

use russh::client::Handle as SshHandle;
use russh::ChannelMsg;
use russh_sftp::client::SftpSession;
use serde::Serialize;
use tauri::State;
use tokio::io::AsyncReadExt;

use crate::{config_dir_pub, log_debug, AppState, Session, SshClient};

// ─── public schemas ────────────────────────────────────────────────────────

#[derive(Clone, Serialize, Debug, Default)]
pub(crate) struct ClaudeSyncResult {
    pub synced: usize,
    pub skipped: usize,
    pub errors: Vec<String>,
    pub total_bytes: u64,
}

#[derive(Clone, Serialize, Debug)]
pub(crate) struct ClaudeLogSummary {
    pub session_id: String,
    pub message_count: usize,
    pub first_user: Option<String>,
    pub last_assistant: Option<String>,
    pub project_path: Option<String>,
    pub file_size: u64,
    pub local_mtime: i64,
}

#[derive(Clone, Serialize, Debug)]
pub(crate) struct ClaudeLogEntry {
    pub line_no: usize,
    pub entry_type: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    /// `tool_use.id` on a call, `tool_result.tool_use_id` on its answer. The
    /// frontend pairs the two into one card by this, so a call and its output
    /// are never shown as two unrelated events.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_id: Option<String>,
    /// Pretty-printed `tool_use.input`, capped. The one-line `text` summary is
    /// what the collapsed card shows; this is what it expands to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_input: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

/// How much of a tool's input or output crosses the IPC boundary. A card shows
/// the head and says how much it cut; the full thing is in the terminal, which
/// is one click away. Without a cap a session with a few `cat` results serialises
/// megabytes on every 2.5s poll.
const TOOL_BODY_MAX: usize = 4000;

// ─── storage paths ─────────────────────────────────────────────────────────

fn claude_logs_dir(workspace_id: &str) -> Result<PathBuf, String> {
    let dir = config_dir_pub()?.join("claude-logs").join(workspace_id);
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("create {dir:?}: {e}"))?;
    Ok(dir)
}

fn local_jsonl_path(workspace_id: &str, session_id: &str) -> Result<PathBuf, String> {
    Ok(claude_logs_dir(workspace_id)?.join(format!("{session_id}.jsonl")))
}

// ─── SSH/SFTP helpers (parallel to file_manager's private versions) ────────

fn pick_ssh_handle(state: &AppState, workspace_id: &str) -> Option<Arc<SshHandle<SshClient>>> {
    let sessions = state.core.sessions.lock().ok()?;
    sessions.values().find_map(|s| match s {
        Session::Ssh(ssh) if ssh.workspace_id == workspace_id => Some(ssh.handle.clone()),
        _ => None,
    })
}

async fn open_sftp(handle: &SshHandle<SshClient>) -> Result<SftpSession, String> {
    let chan = handle
        .channel_open_session()
        .await
        .map_err(|e| format!("open channel: {e}"))?;
    chan.request_subsystem(true, "sftp")
        .await
        .map_err(|e| format!("request sftp: {e}"))?;
    let stream = chan.into_stream();
    SftpSession::new(stream)
        .await
        .map_err(|e| format!("sftp init: {e}"))
}

/// Run a one-shot exec channel and capture stdout. Used to enumerate
/// remote jsonl paths via `find`. Same shape as the snippets in
/// claude_summary.rs / pane_list_claude_sessions in lib.rs — kept
/// local here so the module stays self-contained.
async fn ssh_exec_capture(
    handle: &SshHandle<SshClient>,
    cmd: &str,
    timeout_secs: u64,
) -> Result<String, String> {
    let mut ch = handle
        .channel_open_session()
        .await
        .map_err(|e| format!("channel_open: {e}"))?;
    ch.exec(true, cmd.as_bytes())
        .await
        .map_err(|e| format!("exec: {e}"))?;
    let mut stdout = Vec::new();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), async {
        while let Some(msg) = ch.wait().await {
            match msg {
                ChannelMsg::Data { ref data } => stdout.extend_from_slice(data),
                ChannelMsg::Eof | ChannelMsg::Close | ChannelMsg::ExitStatus { .. } => break,
                _ => {}
            }
        }
    })
    .await;
    let _ = ch.close().await;
    Ok(String::from_utf8_lossy(&stdout).to_string())
}

// ─── remote enumeration ────────────────────────────────────────────────────

/// One remote jsonl file, parsed from `find -printf '%T@\t%s\t%p\n'`.
/// Size column is parsed and discarded — total_bytes in the result
/// comes from actually-downloaded bytes, not the find-reported size.
struct RemoteJsonl {
    mtime: i64,
    path: String,
    session_id: String,
}

fn parse_find_output(text: &str) -> Vec<RemoteJsonl> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.splitn(3, '\t').collect();
        if parts.len() < 3 {
            continue;
        }
        // `%T@` is "seconds.nanos"; take just the seconds. Column 2
        // (parts[1]) is the reported size — we drop it.
        let mtime = parts[0]
            .split('.')
            .next()
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(0);
        let path = parts[2].to_string();
        let session_id = std::path::Path::new(&path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        if session_id.is_empty() {
            continue;
        }
        out.push(RemoteJsonl {
            mtime,
            path,
            session_id,
        });
    }
    out
}

async fn list_remote_jsonls(
    handle: &SshHandle<SshClient>,
    session_id_filter: Option<&str>,
) -> Result<Vec<RemoteJsonl>, String> {
    let name_filter = match session_id_filter {
        Some(sid) => {
            // Defensive: session_id format is UUID-like
            // (alphanumerics + dashes). Reject anything weirder so the
            // raw value never reaches the shell.
            if !sid.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
                return Err(format!("invalid session_id {sid:?}"));
            }
            format!("'{sid}.jsonl'")
        }
        None => "'*.jsonl'".to_string(),
    };
    let script = format!(
        "find \"$HOME/.claude/projects\" -maxdepth 4 -name {name_filter} \
         -printf '%T@\\t%s\\t%p\\n' 2>/dev/null",
    );
    let raw = ssh_exec_capture(handle, &script, 10).await?;
    Ok(parse_find_output(&raw))
}

// ─── SFTP download with atomic-ish write ───────────────────────────────────

async fn fetch_jsonl(
    sftp: &SftpSession,
    remote_path: &str,
    local_path: &std::path::Path,
) -> Result<u64, String> {
    let mut file = sftp
        .open(remote_path)
        .await
        .map_err(|e| format!("sftp open {remote_path}: {e}"))?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)
        .await
        .map_err(|e| format!("sftp read {remote_path}: {e}"))?;
    drop(file);

    // Write to a sibling temp file then rename so the local jsonl is
    // never observed in a half-written state by claude_log_list /
    // claude_log_read calls that might race with sync.
    let parent = local_path
        .parent()
        .ok_or_else(|| format!("no parent for {local_path:?}"))?;
    std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {parent:?}: {e}"))?;
    let tmp = parent.join(format!(
        ".{}.tmp.{}",
        local_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("download"),
        std::process::id()
    ));
    std::fs::write(&tmp, &buf).map_err(|e| format!("write tmp {tmp:?}: {e}"))?;
    std::fs::rename(&tmp, local_path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("rename {tmp:?} -> {local_path:?}: {e}")
    })?;
    Ok(buf.len() as u64)
}

fn local_mtime_secs(path: &std::path::Path) -> Option<i64> {
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?;
    let dur = modified.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some(dur.as_secs() as i64)
}

// ─── tauri commands ────────────────────────────────────────────────────────

#[tauri::command]
pub(crate) async fn claude_log_sync(
    state: State<'_, AppState>,
    workspace_id: String,
    session_id: Option<String>,
) -> Result<ClaudeSyncResult, String> {
    let handle = pick_ssh_handle(&state, &workspace_id)
        .ok_or_else(|| "no active SSH session for this workspace — connect a terminal pane first".to_string())?;

    let remotes = list_remote_jsonls(&handle, session_id.as_deref()).await?;
    if remotes.is_empty() && session_id.is_some() {
        return Err(format!(
            "no jsonl found for session_id {:?}",
            session_id.unwrap()
        ));
    }

    // Single SFTP session for the whole batch — opening one channel
    // per file would be wasteful when syncing All.
    let sftp = open_sftp(&handle).await?;

    let mut result = ClaudeSyncResult::default();
    for remote in &remotes {
        let local = match local_jsonl_path(&workspace_id, &remote.session_id) {
            Ok(p) => p,
            Err(e) => {
                result.errors.push(format!("{}: {e}", remote.session_id));
                continue;
            }
        };
        let local_mt = local_mtime_secs(&local).unwrap_or(0);
        if local.exists() && local_mt >= remote.mtime {
            result.skipped += 1;
            continue;
        }
        match fetch_jsonl(&sftp, &remote.path, &local).await {
            Ok(bytes) => {
                result.synced += 1;
                result.total_bytes += bytes;
            }
            Err(e) => {
                result.errors.push(format!("{}: {e}", remote.session_id));
            }
        }
    }
    let _ = sftp.close().await;
    log_debug("CLAUDE", &format!(
        "claude_log_sync ws={workspace_id} sid={:?} synced={} skipped={} errors={}",
        session_id, result.synced, result.skipped, result.errors.len()
    ));
    Ok(result)
}

#[tauri::command]
pub(crate) fn claude_log_list(workspace_id: String) -> Result<Vec<ClaudeLogSummary>, String> {
    let dir = claude_logs_dir(&workspace_id)?;
    let read = match std::fs::read_dir(&dir) {
        Ok(r) => r,
        Err(_) => return Ok(vec![]),
    };
    let mut out: Vec<ClaudeLogSummary> = Vec::new();
    for ent in read.flatten() {
        let path = ent.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.ends_with(".jsonl") {
            continue;
        }
        let session_id = name.trim_end_matches(".jsonl").to_string();
        let meta = match std::fs::metadata(&path) {
            Ok(m) => m,
            Err(_) => continue,
        };
        let file_size = meta.len();
        let local_mtime = meta
            .modified()
            .ok()
            .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let (message_count, first_user, last_assistant, project_path) = summarize_jsonl(&path);
        out.push(ClaudeLogSummary {
            session_id,
            message_count,
            first_user,
            last_assistant,
            project_path,
            file_size,
            local_mtime,
        });
    }
    out.sort_by(|a, b| b.local_mtime.cmp(&a.local_mtime));
    Ok(out)
}

// Phase 24.D: claude_log_pane_set was removed alongside the
// ClaudeLog pane kind (it persisted picker selection / filter to a
// `claudelog` field on LayoutNode::Pane that no longer exists). If
// the unified-view rebuild brings the pane back, restore both the
// field on LayoutNode::Pane and this command.

#[tauri::command]
pub(crate) fn claude_log_read(
    workspace_id: String,
    session_id: String,
) -> Result<Vec<ClaudeLogEntry>, String> {
    let path = local_jsonl_path(&workspace_id, &session_id)?;
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("read {path:?}: {e}"))?;
    let mut out: Vec<ClaudeLogEntry> = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue, // skip malformed lines silently
        };
        out.extend(entries_from_json(&v, idx + 1));
    }
    Ok(out)
}

// ─── this machine's own sessions (no SSH, no mirror) ───────────────────────
//
// The two readers above answer for a REMOTE workspace: `claude_log_sync`
// SFTP-mirrors the server's transcripts into `claude-logs/<workspace_id>/`
// and they read that copy. A local workspace has no server to mirror from,
// so its transcripts were unreachable — `claude_log_list` returned an empty
// vec for it, forever. These two point at the real directory instead and
// share every parser with the mirrored path.

/// Where Claude Code keeps transcripts on this machine.
/// Same shape as `claude_usage_local::projects_dir`.
fn home_projects_dir() -> Result<PathBuf, String> {
    dirs::home_dir()
        .map(|h| h.join(".claude").join("projects"))
        .ok_or_else(|| "no home directory".to_string())
}

/// Every `<project>/<session>.jsonl` below the projects dir. Claude Code
/// nests exactly one project level, so this walks one level and stops —
/// no recursion into whatever else a project directory holds.
///
/// A missing projects dir is not an error: Claude Code has simply never
/// run here, and an empty list says that more usefully than a failure.
fn home_jsonls() -> Result<Vec<PathBuf>, String> {
    let root = home_projects_dir()?;
    let Ok(projects) = std::fs::read_dir(&root) else {
        return Ok(Vec::new());
    };
    let mut out: Vec<PathBuf> = Vec::new();
    for proj in projects.flatten() {
        let dir = proj.path();
        if !dir.is_dir() {
            continue;
        }
        let Ok(files) = std::fs::read_dir(&dir) else {
            continue;
        };
        for f in files.flatten() {
            let p = f.path();
            if p.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                out.push(p);
            }
        }
    }
    Ok(out)
}

/// `claude_log_list` for this machine. Newest first, same as the
/// mirrored variant.
#[tauri::command]
pub(crate) fn claude_log_list_local() -> Result<Vec<ClaudeLogSummary>, String> {
    let mut out: Vec<ClaudeLogSummary> = Vec::new();
    for path in home_jsonls()? {
        let Some(session_id) = path
            .file_stem()
            .and_then(|n| n.to_str())
            .map(|s| s.to_string())
        else {
            continue;
        };
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        let local_mtime = meta
            .modified()
            .ok()
            .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let (message_count, first_user, last_assistant, project_path) = summarize_jsonl(&path);
        out.push(ClaudeLogSummary {
            session_id,
            message_count,
            first_user,
            last_assistant,
            project_path,
            file_size: meta.len(),
            local_mtime,
        });
    }
    out.sort_by(|a, b| b.local_mtime.cmp(&a.local_mtime));
    Ok(out)
}

/// `claude_log_read` for this machine. The session id is a filename
/// stem, so it is rejected unless it looks like one — a `..` or a
/// separator here would otherwise read an arbitrary file.
#[tauri::command]
pub(crate) fn claude_log_read_local(session_id: String) -> Result<Vec<ClaudeLogEntry>, String> {
    if session_id.is_empty()
        || session_id.contains(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
    {
        return Err(format!("invalid session id {session_id:?}"));
    }
    let target = format!("{session_id}.jsonl");
    let path = home_jsonls()?
        .into_iter()
        .find(|p| p.file_name().and_then(|n| n.to_str()) == Some(target.as_str()))
        .ok_or_else(|| format!("session {session_id} not found under ~/.claude/projects/"))?;
    let text = std::fs::read_to_string(&path).map_err(|e| format!("read {path:?}: {e}"))?;
    let mut out: Vec<ClaudeLogEntry> = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue; // skip malformed lines silently, as the mirrored reader does
        };
        out.extend(entries_from_json(&v, idx + 1));
    }
    Ok(out)
}

// ─── jsonl parsing helpers ─────────────────────────────────────────────────

/// Light summary read — just enumerate `type`s and pull the first
/// user / last assistant text. Stops at first user found and keeps
/// updating last_assistant. Also pulls `cwd` from any line that has
/// one (first-found-wins).
fn summarize_jsonl(
    path: &std::path::Path,
) -> (usize, Option<String>, Option<String>, Option<String>) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return (0, None, None, None);
    };
    let mut count: usize = 0;
    let mut first_user: Option<String> = None;
    let mut last_assistant: Option<String> = None;
    let mut project_path: Option<String> = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let ty = v.get("type").and_then(|x| x.as_str()).unwrap_or("");
        if matches!(ty, "user" | "assistant") {
            count += 1;
        }
        if project_path.is_none() {
            if let Some(cwd) = v.get("cwd").and_then(|x| x.as_str()) {
                if !cwd.is_empty() {
                    project_path = Some(cwd.to_string());
                }
            }
        }
        let snippet = extract_text(&v);
        match ty {
            "user" if first_user.is_none() && !snippet.is_empty() => {
                first_user = Some(truncate(&snippet, 240));
            }
            "assistant" if !snippet.is_empty() => {
                last_assistant = Some(truncate(&snippet, 240));
            }
            _ => {}
        }
    }
    (count, first_user, last_assistant, project_path)
}

/// Build a ClaudeLogEntry from one parsed jsonl line. Returns None
/// for entries we don't recognize (so callers can silently skip).
/// One transcript line becomes **zero or more** entries.
///
/// This used to return a single entry, and that was the bug behind "the tool
/// shows up as a message". A `tool_use` almost never appears at the top level —
/// it is one block inside `message.content`, alongside the assistant's prose —
/// so flattening a line into one entry meant `extract_text` had to render the
/// call as the literal string `[Tool: Bash]` inside the message body. There was
/// no separate event to draw a card from, and the input and output were gone by
/// the time the frontend saw anything.
///
/// So a line is expanded instead: each content block becomes its own entry, in
/// order, and a tool call keeps its `id` so the frontend can pair it with the
/// `tool_result` that answers it.
fn entries_from_json(v: &serde_json::Value, line_no: usize) -> Vec<ClaudeLogEntry> {
    let Some(ty) = v.get("type").and_then(|x| x.as_str()).map(|s| s.to_string()) else {
        return Vec::new();
    };
    let timestamp = v
        .get("timestamp")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string());
    let session_id = v
        .get("sessionId")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string());

    // Every entry off this line shares the line's metadata.
    let mk = |entry_type: &str,
              text: String,
              tool_name: Option<String>,
              tool_id: Option<String>,
              tool_input: Option<String>| ClaudeLogEntry {
        line_no,
        entry_type: entry_type.to_string(),
        text,
        tool_name,
        tool_id,
        tool_input,
        timestamp: timestamp.clone(),
        session_id: session_id.clone(),
    };

    match ty.as_str() {
        "user" | "assistant" => {
            let content = v
                .get("message")
                .and_then(|m| m.get("content"))
                .or_else(|| v.get("content"));
            // A plain string body is the whole turn, with no blocks to expand.
            if let Some(s) = content.and_then(|c| c.as_str()) {
                let s = s.trim();
                if s.is_empty() {
                    return Vec::new();
                }
                return vec![mk(&ty, s.to_string(), None, None, None)];
            }
            let Some(blocks) = content.and_then(|c| c.as_array()) else {
                return Vec::new();
            };
            let mut out: Vec<ClaudeLogEntry> = Vec::new();
            for block in blocks {
                let bty = block.get("type").and_then(|x| x.as_str()).unwrap_or("");
                match bty {
                    "text" => {
                        let t = block
                            .get("text")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .trim();
                        if !t.is_empty() {
                            out.push(mk(&ty, t.to_string(), None, None, None));
                        }
                    }
                    "tool_use" => out.push(mk(
                        "tool_use",
                        extract_tool_use_summary(block),
                        block.get("name").and_then(|x| x.as_str()).map(String::from),
                        block.get("id").and_then(|x| x.as_str()).map(String::from),
                        Some(pretty_tool_input(block)),
                    )),
                    "tool_result" => out.push(mk(
                        "tool_result",
                        tool_result_body(block),
                        None,
                        block
                            .get("tool_use_id")
                            .and_then(|x| x.as_str())
                            .map(String::from),
                        None,
                    )),
                    "image" => out.push(mk("image", String::new(), None, None, None)),
                    _ => {}
                }
            }
            out
        }
        "system" => {
            let t = v.get("content").and_then(|x| x.as_str()).unwrap_or("").trim();
            if t.is_empty() {
                Vec::new()
            } else {
                vec![mk(&ty, t.to_string(), None, None, None)]
            }
        }
        "summary" => {
            let t = v.get("summary").and_then(|x| x.as_str()).unwrap_or("").trim();
            if t.is_empty() {
                Vec::new()
            } else {
                vec![mk(&ty, t.to_string(), None, None, None)]
            }
        }
        // Some Claude versions do emit these as their own top-level lines.
        "tool_use" => vec![mk(
            "tool_use",
            extract_tool_use_summary(v),
            v.get("name").and_then(|x| x.as_str()).map(String::from),
            v.get("id").and_then(|x| x.as_str()).map(String::from),
            Some(pretty_tool_input(v)),
        )],
        "tool_result" => vec![mk(
            "tool_result",
            tool_result_body(v),
            None,
            v.get("tool_use_id")
                .and_then(|x| x.as_str())
                .map(String::from),
            None,
        )],
        _ => Vec::new(), // unknown type — skip silently
    }
}

/// `tool_use.input` as indented JSON, capped. A string field is unwrapped to
/// its raw value first: a Bash `command` or an Edit `new_string` is what the
/// reader came for, and JSON-escaping the newlines out of it makes a shell
/// script unreadable for no gain.
fn pretty_tool_input(block: &serde_json::Value) -> String {
    let Some(input) = block.get("input") else {
        return String::new();
    };
    if let Some(map) = input.as_object() {
        if map.len() == 1 {
            if let Some(s) = map.values().next().and_then(|x| x.as_str()) {
                return cap_body(s);
            }
        }
    }
    match serde_json::to_string_pretty(input) {
        Ok(s) => cap_body(&s),
        Err(_) => String::new(),
    }
}

/// A `tool_result`'s content, string or block array, capped for transport.
fn tool_result_body(block: &serde_json::Value) -> String {
    let content = block.get("content");
    if let Some(s) = content.and_then(|x| x.as_str()) {
        return cap_body(s);
    }
    if let Some(arr) = content.and_then(|x| x.as_array()) {
        let mut buf = String::new();
        for b in arr {
            if let Some(t) = b.get("text").and_then(|x| x.as_str()) {
                if !buf.is_empty() {
                    buf.push('\n');
                }
                buf.push_str(t);
                if buf.len() > TOOL_BODY_MAX {
                    break;
                }
            }
        }
        return cap_body(&buf);
    }
    String::new()
}

/// Cap by chars, and say so rather than trailing off — a reader who cannot tell
/// truncation from a tool that returned nothing will go looking for a bug.
fn cap_body(s: &str) -> String {
    if s.chars().count() <= TOOL_BODY_MAX {
        return s.to_string();
    }
    let head: String = s.chars().take(TOOL_BODY_MAX).collect();
    let cut = s.chars().count() - TOOL_BODY_MAX;
    format!("{head}\n… {cut} more characters (see the terminal for the rest)")
}

/// Pull the text content out of a user/assistant entry. Handles
/// `message.content` as either a plain string OR an array of typed
/// blocks (text / tool_use / tool_result / image / etc.).
fn extract_text(v: &serde_json::Value) -> String {
    let content = v
        .get("message")
        .and_then(|m| m.get("content"))
        .or_else(|| v.get("content"));
    let Some(content) = content else {
        return String::new();
    };
    if let Some(s) = content.as_str() {
        return s.to_string();
    }
    if let Some(arr) = content.as_array() {
        let mut buf = String::new();
        for block in arr {
            let bty = block.get("type").and_then(|x| x.as_str()).unwrap_or("");
            match bty {
                "text" => {
                    if let Some(t) = block.get("text").and_then(|x| x.as_str()) {
                        if !buf.is_empty() {
                            buf.push('\n');
                        }
                        buf.push_str(t);
                    }
                }
                "tool_use" => {
                    if !buf.is_empty() {
                        buf.push('\n');
                    }
                    let name = block.get("name").and_then(|x| x.as_str()).unwrap_or("?");
                    buf.push_str(&format!("[Tool: {name}]"));
                }
                "tool_result" => {
                    if !buf.is_empty() {
                        buf.push('\n');
                    }
                    let snippet = extract_tool_result_summary(block);
                    buf.push_str(&format!("[Result: {}]", truncate(&snippet, 120)));
                }
                "image" => {
                    if !buf.is_empty() {
                        buf.push('\n');
                    }
                    buf.push_str("[Image]");
                }
                _ => {}
            }
        }
        return buf;
    }
    String::new()
}

fn extract_tool_use_summary(v: &serde_json::Value) -> String {
    let name = v.get("name").and_then(|x| x.as_str()).unwrap_or("?");
    // Show the first input field as a hint (e.g., `command` for Bash).
    let input_hint = v
        .get("input")
        .and_then(|i| i.as_object())
        .and_then(|m| {
            // Prefer common fields likely to be human-meaningful.
            for k in ["command", "pattern", "file_path", "path", "url", "prompt"] {
                if let Some(val) = m.get(k).and_then(|x| x.as_str()) {
                    return Some(format!("{k}: {}", truncate(val, 120)));
                }
            }
            None
        })
        .unwrap_or_default();
    if input_hint.is_empty() {
        format!("[Tool: {name}]")
    } else {
        format!("[Tool: {name}] {input_hint}")
    }
}

fn extract_tool_result_summary(v: &serde_json::Value) -> String {
    // tool_result.content may be a string or an array of {type, text}.
    let content = v.get("content");
    if let Some(s) = content.and_then(|x| x.as_str()) {
        return truncate(s, 240);
    }
    if let Some(arr) = content.and_then(|x| x.as_array()) {
        let mut buf = String::new();
        for block in arr {
            if let Some(t) = block.get("text").and_then(|x| x.as_str()) {
                if !buf.is_empty() {
                    buf.push('\n');
                }
                buf.push_str(t);
                if buf.len() > 240 {
                    break;
                }
            }
        }
        return truncate(&buf, 240);
    }
    String::new()
}

fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max_chars).collect();
    out.push('…');
    out
}
