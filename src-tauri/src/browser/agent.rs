//! Expressions that drive the vendored aria engine (`js/agent.bundle.js`,
//! global `__codegAgent`) inside the isolated world. Ported from codeg
//! `browser/agent.rs` — reduced to snapshot + act; the serialization rules
//! (everything through `serde_json`, never string concatenation) travel with
//! it: the day one of these fields comes from somewhere less trusted is not
//! the day to discover it was concatenation.

use serde::{Deserialize, Serialize};
use serde_json::json;

/// The world's global; see js/VENDOR.md for why the name stays.
pub const AGENT_GLOBAL: &str = "__codegAgent";

/// Bare-string sentinel where a snapshot would be an object: the engine is
/// not in this document yet.
pub const ENGINE_ABSENT: &str = "absent";

/// What a snapshot asks for. `max_chars` caps the rendered tree; the engine
/// answers `truncated: true` and invites a narrower follow-up.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotRequest {
    pub max_chars: u64,
}

/// The host's half of ref freshness: `generation.nav_epoch`. The world bumps
/// its half on mutations it can see; the host's half moves on navigations
/// only the host can see. A ref quoting an older epoch is refused.
pub fn epoch(generation: u64, nav_epoch: u64) -> String {
    format!("{generation}.{nav_epoch}")
}

/// `JSON.stringify(__codegAgent.snapshot({…}))`.
fn snapshot_call(request: &SnapshotRequest, epoch: &str) -> String {
    let options = json!({
        "maxChars": request.max_chars,
        "epoch": epoch,
    });
    format!("JSON.stringify(globalThis.{AGENT_GLOBAL}.snapshot({options}))")
}

/// Take a snapshot, or say the engine is not here yet. One round trip in the
/// steady state, one more on a cold document — a remembered "installed" flag
/// would have to be cleared from every path that can replace a document.
pub fn probe_and_snapshot(request: &SnapshotRequest, epoch: &str) -> String {
    format!(
        "typeof globalThis.{AGENT_GLOBAL} === 'undefined' ? {absent} : {call}",
        absent = serde_json::Value::from(ENGINE_ABSENT),
        call = snapshot_call(request, epoch),
    )
}

/// Put the engine in this document and take the snapshot, in ONE evaluation —
/// two would straddle a gap in which the page can navigate. Wrapped in a
/// function body because the shim evaluates an expression while the bundle is
/// a program; the bundle publishes itself through `globalThis`, so running it
/// inside a function installs it just the same and its top-level names stay
/// out of the world's globals.
pub fn install_and_snapshot(request: &SnapshotRequest, epoch: &str) -> String {
    format!(
        "(function(){{\n{AGENT_BUNDLE}\n;return {call};}})()",
        call = snapshot_call(request, epoch),
    )
}

const AGENT_BUNDLE: &str = include_str!("js/agent.bundle.js");

/// What an agent asks to do to an element. Serialized exactly as the world's
/// `ActionRequest` expects (`{"kind": "click", …}`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ActionKind {
    Click {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        count: Option<u8>,
    },
    Hover,
    /// Replace a field's value with `text`. `submit` presses Enter after.
    Type {
        text: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        submit: bool,
    },
    /// Press a key — `"Enter"`, `"a"`, `"Control+Shift+k"` — on the element,
    /// or on whatever has focus when the request names none.
    Press { key: String },
    /// Choose options of a `<select>`, by value or by label.
    Select { values: Vec<String> },
}

/// One action on one tab.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionRequest {
    /// The snapshot generation that named `target` — echoed back, checked by
    /// both the host and the world.
    pub generation: String,
    /// The element's ref. `None` only for `Press` without a target.
    #[serde(rename = "ref", default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    pub action: ActionKind,
}

/// `JSON.stringify(__codegAgent.act(generation, target, action))`.
pub fn act_call(request: &ActionRequest) -> String {
    format!(
        "typeof globalThis.{AGENT_GLOBAL} === 'undefined' ? {absent} : \
         JSON.stringify(globalThis.{AGENT_GLOBAL}.act({generation}, {target}, {action}))",
        absent = serde_json::Value::from(ENGINE_ABSENT),
        generation = serde_json::to_string(&request.generation).unwrap_or_else(|_| "\"\"".to_string()),
        target = match &request.target {
            Some(target) => serde_json::to_string(target).unwrap_or_else(|_| "null".to_string()),
            None => "null".to_string(),
        },
        action = serde_json::to_string(&request.action).unwrap_or_else(|_| "null".to_string()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_quotes_the_absent_sentinel() {
        let expr = probe_and_snapshot(&SnapshotRequest { max_chars: 100 }, "3.7");
        assert!(expr.starts_with("typeof globalThis.__codegAgent === 'undefined' ? \"absent\" :"));
        assert!(expr.contains("\"maxChars\":100"));
        assert!(expr.contains("\"epoch\":\"3.7\""));
    }

    #[test]
    fn install_wraps_the_bundle_in_a_function() {
        let expr = install_and_snapshot(&SnapshotRequest { max_chars: 50 }, "1.0");
        assert!(expr.starts_with("(function(){"));
        assert!(expr.contains("__codegAgent"));
        assert!(expr.trim_end_matches('\n').ends_with("})()"));
    }

    #[test]
    fn action_serializes_the_worlds_shape() {
        let request = ActionRequest {
            generation: "2.1".to_string(),
            target: Some("e12".to_string()),
            action: ActionKind::Click { count: Some(1) },
        };
        let expr = act_call(&request);
        // kebab-case tag + camelCase fields, exactly what the bundle parses.
        assert!(expr.contains("{\"kind\":\"click\",\"count\":1}"));
        assert!(expr.contains("__codegAgent.act(\"2.1\", \"e12\""));
    }

    #[test]
    fn press_without_target_serializes_null() {
        let request = ActionRequest {
            generation: "0.1".to_string(),
            target: None,
            action: ActionKind::Press { key: "Enter".to_string() },
        };
        let expr = act_call(&request);
        assert!(expr.contains("__codegAgent.act(\"0.1\", null, {\"kind\":\"press\",\"key\":\"Enter\"})"));
    }
}
