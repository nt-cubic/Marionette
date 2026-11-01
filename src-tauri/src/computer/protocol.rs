//! Computer Use: the main process ↔ helper protocol (codeg `protocol.rs`
//! shape, reduced to our op set).
//!
//! The helper is this same exe run with `--marionette-computer-helper`; the
//! channel is stdio with length-prefixed JSON frames (16 MiB cap). The op set
//! is CLOSED: every field of every op is re-assembled by the helper into a
//! fixed cua-driver call — no op carries a driver tool name, so the dozens of
//! driver tools an agent must never reach (kill processes, record input
//! replay…) are unreachable by construction, not by filtering.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Bump on any incompatible change; a stale helper from a half-update refuses
/// to talk (both sides check).
pub const PROTOCOL_VERSION: u32 = 1;

/// Frame cap: window trees and screenshots are the big payloads.
pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Envelopes
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Frame {
    /// Helper → main, once, before anything else (the main process built the
    /// pipes; until this arrives it knows only that the process is alive).
    Ready { protocol: u32 },
    /// Main → helper.
    Request {
        id: u64,
        op: String,
        params: Value,
    },
    /// Helper → main.
    Reply {
        id: u64,
        #[serde(default)]
        ok: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result: Option<Value>,
        /// HelperErrorCode-style slug + message; refusals ride here.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<Value>,
    },
}

pub const OP_CONFIGURE: &str = "configure";
pub const OP_LIST_WINDOWS: &str = "list-windows";
pub const OP_CAPTURE: &str = "capture";
pub const OP_SNAPSHOT: &str = "snapshot";
pub const OP_ACT: &str = "act";
pub const OP_CLIPBOARD_READ: &str = "clipboard-read";
pub const OP_CLIPBOARD_WRITE: &str = "clipboard-write";
pub const OP_HALT: &str = "halt";

/// Every op the main process may send. Dispatch in the helper matches these
/// constants and nothing else.
pub const OPS: &[&str] = &[
    OP_CONFIGURE,
    OP_LIST_WINDOWS,
    OP_CAPTURE,
    OP_SNAPSHOT,
    OP_ACT,
    OP_CLIPBOARD_READ,
    OP_CLIPBOARD_WRITE,
    OP_HALT,
];

// ---------------------------------------------------------------------------
// Helper → driver action set (the "WindowAction" half)
// ---------------------------------------------------------------------------

/// One action on one window. Serialized into the helper op params by the main
/// process; the helper turns it into the exact driver call.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum WindowAction {
    /// Click a point (window-relative px) or an element token
    /// (`snapshot_id:index`, from a snapshot the driver took).
    Click {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        x: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        y: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token: Option<String>,
        /// left (default) / right / middle
        #[serde(default, skip_serializing_if = "Option::is_none")]
        button: Option<String>,
        /// 1 = click (default), 2 = double
        #[serde(default, skip_serializing_if = "Option::is_none")]
        count: Option<u8>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        modifiers: Vec<String>,
    },
    Drag {
        from_x: f64,
        from_y: f64,
        to_x: f64,
        to_y: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u64>,
    },
    Scroll {
        direction: String,
        amount: u32,
        #[serde(default = "default_unit")]
        unit: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        x: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        y: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token: Option<String>,
    },
    /// Replace a field's value; `submit` presses Enter after.
    Type {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token: Option<String>,
        text: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        submit: bool,
    },
    /// Press a key — `"Enter"`, `"a"`, `"Control+Shift+k"` — on the element,
    /// or whatever has focus when no token is given. Key chords go through
    /// the whitelist (keys.rs) before they reach the driver.
    Press {
        key: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token: Option<String>,
    },
    SetValue {
        token: String,
        value: String,
    },
    /// Restore a minimized/hidden window without stealing the foreground.
    Restore,
}

fn default_unit() -> String {
    "line".to_string()
}

impl WindowAction {
    /// One line for the permission card the user reads.
    pub fn summary(&self) -> String {
        match self {
            WindowAction::Click { .. } => "click".to_string(),
            WindowAction::Drag { .. } => "drag".to_string(),
            WindowAction::Scroll { direction, amount, .. } => format!("scroll {direction} ×{amount}"),
            WindowAction::Type { text, .. } => format!("type \"{}\"", truncate(text, 30)),
            WindowAction::Press { key, .. } => format!("press {key}"),
            WindowAction::SetValue { .. } => "set field value".to_string(),
            WindowAction::Restore => "restore window".to_string(),
        }
    }
}

fn truncate(text: &str, max: usize) -> String {
    let mut out: String = text.chars().take(max).collect();
    if out.len() < text.len() {
        out.push('…');
    }
    out
}

// ---------------------------------------------------------------------------
// Frame codec (sync, std io)
// ---------------------------------------------------------------------------

pub fn write_frame<W: std::io::Write>(out: &mut W, frame: &Frame) -> Result<(), String> {
    let payload =
        serde_json::to_vec(frame).map_err(|e| format!("frame encode: {e}"))?;
    if payload.len() > MAX_FRAME_BYTES {
        return Err("frame too large".to_string());
    }
    let header = (payload.len() as u32).to_le_bytes();
    out.write_all(&header)
        .and_then(|_| out.write_all(&payload))
        .and_then(|_| out.flush())
        .map_err(|e| format!("frame write: {e}"))
}

pub fn read_frame<R: std::io::Read>(input: &mut R) -> Result<Frame, String> {
    let mut header = [0u8; 4];
    input
        .read_exact(&mut header)
        .map_err(|e| format!("frame read: {e}"))?;
    let len = u32::from_le_bytes(header) as usize;
    if len == 0 || len > MAX_FRAME_BYTES {
        return Err(format!("frame length out of range: {len}"));
    }
    let mut payload = vec![0u8; len];
    input
        .read_exact(&mut payload)
        .map_err(|e| format!("frame read: {e}"))?;
    serde_json::from_slice(&payload).map_err(|e| format!("frame decode: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip() {
        let frames = vec![
            Frame::Ready { protocol: PROTOCOL_VERSION },
            Frame::Request {
                id: 7,
                op: OP_ACT.to_string(),
                params: serde_json::json!({
                    "pid": 12, "windowId": 34,
                    "action": {"kind": "click", "x": 10.5, "y": 20.0, "count": 2}
                }),
            },
            Frame::Reply { id: 7, ok: true, result: Some(serde_json::json!({"done": true})), error: None },
            Frame::Reply {
                id: 8,
                ok: false,
                result: None,
                error: Some(serde_json::json!({"code": "occluded", "message": "covered"})),
            },
        ];
        for frame in frames {
            let mut bytes = Vec::new();
            write_frame(&mut bytes, &frame).unwrap();
            // The codec must not be able to hand back something else.
            let back: Frame = read_frame(&mut &bytes[..]).unwrap();
            let re = serde_json::to_vec(&back).unwrap();
            let orig = serde_json::to_vec(&frame).unwrap();
            assert_eq!(re, orig);
        }
    }

    #[test]
    fn oversized_frames_are_refused_not_read() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&((MAX_FRAME_BYTES as u32) + 1).to_le_bytes());
        assert!(read_frame(&mut &bytes[..]).is_err());
    }

    #[test]
    fn action_summaries_are_short() {
        let action = WindowAction::Type {
            token: None,
            text: "x".repeat(300),
            submit: false,
        };
        assert!(action.summary().chars().count() <= 42);
    }

    #[test]
    fn the_op_set_is_closed_and_spelled() {
        assert_eq!(OPS.len(), 8);
        for op in OPS {
            assert!(!op.contains(' '), "op names are kebab: {op}");
        }
    }
}
