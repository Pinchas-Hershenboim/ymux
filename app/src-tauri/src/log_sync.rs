//! Unified logging — remote side.
//!
//! Two responsibilities:
//!
//! 1. **Level push** (`push_log_level*`): write the Settings → Logs level
//!    into `~/.ymux/log-level` on remote hosts. The Go server watches the
//!    file (30s ticker, no restart) and the CLI hooks read it once per
//!    process, so the whole fleet converges on the desktop's setting.
//!
//! 2. **Log sync** (`spawn_log_sync`): every `SYNC_INTERVAL_SECS`, pull the
//!    *new* bytes of the three remote log files (server / hooks / install)
//!    over the existing SSH sessions and append them verbatim to the local
//!    `debug.log`, so a non-technical user reads ONE file. Remote files stay
//!    in place as backup. Per-host byte cursors persist in `log-sync.json`
//!    (atomic tmp+rename, Rule 7 style).
//!
//! Security note: pulled lines land in an LLM-readable local file — the same
//! exposure as the existing Addon Logs panel. Every remote writer already
//! enforces metadata-only content (Rule 1; see the CLI's v0.4.5 hook-log
//! migration). Nothing here relaxes that: we copy lines, never execute or
//! interpret them.

use std::collections::HashMap;
use std::sync::Arc;

use russh::client::Handle as SshHandle;
use serde::{Deserialize, Serialize};

use ymux_core::{append_raw_lines, log_debug, log_info, log_warn, shell_quote};

use crate::addons::exec;
use crate::{AppState, Session, SshClient};

/// Fixed pull cadence. Deliberately not a setting yet — one less knob.
pub(crate) const SYNC_INTERVAL_SECS: u64 = 60;

/// Per-file per-cycle byte cap: a flood on the remote can't blow past the
/// local 5 MB debug.log rotation in a couple of cycles.
const MAX_PULL_BYTES: u64 = 256 * 1024;

/// After a remote rotation/truncation, resume this far back from the new
/// EOF instead of re-ingesting the whole file.
const TRUNCATE_RESUME_BYTES: u64 = 64 * 1024;

/// The three remote logs, id → path relative to the remote `$HOME`.
const REMOTE_FILES: [(&str, &str); 3] = [
    ("server", ".ymux/server/insights.log"),
    ("hooks", ".ymux/hook-debug.log"),
    ("install", ".ymux/logs/mobile-install.log"),
];

// ─── level push ─────────────────────────────────────────────────────────────

/// Write `~/.ymux/log-level` on one remote host (Rule 3: quoted args, no
/// interpolation of anything user-shaped — `level` is a validated enum str).
pub(crate) async fn push_log_level(handle: &SshHandle<SshClient>, level: &str) -> Result<(), String> {
    let lvl = ymux_core::LogLevel::from_str(level).as_str();
    let cmd = format!(
        "mkdir -p \"$HOME/.ymux\" && printf %s {} > \"$HOME/.ymux/log-level\"",
        shell_quote(lvl)
    );
    exec(handle, &cmd, 8).await.map(|_| ())
}

/// Best-effort push to every live SSH session (deduped by host). Spawned in
/// the background by `settings::mutate` when `logs.level` changes.
pub(crate) fn push_log_level_to_all(state: &AppState, level: String) {
    let handles = collect_host_handles(state);
    if handles.is_empty() {
        return;
    }
    tauri::async_runtime::spawn(async move {
        for (host, handle) in handles {
            match push_log_level(&handle, &level).await {
                Ok(()) => log_info("LOGS", &format!("pushed log-level={level} to {host}")),
                Err(e) => log_warn("LOGS", &format!("log-level push to {host} failed: {e}")),
            }
        }
    });
}

/// One live SSH handle per distinct host.
fn collect_host_handles(state: &AppState) -> Vec<(String, Arc<SshHandle<SshClient>>)> {
    let mut by_host: HashMap<String, Arc<SshHandle<SshClient>>> = HashMap::new();
    if let Ok(sessions) = state.core.sessions.lock() {
        for sess in sessions.values() {
            if let Session::Ssh(s) = sess {
                by_host
                    .entry(format!("{}@{}:{}", s.user, s.host, s.port))
                    .or_insert_with(|| s.handle.clone());
            }
        }
    }
    by_host.into_iter().collect()
}

// ─── cursor store ───────────────────────────────────────────────────────────

/// `log-sync.json`: `{ "<host_key>": { "<file_id>": offset } }`. Offsets are
/// byte positions already ingested (0 = from the start).
type CursorMap = HashMap<String, HashMap<String, u64>>;

fn cursor_path() -> Result<std::path::PathBuf, String> {
    Ok(ymux_core::config_dir_pub()?.join("log-sync.json"))
}

fn load_cursors() -> CursorMap {
    let Ok(path) = cursor_path() else {
        return CursorMap::new();
    };
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(s.trim_start_matches('\u{FEFF}')).ok())
        .unwrap_or_default()
}

/// Rule 7: atomic write (tmp + rename) — a crash mid-write can't corrupt the
/// cursor file and trigger a full re-ingest.
fn save_cursors(cursors: &CursorMap) -> Result<(), String> {
    let path = cursor_path()?;
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    let text = serde_json::to_string(cursors).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, text).map_err(|e| format!("write {tmp:?}: {e}"))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("rename: {e}"))
}

// ─── pull cycle ─────────────────────────────────────────────────────────────

/// One remote file's section of the combined exec output.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Section {
    file_id: String,
    /// Remote size as reported by `wc -c` (0 when the file is missing).
    size: u64,
    /// Raw bytes pulled from the stored offset (may end mid-line).
    body: String,
}

/// Build the one-round-trip command: for each file, a size marker line then
/// the delta bytes. `===WMX <id> <size>===` markers are chosen to never
/// collide with log content (they start a line and we generate them).
///
/// A file with no cursor yet (first contact) gets a size-probe ONLY — no
/// body. The caller seeds its cursor near the remote EOF, so history isn't
/// back-filled into the local log (live evidence: three hosts' full logs
/// flooded local rotation within minutes).
///
/// The whole command runs with stderr dropped: exec() captures stdout and
/// stderr INTERLEAVED, and a shell redirection failure (missing file) used
/// to leak "bash: line 1: …: No such file or directory" into the middle of
/// pulled lines.
fn build_pull_cmd(offsets: &HashMap<String, u64>) -> String {
    let mut parts: Vec<String> = Vec::new();
    for (id, rel) in REMOTE_FILES {
        let path = format!("$HOME/{rel}");
        let size_marker = format!(
            "printf '===WMX %s %s===\\n' {id} \"$([ -f \"{path}\" ] && wc -c < \"{path}\" || echo 0)\""
        );
        match offsets.get(id) {
            Some(off) => parts.push(format!(
                "{size_marker}; tail -c +{start} \"{path}\" | head -c {max}",
                start = off + 1,
                max = MAX_PULL_BYTES,
            )),
            None => parts.push(size_marker),
        }
    }
    format!("{{ {}; }} 2>/dev/null", parts.join("; "))
}

/// Split the combined output back into per-file sections. Pure — unit-tested.
fn parse_sections(out: &str) -> Vec<Section> {
    let mut sections: Vec<Section> = Vec::new();
    for chunk in out.split("===WMX ") {
        if chunk.is_empty() {
            continue;
        }
        let Some((header, body)) = chunk.split_once('\n') else {
            continue;
        };
        let Some(header) = header.strip_suffix("===") else {
            continue;
        };
        let mut it = header.split_whitespace();
        let (Some(id), Some(size)) = (it.next(), it.next()) else {
            continue;
        };
        let size: u64 = size.trim().parse().unwrap_or(0);
        sections.push(Section {
            file_id: id.to_string(),
            size,
            body: body.to_string(),
        });
    }
    sections
}

/// Apply one section: append complete lines to debug.log, return the new
/// cursor offset. Pure with respect to cursor math — unit-tested directly.
///
/// `old_offset = None` means first contact with this file: seed the cursor
/// near the remote EOF and append nothing — the merged log starts "from
/// now", it doesn't import history.
fn ingest_section(host: &str, sec: &Section, old_offset: Option<u64>) -> u64 {
    let Some(old_offset) = old_offset else {
        return sec.size.saturating_sub(TRUNCATE_RESUME_BYTES);
    };
    // Remote rotated/truncated (size shrank below our cursor): resume near
    // the new EOF next cycle — never re-ingest a whole file.
    if sec.size < old_offset {
        return sec.size.saturating_sub(TRUNCATE_RESUME_BYTES);
    }
    if sec.body.is_empty() {
        return old_offset;
    }
    // Hold back a trailing partial line: the cursor only advances past the
    // last '\n' so a mid-write line is pulled whole next cycle.
    let complete_end = match sec.body.rfind('\n') {
        Some(i) => i + 1,
        None => return old_offset,
    };
    let complete = &sec.body[..complete_end];
    let n = complete.lines().count();
    log_info(
        "SYNC",
        &format!("--- {host} {}: {n} lines ---", sec.file_id),
    );
    // Phase 80: ONE submission, not one per line. A cycle can carry
    // 256 KiB x 3 files x N hosts, and feeding those in individually is what
    // turned a routine sync into thousands of open/stat/write/write/close
    // sequences — the biggest single source of interleaved log records.
    append_raw_lines(complete.lines());
    old_offset + complete_end as u64
}

/// Spawn the periodic sync task. Called once from `setup()`.
pub(crate) fn spawn_log_sync(app: tauri::AppHandle) {
    use tauri::Manager as _;
    tauri::async_runtime::spawn(async move {
        // One "sync degraded" warning per state change, not per cycle.
        let mut last_cycle_failed = false;
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(SYNC_INTERVAL_SECS)).await;
            let state = app.state::<AppState>();
            let enabled = state
                .settings
                .lock()
                .map(|s| s.logs.remote_sync)
                .unwrap_or(true);
            if !enabled {
                continue;
            }
            let handles = collect_host_handles(&state);
            if handles.is_empty() {
                continue;
            }
            let mut cursors = load_cursors();
            let mut any_err: Option<String> = None;
            for (host, handle) in handles {
                let offsets = cursors.entry(host.clone()).or_default();
                let cmd = build_pull_cmd(offsets);
                match exec(&handle, &cmd, 20).await {
                    Ok(out) => {
                        for sec in parse_sections(&out) {
                            let old = offsets.get(&sec.file_id).copied();
                            let new = ingest_section(&host, &sec, old);
                            if old != Some(new) {
                                offsets.insert(sec.file_id.clone(), new);
                            }
                        }
                    }
                    Err(e) => any_err = Some(format!("{host}: {e}")),
                }
            }
            if let Err(e) = save_cursors(&cursors) {
                log_warn("SYNC", &format!("cursor save failed: {e}"));
            }
            match (&any_err, last_cycle_failed) {
                (Some(e), false) => {
                    log_warn("SYNC", &format!("remote pull failed (will retry): {e}"));
                    last_cycle_failed = true;
                }
                (None, true) => {
                    log_debug("SYNC", "remote pull recovered");
                    last_cycle_failed = false;
                }
                _ => {}
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_sections_splits_and_reads_sizes() {
        let out = "===WMX server 120===\nline a\nline b\n===WMX hooks 0===\n===WMX install 55===\npartial tail";
        let secs = parse_sections(out);
        assert_eq!(secs.len(), 3);
        assert_eq!(secs[0].file_id, "server");
        assert_eq!(secs[0].size, 120);
        assert_eq!(secs[0].body, "line a\nline b\n");
        assert_eq!(secs[1].size, 0);
        assert_eq!(secs[1].body, "");
        assert_eq!(secs[2].body, "partial tail");
    }

    #[test]
    fn parse_sections_tolerates_garbage() {
        assert!(parse_sections("").is_empty());
        assert!(parse_sections("random output no markers").is_empty());
        // Header without the closing marker is skipped, not a panic.
        assert!(parse_sections("===WMX broken\nbody").is_empty());
    }

    #[test]
    fn cursor_resets_near_eof_after_remote_truncate() {
        let sec = Section {
            file_id: "server".into(),
            size: 100,
            body: String::new(),
        };
        // Stored offset (5000) is beyond the new size (100) → resume near EOF.
        assert_eq!(ingest_section("h", &sec, Some(5000)), 0); // 100 < 64KB → 0
        let sec_big = Section {
            file_id: "server".into(),
            size: 200_000,
            body: String::new(),
        };
        assert_eq!(
            ingest_section("h", &sec_big, Some(500_000)),
            200_000 - TRUNCATE_RESUME_BYTES
        );
    }

    #[test]
    fn first_contact_seeds_cursor_near_eof_without_ingesting() {
        // No cursor yet → seed near EOF, append nothing (no history import).
        let sec = Section {
            file_id: "server".into(),
            size: 700_000,
            body: String::new(),
        };
        assert_eq!(
            ingest_section("h", &sec, None),
            700_000 - TRUNCATE_RESUME_BYTES
        );
        // Small/missing file → 0.
        let small = Section {
            file_id: "hooks".into(),
            size: 100,
            body: String::new(),
        };
        assert_eq!(ingest_section("h", &small, None), 0);
    }

    #[test]
    fn build_pull_cmd_first_contact_probes_size_only_and_drops_stderr() {
        // Unknown files get the size marker but NO tail (no history pull),
        // and the whole command's stderr is dropped so shell redirection
        // errors can't interleave into captured output.
        let cmd = build_pull_cmd(&HashMap::new());
        assert!(!cmd.contains("tail -c"), "no body pull on first contact: {cmd}");
        assert!(cmd.starts_with("{ "), "stderr-dropping group: {cmd}");
        assert!(cmd.ends_with("; } 2>/dev/null"), "stderr dropped: {cmd}");
        assert!(cmd.contains("[ -f"), "missing-file guard: {cmd}");
    }

    #[test]
    fn cursor_holds_back_partial_trailing_line() {
        // YMUX_CONFIG_DIR isolation so the sync writes to a temp dir.
        let dir = std::env::temp_dir().join(format!("ymux-logsync-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::env::set_var("YMUX_CONFIG_DIR", &dir);

        let sec = Section {
            file_id: "hooks".into(),
            size: 1000,
            body: "full line one\nfull line two\npartial without newl".into(),
        };
        let new = ingest_section("host1", &sec, Some(10));
        // 10 + len("full line one\nfull line two\n")
        assert_eq!(new, 10 + 28);
        // Phase 80: writes are asynchronous now — read-back needs a flush.
        ymux_core::flush_log();
        let text = std::fs::read_to_string(dir.join("debug.log")).expect("log written");
        assert!(text.contains("full line two"));
        assert!(!text.contains("partial without"));

        // Body with NO newline at all → cursor unchanged, nothing written.
        let sec2 = Section {
            file_id: "hooks".into(),
            size: 1000,
            body: "no newline here".into(),
        };
        assert_eq!(ingest_section("host1", &sec2, Some(42)), 42);

        std::env::remove_var("YMUX_CONFIG_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn build_pull_cmd_uses_offsets() {
        let mut offsets = HashMap::new();
        offsets.insert("server".to_string(), 500u64);
        let cmd = build_pull_cmd(&offsets);
        // server (known cursor) resumes at offset+1; the others (first
        // contact) get a size probe only.
        assert!(cmd.contains("tail -c +501"));
        assert_eq!(cmd.matches("tail -c").count(), 1);
        assert!(cmd.contains("===WMX %s %s==="));
        assert!(cmd.contains(".ymux/hook-debug.log"));
        assert!(cmd.contains("head -c 262144"));
    }
}
