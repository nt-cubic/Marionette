//! Expressions that drive the element picker (`js/picker.js`, global
//! `__marionettePicker`) inside the isolated world. The picker is the
//! browser toolbar's annotate mode: the user points at a page element or
//! boxes a region, the page reports the pick over the `__marionettePick`
//! binding, and the frontend collects notes before handing the whole thing
//! to the composer.
//!
//! Same serialization rules as `agent.rs` / `probe.rs`: everything goes
//! through `serde_json`, never string concatenation — `mode` is a UI string
//! today, but the day anything here comes from the page it must already be
//! safe.

use serde_json::Value;

/// The picker's global; see js/picker.js.
pub const PICKER_GLOBAL: &str = "__marionettePicker";

/// The picker's own binding — separate from the vendored helper's
/// `__codegSend` (js/VENDOR.md); helper.js is upstream's and stays theirs.
pub const PICK_BINDING: &str = "__marionettePick";

const PICKER_JS: &str = include_str!("js/picker.js");

/// `typeof __marionettePicker === 'undefined' ? install-and-ask : ask` —
/// the probe.rs shape. The install lives in the same evaluation as the call:
/// two would straddle a gap in which the page can navigate, and the picker
/// would start on a document that is already gone.
fn call(expression: &str) -> String {
    format!(
        "typeof globalThis.{PICKER_GLOBAL} === 'undefined' ? \
         (function(){{\n{PICKER_JS}\n;return {expression};}})() : {expression}"
    )
}

fn stringify(expression: &str) -> String {
    format!("JSON.stringify({expression})")
}

/// Start picking (or switch mode while running). `mode` is `"element"` or
/// `"region"`; anything else lands on `"element"` inside the script.
pub fn start_call(mode: &str) -> String {
    let mode = serde_json::to_string(mode).unwrap_or_else(|_| "\"element\"".to_string());
    stringify(&call(&format!("globalThis.{PICKER_GLOBAL}.start({mode})")))
}

/// Stop picking. The overlay comes down and page events flow again; picked
/// elements stay retained for `resolve_refs_call` until the document goes.
pub fn stop_call() -> String {
    stringify(&call(&format!("globalThis.{PICKER_GLOBAL}.stop()")))
}

/// Stop and forget every retained element. Used after a batch is sent,
/// cleared, or abandoned so a fresh annotation starts from zero.
pub fn clear_call() -> String {
    stringify(&call(&format!("globalThis.{PICKER_GLOBAL}.clear()")))
}

/// Take the overlay off-screen for the duration of a capture — the highlight
/// must not photobomb the screenshot the user is about to attach.
pub fn capture_hide_call() -> String {
    stringify(&call(&format!("globalThis.{PICKER_GLOBAL}.captureHide()")))
}

/// Pair of `capture_hide_call`. Restore is best-effort and never fatal.
pub fn capture_restore_call() -> String {
    stringify(&call(&format!(
        "globalThis.{PICKER_GLOBAL}.captureRestore()"
    )))
}

/// Map every retained pick to the aria ref it carries, as of the snapshot
/// generation the caller just took. Answers `{pickId: {ref, role, name}|null}`.
pub fn resolve_refs_call(generation: &str) -> String {
    let generation = serde_json::to_string(generation).unwrap_or_else(|_| "\"\"".to_string());
    stringify(&call(&format!(
        "globalThis.{PICKER_GLOBAL}.resolveRefs({generation})"
    )))
}

/// The generation string the world answers with is four dot-separated
/// segments (`GENERATION.snapshotSeq.hostGen.navEpoch`); `elementForRef`
/// refuses anything else. Nothing here builds one — the caller passes the
/// `generation` field of a snapshot answer verbatim — but the round trip is
/// worth pinning: a hand-rolled `1.2` would silently fail every validation.
pub fn generation_from_snapshot(snapshot: &Value) -> Option<String> {
    let generation = snapshot["generation"].as_str()?;
    if generation.split('.').count() == 4 && !generation.is_empty() {
        Some(generation.to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_call_installs_on_demand_and_stringifies() {
        for expression in [
            start_call("element"),
            stop_call(),
            clear_call(),
            capture_hide_call(),
            capture_restore_call(),
            resolve_refs_call("3.7.1.2"),
        ] {
            assert!(expression.starts_with("JSON.stringify("), "{expression}");
            assert!(
                expression.contains("typeof globalThis.__marionettePicker === 'undefined'"),
                "{expression}"
            );
            // The install path must ask the same question the warm path does.
            assert_eq!(
                expression.matches("__marionettePicker.").count(),
                2,
                "{expression}"
            );
        }
    }

    #[test]
    fn start_carries_the_mode_as_json() {
        assert!(
            start_call("region").contains("__marionettePicker.start(\"region\")"),
            "{}",
            start_call("region")
        );
        // A quote in the mode string cannot escape into the expression.
        let sneaky = start_call("element\"); globalThis.evil();//");
        assert!(sneaky.contains("__marionettePicker.start(\"element\\\"); globalThis.evil();//\")"));
    }

    #[test]
    fn resolve_refs_quotes_the_generation_verbatim() {
        let expression = resolve_refs_call("12.3.4.5");
        assert!(
            expression.contains("__marionettePicker.resolveRefs(\"12.3.4.5\")"),
            "{expression}"
        );
    }

    #[test]
    fn the_picker_script_is_embedded() {
        // A blank include_str would compile; this keeps the wiring honest.
        assert!(PICKER_JS.contains("__marionettePicker"));
        assert!(PICKER_JS.contains("__marionettePick"));
        assert!(PICKER_JS.contains("attachShadow"));
    }

    #[test]
    fn generation_comes_from_the_snapshot_answer() {
        let snapshot = json!({ "generation": "GENERATION.7.3.2", "tree": "- button" });
        assert_eq!(
            generation_from_snapshot(&snapshot).as_deref(),
            Some("GENERATION.7.3.2")
        );
        // A host-side epoch (`gen.navEpoch`) is not a world generation —
        // elementForRef would refuse it and every ref would resolve null.
        assert_eq!(
            generation_from_snapshot(&json!({ "generation": "3.2" })),
            None
        );
        assert_eq!(generation_from_snapshot(&json!({})), None);
    }
}
