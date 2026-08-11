//! Dev-mode tickets.
//!
//! When a workspace browser has Dev Mode on, right-clicking an element
//! captures it as a "ticket": xpath + css selector + bounded HTML +
//! computed-style summary + optional screenshot + a user-written
//! description. All CRUD flows through the four Tauri commands at the
//! bottom.
//!
//! Storage is app-local, NOT project-local: tickets live under
//! `<config_dir>/tickets/<workspace_id>/<ticket-id>.json`, alongside
//! `settings.json` and `workspaces.json`. That deliberately avoids
//! adding a `project_path` to the `Workspace` schema (which would mean
//! a `workspaces.json` migration) and keeps tickets working for remote
//! workspaces whose `cwd` lives on another machine.
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

/// Directory under `config_dir()` holding every workspace's tickets.
const TICKETS_DIRNAME: &str = "tickets";

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
    /// Placeholder for a later auto-fix hand-off (Claude Code hint).
    /// Always serialized so the schema is stable.
    #[serde(default)]
    pub source_hint: Option<String>,
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

/// `<config_dir>/tickets/<workspace_id>/`.
fn tickets_dir(workspace_id: &str) -> Result<PathBuf, String> {
    if !valid_ws_id(workspace_id) {
        return Err(format!("invalid workspace id {workspace_id:?}"));
    }
    Ok(config_dir()?.join(TICKETS_DIRNAME).join(workspace_id))
}

fn ticket_json_path(workspace_id: &str, id: &str) -> Result<PathBuf, String> {
    if !valid_id(id) {
        return Err(format!("invalid ticket id {id:?}"));
    }
    Ok(tickets_dir(workspace_id)?.join(format!("{id}.json")))
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

fn list_dir(workspace_id: &str) -> Result<Vec<Ticket>, String> {
    let dir = tickets_dir(workspace_id)?;
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    let iter = fs::read_dir(&dir).map_err(|e| format!("read_dir {:?}: {e}", dir))?;
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

#[tauri::command]
pub async fn tickets_list(workspace_id: String) -> Result<Vec<Ticket>, String> {
    let out = list_dir(&workspace_id)?;
    log_debug(
        "TICKETS",
        &format!("list ws={} count={}", workspace_id, out.len()),
    );
    Ok(out)
}

#[tauri::command]
pub async fn tickets_create(workspace_id: String, data: NewTicket) -> Result<Ticket, String> {
    let dir = tickets_dir(&workspace_id)?;
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
        source_hint: None,
    };
    let json = serde_json::to_vec_pretty(&ticket).map_err(|e| format!("serialize ticket: {e}"))?;
    write_atomic(&ticket_json_path(&workspace_id, &id)?, &json)?;

    // Rule #1: lengths, never the captured markup or the selector text.
    log_info(
        "TICKETS",
        &format!(
            "created id={} ws={} selector_len={} html_len={} shot={}",
            id,
            workspace_id,
            ticket.element.selector.len(),
            ticket.element.html.len(),
            ticket.screenshot_path.is_some()
        ),
    );
    Ok(ticket)
}

#[tauri::command]
pub async fn tickets_update(
    workspace_id: String,
    id: String,
    status: String,
) -> Result<(), String> {
    if status != "open" && status != "resolved" {
        return Err(format!("invalid status {status:?}"));
    }
    let path = ticket_json_path(&workspace_id, &id)?;
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
pub async fn tickets_delete(workspace_id: String, id: String) -> Result<(), String> {
    let path = ticket_json_path(&workspace_id, &id)?;
    // Also drop the sibling screenshot, if any.
    let png = tickets_dir(&workspace_id)?.join(png_filename_for(&id));
    if png.exists() {
        let _ = fs::remove_file(&png);
    }
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
    fn gmdate_matches_known() {
        // 2026-07-13 00:00:00 UTC
        let (y, m, d) = gmdate(1_783_900_800);
        assert_eq!((y, m, d), (2026, 7, 13));
    }
}
