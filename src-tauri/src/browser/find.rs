//! In-page find (Ctrl F on the browser surface): count matches, step between
//! them, clear the selection. The script is `js/find.js`, embedded by the same
//! install-then-call evaluation shape as `probe.rs`: one evaluation installs
//! the helper and asks it in the same turn, so a navigation can never leave a
//! stale range table answering.
//!
//! Deliberately separate from the agent world's aria engine: this does not
//! touch refs, the ref lease, or any agent tool. The page is not reloaded and
//! nothing here reaches the conversation.

/// The finder's global; see js/find.js.
pub const FIND_GLOBAL: &str = "__marionetteFind";

const FIND_JS: &str = include_str!("js/find.js");

fn call(call: &str) -> String {
    format!(
        "typeof globalThis.{FIND_GLOBAL} === 'undefined' ? \
         (function(){{\n{FIND_JS}\n;return {call};}})() : {call}"
    )
}

fn stringify(expression: &str) -> String {
    format!("JSON.stringify({expression})")
}

/// Start a search. The query is passed as a JSON string, never spliced into
/// the expression, so quotes and backslashes cannot escape.
pub fn search_call(query: &str, case_sensitive: bool) -> String {
    let query = serde_json::to_string(query).unwrap_or_else(|_| "\"\"".to_string());
    stringify(&call(&format!(
        "globalThis.{FIND_GLOBAL}.search({query}, {case_sensitive})"
    )))
}

/// Move to the next (`false`) or previous (`true`) match.
pub fn step_call(backwards: bool) -> String {
    stringify(&call(&format!(
        "globalThis.{FIND_GLOBAL}.step({backwards})"
    )))
}

/// Drop the selection and the highlight registry entries.
pub fn clear_call() -> String {
    stringify(&call(&format!("globalThis.{FIND_GLOBAL}.clear()")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_call_installs_on_demand_and_stringifies() {
        for expression in [search_call("needle", true), step_call(false), clear_call()] {
            assert!(expression.starts_with("JSON.stringify("), "{expression}");
            assert!(
                expression.contains("typeof globalThis.__marionetteFind === 'undefined'"),
                "{expression}"
            );
            // The install path asks the same question the warm path does.
            assert_eq!(
                expression.matches("__marionetteFind.").count(),
                2,
                "{expression}"
            );
        }
    }

    #[test]
    fn search_quotes_the_query_as_json() {
        let expression = search_call("a \"quote\" \\ and $HOME", false);
        assert!(
            expression.contains("search(\"a \\\"quote\\\" \\\\ and $HOME\", false)"),
            "{expression}"
        );
        // A parenthesis in the query stays inside the JSON string.
        let sneaky = search_call("x); globalThis.evil();//", false);
        assert!(
            sneaky.contains("search(\"x); globalThis.evil();//\", false)"),
            "{sneaky}"
        );
    }

    #[test]
    fn step_carries_a_boolean_not_a_string() {
        assert!(step_call(true).contains("step(true)"));
        assert!(step_call(false).contains("step(false)"));
    }

    #[test]
    fn the_find_script_is_embedded() {
        assert!(FIND_JS.contains("__marionetteFind"));
        assert!(FIND_JS.contains("function build"));
        assert!(FIND_JS.contains("function reveal"));
        assert!(FIND_JS.contains("MAX_MATCHES"));
    }
}
