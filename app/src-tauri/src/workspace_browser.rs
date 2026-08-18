//! Phase 53 (rebased): workspace-level Browser singleton.
//!
//! Replaces the Phase 53.A per-pane Browser surface. Each workspace
//! owns AT MOST ONE child Webview attached to the main window. When
//! the user opens the floating Browser window for workspace `w_X`, the
//! frontend calls `workspace_browser_show(w_X, url, x, y, w, h)`:
//!
//! - If no Webview exists for `w_X` yet, we spawn one via
//!   `Window::add_child(WebviewBuilder, ...)`. Phase 62.A (item D):
//!   all browser webviews share the process-DEFAULT WebView2
//!   environment (same as the main window). We do NOT pass a
//!   per-workspace `--user-data-dir` — that forced a SEPARATE WebView2
//!   environment per workspace, and WebView2 does not support multiple
//!   environments in one process: the conflict surfaced as
//!   intermittent 0x8007139F (ERROR_INVALID_STATE) on creation. The
//!   supported shape is one environment with many webviews. Trade-off:
//!   workspaces share browser cookies / cache (acceptable — the
//!   browser only views tunneled localhost services).
//! - If one already exists, we reposition/resize it and call
//!   `.show()` (the floating-window pattern can hide a workspace's
//!   browser when the user closes the floating panel, then bring it
//!   back when they reopen it without losing the page state).
//!
//! Z-order: native Webview always paints above HTML, so any modal
//! opening in the SolidJS layer broadcasts `workspace_browser_hide`
//! for the active workspace and `workspace_browser_show` again on
//! close.
//!
//! Pinned tauri =2.10.3 with `features = ["unstable"]` — the
//! `Window::add_child(WebviewBuilder, ...)` API still lives behind
//! the unstable gate in this version. See CLAUDE.md "Pinned deps".

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tauri::webview::WebviewBuilder;
use tauri::{
    AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, State, Url, Webview, WebviewUrl,
};

use crate::{config_dir, log_debug, log_info, log_warn, AppState};

/// URL scheme the Dev-Mode inspect script uses to hand a captured
/// element back to the app. See `install_ticket_bridge`.
const TICKET_SCHEME: &str = "ymux-ticket";

/// Event the main window listens on to open the ticket modal.
const TICKET_EVENT: &str = "browser:ticket-captured";

/// Payload delivered to the frontend for one captured element.
#[derive(Clone, serde::Serialize)]
struct TicketCapture {
    workspace_id: String,
    /// The raw object the inspect script built (xpath / selector /
    /// html / style / url). Round-tripped as-is — `tickets_create`
    /// is what gives it a schema.
    capture: serde_json::Value,
}

/// The child webview is a plain external page: no Tauri IPC is injected
/// into it (and injecting it would hand a tunneled third-party service
/// the app's command surface). Tauri 2.10's `WebviewBuilder` has no
/// `on_ipc` either. So the Dev-Mode inspect script talks back through
/// the one hook that does exist — navigation.
///
/// The script sets `location.href = "ymux-ticket:<base64url>"`. This
/// handler decodes it, emits `browser:ticket-captured`, and returns
/// `false` so the navigation itself never happens. **Every other URL
/// returns `true`** — normal browsing, the port+path model, tabs and
/// `workspace_browser_navigate` all take the exact same code path they
/// did before this existed.
///
/// Note the scheme is used WITHOUT `//`: `ymux-ticket:<data>` parses
/// as a cannot-be-a-base URL, so the payload lands in `path()` verbatim.
/// The `//` form would put it in the *authority* slot instead —
/// `host_str()` gets the data and `path()` comes back empty — and routes
/// it through host normalization. Case happens to survive there (this is
/// a non-special scheme), but the payload is data, not an authority, so
/// the opaque form is the correct shape. See `ticket_bridge_tests`.
fn handle_ticket_navigation(app: &AppHandle, workspace_id: &str, url: &Url) -> bool {
    if url.scheme() != TICKET_SCHEME {
        return true; // not ours — let the webview navigate normally
    }
    // `ymux-ticket:AAAA` → path() is "AAAA" (cannot-be-a-base URL).
    let encoded = url.path();
    match decode_capture(encoded) {
        Ok(capture) => {
            let _ = app.emit(
                TICKET_EVENT,
                TicketCapture {
                    workspace_id: workspace_id.to_string(),
                    capture,
                },
            );
            // Rule #1: the capture holds page markup. Length only.
            log_info(
                "TICKETS",
                &format!(
                    "capture from browser ws={} payload_len={}",
                    workspace_id,
                    encoded.len()
                ),
            );
        }
        Err(e) => log_warn(
            "TICKETS",
            &format!("bad capture payload ws={workspace_id}: {e}"),
        ),
    }
    false // never actually navigate to the sentinel
}

fn decode_capture(encoded: &str) -> Result<serde_json::Value, String> {
    let bytes = crate::tickets::base64_decode(encoded)?;
    let text = String::from_utf8(bytes).map_err(|e| format!("capture is not utf-8: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("capture is not json: {e}"))
}

/// Map of `workspace_id -> Webview`. Exactly one entry per workspace
/// that has opened its Browser at least once this session. Cleared
/// by `workspace_browser_close` (user closed the floating window
/// explicitly) and by `cleanup_workspace_sessions` (user deleted the
/// workspace).
pub(crate) type WorkspaceBrowserMap = Arc<Mutex<HashMap<String, Webview>>>;

fn webview_label(workspace_id: &str) -> String {
    // Tauri webview labels are constrained to [a-zA-Z-/:_].
    // workspace_id is `w_<hex>` which is alnum+underscore — safe.
    format!("workspace-browser-{workspace_id}")
}

/// Workspace IDs are `w_<hex>` so this is defence-in-depth — any
/// `..`, `/`, `\` would already be rejected by the ID format.
fn sanitize_for_path(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Spawn (if absent) + reposition + show. Frontend calls this every
/// time the floating Browser window mounts or its rect changes.
///
/// `url` is only consulted when we actually spawn a new Webview —
/// re-shows leave the existing Webview's URL alone (Browser window
/// preserves page state across hide/show cycles).
#[tauri::command]
pub(crate) async fn workspace_browser_show(
    state: State<'_, AppState>,
    app: AppHandle,
    workspace_id: String,
    url: String,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
) -> Result<(), String> {
    // Fast path: the Webview already exists. Reposition + .show().
    {
        let map = state.workspace_browsers.lock().unwrap();
        if let Some(webview) = map.get(&workspace_id).cloned() {
            drop(map);
            webview
                .set_position(LogicalPosition::new(x, y))
                .map_err(|e| format!("set_position: {e}"))?;
            webview
                .set_size(LogicalSize::new(w.max(1.0), h.max(1.0)))
                .map_err(|e| format!("set_size: {e}"))?;
            webview.show().map_err(|e| e.to_string())?;
            return Ok(());
        }
    }
    // Slow path: spawn a new child Webview. Phase 62.A (item D):
    // serialize creation across ALL workspaces — WebView2 dislikes
    // concurrent environment creation and returns 0x8007139F
    // (ERROR_INVALID_STATE). The guard is held across the whole creation
    // (including the retry backoff) so two rapid opens can't race.
    let _create_guard = state.browser_create_lock.lock().await;

    // Re-check under the creation lock: another call may have created
    // the webview while we were waiting on the lock. Without this, two
    // concurrent show() calls for the same workspace would both miss the
    // fast path above and both call add_child → a duplicate-label /
    // same-user-data-dir failure (a second ERROR_INVALID_STATE source).
    {
        let map = state.workspace_browsers.lock().unwrap();
        if let Some(webview) = map.get(&workspace_id).cloned() {
            drop(map);
            webview
                .set_position(LogicalPosition::new(x, y))
                .map_err(|e| format!("set_position: {e}"))?;
            webview
                .set_size(LogicalSize::new(w.max(1.0), h.max(1.0)))
                .map_err(|e| format!("set_size: {e}"))?;
            webview.show().map_err(|e| e.to_string())?;
            return Ok(());
        }
    }

    let main_window = app
        .get_window("main")
        .ok_or_else(|| "main window not found".to_string())?;
    let parsed_url: Url = url
        .parse()
        .map_err(|e| format!("invalid url {url:?}: {e}"))?;
    let label = webview_label(&workspace_id);

    // Retry the transient WebView2 ERROR_INVALID_STATE a couple of times
    // with a short backoff (the builder is consumed by add_child, so we
    // rebuild it each attempt). A clean failure is surfaced to the FE
    // only after all attempts are exhausted. NOTE: the real fix for the
    // 0x8007139F Yossi hit is using the DEFAULT WebView2 environment (no
    // per-workspace --user-data-dir) — see the module doc. The retry /
    // creation lock stay as defense-in-depth for genuine transients.
    const MAX_ATTEMPTS: u32 = 3;
    let mut last_err = String::new();
    let mut created = None;
    for attempt in 1..=MAX_ATTEMPTS {
        // Default environment — shared with the main window + every other
        // workspace browser. One WebView2 environment per process.
        // Only addition to the builder: the ticket bridge. It is inert
        // for every URL that isn't `ymux-ticket:` (returns true =
        // navigate as before), and nothing is injected into the page.
        let bridge_app = app.clone();
        let bridge_ws = workspace_id.clone();
        let builder = WebviewBuilder::new(&label, WebviewUrl::External(parsed_url.clone()))
            .on_navigation(move |url| handle_ticket_navigation(&bridge_app, &bridge_ws, url));
        match main_window.add_child(
            builder,
            LogicalPosition::new(x, y),
            LogicalSize::new(w.max(1.0), h.max(1.0)),
        ) {
            Ok(wv) => {
                log_debug("BROWSER", &format!(
                    "[workspace_browser_show] add_child ws={} ok (attempt {}/{})",
                    workspace_id, attempt, MAX_ATTEMPTS
                ));
                created = Some(wv);
                break;
            }
            Err(e) => {
                last_err = e.to_string();
                log_warn("BROWSER", &format!(
                    "[workspace_browser_show] add_child ws={} attempt {}/{} FAILED: {}",
                    workspace_id, attempt, MAX_ATTEMPTS, last_err
                ));
                if attempt < MAX_ATTEMPTS {
                    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                }
            }
        }
    }
    let webview =
        created.ok_or_else(|| format!("add_child failed after {MAX_ATTEMPTS} attempts: {last_err}"))?;

    state
        .workspace_browsers
        .lock()
        .unwrap()
        .insert(workspace_id.clone(), webview);

    log_info("BROWSER", &format!(
        "[workspace_browser_show] spawned ws={} url={} rect=({:.0},{:.0},{:.0},{:.0})",
        workspace_id, url, x, y, w, h
    ));
    Ok(())
}

/// Hide the workspace's Browser Webview if one exists. No-op if not
/// (modal effect may broadcast hide for every workspace; spurious
/// hides for never-opened workspaces are silent).
#[tauri::command]
pub(crate) async fn workspace_browser_hide(
    state: State<'_, AppState>,
    workspace_id: String,
) -> Result<(), String> {
    let webview = state
        .workspace_browsers
        .lock()
        .unwrap()
        .get(&workspace_id)
        .cloned();
    if let Some(w) = webview {
        w.hide().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub(crate) async fn workspace_browser_navigate(
    state: State<'_, AppState>,
    workspace_id: String,
    url: String,
) -> Result<(), String> {
    let parsed: Url = url
        .parse()
        .map_err(|e| format!("invalid url {url:?}: {e}"))?;
    let webview = state
        .workspace_browsers
        .lock()
        .unwrap()
        .get(&workspace_id)
        .cloned()
        .ok_or_else(|| format!("no browser webview for workspace {workspace_id}"))?;
    // Phase 62.C (F.1): log the destination so a "browser isn't reaching
    // the service" report can be checked against the actual URL (should
    // be http://127.0.0.1:<port>, never localhost / an external IP).
    log_info("BROWSER", &format!(
        "[workspace_browser_navigate] ws={} url={}",
        workspace_id, url
    ));
    webview.navigate(parsed).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub(crate) async fn workspace_browser_eval(
    state: State<'_, AppState>,
    workspace_id: String,
    js: String,
) -> Result<(), String> {
    let webview = state
        .workspace_browsers
        .lock()
        .unwrap()
        .get(&workspace_id)
        .cloned()
        .ok_or_else(|| format!("no browser webview for workspace {workspace_id}"))?;
    webview.eval(js).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub(crate) async fn workspace_browser_close(
    state: State<'_, AppState>,
    workspace_id: String,
) -> Result<(), String> {
    let webview = state
        .workspace_browsers
        .lock()
        .unwrap()
        .remove(&workspace_id);
    if let Some(w) = webview {
        let _ = w.close();
        log_debug("BROWSER", &format!(
            "[workspace_browser_close] dropped webview ws={}",
            workspace_id
        ));
    }
    Ok(())
}

#[tauri::command]
pub(crate) async fn workspace_browser_resize(
    state: State<'_, AppState>,
    workspace_id: String,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
) -> Result<(), String> {
    let webview = state
        .workspace_browsers
        .lock()
        .unwrap()
        .get(&workspace_id)
        .cloned()
        .ok_or_else(|| format!("no browser webview for workspace {workspace_id}"))?;
    webview
        .set_position(LogicalPosition::new(x, y))
        .map_err(|e| format!("set_position: {e}"))?;
    webview
        .set_size(LogicalSize::new(w.max(1.0), h.max(1.0)))
        .map_err(|e| format!("set_size: {e}"))?;
    Ok(())
}

/// `workspace_delete` hooks here to wipe the per-workspace session
/// dir (the user explicitly deleted the workspace — they don't want
/// cookies surviving). Best-effort; errors are logged not raised.
pub(crate) fn cleanup_workspace_sessions(workspace_id: &str) {
    let Ok(base) = config_dir() else {
        return;
    };
    let dir = base
        .join("browser-sessions")
        .join(sanitize_for_path(workspace_id));
    if !dir.exists() {
        return;
    }
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => log_info("BROWSER", &format!(
            "[workspace_browser] cleaned sessions dir for ws={} at {}",
            workspace_id,
            dir.display()
        )),
        Err(e) => log_warn("BROWSER", &format!(
            "[workspace_browser] FAILED to clean sessions dir {}: {}",
            dir.display(),
            e
        )),
    }
}

#[cfg(test)]
mod ticket_bridge_tests {
    use super::*;

    /// The whole bridge rests on this: `ymux-ticket:<data>` must parse
    /// as an opaque URL that hands the payload back byte-for-byte. Base64
    /// is case-sensitive, so any host-style normalization would corrupt it.
    #[test]
    fn opaque_scheme_preserves_base64_case() {
        let payload = "SGVsbG8tV29ybGRfMTIz"; // mixed case + `-` + `_`
        let url: Url = format!("{TICKET_SCHEME}:{payload}")
            .parse()
            .expect("sentinel url parses");
        assert_eq!(url.scheme(), TICKET_SCHEME);
        assert!(url.cannot_be_a_base(), "must be opaque, not host-based");
        assert_eq!(url.path(), payload, "payload must survive verbatim");
    }

    /// Contrast, and the actual reason for the opaque form: with `//`
    /// the payload is parsed as the authority, so `path()` — what the
    /// handler reads — comes back EMPTY and the capture would be lost.
    /// (Case survives either way; this is a non-special scheme.)
    #[test]
    fn authority_form_would_strand_the_payload_in_the_host() {
        let url: Url = format!("{TICKET_SCHEME}://SGVsbG8=")
            .parse()
            .expect("parses");
        assert_eq!(url.host_str(), Some("SGVsbG8="));
        assert_eq!(url.path(), "", "handler reads path() — would get nothing");
    }

    /// base64 padding and the standard alphabet's `/` and `+` must also
    /// survive the opaque form, so the bridge is not silently dependent
    /// on the JS side stripping padding.
    #[test]
    fn opaque_form_survives_padding_and_standard_alphabet() {
        let url: Url = format!("{TICKET_SCHEME}:a/b+c=").parse().expect("parses");
        assert_eq!(url.path(), "a/b+c=");
    }

    #[test]
    fn decode_capture_round_trips_json() {
        // base64url of {"selector":"#a","xpath":"/html[1]"}
        // r##…##: the payload contains `"#`, which would close an r#"…"#.
        let json = r##"{"selector":"#a","xpath":"/html[1]"}"##;
        let b64 = base64_encode_for_test(json.as_bytes());
        let v = decode_capture(&b64).expect("decodes");
        assert_eq!(v["selector"], "#a");
        assert_eq!(v["xpath"], "/html[1]");
    }

    #[test]
    fn decode_capture_rejects_garbage() {
        assert!(decode_capture("!!!not-base64!!!").is_err());
        // valid base64, but not JSON
        assert!(decode_capture(&base64_encode_for_test(b"hello")).is_err());
    }

    /// Non-ticket URLs must be left completely alone. We can't build an
    /// AppHandle in a unit test, so assert the scheme check directly —
    /// it is the first line of `handle_ticket_navigation`.
    #[test]
    fn ordinary_urls_are_not_ours() {
        for raw in [
            "http://127.0.0.1:5173/",
            "https://example.com/a?b=c",
            "about:blank",
        ] {
            let url: Url = raw.parse().expect("parses");
            assert_ne!(url.scheme(), TICKET_SCHEME, "{raw} must pass through");
        }
    }

    fn base64_encode_for_test(bytes: &[u8]) -> String {
        const ALPH: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let b = [
                chunk[0],
                *chunk.get(1).unwrap_or(&0),
                *chunk.get(2).unwrap_or(&0),
            ];
            let v = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
            let n = chunk.len();
            out.push(ALPH[(v >> 18) as usize & 63] as char);
            out.push(ALPH[(v >> 12) as usize & 63] as char);
            if n > 1 {
                out.push(ALPH[(v >> 6) as usize & 63] as char);
            }
            if n > 2 {
                out.push(ALPH[v as usize & 63] as char);
            }
        }
        out
    }
}

