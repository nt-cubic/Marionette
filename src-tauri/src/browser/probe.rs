//! Cheap side questions about a live page: did anything change, has it
//! settled, write one control. Each is a single `Runtime.evaluate` in the
//! agent world and answers in a few hundred bytes — no tree walk, nothing
//! retained.
//!
//! These exist because a full aria snapshot is the expensive way to ask a
//! small question: an agent that just clicked a button wants "did the page
//! move" for the price of a counter read, not a fresh 40 KB tree. The idea
//! (a DOM mutation counter instead of a tree; writing a control by node
//! reference instead of synthesizing keystrokes) is taken from Cloudflare's
//! kitesurf CDP extensions `Kitesurf.getPageState` / `Kitesurf.setControlValue`;
//! the implementation is ours and rides in an isolated world.
//!
//! The script itself is `js/probe.js`. Like `agent.rs`, everything goes
//! through `serde_json` — never string concatenation — because one of these
//! values is a ref and another is user-authored text.

use serde_json::Value;

/// The probe's global; see js/probe.js.
pub const PROBE_GLOBAL: &str = "__marionetteProbe";

const PROBE_JS: &str = include_str!("js/probe.js");

/// `typeof __marionetteProbe === 'undefined' ? install-and-ask : ask`.
///
/// The install lives in the same evaluation as the call: two would straddle a
/// gap in which the page can navigate, and the probe would answer about a
/// document that is already gone.
fn call(call: &str) -> String {
    format!(
        "typeof globalThis.{PROBE_GLOBAL} === 'undefined' ? \
         (function(){{\n{PROBE_JS}\n;return {call};}})() : {call}"
    )
}

fn stringify(expression: &str) -> String {
    format!("JSON.stringify({expression})")
}

/// Page identity and the DOM mutation counter.
pub fn state_call() -> String {
    stringify(&call(&format!("globalThis.{PROBE_GLOBAL}.state()")))
}

/// Whether the document looks settled: complete, no pending images, fonts not
/// loading. Deliberately claims less than it can know — stylesheets are not
/// observable, so this never reports "settled" on their account.
pub fn ready_call() -> String {
    stringify(&call(&format!("globalThis.{PROBE_GLOBAL}.ready()")))
}

/// Write one control by ref. `value` is a JSON value: a string for a text
/// field, a boolean for a checkbox, a string or array of strings for a select.
pub fn set_value_call(generation: &str, target: &str, value: &Value) -> String {
    let generation = serde_json::to_string(generation).unwrap_or_else(|_| "\"\"".to_string());
    let target = serde_json::to_string(target).unwrap_or_else(|_| "null".to_string());
    let value = serde_json::to_string(value).unwrap_or_else(|_| "null".to_string());
    stringify(&call(&format!(
        "globalThis.{PROBE_GLOBAL}.setValue({generation}, {target}, {value})"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_call_installs_on_demand_and_stringifies() {
        for expression in [state_call(), ready_call()] {
            assert!(expression.starts_with("JSON.stringify("), "{expression}");
            assert!(
                expression.contains("typeof globalThis.__marionetteProbe === 'undefined'"),
                "{expression}"
            );
            // The install path must ask the same question the warm path does.
            assert_eq!(
                expression.matches("__marionetteProbe.state()").count()
                    + expression.matches("__marionetteProbe.ready()").count(),
                2,
                "{expression}"
            );
        }
    }

    #[test]
    fn set_value_carries_the_ref_and_value_as_json() {
        let expression = set_value_call("3.1", "e12", &json!("hello \"world\""));
        assert!(expression.contains("setValue(\"3.1\", \"e12\", \"hello \\\"world\\\"\")"), "{expression}");
        // A boolean and an array travel as their JSON shapes, not stringified
        // text — the probe compares them against `checked` and option values.
        assert!(set_value_call("1.0", "e3", &json!(true)).contains("setValue(\"1.0\", \"e3\", true)"));
        assert!(
            set_value_call("1.0", "e4", &json!(["a", "b"])).contains("setValue(\"1.0\", \"e4\", [\"a\",\"b\"])")
        );
    }

    #[test]
    fn the_probe_script_is_embedded() {
        // A blank include_str would compile; this keeps the wiring honest.
        assert!(PROBE_JS.contains("__marionetteProbe"));
        assert!(PROBE_JS.contains("MutationObserver"));
    }
}
