//! Typed `browser_batch` — the restricted multi-step op runner (ego-lite M2).
//!
//! One MCP call may chain several operations against one snapshot's refs.
//! This is NOT arbitrary scripting: the op set is closed, every field is
//! whitelisted, control steps still walk the same permission gate one by
//! one, and a snapshot step can only be terminal (it replaces the world's
//! ref table, so any later ref-based step would be doomed). All policy
//! questions live here as pure functions; the executor in `tools_mcp.rs`
//! only drives the loop.

use serde_json::{json, Value};

use super::agent::{ActionKind, ActionRequest};

/// Hard cap on chained steps — the schema rejects more before anything runs.
pub const MAX_STEPS: usize = 32;
/// Per-wait hard deadline ceiling (waitForSelector).
pub const DEFAULT_WAIT_MS: u64 = 3_000;
pub const MAX_WAIT_MS: u64 = 15_000;
/// Load-state waits get a wider ceiling: first paint of a dev server is
/// slower than an element flip, and the roadmap scopes networkidle out.
pub const DEFAULT_LOAD_WAIT_MS: u64 = 5_000;
pub const MAX_LOAD_WAIT_MS: u64 = 30_000;
/// Whole-batch wall-clock budget. Permission-card waits pause this budget
/// (the executor resets the deadline after an approval) so a slow approval
/// cannot burn execution time.
pub const BATCH_DEADLINE_MS: u64 = 120_000;
/// The terminal snapshot's tree cap.
pub const SNAPSHOT_DEFAULT_MAX_CHARS: u64 = 20_000;
pub const SNAPSHOT_MAX_MAX_CHARS: u64 = 100_000;

pub const VALID_OPS: &[&str] = &[
    "click",
    "hover",
    "type",
    "setValue",
    "pressKey",
    "selectOption",
    "waitForSelector",
    "waitForLoadState",
    "pageState",
    "pageReady",
    "snapshot",
];

pub const WAIT_STATES: &[&str] = &["attached", "visible", "hidden", "detached"];
pub const LOAD_STATES: &[&str] = &["domcontentloaded", "load"];

/// One parsed, validated step. Field names mirror the wire schema; anything
/// the schema did not whitelist never reaches this enum.
#[derive(Debug, PartialEq)]
pub enum BatchStep {
    Click { target: String, count: Option<u8> },
    Hover { target: String },
    Type { target: String, text: String, submit: bool },
    SetValue { target: String, value: Value },
    PressKey { key: String, target: Option<String> },
    SelectOption { target: String, values: Vec<String> },
    WaitForSelector { target: String, state: &'static str, timeout_ms: u64 },
    WaitForLoadState { state: &'static str, timeout_ms: u64 },
    PageState,
    PageReady,
    Snapshot { max_chars: u64 },
}

impl BatchStep {
    pub fn op(&self) -> &'static str {
        match self {
            BatchStep::Click { .. } => "click",
            BatchStep::Hover { .. } => "hover",
            BatchStep::Type { .. } => "type",
            BatchStep::SetValue { .. } => "setValue",
            BatchStep::PressKey { .. } => "pressKey",
            BatchStep::SelectOption { .. } => "selectOption",
            BatchStep::WaitForSelector { .. } => "waitForSelector",
            BatchStep::WaitForLoadState { .. } => "waitForLoadState",
            BatchStep::PageState => "pageState",
            BatchStep::PageReady => "pageReady",
            BatchStep::Snapshot { .. } => "snapshot",
        }
    }
}

/// Control-level steps walk `control_gate` exactly like their single-tool
/// twins; everything else is a read.
pub fn is_control(step: &BatchStep) -> bool {
    matches!(
        step,
        BatchStep::Click { .. }
            | BatchStep::Hover { .. }
            | BatchStep::Type { .. }
            | BatchStep::SetValue { .. }
            | BatchStep::PressKey { .. }
            | BatchStep::SelectOption { .. }
    )
}

/// The permission-card summary — same wording family as the single tools so
/// the user sees one consistent vocabulary.
pub fn step_summary(step: &BatchStep) -> String {
    match step {
        BatchStep::Click { count, .. } => match count {
            Some(2) => "double click".to_string(),
            _ => "click".to_string(),
        },
        BatchStep::Hover { .. } => "hover".to_string(),
        BatchStep::Type { text, .. } => format!("type \"{}\"", truncate_card(text)),
        BatchStep::SetValue { .. } => "set a control's value".to_string(),
        BatchStep::PressKey { key, .. } => format!("press {key}"),
        BatchStep::SelectOption { values, .. } => {
            format!("select {}", values.join(", "))
        }
        _ => "read the page".to_string(),
    }
}

fn truncate_card(text: &str) -> String {
    if text.chars().count() <= 60 {
        return text.to_string();
    }
    let cut: String = text.chars().take(57).collect();
    format!("{cut}...")
}

fn as_str_field(step: &Value, key: &str, op: &str) -> Result<String, String> {
    step[key]
        .as_str()
        .filter(|v| !v.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("batch step {op} needs a non-empty string `{key}`"))
}

fn wait_ms_field(step: &Value, key: &str, default: u64, max: u64) -> u64 {
    step[key]
        .as_u64()
        .unwrap_or(default)
        .clamp(100, max)
}

fn wait_state_field(
    step: &Value,
    key: &str,
    allowed: &'static [&'static str],
    default: &str,
) -> Result<&'static str, String> {
    let raw = step[key].as_str().unwrap_or(default);
    let hit = allowed.iter().find(|s| **s == raw);
    hit.copied()
        .ok_or_else(|| format!("batch step wait state must be one of {}", allowed.join("|")))
}

fn reject_unknown_fields(step: &Value, op: &str, allowed: &[&str]) -> Result<(), String> {
    let Some(object) = step.as_object() else {
        return Err(format!("batch step {op} must be an object"));
    };
    for key in object.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(format!(
                "batch step {op} has unknown field `{key}` — allowed: {}",
                allowed.join(", ")
            ));
        }
    }
    Ok(())
}

/// Parse and validate the `steps` array. Strict: unknown ops, unknown
/// fields, missing refs, oversized chains and non-terminal snapshots are
/// all schema errors before anything executes.
pub fn validate(steps: &Value) -> Result<Vec<BatchStep>, String> {
    let Some(items) = steps.as_array() else {
        return Err("missing argument: steps (must be an array)".to_string());
    };
    if items.is_empty() {
        return Err("batch needs at least one step".to_string());
    }
    if items.len() > MAX_STEPS {
        return Err(format!("batch accepts at most {MAX_STEPS} steps, got {}", items.len()));
    }
    let last = items.len() - 1;
    let mut out = Vec::with_capacity(items.len());
    for (index, step) in items.iter().enumerate() {
        let op = step.get("op").and_then(Value::as_str).unwrap_or("");
        let parsed = match op {
            "click" => {
                reject_unknown_fields(step, op, &["op", "ref", "count"])?;
                let count = step.get("count").and_then(Value::as_u64).map(|c| c as u8);
                BatchStep::Click {
                    target: as_str_field(step, "ref", op)?,
                    count,
                }
            }
            "hover" => {
                reject_unknown_fields(step, op, &["op", "ref"])?;
                BatchStep::Hover { target: as_str_field(step, "ref", op)? }
            }
            "type" => {
                reject_unknown_fields(step, op, &["op", "ref", "text", "submit"])?;
                BatchStep::Type {
                    target: as_str_field(step, "ref", op)?,
                    text: as_str_field(step, "text", op)?,
                    submit: step.get("submit").and_then(Value::as_bool).unwrap_or(false),
                }
            }
            "setValue" => {
                reject_unknown_fields(step, op, &["op", "ref", "value"])?;
                if step.get("value").map(Value::is_null).unwrap_or(true) {
                    return Err("batch step setValue needs a `value`".to_string());
                }
                BatchStep::SetValue {
                    target: as_str_field(step, "ref", op)?,
                    value: step["value"].clone(),
                }
            }
            "pressKey" => {
                reject_unknown_fields(step, op, &["op", "key", "ref"])?;
                BatchStep::PressKey {
                    key: as_str_field(step, "key", op)?,
                    target: step.get("ref").and_then(Value::as_str).map(str::to_string),
                }
            }
            "selectOption" => {
                reject_unknown_fields(step, op, &["op", "ref", "values"])?;
                let values: Vec<String> = step["values"]
                    .as_array()
                    .map(|list| {
                        list.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
                if values.is_empty() {
                    return Err("batch step selectOption needs a non-empty `values` array".to_string());
                }
                BatchStep::SelectOption { target: as_str_field(step, "ref", op)?, values }
            }
            "waitForSelector" => {
                reject_unknown_fields(step, op, &["op", "ref", "state", "timeoutMs"])?;
                BatchStep::WaitForSelector {
                    target: as_str_field(step, "ref", op)?,
                    state: wait_state_field(step, "state", WAIT_STATES, "visible")?,
                    timeout_ms: wait_ms_field(step, "timeoutMs", DEFAULT_WAIT_MS, MAX_WAIT_MS),
                }
            }
            "waitForLoadState" => {
                reject_unknown_fields(step, op, &["op", "state", "timeoutMs"])?;
                BatchStep::WaitForLoadState {
                    state: wait_state_field(step, "state", LOAD_STATES, "load")?,
                    timeout_ms: wait_ms_field(step, "timeoutMs", DEFAULT_LOAD_WAIT_MS, MAX_LOAD_WAIT_MS),
                }
            }
            "pageState" => {
                reject_unknown_fields(step, op, &["op"])?;
                BatchStep::PageState
            }
            "pageReady" => {
                reject_unknown_fields(step, op, &["op"])?;
                BatchStep::PageReady
            }
            "snapshot" => {
                reject_unknown_fields(step, op, &["op", "maxChars"])?;
                if index != last {
                    return Err(
                        "batch step snapshot must be the final step: a snapshot replaces the ref \
                         table every later ref-based step relies on"
                            .to_string(),
                    );
                }
                BatchStep::Snapshot {
                    max_chars: step["maxChars"]
                        .as_u64()
                        .unwrap_or(SNAPSHOT_DEFAULT_MAX_CHARS)
                        .clamp(1_000, SNAPSHOT_MAX_MAX_CHARS),
                }
            }
            other => {
                return Err(format!(
                    "batch step {index} has unknown op `{other}` — valid ops: {}",
                    VALID_OPS.join(", ")
                ));
            }
        };
        out.push(parsed);
    }
    Ok(out)
}

/// Build the action request for a Control step, mirroring the single-tool
/// dispatch arms (press without a ref acts on whatever has focus).
pub fn action_request(step: &BatchStep, generation: &str) -> Option<ActionRequest> {
    let (target, action) = match step {
        BatchStep::Click { target, count } => (Some(target.clone()), ActionKind::Click { count: *count }),
        BatchStep::Hover { target } => (Some(target.clone()), ActionKind::Hover),
        BatchStep::Type { target, text, submit } => (
            Some(target.clone()),
            ActionKind::Type { text: text.clone(), submit: *submit },
        ),
        BatchStep::PressKey { key, target } => (target.clone(), ActionKind::Press { key: key.clone() }),
        BatchStep::SelectOption { target, values } => (
            Some(target.clone()),
            ActionKind::Select { values: values.clone() },
        ),
        BatchStep::SetValue { .. } => return None, // rides set_control_value instead
        _ => return None,
    };
    Some(ActionRequest {
        generation: generation.to_string(),
        target,
        action,
    })
}

/// Condense one step's raw answer into an audit record — the batch result
/// carries summaries, not full payloads.
pub fn condense(index: usize, step: &BatchStep, value: &mut Value) {
    let ok = value.get("ok").and_then(Value::as_bool).unwrap_or(false);
    let error = value.get("error").and_then(Value::as_str).map(str::to_string);
    let detail = value.get("detail").and_then(Value::as_str).map(str::to_string);
    let mut record = json!({ "index": index, "op": step.op(), "ok": ok });
    if let Some(error) = error {
        record["error"] = json!(error);
    }
    if let Some(detail) = detail {
        record["detail"] = json!(detail);
    }
    *value = record;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn steps(list: Value) -> Result<Vec<BatchStep>, String> {
        validate(&list)
    }

    #[test]
    fn a_form_fill_chain_validates() {
        let out = steps(json!([
            { "op": "waitForSelector", "ref": "e12", "state": "visible", "timeoutMs": 3000 },
            { "op": "click", "ref": "e12" },
            { "op": "setValue", "ref": "e18", "value": "demo@example.com" },
            { "op": "pressKey", "key": "Enter" },
            { "op": "waitForLoadState", "state": "domcontentloaded", "timeoutMs": 5000 },
            { "op": "snapshot", "maxChars": 20000 },
        ]))
        .unwrap();
        assert_eq!(out.len(), 6);
        assert_eq!(out[0], BatchStep::WaitForSelector { target: "e12".into(), state: "visible", timeout_ms: 3000 });
        assert_eq!(out[2], BatchStep::SetValue { target: "e18".into(), value: json!("demo@example.com") });
        assert_eq!(
            out[3],
            BatchStep::PressKey { key: "Enter".into(), target: None }
        );
        assert_eq!(out[5], BatchStep::Snapshot { max_chars: 20000 });
    }

    #[test]
    fn defaults_fill_in_and_submit_defaults_false() {
        let out = steps(json!([
            { "op": "waitForSelector", "ref": "e1" },
            { "op": "waitForLoadState" },
            { "op": "type", "ref": "e2", "text": "hi" },
            { "op": "pageState" },
        ]))
        .unwrap();
        assert_eq!(out[0], BatchStep::WaitForSelector { target: "e1".into(), state: "visible", timeout_ms: DEFAULT_WAIT_MS });
        assert_eq!(out[1], BatchStep::WaitForLoadState { state: "load", timeout_ms: DEFAULT_LOAD_WAIT_MS });
        assert_eq!(out[2], BatchStep::Type { target: "e2".into(), text: "hi".into(), submit: false });
    }

    #[test]
    fn the_chain_is_capped() {
        let many: Vec<_> = (0..=MAX_STEPS)
            .map(|_| json!({ "op": "pageState" }))
            .collect();
        let error = steps(json!(many)).unwrap_err();
        assert!(error.contains("at most 32"), "{error}");
        assert!(steps(json!([])).unwrap_err().contains("at least one"));
    }

    #[test]
    fn unknown_ops_and_fields_are_rejected() {
        let error = steps(json!([{ "op": "evaluate", "expression": "1+1" }])).unwrap_err();
        assert!(error.contains("unknown op `evaluate`"), "{error}");
        let error = steps(json!([{ "op": "click", "ref": "e1", "selector": "#x" }])).unwrap_err();
        assert!(error.contains("unknown field `selector`"), "{error}");
        let error = steps(json!([{ "op": "click", "ref": "e1", "shell": "dir" }])).unwrap_err();
        assert!(error.contains("unknown field `shell`"), "{error}");
    }

    #[test]
    fn refs_texts_and_values_are_required() {
        assert!(steps(json!([{ "op": "click" }])).is_err());
        assert!(steps(json!([{ "op": "type", "ref": "e1" }])).is_err());
        assert!(steps(json!([{ "op": "setValue", "ref": "e1" }])).is_err());
        assert!(steps(json!([{ "op": "setValue", "ref": "e1", "value": null }])).is_err());
        assert!(steps(json!([{ "op": "selectOption", "ref": "e1", "values": [] }])).is_err());
        assert!(steps(json!([{ "op": "pressKey" }])).is_err());
        assert!(steps(json!([{ "op": "pressKey", "key": "" }])).is_err());
        // pressKey may omit the ref (acts on focus).
        assert!(steps(json!([{ "op": "pressKey", "key": "Enter" }])).is_ok());
    }

    #[test]
    fn wait_states_and_timeouts_are_clamped() {
        let error = steps(json!([{ "op": "waitForSelector", "ref": "e1", "state": "networkidle" }]))
            .unwrap_err();
        assert!(error.contains("attached|visible|hidden|detached"), "{error}");
        let error = steps(json!([{ "op": "waitForLoadState", "state": "networkidle" }])).unwrap_err();
        assert!(error.contains("domcontentloaded|load"), "{error}");
        let out = steps(json!([
            { "op": "waitForSelector", "ref": "e1", "timeoutMs": 999_999 },
            { "op": "waitForLoadState", "timeoutMs": 1 },
        ]))
        .unwrap();
        assert_eq!(out[0], BatchStep::WaitForSelector { target: "e1".into(), state: "visible", timeout_ms: MAX_WAIT_MS });
        assert_eq!(out[1], BatchStep::WaitForLoadState { state: "load", timeout_ms: 100 });
    }

    #[test]
    fn snapshot_is_terminal_only() {
        let error = steps(json!([
            { "op": "snapshot" },
            { "op": "click", "ref": "e1" },
        ]))
        .unwrap_err();
        assert!(error.contains("must be the final step"), "{error}");
        // Terminal snapshot is fine and its maxChars clamps.
        let out = steps(json!([
            { "op": "click", "ref": "e1" },
            { "op": "snapshot", "maxChars": 5_000_000 },
        ]))
        .unwrap();
        assert_eq!(out[1], BatchStep::Snapshot { max_chars: SNAPSHOT_MAX_MAX_CHARS });
    }

    #[test]
    fn control_steps_match_the_single_tool_set() {
        let out = steps(json!([
            { "op": "click", "ref": "e1" },
            { "op": "hover", "ref": "e1" },
            { "op": "type", "ref": "e1", "text": "x" },
            { "op": "setValue", "ref": "e1", "value": true },
            { "op": "pressKey", "key": "Enter" },
            { "op": "selectOption", "ref": "e1", "values": ["a"] },
            { "op": "pageState" },
            { "op": "waitForSelector", "ref": "e1" },
        ]))
        .unwrap();
        for step in &out[..6] {
            assert!(is_control(step), "{:?} must be control", step);
        }
        for step in &out[6..] {
            assert!(!is_control(step), "{:?} must be read", step);
        }
    }

    #[test]
    fn summaries_speak_the_single_tool_vocabulary() {
        let out = steps(json!([
            { "op": "click", "ref": "e1", "count": 2 },
            { "op": "type", "ref": "e1", "text": "hello" },
            { "op": "setValue", "ref": "e1", "value": "v" },
            { "op": "selectOption", "ref": "e1", "values": ["a", "b"] },
        ]))
        .unwrap();
        assert_eq!(step_summary(&out[0]), "double click");
        assert_eq!(step_summary(&out[1]), "type \"hello\"");
        assert_eq!(step_summary(&out[2]), "set a control's value");
        assert_eq!(step_summary(&out[3]), "select a, b");
        let long = steps(json!([{ "op": "type", "ref": "e1", "text": "x".repeat(200) }])).unwrap();
        let summary = step_summary(&long[0]);
        assert!(summary.len() < 80, "{summary}");
        assert!(summary.ends_with("...\""), "{summary}");
    }

    #[test]
    fn action_requests_mirror_the_single_tool_shape() {
        let out = steps(json!([
            { "op": "click", "ref": "e12", "count": 2 },
            { "op": "pressKey", "key": "Enter" },
            { "op": "setValue", "ref": "e18", "value": "v" },
            { "op": "pageState" },
        ]))
        .unwrap();
        let request = action_request(&out[0], "G.1.4.2").unwrap();
        assert_eq!(request.generation, "G.1.4.2");
        assert_eq!(request.target.as_deref(), Some("e12"));
        assert!(matches!(request.action, ActionKind::Click { count: Some(2) }));
        let request = action_request(&out[1], "G.1.4.2").unwrap();
        assert_eq!(request.target, None, "press without ref acts on focus");
        assert!(action_request(&out[2], "G.1.4.2").is_none(), "setValue rides set_control_value");
        assert!(action_request(&out[3], "G.1.4.2").is_none());
    }

    #[test]
    fn records_condense_to_summaries() {
        let out = steps(json!([{ "op": "click", "ref": "e1" }])).unwrap();
        let mut value = json!({ "ok": true, "url": "https://x.test/", "generation": "G.1.4.2" });
        condense(3, &out[0], &mut value);
        assert_eq!(value, json!({ "index": 3, "op": "click", "ok": true }));
        let mut value = json!({ "ok": false, "url": "https://x.test/", "error": "stale", "detail": "moved" });
        condense(1, &out[0], &mut value);
        assert_eq!(value["error"], "stale");
        assert_eq!(value["detail"], "moved");
        assert!(value.get("url").is_none(), "page-controlled fields stay out of the audit record");
    }
}
