// Server-side session metadata (multi-machine sync).
//
// `~/.winmux/session-meta.json` maps a tmux session name to the Claude
// session running inside it plus display metadata, so ANY winmux desktop
// connecting to this server (home, office, laptop) can label the same
// sessions identically. Written from two directions:
//   - the Claude `stop` hook (this CLI, inside the pane): claude_session_id
//     + claude_title extracted from the transcript, every turn;
//   - the desktop app over an SSH exec (`winmux session-meta set`): the
//     creating machine's `origin` id and the user's manual `label`.
//
// Schema:
//   { "version": 1, "sessions": { "<tmux_session_name>": {
//       "claude_session_id": "...", "claude_title": "...",
//       "label": "...", "origin": "...", "updated_at": "..." } } }
//
// Concurrency: last-writer-wins over an atomic tmp+rename. Each key has a
// single effective writer (its own pane's hooks; origin/label writes are
// one-shot user actions), and the stop hook re-writes every turn, so a
// lost read-modify-write self-heals within one turn. No flock — home dirs
// are sometimes NFS where flock lies.
//
// Cleanup: no tmux session-closed hook (it wouldn't fire when the user
// disables the bundled tmux.conf). Instead every write prunes keys that
// no longer exist in `tmux ls`, and the desktop-side join ignores keys
// it can't match, so stale entries are invisible even before pruning.
//
// Rule #1: claude_title / label contain user content. They live in the
// meta FILE by design (that's the feature) but must never be written to
// hook-debug.log — callers log error kinds and session names only.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::BufRead;
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct SessionMetaEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SessionMetaFile {
    pub version: u32,
    #[serde(default)]
    pub sessions: BTreeMap<String, SessionMetaEntry>,
}

impl Default for SessionMetaFile {
    fn default() -> Self {
        Self { version: 1, sessions: BTreeMap::new() }
    }
}

fn meta_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(PathBuf::from(home).join(".winmux").join("session-meta.json"))
}

/// Missing / unreadable / corrupt file all degrade to an empty map — the
/// next save rebuilds it.
pub fn load_meta() -> SessionMetaFile {
    let Some(path) = meta_path() else { return SessionMetaFile::default() };
    match std::fs::read_to_string(&path) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
        Err(_) => SessionMetaFile::default(),
    }
}

/// Atomic write: `session-meta.<pid>.tmp` then rename over the target
/// (same pattern as the desktop's workspaces.json / tmux-labels.json).
pub fn save_meta_atomic(meta: &SessionMetaFile) -> Result<(), String> {
    let path = meta_path().ok_or("no HOME")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("mkdir: {e}"))?;
    }
    let json = serde_json::to_string_pretty(meta).map_err(|e| format!("serialize: {e}"))?;
    let tmp = path.with_file_name(format!("session-meta.{}.tmp", std::process::id()));
    std::fs::write(&tmp, json).map_err(|e| format!("write tmp: {e}"))?;
    std::fs::rename(&tmp, &path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("rename: {e}")
    })
}

/// Names of the tmux sessions currently alive on this server.
/// `None` = tmux binary couldn't even be spawned (skip pruning rather
/// than wrongly wiping the file). A running spawn with nonzero exit
/// means "no server / no sessions" — an accurate empty set.
fn live_tmux_sessions() -> Option<Vec<String>> {
    let out = std::process::Command::new("tmux")
        .args(["list-sessions", "-F", "#{session_name}"])
        .output()
        .ok()?;
    if !out.status.success() {
        return Some(Vec::new());
    }
    Some(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect(),
    )
}

/// Drop entries whose tmux session no longer exists. Returns true when
/// anything was removed.
pub fn prune(meta: &mut SessionMetaFile) -> bool {
    let Some(live) = live_tmux_sessions() else { return false };
    let before = meta.sessions.len();
    meta.sessions.retain(|name, _| live.iter().any(|l| l == name));
    meta.sessions.len() != before
}

/// Copy of the desktop's `sanitize_tmux_session_name` (lib.rs) — keep in
/// sync. Pane ids are `p_<hex>_<n>` so this is a no-op in practice.
fn sanitize_tmux_session_name(pane_id: &str) -> String {
    let cleaned: String = pane_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .collect();
    format!("winmux-{}", cleaned)
}

/// The tmux session this process runs inside. Preferred: ask tmux itself
/// (exact even when WINMUX_PANE_ID is stale — `set-environment -g` is
/// tmux-server-global, so on multi-machine servers the env var is
/// last-connector-wins). Fallback: derive from WINMUX_PANE_ID.
pub fn resolve_session_name() -> Option<String> {
    if std::env::var_os("TMUX").is_some() {
        let mut cmd = std::process::Command::new("tmux");
        cmd.arg("display-message").arg("-p");
        if let Ok(pane) = std::env::var("TMUX_PANE") {
            if !pane.is_empty() {
                cmd.arg("-t").arg(pane);
            }
        }
        cmd.arg("#S");
        if let Ok(out) = cmd.output() {
            if out.status.success() {
                let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !name.is_empty() {
                    return Some(name);
                }
            }
        }
    }
    let pane_id = std::env::var("WINMUX_PANE_ID").ok()?;
    if pane_id.is_empty() {
        return None;
    }
    Some(sanitize_tmux_session_name(&pane_id))
}

/// Best title for a Claude session, from its transcript JSONL: the LAST
/// `{"type":"summary","summary":...}` line wins (Claude Code's own session
/// title, updated over time); fallback is the first real user message,
/// truncated. Scans only the head of the file — summaries live at the
/// top, and a bounded read keeps the per-turn hook cheap on huge logs.
pub fn extract_transcript_title(path: &str) -> Option<String> {
    const MAX_LINES: usize = 250;
    const MAX_TITLE_CHARS: usize = 80;
    let file = std::fs::File::open(path).ok()?;
    let reader = std::io::BufReader::new(file);
    let mut summary: Option<String> = None;
    let mut first_user: Option<String> = None;
    for line in reader.lines().take(MAX_LINES) {
        let Ok(line) = line else { break };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
        match v.get("type").and_then(|t| t.as_str()) {
            Some("summary") => {
                if let Some(s) = v.get("summary").and_then(|s| s.as_str()) {
                    if !s.trim().is_empty() {
                        summary = Some(s.trim().to_string());
                    }
                }
            }
            Some("user") if first_user.is_none() => {
                if let Some(text) = extract_user_text(&v) {
                    first_user = Some(text);
                }
            }
            _ => {}
        }
    }
    let title = summary.or(first_user)?;
    Some(truncate_chars(&title, MAX_TITLE_CHARS))
}

/// First-user-message fallback text. Skips Claude Code's synthetic
/// user entries (slash-command echoes etc. start with `<`).
fn extract_user_text(v: &serde_json::Value) -> Option<String> {
    let content = v.get("message")?.get("content")?;
    let text = if let Some(s) = content.as_str() {
        s.to_string()
    } else {
        content
            .as_array()?
            .iter()
            .find_map(|b| {
                (b.get("type").and_then(|t| t.as_str()) == Some("text"))
                    .then(|| b.get("text").and_then(|t| t.as_str()))
                    .flatten()
            })?
            .to_string()
    };
    let text = text.trim();
    if text.is_empty() || text.starts_with('<') {
        return None;
    }
    Some(text.replace(['\n', '\r'], " "))
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", cut.trim_end())
}

pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Decode `--label-hex` (hex-encoded UTF-8). Hex sidesteps every shell /
/// SSH-exec quoting hazard for Hebrew/RTL labels.
pub fn decode_hex_utf8(hex: &str) -> Result<String, String> {
    let hex = hex.trim();
    if hex.len() % 2 != 0 {
        return Err("odd hex length".into());
    }
    let mut bytes = Vec::with_capacity(hex.len() / 2);
    let chars: Vec<char> = hex.chars().collect();
    for pair in chars.chunks(2) {
        let hi = pair[0].to_digit(16).ok_or("bad hex digit")?;
        let lo = pair[1].to_digit(16).ok_or("bad hex digit")?;
        bytes.push(((hi << 4) | lo) as u8);
    }
    String::from_utf8(bytes).map_err(|_| "not valid UTF-8".into())
}

/// Called from the Claude `stop` / `session-end` hooks (env-gated to
/// winmux panes by the caller). Best-effort: returns what it learned so
/// the caller can enrich its feed.push params; `Err` carries an error
/// KIND string safe for hook-debug.log (no user content).
///
/// Returns `(claude_title, tmux_session_name)`.
pub fn handle_hook(
    subcommand: &str,
    payload: &serde_json::Value,
) -> Result<(Option<String>, Option<String>), String> {
    let name = resolve_session_name();
    match subcommand {
        "stop" => {
            let Some(name) = name else { return Err("no-session-name".into()) };
            let session_id = payload.get("session_id").and_then(|v| v.as_str());
            let title = payload
                .get("transcript_path")
                .and_then(|v| v.as_str())
                .and_then(extract_transcript_title);
            let mut meta = load_meta();
            let entry = meta.sessions.entry(name.clone()).or_default();
            if let Some(sid) = session_id {
                entry.claude_session_id = Some(sid.to_string());
            }
            if title.is_some() {
                entry.claude_title = title.clone();
            }
            entry.updated_at = Some(now_rfc3339());
            prune(&mut meta);
            save_meta_atomic(&meta).map_err(|_| "save-failed".to_string())?;
            Ok((title, Some(name)))
        }
        "session-end" => {
            let mut meta = load_meta();
            if prune(&mut meta) {
                save_meta_atomic(&meta).map_err(|_| "save-failed".to_string())?;
            }
            Ok((None, name))
        }
        _ => Ok((None, name)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizer_matches_desktop() {
        assert_eq!(sanitize_tmux_session_name("p_18f3a2c_1"), "winmux-p_18f3a2c_1");
        assert_eq!(sanitize_tmux_session_name("a.b:c d"), "winmux-a_b_c_d");
    }

    #[test]
    fn hex_roundtrip_hebrew() {
        let label = "מחקר X";
        let hex: String = label.as_bytes().iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(decode_hex_utf8(&hex).unwrap(), label);
        assert!(decode_hex_utf8("zz").is_err());
        assert!(decode_hex_utf8("abc").is_err());
    }

    #[test]
    fn truncate_respects_char_boundaries() {
        let s = "א".repeat(100);
        let t = truncate_chars(&s, 80);
        assert!(t.chars().count() <= 80);
        assert!(t.ends_with('…'));
        assert_eq!(truncate_chars("short", 80), "short");
    }

    #[test]
    fn transcript_title_prefers_last_summary() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("winmux-meta-test-{}.jsonl", std::process::id()));
        let lines = [
            r#"{"type":"summary","summary":"Old title","leafUuid":"a"}"#,
            r#"{"type":"summary","summary":"Fix auth bug","leafUuid":"b"}"#,
            r#"{"type":"user","message":{"role":"user","content":"hello world"}}"#,
        ]
        .join("\n");
        std::fs::write(&path, lines).unwrap();
        let title = extract_transcript_title(path.to_str().unwrap());
        let _ = std::fs::remove_file(&path);
        assert_eq!(title.as_deref(), Some("Fix auth bug"));
    }

    #[test]
    fn transcript_title_falls_back_to_first_user_message() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("winmux-meta-test2-{}.jsonl", std::process::id()));
        let lines = [
            r#"{"type":"user","message":{"role":"user","content":"<command-name>/clear</command-name>"}}"#,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"real question here"}]}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"answer"}]}}"#,
        ]
        .join("\n");
        std::fs::write(&path, lines).unwrap();
        let title = extract_transcript_title(path.to_str().unwrap());
        let _ = std::fs::remove_file(&path);
        assert_eq!(title.as_deref(), Some("real question here"));
    }
}
