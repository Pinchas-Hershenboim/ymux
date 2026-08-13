//! Dev-mode tickets.
//!
//! When a workspace browser has Dev Mode on, right-clicking an element
//! captures it as a "ticket": xpath + css selector + bounded HTML +
//! computed-style summary + optional screenshot + a user-written
//! description. All CRUD flows through the four Tauri commands at the
//! bottom.
//!
//! Tickets belong to a PROJECT, not just to a workspace — the whole
//! point is to hand one to Claude Code inside the right repo. So when
//! the workspace's project is reachable from this machine they are
//! written to `<project>/.winmux-tickets/`; otherwise they fall back to
//! `<config_dir>/tickets/<workspace_id>/` and still record the project
//! path as metadata, so nothing is ever orphaned.
//!
//! The project is DERIVED, not asked for (see `resolve_project`). The
//! workspace already carries everything needed: `git_worktree`, `cwd`
//! and `connection`. Phase 54 put a `project_path` on the `Workspace`
//! struct and made the user pick a folder through an SFTP browser —
//! that meant a workspaces.json migration and a dialog for something
//! the app already knew. Here `project_path` lives on the TICKET
//! instead: every ticket is self-describing and no schema moves.
//!
//! Writes are atomic (`<name>.<pid>.tmp` then rename) per Rule #7 —
//! never a partial ticket on disk.
//!
//! Privacy (Rule #1): captured element HTML can contain whatever the
//! page rendered, including user data. Nothing in this module logs
//! element content — only ids, workspace ids, and lengths.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{config_dir, log_debug, log_info, log_warn};

/// Directory under `config_dir()` holding tickets that could not be
/// written into their project (remote workspace, folder missing).
const TICKETS_DIRNAME: &str = "tickets";

/// Folder created inside a project to hold its tickets. Dot-prefixed so
/// it sorts out of the way; committing it or gitignoring it is the
/// user's call — we never touch their .gitignore.
const PROJECT_DIRNAME: &str = ".winmux-tickets";

#[derive(Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../src/bindings/")]
pub struct TicketElement {
    #[serde(default)]
    pub xpath: String,
    #[serde(default)]
    pub selector: String,
    #[serde(default)]
    pub html: String,
    /// Free-form JSON blob of computed style / bounding box. The
    /// frontend fills whatever it can capture; the backend just
    /// round-trips it.
    #[serde(default)]
    #[ts(type = "unknown")]
    pub style: serde_json::Value,
}

#[derive(Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../src/bindings/")]
pub struct Ticket {
    pub id: String,
    /// ISO-8601 (UTC).
    pub created: String,
    pub url: String,
    pub element: TicketElement,
    /// Filename inside the workspace's tickets dir (relative), not an
    /// absolute path. `None` when no screenshot was captured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screenshot_path: Option<String>,
    #[serde(default)]
    pub description: String,
    /// Persisted as one of `"open" | "resolved"`. Kept as a String so
    /// future statuses don't need a migration.
    #[serde(default = "default_status")]
    pub status: String,
    pub workspace_id: String,
    /// Project this ticket belongs to, as an absolute path. Recorded
    /// even when the ticket had to be stored app-locally (remote
    /// workspace), so it is always attributable to a repo. `None` only
    /// when the workspace has no cwd and no worktree at all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_path: Option<String>,
    /// Placeholder for a later auto-fix hand-off (Claude Code hint).
    /// Always serialized so the schema is stable.
    #[serde(default)]
    pub source_hint: Option<String>,
}

/// Where a workspace's tickets live, and why. Returned to the frontend
/// so the capture modal can show the destination BEFORE saving — writing
/// into someone's repo should never be a surprise.
#[derive(Clone, Serialize, ts_rs::TS)]
#[ts(export, export_to = "../../src/bindings/")]
pub struct ProjectResolution {
    /// Absolute project root, when one could be derived.
    pub project_path: Option<String>,
    /// Directory tickets are actually written to.
    pub tickets_dir: String,
    /// True when `tickets_dir` is inside the project (the good case);
    /// false when we fell back to app-local storage.
    pub in_project: bool,
    /// Which rung of the ladder produced `project_path`, for the UI to
    /// explain itself: "override" | "worktree" | "git" | "cwd" | "none".
    pub source: String,
    /// Why we fell back, when `in_project` is false. Empty otherwise.
    pub fallback_reason: String,
}

fn default_status() -> String {
    "open".to_string()
}

#[derive(Clone, Deserialize)]
pub struct NewTicket {
    pub url: String,
    pub element: TicketElement,
    /// Optional PNG data-url (e.g. `data:image/png;base64,…`). When
    /// present we decode and drop it next to the JSON.
    #[serde(default)]
    pub screenshot_data_url: Option<String>,
    #[serde(default)]
    pub description: String,
}

// ─── paths ──────────────────────────────────────────────────────────

/// Best-effort validation of a ticket id: it becomes a filename, so
/// reject anything that isn't shaped like one we generated.
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() < 128
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Same guard for workspace ids, which become a directory name.
/// `new_workspace_id()` produces `w_<hex>`, so underscore is allowed
/// here but `.` and any separator still are not — no traversal.
fn valid_ws_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() < 128
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// App-local fallback: `<config_dir>/tickets/<workspace_id>/`.
fn fallback_dir(workspace_id: &str) -> Result<PathBuf, String> {
    if !valid_ws_id(workspace_id) {
        return Err(format!("invalid workspace id {workspace_id:?}"));
    }
    Ok(config_dir()?.join(TICKETS_DIRNAME).join(workspace_id))
}

// ─── remote command builders + parsers (pure, unit-tested) ──────────
//
// These build the shell one-liners the SSH backend runs, and parse what
// comes back. They are pure fn(&str)->String / fn(&str)->T on purpose:
// the shell-quoting here is the Rule #3 boundary for the whole tickets
// feature, so it has to be testable without a server.

/// Join a POSIX directory and a basename. `dir` is validated absolute by
/// `remote_project_dir`; `name` is always a `valid_id`-derived filename.
fn posix_join(dir: &str, name: &str) -> String {
    format!("{}/{}", dir.trim_end_matches('/'), name)
}

/// `<project>/.winmux-tickets` for an absolute POSIX project path.
fn remote_project_dir(project: &str) -> Result<String, String> {
    let p = project.trim_end_matches('/');
    if !p.starts_with('/') {
        return Err(format!("remote project path must be absolute: {project:?}"));
    }
    if p.contains('\\') {
        return Err(format!("remote project path must be POSIX: {project:?}"));
    }
    Ok(format!("{p}/{PROJECT_DIRNAME}"))
}

fn q(s: &str) -> String {
    winmux_core::shell_quote(s)
}

/// `git -C <cwd> rev-parse --show-toplevel`. One round trip, and it
/// resolves a linked worktree to that worktree's root, which is what we
/// want — unlike walking, which would be N SFTP round trips.
fn cmd_git_root(cwd: &str) -> String {
    format!("git -C {} rev-parse --show-toplevel 2>/dev/null", q(cwd))
}

fn cmd_mkdir_p(dir: &str) -> String {
    format!("mkdir -p {}", q(dir))
}

/// `mv -f` is rename(2): an atomic REPLACE. This is the only way to
/// overwrite an existing ticket — SFTP's rename fails when the
/// destination exists (russh-sftp negotiates no posix-rename).
fn cmd_mv_into_place(tmp: &str, dst: &str) -> String {
    format!("mv -f {} {}", q(tmp), q(dst))
}

/// One round trip for the whole listing. `tr -d` rather than
/// `base64 -w0` so BSD/macOS remotes work too; base64 so a CRLF-
/// translating server cannot corrupt the JSON framing.
fn cmd_list_json_b64(dir: &str) -> String {
    format!(
        "d={}; [ -d \"$d\" ] || exit 0; for f in \"$d\"/*.json; do [ -f \"$f\" ] || continue; printf '%s\\t' \"${{f##*/}}\"; base64 \"$f\" | tr -d '\\n'; printf '\\n'; done",
        q(dir)
    )
}

fn cmd_rm_f(paths: &[String]) -> String {
    let mut out = String::from("rm -f");
    for p in paths {
        out.push(' ');
        out.push_str(&q(p));
    }
    out
}

/// `None` = not a repo / no git / unusable output. Never an error: the
/// caller falls back to the `cwd` rung, which is correct behaviour for a
/// project that simply is not under git.
fn parse_git_root(out: &str, code: i32) -> Option<String> {
    if code != 0 {
        return None;
    }
    let first = out.lines().next()?.trim_end_matches('\r').trim();
    (first.starts_with('/')).then(|| first.to_string())
}

/// Cap on the bulk-list payload. Tickets are a few KB each; anything
/// past this is a misconfigured directory, not a ticket list.
const LIST_MAX_BYTES: usize = 8 * 1024 * 1024;

/// Parse `name	<base64>` lines. Bad lines are skipped with a warning
/// naming only the FILENAME — never the contents (Rule #1).
fn parse_list_b64(out: &str) -> Vec<(String, Vec<u8>)> {
    let mut res = Vec::new();
    for line in out.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        let Some((name, b64)) = line.split_once('\t') else {
            log_warn("TICKETS", "skipping a list line with no separator");
            continue;
        };
        if !name.ends_with(".json") {
            continue;
        }
        if b64.is_empty() {
            log_warn(
                "TICKETS",
                &format!("skip {name}: empty payload (no base64 on the remote?)"),
            );
            continue;
        }
        match base64_decode(b64) {
            Ok(bytes) => res.push((name.to_string(), bytes)),
            Err(e) => log_warn("TICKETS", &format!("skip {name}: base64: {e}")),
        }
    }
    res
}

/// Map a `\wsl.localhost\<distro>\...` (or legacy `\wsl$\...`) path back
/// to the Linux path the agent inside the distro sees.
///
/// The two forms must NOT be conflated: `project_path` on a ticket has to
/// be the LINUX path (that is what Claude Code sees), while the store has
/// to be the UNC path (that is what std::fs can open).
fn wsl_linux_from_unc(unc: &Path, distro: &str) -> Option<String> {
    let s = unc.to_string_lossy().replace('/', "\\");
    for prefix in [
        format!("\\\\wsl.localhost\\{distro}\\"),
        format!("\\\\wsl$\\{distro}\\"),
    ] {
        if let Some(rest) = s.strip_prefix(&prefix) {
            return Some(format!("/{}", rest.replace('\\', "/")));
        }
    }
    None
}

/// Rungs 1-5 of the project ladder. No I/O — `git_root` is supplied by
/// the caller because where it comes from is transport-specific (a local
/// walk, a walk over the WSL share, or `git rev-parse` over SSH).
///
/// Returns (project, which rung). The override is rung 1 and is NEVER
/// discarded here; whether it is reachable is a separate question the
/// caller answers with a transport.
fn pick_project(
    override_path: Option<&str>,
    worktree: Option<&str>,
    cwd: Option<&str>,
    git_root: Option<&str>,
) -> (Option<String>, &'static str) {
    let clean = |s: &str| {
        let t = s.trim();
        (!t.is_empty()).then(|| t.to_string())
    };
    if let Some(o) = override_path.and_then(clean) {
        return (Some(o), "override");
    }
    if let Some(w) = worktree.and_then(clean) {
        return (Some(w), "worktree");
    }
    if let Some(g) = git_root.and_then(clean) {
        return (Some(g), "git");
    }
    if let Some(c) = cwd.and_then(clean) {
        return (Some(c), "cwd");
    }
    (None, "none")
}

/// Walk up from `start` looking for a `.git` entry (file or directory —
/// a worktree's `.git` is a file). Bounded so a pathological path can't
/// spin. Returns the repo root, not the `.git` itself.
fn git_root_of(start: &Path) -> Option<PathBuf> {
    let mut cur = start;
    for _ in 0..64 {
        if cur.join(".git").exists() {
            return Some(cur.to_path_buf());
        }
        cur = cur.parent()?;
    }
    None
}

/// Translate a WSL path to its Windows UNC form so tickets can be
/// written into a WSL project from the Windows side. Only returns a
/// path that actually exists — if the distro isn't running or the share
/// isn't reachable, the caller falls back to app-local storage rather
/// than failing the save.
#[cfg(windows)]
fn wsl_unc_path(distro: Option<&str>, linux_path: &str) -> Option<PathBuf> {
    if !linux_path.starts_with('/') {
        return None;
    }
    // Without an explicit distro we can't build a share name; the
    // default-distro case falls back to app-local.
    let distro = distro?;
    let rel = linux_path.trim_start_matches('/').replace('/', "\\");
    let unc = PathBuf::from(format!("\\\\wsl.localhost\\{distro}\\{rel}"));
    if unc.exists() {
        return Some(unc);
    }
    // Older Windows builds expose the same share as \\wsl$\<distro>.
    let legacy = PathBuf::from(format!("\\\\wsl$\\{distro}\\{rel}"));
    legacy.exists().then_some(legacy)
}

#[cfg(not(windows))]
fn wsl_unc_path(_distro: Option<&str>, _linux_path: &str) -> Option<PathBuf> {
    None
}

/// Derive the project for a workspace. Ladder, most-specific first:
///   1. explicit override (the user pointed us somewhere)
///   2. `git_worktree`
///   3. git root walked up from `cwd` — a pane usually sits in a subdir
///   4. `cwd` itself
///   5. nothing
///
/// Whether we can WRITE there is a separate question, answered by the
/// connection: Local is on this machine, Wsl may be via the UNC share,
/// Ssh never is.
fn resolve_project(
    state: &crate::AppState,
    workspace_id: &str,
    override_path: Option<&str>,
) -> Result<ProjectResolution, String> {
    let fallback = fallback_dir(workspace_id)?;
    let fallback_s = fallback.to_string_lossy().to_string();

    // Snapshot what we need and drop the lock — no I/O while holding it.
    let (cwd, worktree, conn) = {
        let file = state.workspaces.lock().map_err(|e| e.to_string())?;
        let ws = file
            .workspaces
            .iter()
            .find(|w| w.id == workspace_id)
            .ok_or_else(|| format!("no workspace {workspace_id}"))?;
        (ws.cwd.clone(), ws.git_worktree.clone(), ws.connection.clone())
    };

    let override_path = override_path.map(str::trim).filter(|s| !s.is_empty());

    let (project, source) = if let Some(o) = override_path {
        (Some(PathBuf::from(o)), "override")
    } else if let Some(wt) = worktree {
        (Some(wt), "worktree")
    } else if let Some(c) = cwd.as_deref().filter(|s| !s.is_empty()) {
        match git_root_of(Path::new(c)) {
            Some(root) => (Some(root), "git"),
            None => (Some(PathBuf::from(c)), "cwd"),
        }
    } else {
        (None, "none")
    };

    let Some(project) = project else {
        return Ok(ProjectResolution {
            project_path: None,
            tickets_dir: fallback_s,
            in_project: false,
            source: source.to_string(),
            fallback_reason: "workspace has no project folder".to_string(),
        });
    };
    let project_s = project.to_string_lossy().to_string();

    // Can we reach it from this machine?
    let (writable, reason) = match &conn {
        Some(winmux_types::Connection::Ssh { host, .. }) => (
            None,
            format!("project is on {host} — stored locally, still linked"),
        ),
        Some(winmux_types::Connection::Wsl { distro }) => {
            match wsl_unc_path(distro.as_deref(), &project_s) {
                Some(unc) => (Some(unc), String::new()),
                None => (
                    None,
                    "WSL project not reachable from Windows — stored locally".to_string(),
                ),
            }
        }
        // Local, or a workspace with no connection recorded yet.
        _ => {
            if project.is_dir() {
                (Some(project.clone()), String::new())
            } else {
                (None, "project folder not found on disk".to_string())
            }
        }
    };

    Ok(match writable {
        Some(root) => ProjectResolution {
            project_path: Some(project_s),
            tickets_dir: root.join(PROJECT_DIRNAME).to_string_lossy().to_string(),
            in_project: true,
            source: source.to_string(),
            fallback_reason: String::new(),
        },
        None => ProjectResolution {
            project_path: Some(project_s),
            tickets_dir: fallback_s,
            in_project: false,
            source: source.to_string(),
            fallback_reason: reason,
        },
    })
}

/// Directory the tickets of a workspace live in, per `resolve_project`.
fn tickets_dir(
    state: &crate::AppState,
    workspace_id: &str,
    override_path: Option<&str>,
) -> Result<PathBuf, String> {
    Ok(PathBuf::from(
        resolve_project(state, workspace_id, override_path)?.tickets_dir,
    ))
}

fn ticket_json_path(
    state: &crate::AppState,
    workspace_id: &str,
    override_path: Option<&str>,
    id: &str,
) -> Result<PathBuf, String> {
    if !valid_id(id) {
        return Err(format!("invalid ticket id {id:?}"));
    }
    Ok(tickets_dir(state, workspace_id, override_path)?.join(format!("{id}.json")))
}

fn png_filename_for(id: &str) -> String {
    format!("{id}.png")
}

fn ensure_dir(dir: &Path) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| format!("create {:?}: {e}", dir))
}

/// Atomic write: `<name>.<pid>.tmp` then rename. Rule #7.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("no parent dir for {:?}", path))?;
    ensure_dir(parent)?;
    let tmp = parent.join(format!(
        "{}.{}.tmp",
        path.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("ticket"),
        std::process::id()
    ));
    {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp)
            .map_err(|e| format!("open {:?}: {e}", tmp))?;
        f.write_all(bytes)
            .map_err(|e| format!("write {:?}: {e}", tmp))?;
        f.sync_all().map_err(|e| format!("fsync {:?}: {e}", tmp))?;
    }
    fs::rename(&tmp, path).map_err(|e| format!("rename {:?} -> {:?}: {e}", tmp, path))
}

// ─── ids + time ─────────────────────────────────────────────────────

/// nanoid-ish: 6 chars from a URL-safe alphabet, seeded off wall-clock
/// nanos + a process-scoped counter. Not cryptographically random —
/// just needs to be collision-free per day per workspace.
fn short_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let bump = N.fetch_add(1, Ordering::Relaxed);
    let mut x = t.wrapping_mul(1_000_003).wrapping_add(bump);
    const ALPH: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut s = String::with_capacity(6);
    for _ in 0..6 {
        let idx = (x % ALPH.len() as u64) as usize;
        s.push(ALPH[idx] as char);
        x /= ALPH.len() as u64;
    }
    s
}

/// `ticket-YYYY-MM-DD-<6char>`. Date is derived from unix seconds
/// without pulling `chrono` in for one caller — a tiny gmtime is fine.
fn make_ticket_id() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (y, m, d) = gmdate(secs as i64);
    format!("ticket-{:04}-{:02}-{:02}-{}", y, m, d, short_id())
}

fn iso8601_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (y, mo, d) = gmdate(secs as i64);
    let day_secs = secs % 86_400;
    let h = day_secs / 3600;
    let mi = (day_secs / 60) % 60;
    let s = day_secs % 60;
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", y, mo, d, h, mi, s)
}

/// Days-from-unix-epoch → (year, month, day) via the civil-from-days
/// algorithm (Howard Hinnant, hh_date). Handles all Gregorian dates.
fn gmdate(secs: i64) -> (i32, u32, u32) {
    let days = secs.div_euclid(86_400);
    // Shift epoch to March 1, 0000: this makes the algorithm simple.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as i64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m, d)
}

// ─── screenshot decoding ────────────────────────────────────────────

fn decode_data_url_png(data_url: &str) -> Result<Vec<u8>, String> {
    // `data:image/png;base64,AAAA…`
    let prefix = "data:image/png;base64,";
    let b64 = data_url
        .strip_prefix(prefix)
        .ok_or_else(|| "screenshot data-url is not image/png;base64".to_string())?;
    base64_decode(b64)
}

/// Minimal base64 decoder — didn't want to pull in a crate for this.
/// Accepts both the standard (`+/`) and URL-safe (`-_`) alphabets and
/// tolerates missing padding, so the browser bridge can hand us
/// base64url straight out of the page. Shared with
/// `workspace_browser`'s `winmux-ticket:` navigation bridge.
pub(crate) fn base64_decode(s: &str) -> Result<Vec<u8>, String> {
    fn idx(b: u8) -> Result<u32, String> {
        match b {
            b'A'..=b'Z' => Ok(u32::from(b - b'A')),
            b'a'..=b'z' => Ok(u32::from(b - b'a' + 26)),
            b'0'..=b'9' => Ok(u32::from(b - b'0' + 52)),
            b'+' | b'-' => Ok(62),
            b'/' | b'_' => Ok(63),
            _ => Err(format!("bad base64 byte 0x{b:02x}")),
        }
    }
    let clean: Vec<u8> = s
        .bytes()
        .filter(|b| !b.is_ascii_whitespace() && *b != b'=')
        .collect();
    let mut out = Vec::with_capacity(clean.len() / 4 * 3);
    let mut i = 0;
    while i + 4 <= clean.len() {
        let a = idx(clean[i])?;
        let b = idx(clean[i + 1])?;
        let c = idx(clean[i + 2])?;
        let d = idx(clean[i + 3])?;
        let v = (a << 18) | (b << 12) | (c << 6) | d;
        out.push(((v >> 16) & 0xff) as u8);
        out.push(((v >> 8) & 0xff) as u8);
        out.push((v & 0xff) as u8);
        i += 4;
    }
    match clean.len() - i {
        0 => {}
        2 => {
            let a = idx(clean[i])?;
            let b = idx(clean[i + 1])?;
            let v = (a << 18) | (b << 12);
            out.push(((v >> 16) & 0xff) as u8);
        }
        3 => {
            let a = idx(clean[i])?;
            let b = idx(clean[i + 1])?;
            let c = idx(clean[i + 2])?;
            let v = (a << 18) | (b << 12) | (c << 6);
            out.push(((v >> 16) & 0xff) as u8);
            out.push(((v >> 8) & 0xff) as u8);
        }
        _ => return Err("truncated base64".into()),
    }
    Ok(out)
}

// ─── listing ────────────────────────────────────────────────────────

fn list_dir(dir: &Path) -> Result<Vec<Ticket>, String> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    let iter = fs::read_dir(dir).map_err(|e| format!("read_dir {:?}: {e}", dir))?;
    for entry in iter {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                log_warn("TICKETS", &format!("skip dir entry: {e}"));
                continue;
            }
        };
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        // Filenames only — never the file body (Rule #1).
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("<unnamed>")
            .to_string();
        let text = match fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                log_warn("TICKETS", &format!("skip {name}: read: {e}"));
                continue;
            }
        };
        match serde_json::from_str::<Ticket>(&text) {
            Ok(t) => out.push(t),
            Err(e) => log_warn("TICKETS", &format!("skip {name}: parse: {e}")),
        }
    }
    out.sort_by(|a, b| b.created.cmp(&a.created));
    Ok(out)
}

// ─── Tauri commands ─────────────────────────────────────────────────
//
// Every command takes an optional `project_override`. It is the user's
// per-workspace escape hatch (kept in localStorage on the frontend, so
// no schema moves) and is fed through the same `resolve_project` ladder
// as everything else — the frontend never picks the directory itself.

/// Where this workspace's tickets go, and why. The capture modal calls
/// this before saving so the destination is visible up front.
#[tauri::command]
pub async fn tickets_resolve_project(
    state: tauri::State<'_, crate::AppState>,
    workspace_id: String,
    project_override: Option<String>,
) -> Result<ProjectResolution, String> {
    resolve_project(&state, &workspace_id, project_override.as_deref())
}

#[tauri::command]
pub async fn tickets_list(
    state: tauri::State<'_, crate::AppState>,
    workspace_id: String,
    project_override: Option<String>,
) -> Result<Vec<Ticket>, String> {
    let dir = tickets_dir(&state, &workspace_id, project_override.as_deref())?;
    let out = list_dir(&dir)?;
    log_debug(
        "TICKETS",
        &format!("list ws={} count={}", workspace_id, out.len()),
    );
    Ok(out)
}

/// Absolute path of the tickets folder, for "reveal in file manager".
/// Created on demand so revealing works before the first ticket exists.
#[tauri::command]
pub async fn tickets_dir_path(
    state: tauri::State<'_, crate::AppState>,
    workspace_id: String,
    project_override: Option<String>,
) -> Result<String, String> {
    let dir = tickets_dir(&state, &workspace_id, project_override.as_deref())?;
    ensure_dir(&dir)?;
    Ok(dir.to_string_lossy().to_string())
}

#[tauri::command]
pub async fn tickets_create(
    state: tauri::State<'_, crate::AppState>,
    workspace_id: String,
    project_override: Option<String>,
    data: NewTicket,
) -> Result<Ticket, String> {
    let resolved = resolve_project(&state, &workspace_id, project_override.as_deref())?;
    let dir = PathBuf::from(&resolved.tickets_dir);
    ensure_dir(&dir)?;
    let id = make_ticket_id();

    let mut screenshot_rel: Option<String> = None;
    if let Some(url) = data.screenshot_data_url.as_deref() {
        match decode_data_url_png(url) {
            Ok(bytes) => {
                let name = png_filename_for(&id);
                write_atomic(&dir.join(&name), &bytes)?;
                screenshot_rel = Some(name);
            }
            Err(e) => log_warn("TICKETS", &format!("screenshot decode skipped: {e}")),
        }
    }

    let ticket = Ticket {
        id: id.clone(),
        created: iso8601_now(),
        url: data.url,
        element: data.element,
        screenshot_path: screenshot_rel,
        description: data.description,
        status: default_status(),
        workspace_id: workspace_id.clone(),
        // Recorded even when we stored app-locally, so the ticket still
        // points at the repo it is about.
        project_path: resolved.project_path.clone(),
        source_hint: None,
    };
    let json = serde_json::to_vec_pretty(&ticket).map_err(|e| format!("serialize ticket: {e}"))?;
    write_atomic(&dir.join(format!("{id}.json")), &json)?;

    // Rule #1: lengths and routing, never the captured markup.
    log_info(
        "TICKETS",
        &format!(
            "created id={} ws={} in_project={} src={} selector_len={} html_len={}",
            id,
            workspace_id,
            resolved.in_project,
            resolved.source,
            ticket.element.selector.len(),
            ticket.element.html.len()
        ),
    );
    Ok(ticket)
}

#[tauri::command]
pub async fn tickets_update(
    state: tauri::State<'_, crate::AppState>,
    workspace_id: String,
    project_override: Option<String>,
    id: String,
    status: String,
) -> Result<(), String> {
    if status != "open" && status != "resolved" {
        return Err(format!("invalid status {status:?}"));
    }
    let path = ticket_json_path(&state, &workspace_id, project_override.as_deref(), &id)?;
    let text = fs::read_to_string(&path).map_err(|e| format!("read {:?}: {e}", path))?;
    let mut ticket: Ticket =
        serde_json::from_str(&text).map_err(|e| format!("parse ticket {id}: {e}"))?;
    ticket.status = status.clone();
    let json = serde_json::to_vec_pretty(&ticket).map_err(|e| format!("serialize ticket: {e}"))?;
    write_atomic(&path, &json)?;
    log_info(
        "TICKETS",
        &format!("update id={id} ws={workspace_id} status={status}"),
    );
    Ok(())
}

#[tauri::command]
pub async fn tickets_delete(
    state: tauri::State<'_, crate::AppState>,
    workspace_id: String,
    project_override: Option<String>,
    id: String,
) -> Result<(), String> {
    let dir = tickets_dir(&state, &workspace_id, project_override.as_deref())?;
    if !valid_id(&id) {
        return Err(format!("invalid ticket id {id:?}"));
    }
    // Also drop the sibling screenshot, if any.
    let png = dir.join(png_filename_for(&id));
    if png.exists() {
        let _ = fs::remove_file(&png);
    }
    let path = dir.join(format!("{id}.json"));
    if path.exists() {
        fs::remove_file(&path).map_err(|e| format!("remove {:?}: {e}", path))?;
    }
    log_info("TICKETS", &format!("delete id={id} ws={workspace_id}"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_roundtrip_small() {
        let decoded = base64_decode("aGVsbG8gd29ybGQ").unwrap();
        assert_eq!(decoded, b"hello world");
    }

    #[test]
    fn short_id_is_six_chars() {
        let s = short_id();
        assert_eq!(s.len(), 6);
        assert!(s.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn ticket_id_shape() {
        let id = make_ticket_id();
        assert!(id.starts_with("ticket-"));
        assert!(valid_id(&id));
    }

    #[test]
    fn valid_id_rejects_traversal() {
        assert!(!valid_id("../foo"));
        assert!(!valid_id("hello/world"));
        assert!(!valid_id(""));
        // ticket ids never contain underscores
        assert!(!valid_id("w_1a2b"));
    }

    #[test]
    fn valid_ws_id_accepts_generated_shape_but_not_traversal() {
        assert!(valid_ws_id("w_18f3a9c2d"));
        assert!(!valid_ws_id("../../etc"));
        assert!(!valid_ws_id("a/b"));
        assert!(!valid_ws_id("a.b"));
        assert!(!valid_ws_id(""));
    }

    #[test]
    fn git_root_walks_up_from_a_subdir() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let root = tmp.path();
        fs::create_dir_all(root.join(".git")).unwrap();
        let deep = root.join("app").join("src").join("components");
        fs::create_dir_all(&deep).unwrap();
        // A pane sitting three levels in must still resolve to the repo
        // root — this is the whole reason we walk instead of using cwd.
        assert_eq!(git_root_of(&deep).as_deref(), Some(root));
        assert_eq!(git_root_of(root).as_deref(), Some(root));
    }

    #[test]
    fn git_root_finds_worktree_dot_git_file() {
        // In a linked worktree `.git` is a FILE, not a directory.
        let tmp = tempfile::tempdir().expect("tmpdir");
        let root = tmp.path();
        fs::write(root.join(".git"), "gitdir: /somewhere/.git/worktrees/x").unwrap();
        let sub = root.join("src");
        fs::create_dir_all(&sub).unwrap();
        assert_eq!(git_root_of(&sub).as_deref(), Some(root));
    }

    #[test]
    fn git_root_is_none_outside_a_repo() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let deep = tmp.path().join("a").join("b");
        fs::create_dir_all(&deep).unwrap();
        // tempdir lives under the system temp root, which is not a repo.
        assert!(git_root_of(&deep).is_none());
    }

    #[test]
    fn wsl_unc_rejects_relative_and_missing_distro() {
        // Non-absolute linux path is never translatable.
        assert!(wsl_unc_path(Some("Ubuntu"), "home/u/p").is_none());
        // Default distro (None) has no share name to build.
        assert!(wsl_unc_path(None, "/home/u/p").is_none());
        // A distro that cannot exist resolves to a path that does not
        // exist, so translation declines rather than handing back a
        // bogus destination.
        assert!(wsl_unc_path(Some("no-such-distro-zzz"), "/home/u/p").is_none());
    }

    // ─── Rule #3 regression guard ───────────────────────────────────
    //
    // Every remote command interpolates a caller-controlled path. If a
    // metacharacter ever escapes the quoting, these fail. This is the
    // highest-value test in the module.

    /// Paths that would be catastrophic if the quoting broke.
    fn nasty_paths() -> Vec<String> {
        vec![
            "/home/u/my project".to_string(),
            "/home/u/it's".to_string(),
            "/home/u/x; rm -rf /".to_string(),
            "/home/u/$(id)".to_string(),
            "/home/u/`id`".to_string(),
            "/home/u/x&&whoami".to_string(),
            "/home/u/x|tee /tmp/pwn".to_string(),
            "/home/u/new\nline".to_string(),
            "/home/u/$HOME".to_string(),
            "/home/u/*".to_string(),
        ]
    }

    /// Everything after the first quote must live inside single quotes,
    /// with the only escape being the '"'"'\''"'"' idiom shell_quote emits.
    fn assert_neutralized(cmd: &str, raw: &str) {
        let quoted = winmux_core::shell_quote(raw);
        assert!(
            cmd.contains(&quoted),
            "path was not shell-quoted into the command.\n  cmd={cmd}\n  want={quoted}"
        );
        // The raw form must never appear unquoted next to a metachar.
        for meta in [";", "&&", "|", "$(", "`"] {
            if raw.contains(meta) {
                let bare = format!(" {raw}");
                assert!(
                    !cmd.contains(&bare),
                    "raw path with {meta:?} leaked into {cmd}"
                );
            }
        }
    }

    #[test]
    fn cmd_builders_quote_every_hostile_path() {
        for raw in nasty_paths() {
            assert_neutralized(&cmd_git_root(&raw), &raw);
            assert_neutralized(&cmd_mkdir_p(&raw), &raw);
            assert_neutralized(&cmd_list_json_b64(&raw), &raw);
            assert_neutralized(&cmd_rm_f(&[raw.clone()]), &raw);
            let dst = format!("{raw}/t.json");
            let mv = cmd_mv_into_place(&format!("{dst}.tmp"), &dst);
            assert_neutralized(&mv, &dst);
        }
    }

    #[test]
    fn cmd_rm_f_quotes_each_path_separately() {
        let cmd = cmd_rm_f(&["/a/x.json".to_string(), "/a/x; reboot.png".to_string()]);
        assert!(cmd.starts_with("rm -f "));
        assert!(cmd.contains(&winmux_core::shell_quote("/a/x.json")));
        assert!(cmd.contains(&winmux_core::shell_quote("/a/x; reboot.png")));
        assert!(!cmd.contains("; reboot.png'") || cmd.matches("'").count() >= 4);
    }

    #[test]
    fn cmd_list_uses_portable_base64() {
        let cmd = cmd_list_json_b64("/p/.winmux-tickets");
        // -w0 is GNU-only; BSD/macOS remotes need the tr form.
        assert!(!cmd.contains("-w0"), "must not depend on GNU base64");
        assert!(cmd.contains("tr -d"));
        // A missing directory must be a clean exit, not an error.
        assert!(cmd.contains("|| exit 0"));
    }

    // ─── parse_git_root ─────────────────────────────────────────────

    #[test]
    fn parse_git_root_accepts_a_clean_path() {
        assert_eq!(
            parse_git_root("/home/u/proj\n", 0).as_deref(),
            Some("/home/u/proj")
        );
    }

    #[test]
    fn parse_git_root_tolerates_crlf() {
        assert_eq!(
            parse_git_root("/home/u/proj\r\n", 0).as_deref(),
            Some("/home/u/proj")
        );
    }

    #[test]
    fn parse_git_root_rejects_failure_and_junk() {
        // not a repo
        assert!(parse_git_root("fatal: not a git repository", 128).is_none());
        // git missing -> shell reports 127
        assert!(parse_git_root("", 127).is_none());
        // success but empty
        assert!(parse_git_root("", 0).is_none());
        // success but not an absolute POSIX path (never trust it)
        assert!(parse_git_root("C:/proj", 0).is_none());
        assert!(parse_git_root("relative/path", 0).is_none());
    }

    // ─── parse_list_b64 ─────────────────────────────────────────────

    fn b64u(bytes: &[u8]) -> String {
        const ALPH: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
            let v = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
            out.push(ALPH[(v >> 18) as usize & 63] as char);
            out.push(ALPH[(v >> 12) as usize & 63] as char);
            if chunk.len() > 1 { out.push(ALPH[(v >> 6) as usize & 63] as char); }
            if chunk.len() > 2 { out.push(ALPH[v as usize & 63] as char); }
        }
        out
    }

    #[test]
    fn parse_list_b64_reads_a_good_line() {
        let payload = b"{\"id\":\"ticket-1\"}";
        let line = format!("ticket-1.json\t{}", b64u(payload));
        let got = parse_list_b64(&line);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, "ticket-1.json");
        assert_eq!(got[0].1, payload);
    }

    #[test]
    fn parse_list_b64_skips_malformed_lines_without_failing_the_batch() {
        let good = format!("ok.json\t{}", b64u(b"{}"));
        let out = [
            "no-separator-here",           // no tab
            "notes.txt\tYWJj",             // not .json
            "empty.json\t",                // no base64 binary on the remote
            "bad.json\t!!!!not-base64!!!!", // garbage
            &good,
            "",                            // blank
        ]
        .join("\n");
        let got = parse_list_b64(&out);
        // One bad file must not lose the rest of the list.
        assert_eq!(got.len(), 1, "only the good line should survive");
        assert_eq!(got[0].0, "ok.json");
    }

    #[test]
    fn parse_list_b64_tolerates_crlf_from_the_remote() {
        let line = format!("a.json\t{}\r", b64u(b"{}"));
        assert_eq!(parse_list_b64(&line).len(), 1);
    }

    // ─── posix paths ────────────────────────────────────────────────

    #[test]
    fn remote_project_dir_requires_an_absolute_posix_path() {
        assert_eq!(
            remote_project_dir("/home/u/proj").unwrap(),
            "/home/u/proj/.winmux-tickets"
        );
        // trailing slash must not double up
        assert_eq!(
            remote_project_dir("/home/u/proj/").unwrap(),
            "/home/u/proj/.winmux-tickets"
        );
        assert!(remote_project_dir("proj").is_err());
        // A Windows path on an SSH workspace is a user mistake worth naming.
        assert!(remote_project_dir("C:\\proj").is_err());
    }

    #[test]
    fn posix_join_does_not_double_the_separator() {
        assert_eq!(posix_join("/a/b", "c.json"), "/a/b/c.json");
        assert_eq!(posix_join("/a/b/", "c.json"), "/a/b/c.json");
    }

    // ─── WSL path translation ───────────────────────────────────────

    #[test]
    fn wsl_unc_and_linux_round_trip() {
        for prefix in ["wsl.localhost", "wsl$"] {
            let unc = std::path::PathBuf::from(format!(
                "\\\\{prefix}\\Ubuntu\\home\\u\\proj"
            ));
            assert_eq!(
                wsl_linux_from_unc(&unc, "Ubuntu").as_deref(),
                Some("/home/u/proj"),
                "{prefix} form must map back to the Linux path"
            );
        }
    }

    #[test]
    fn wsl_linux_from_unc_rejects_another_distro() {
        let unc = std::path::PathBuf::from(format!(
            "\\\\wsl.localhost\\Debian\\home\\u"
        ));
        assert!(wsl_linux_from_unc(&unc, "Ubuntu").is_none());
    }

    // ─── the ladder (bug #1 lives here) ─────────────────────────────

    #[test]
    fn pick_project_prefers_the_override_above_everything() {
        // THE regression guard for the bug where an override was silently
        // discarded on SSH workspaces.
        let (p, src) = pick_project(
            Some("/srv/chosen"),
            Some("/srv/worktree"),
            Some("/srv/cwd"),
            Some("/srv/gitroot"),
        );
        assert_eq!(p.as_deref(), Some("/srv/chosen"));
        assert_eq!(src, "override");
    }

    #[test]
    fn pick_project_ladder_order() {
        let cases: Vec<(Option<&str>, Option<&str>, Option<&str>, Option<&str>, &str, &str)> = vec![
            (None, Some("/w"), Some("/c"), Some("/g"), "/w", "worktree"),
            (None, None, Some("/c"), Some("/g"), "/g", "git"),
            (None, None, Some("/c"), None, "/c", "cwd"),
        ];
        for (o, w, c, g, want, want_src) in cases {
            let (p, src) = pick_project(o, w, c, g);
            assert_eq!(p.as_deref(), Some(want));
            assert_eq!(src, want_src);
        }
        let (p, src) = pick_project(None, None, None, None);
        assert!(p.is_none());
        assert_eq!(src, "none");
    }

    #[test]
    fn pick_project_treats_blank_strings_as_absent() {
        // workspace_update writes "" to clear cwd, so empties must not
        // win a rung and produce a path of "".
        let (p, src) = pick_project(Some("  "), Some(""), Some("/c"), None);
        assert_eq!(p.as_deref(), Some("/c"));
        assert_eq!(src, "cwd");
    }

    #[test]
    fn gmdate_matches_known() {
        // 2026-07-13 00:00:00 UTC
        let (y, m, d) = gmdate(1_783_900_800);
        assert_eq!((y, m, d), (2026, 7, 13));
    }
}
