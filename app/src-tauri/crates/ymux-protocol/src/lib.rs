//! Phase 67.A.1 — ymux mobile/web wire protocol.
//!
//! Shared types between the ymux **server** (`ymux serve`, runs on the
//! user's local desktop machine) and future **clients** (the Android app
//! first, a web client later). Pure data types — no IO, no tokio — so any
//! consumer links it cheaply, mirroring `ymux-types` / `ymux-policy`.
//!
//! Transport (decided with Yossi, 67.A.1):
//!   - WebSocket. Control messages = JSON **text** frames ([`ServerMsg`] /
//!     [`ClientMsg`]). High-rate PTY bytes = **binary** frames (see
//!     [`frame`]) to avoid base64/JSON overhead on the hot path.
//!   - Default port **7878**.
//!   - Each in-flight hook approval carries a `req_id` so it can be
//!     resolved from either the desktop or the mobile (whoever answers
//!     first); the other side gets a [`ServerMsg::HookResolved`].
//!
//! Versioning: [`PROTOCOL_VERSION`] is exchanged in the [`Hello`]
//! handshake; a major mismatch is rejected with [`HelloResult::Denied`]
//! so an old mobile build fails cleanly against a newer server.

use serde::{Deserialize, Serialize};

/// Bumped on any breaking change to the message shapes below.
pub const PROTOCOL_VERSION: u16 = 1;

/// Default TCP port for `ymux serve` (overridable in settings).
pub const DEFAULT_PORT: u16 = 7878;

// ts-rs derive is opt-in (the `ts` feature). These aliases keep the
// per-type attributes terse.
macro_rules! wire_type {
    ($item:item) => {
        #[cfg_attr(feature = "ts", derive(ts_rs::TS))]
        #[cfg_attr(feature = "ts", ts(export, export_to = "../../../../src/bindings/"))]
        $item
    };
}

// ─── Pairing (QR) ────────────────────────────────────────────────────

wire_type! {
    /// Encoded into the pairing QR the desktop shows. The mobile scans it,
    /// connects to `host:port`, and proves `token` in the [`Hello`]. The
    /// token is single-use and expires (~5 min) until the first successful
    /// pairing; `fingerprint` lets the client pin the server's TLS cert.
    #[derive(Clone, Debug, Serialize, Deserialize)]
    pub struct PairingPayload {
        pub host: String,
        pub port: u16,
        pub token: String,
        /// Server TLS cert fingerprint (hex sha256), or empty for plain ws.
        #[serde(default)]
        pub fingerprint: String,
        /// Protocol version the desktop speaks (sanity pre-check before connect).
        pub proto: u16,
    }
}

// ─── Handshake ───────────────────────────────────────────────────────

wire_type! {
    /// First message the client sends after the socket opens.
    #[derive(Clone, Debug, Serialize, Deserialize)]
    pub struct Hello {
        pub token: String,
        /// User-friendly device label, e.g. "Yossi's Pixel".
        pub device_name: String,
        pub client_proto: u16,
        pub client_version: String,
    }
}

wire_type! {
    #[derive(Clone, Debug, Serialize, Deserialize)]
    #[serde(tag = "result", rename_all = "snake_case")]
    pub enum HelloResult {
        Ok {
            server_proto: u16,
            server_version: String,
        },
        Denied {
            reason: String,
        },
    }
}

// ─── Hook approvals ──────────────────────────────────────────────────

wire_type! {
    /// A client's verdict on a pending [`ServerMsg::HookRequest`].
    #[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum HookDecision {
        Allow,
        Deny,
    }
}

// ─── Server → Client ─────────────────────────────────────────────────

wire_type! {
    /// JSON (text-frame) messages from server to client. PTY output is NOT
    /// here — it travels as a binary frame (see [`frame`]).
    // No `Debug`: ServerMsg::PaneList embeds ymux_types::LayoutNode,
    // which doesn't derive Debug.
    #[derive(Clone, Serialize, Deserialize)]
    #[serde(tag = "t", rename_all = "snake_case")]
    pub enum ServerMsg {
        /// Handshake reply.
        Hello(HelloResult),
        /// Full workspace list with live status.
        WorkspaceList {
            workspaces: Vec<WorkspaceSummary>,
        },
        /// The pane tree for one workspace (reuses the canonical layout type).
        PaneList {
            workspace_id: String,
            layout: Option<ymux_types::LayoutNode>,
        },
        /// A pane's terminal dimensions changed (so the client can reflow).
        PaneSize {
            pane_id: String,
            cols: u16,
            rows: u16,
        },
        /// A PreToolUse hook is waiting for approval.
        HookRequest {
            req_id: String,
            pane_id: String,
            tool_name: String,
            tool_input: serde_json::Value,
            timeout_secs: u32,
        },
        /// A previously-sent [`ServerMsg::HookRequest`] was answered (or
        /// timed out) elsewhere — the client should dismiss its card.
        HookResolved {
            req_id: String,
            decision: HookDecision,
        },
        /// Generic notification (workspace event, update, etc.).
        Notification {
            kind: String,
            title: String,
            body: String,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            pane_id: Option<String>,
        },
        /// Recoverable server-side error tied to a prior client action.
        Error {
            message: String,
        },
    }
}

wire_type! {
    /// Lightweight workspace row for the mobile list (not the full
    /// `ymux_types::Workspace` — the client only needs these to render
    /// the list; it fetches the layout via [`ServerMsg::PaneList`]).
    #[derive(Clone, Debug, Serialize, Deserialize)]
    pub struct WorkspaceSummary {
        pub id: String,
        pub name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub emoji: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub color: Option<String>,
        /// True if the workspace currently has a live SSH session.
        pub connected: bool,
        pub pane_count: u32,
    }
}

// ─── Client → Server ─────────────────────────────────────────────────

wire_type! {
    /// JSON (text-frame) messages from client to server. PTY input is NOT
    /// here — it travels as a binary frame (see [`frame`]).
    #[derive(Clone, Debug, Serialize, Deserialize)]
    #[serde(tag = "t", rename_all = "snake_case")]
    pub enum ClientMsg {
        Hello(Hello),
        /// Start receiving output for a pane (server replies with a binary
        /// stream tagged by the pane's short index — see [`frame`]).
        PaneSubscribe {
            pane_id: String,
        },
        PaneUnsubscribe {
            pane_id: String,
        },
        PaneResize {
            pane_id: String,
            cols: u16,
            rows: u16,
        },
        HookDecision {
            req_id: String,
            decision: HookDecision,
        },
        PaneCreate {
            workspace_id: String,
        },
        PaneClose {
            pane_id: String,
        },
        /// Keepalive.
        Ping,
    }
}

// ─── Binary PTY frames ───────────────────────────────────────────────

/// Compact binary framing for the high-rate PTY path. Layout:
///
/// ```text
/// byte 0      : opcode  (OPCODE_OUTPUT | OPCODE_INPUT)
/// bytes 1..3  : pane index, u16 big-endian (mapped at subscribe time, so
///               we don't ship a UUID on every chunk)
/// bytes 3..   : raw PTY bytes
/// ```
///
/// Sent as WebSocket *binary* frames; control messages stay JSON text.
pub mod frame {
    /// Server → client: a chunk of a pane's PTY output.
    pub const OPCODE_OUTPUT: u8 = 0x01;
    /// Client → server: typed keystrokes for the focused pane.
    pub const OPCODE_INPUT: u8 = 0x02;

    /// The fixed header length (opcode + pane index).
    pub const HEADER_LEN: usize = 3;

    /// Encode a PTY frame: `[opcode][pane_idx BE][payload]`.
    pub fn encode(opcode: u8, pane_idx: u16, payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
        out.push(opcode);
        out.extend_from_slice(&pane_idx.to_be_bytes());
        out.extend_from_slice(payload);
        out
    }

    /// Decode a PTY frame into `(opcode, pane_idx, payload)`. Returns
    /// `None` if the buffer is shorter than the header.
    pub fn decode(buf: &[u8]) -> Option<(u8, u16, &[u8])> {
        if buf.len() < HEADER_LEN {
            return None;
        }
        let opcode = buf[0];
        let pane_idx = u16::from_be_bytes([buf[1], buf[2]]);
        Some((opcode, pane_idx, &buf[HEADER_LEN..]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_msg_tagged_round_trip() {
        let m = ServerMsg::PaneSize {
            pane_id: "p1".into(),
            cols: 80,
            rows: 24,
        };
        let v = serde_json::to_value(&m).unwrap();
        assert_eq!(v["t"], "pane_size");
        assert_eq!(v["cols"], 80);
        let back: ServerMsg = serde_json::from_value(v).unwrap();
        assert!(matches!(back, ServerMsg::PaneSize { cols: 80, .. }));
    }

    #[test]
    fn client_hello_round_trip() {
        let m = ClientMsg::Hello(Hello {
            token: "t".into(),
            device_name: "Pixel".into(),
            client_proto: PROTOCOL_VERSION,
            client_version: "0.1.0".into(),
        });
        let s = serde_json::to_string(&m).unwrap();
        let back: ClientMsg = serde_json::from_str(&s).unwrap();
        match back {
            ClientMsg::Hello(h) => assert_eq!(h.device_name, "Pixel"),
            _ => panic!("expected Hello"),
        }
    }

    #[test]
    fn hello_result_variants() {
        let ok = serde_json::to_value(HelloResult::Ok {
            server_proto: 1,
            server_version: "x".into(),
        })
        .unwrap();
        assert_eq!(ok["result"], "ok");
        let denied = serde_json::to_value(HelloResult::Denied {
            reason: "bad token".into(),
        })
        .unwrap();
        assert_eq!(denied["result"], "denied");
    }

    #[test]
    fn hook_decision_lowercase() {
        assert_eq!(
            serde_json::to_value(HookDecision::Allow).unwrap(),
            serde_json::json!("allow")
        );
        assert_eq!(
            serde_json::to_value(HookDecision::Deny).unwrap(),
            serde_json::json!("deny")
        );
    }

    #[test]
    fn binary_frame_round_trip() {
        let payload = b"hello\x1b[0m";
        let enc = frame::encode(frame::OPCODE_OUTPUT, 42, payload);
        let (op, idx, body) = frame::decode(&enc).unwrap();
        assert_eq!(op, frame::OPCODE_OUTPUT);
        assert_eq!(idx, 42);
        assert_eq!(body, payload);
    }

    #[test]
    fn decode_rejects_short_buffer() {
        assert!(frame::decode(&[0x01, 0x00]).is_none());
    }
}
