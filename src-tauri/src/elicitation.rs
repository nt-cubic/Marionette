//! ACP `elicitation/create` (form mode) — Codex Plan / request_user_input / MCP approvals.
//!
//! Wire semantics aligned with Codeg `acp/question.rs` (codex-acp + Grok-free path).
//! Pure `serde_json` — no sacp dependency. UI reuses permission + ask events.

use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap, HashSet};

const MAX_QUESTIONS: usize = 4;
const MAX_OPTIONS: usize = 4;
const MAX_TEXT: usize = 4096;
pub const DECLINE_OPTION_ID: &str = "decline";

#[derive(Debug, Clone)]
pub struct ElicitationChoice {
    pub label: String,
    pub value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    Text,
    MultiSelect,
    Boolean,
    Number,
    Integer,
}

#[derive(Debug, Clone)]
pub struct ElicitationField {
    pub id: String,
    pub kind: FieldKind,
    /// Display label → wire value (const).
    pub value_by_label: HashMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct QuestionSpec {
    pub id: String,
    pub question: String,
    pub header: String,
    pub multi_select: bool,
    pub options: Vec<(String, String)>, // label, description
}

#[derive(Debug, Clone)]
pub struct ElicitationApproval {
    pub message: String,
    pub tool_call_id: Option<String>,
    pub options: Vec<ApprovalOption>,
    pub persist_in_content: bool,
}

#[derive(Debug, Clone)]
pub struct ApprovalOption {
    pub option_id: String,
    pub label: String,
    pub kind: String,
}

#[derive(Debug, Clone)]
pub struct ElicitationQuestions {
    pub specs: Vec<QuestionSpec>,
    pub fields: Vec<ElicitationField>,
    pub tool_call_id: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone)]
pub enum ElicitationPlan {
    Approval(ElicitationApproval),
    Questions(ElicitationQuestions),
}

fn clip(s: &str, max: usize) -> String {
    s.trim().chars().take(max).collect()
}

fn is_mcp_tool_call_approval(raw: &Value) -> bool {
    raw.get("_meta")
        .and_then(|m| m.get("codex_approval_kind"))
        .and_then(Value::as_str)
        == Some("mcp_tool_call")
}

fn is_other_companion(id: &str) -> bool {
    id.ends_with("__other") || id.ends_with("_other")
}

/// Classify form elicitation params into permission-style or ask-style plan.
pub fn classify_elicitation(raw: &Value) -> Result<ElicitationPlan, String> {
    let mode = raw
        .get("mode")
        .ok_or_else(|| "elicitation missing mode".to_string())?;
    let mode_type = mode
        .get("type")
        .or_else(|| mode.get("mode"))
        .and_then(Value::as_str)
        .unwrap_or("form");
    if mode_type != "form" {
        return Err(format!("elicitation mode {mode_type:?} not supported"));
    }

    let message = raw
        .get("message")
        .and_then(Value::as_str)
        .map(|s| clip(s, MAX_TEXT))
        .unwrap_or_default();

    let tool_call_id = mode
        .get("scope")
        .and_then(|s| {
            s.get("toolCallId")
                .or_else(|| s.get("tool_call_id"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .filter(|s| !s.is_empty());

    if is_mcp_tool_call_approval(raw) {
        return Ok(ElicitationPlan::Approval(approval_from_form(
            mode,
            message,
            tool_call_id,
        )));
    }

    let questions = parse_form_questions(mode, raw, message.clone(), tool_call_id.clone());
    if questions.specs.is_empty() {
        return Ok(ElicitationPlan::Approval(ElicitationApproval {
            message,
            tool_call_id,
            options: vec![
                ApprovalOption {
                    option_id: "accept".into(),
                    label: "Accept".into(),
                    kind: "allow_once".into(),
                },
                decline_option(),
            ],
            persist_in_content: false,
        }));
    }
    Ok(ElicitationPlan::Questions(questions))
}

fn decline_option() -> ApprovalOption {
    ApprovalOption {
        option_id: DECLINE_OPTION_ID.into(),
        label: "Decline".into(),
        kind: "reject_once".into(),
    }
}

fn approval_from_form(
    mode: &Value,
    message: String,
    tool_call_id: Option<String>,
) -> ElicitationApproval {
    let props = mode
        .get("requestedSchema")
        .or_else(|| mode.get("requested_schema"))
        .and_then(|s| s.get("properties"))
        .and_then(Value::as_object);

    let persist_choices = props
        .and_then(|p| p.get("persist"))
        .map(string_prop_choices)
        .filter(|c| !c.is_empty());

    let persist_in_content = persist_choices.is_some();
    let mut options = Vec::new();
    let mut seen = HashSet::new();
    if let Some(choices) = persist_choices {
        for c in choices {
            if options.len() >= MAX_OPTIONS {
                break;
            }
            let value = c.value.trim();
            if value.is_empty() || value == DECLINE_OPTION_ID || !seen.insert(value.to_string()) {
                continue;
            }
            options.push(ApprovalOption {
                option_id: value.to_string(),
                label: if c.label.trim().is_empty() {
                    value.to_string()
                } else {
                    clip(&c.label, MAX_TEXT)
                },
                kind: if value == "once" {
                    "allow_once".into()
                } else {
                    "allow_always".into()
                },
            });
        }
    }
    if options.is_empty() {
        options.push(ApprovalOption {
            option_id: "accept".into(),
            label: "Allow".into(),
            kind: "allow_once".into(),
        });
    }
    options.push(decline_option());
    ElicitationApproval {
        message,
        tool_call_id,
        options,
        persist_in_content,
    }
}

fn string_prop_choices(prop: &Value) -> Vec<ElicitationChoice> {
    let mut out = Vec::new();
    // oneOf: [{ const, title }]
    if let Some(arr) = prop.get("oneOf").and_then(Value::as_array) {
        for item in arr {
            let value = item
                .get("const")
                .and_then(|v| match v {
                    Value::String(s) => Some(s.clone()),
                    Value::Number(n) => Some(n.to_string()),
                    Value::Bool(b) => Some(b.to_string()),
                    _ => None,
                })
                .or_else(|| item.get("enum").and_then(Value::as_array).and_then(|a| a.first()).and_then(|v| v.as_str().map(str::to_string)))
                .unwrap_or_default();
            if value.is_empty() {
                continue;
            }
            let label = item
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or(value.as_str())
                .to_string();
            out.push(ElicitationChoice { label, value });
        }
    }
    // enum: ["a","b"] with optional enumNames
    if out.is_empty() {
        if let Some(arr) = prop.get("enum").and_then(Value::as_array) {
            let names = prop
                .get("enumNames")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            for (i, item) in arr.iter().enumerate() {
                let value = match item {
                    Value::String(s) => s.clone(),
                    Value::Number(n) => n.to_string(),
                    Value::Bool(b) => b.to_string(),
                    _ => continue,
                };
                let label = names
                    .get(i)
                    .and_then(Value::as_str)
                    .unwrap_or(value.as_str())
                    .to_string();
                out.push(ElicitationChoice { label, value });
            }
        }
    }
    out
}

fn multi_select_choices(items: &Value) -> Vec<ElicitationChoice> {
    // items may be { type: string, oneOf: [...] } or { enum: [...] }
    string_prop_choices(items)
}

fn parse_form_questions(
    mode: &Value,
    raw: &Value,
    message: String,
    tool_call_id: Option<String>,
) -> ElicitationQuestions {
    let props = mode
        .get("requestedSchema")
        .or_else(|| mode.get("requested_schema"))
        .and_then(|s| s.get("properties"))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    let mut specs = Vec::new();
    let mut fields = Vec::new();

    for (id, prop) in &props {
        if specs.len() >= MAX_QUESTIONS {
            break;
        }
        if is_other_companion(id) {
            continue;
        }
        let prop_type = prop
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("string")
            .to_ascii_lowercase();

        let title = prop
            .get("title")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        let description = prop
            .get("description")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());

        let (kind, multi_select, choices) = match prop_type.as_str() {
            "array" => {
                let items = prop.get("items").cloned().unwrap_or(Value::Null);
                (
                    FieldKind::MultiSelect,
                    true,
                    multi_select_choices(&items),
                )
            }
            "boolean" => (
                FieldKind::Boolean,
                false,
                vec![
                    ElicitationChoice {
                        label: "Yes".into(),
                        value: "true".into(),
                    },
                    ElicitationChoice {
                        label: "No".into(),
                        value: "false".into(),
                    },
                ],
            ),
            "number" => (FieldKind::Number, false, Vec::new()),
            "integer" => (FieldKind::Integer, false, Vec::new()),
            _ => {
                // string (or missing type with oneOf/enum)
                let c = string_prop_choices(prop);
                (FieldKind::Text, false, c)
            }
        };

        let question = description
            .clone()
            .or_else(|| title.clone())
            .unwrap_or_else(|| id.clone());
        let question = clip(&question, MAX_TEXT);

        let mut options = Vec::new();
        let mut value_by_label = HashMap::new();
        let mut seen = HashSet::new();
        for c in &choices {
            if options.len() >= MAX_OPTIONS {
                break;
            }
            let label = clip(&c.label, MAX_TEXT);
            if label.is_empty() || !seen.insert(label.clone()) {
                continue;
            }
            value_by_label.insert(label.clone(), c.value.clone());
            options.push((label, String::new()));
        }

        // Free-text only fields: still one question with empty options (Other path in UI).
        // If no options and not free-text-ish, still emit so user can type Other.
        let header = title
            .as_deref()
            .map(|h| clip(h, 12))
            .unwrap_or_else(|| clip(&question, 12));

        let _is_secret = prop
            .get("_meta")
            .and_then(|m| m.get("codex"))
            .and_then(|c| c.get("isSecret"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let _ = raw; // reserved for secret meta on raw path

        specs.push(QuestionSpec {
            id: id.clone(),
            question: question.clone(),
            header,
            multi_select,
            options,
        });
        fields.push(ElicitationField {
            id: id.clone(),
            kind,
            value_by_label,
        });
    }

    ElicitationQuestions {
        specs,
        fields,
        tool_call_id,
        message,
    }
}

pub fn elicitation_decline() -> Value {
    json!({ "action": "decline" })
}

pub fn elicitation_cancel() -> Value {
    json!({ "action": "cancel" })
}

/// Build Accept response for approval-style elicitation (permission card).
pub fn build_approval_response(approval: &ElicitationApproval, option_id: &str) -> Value {
    let accepted = option_id != DECLINE_OPTION_ID
        && approval.options.iter().any(|o| o.option_id == option_id);
    if !accepted {
        return elicitation_decline();
    }
    if approval.persist_in_content {
        return json!({
            "action": "accept",
            "content": { "persist": option_id }
        });
    }
    json!({ "action": "accept" })
}

/// Build Accept response from UI ask answers `[{ question, selected: string[] }]`.
pub fn build_questions_response(questions: &ElicitationQuestions, answers: &Value, declined: bool) -> Value {
    if declined {
        return elicitation_decline();
    }
    let arr = answers.as_array().cloned().unwrap_or_default();
    let mut content = Map::new();

    for item in &arr {
        let qtext = item
            .get("question")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if qtext.is_empty() {
            continue;
        }
        let selected: Vec<String> = item
            .get("selected")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();

        let Some(idx) = questions
            .specs
            .iter()
            .position(|s| s.question == qtext || s.id == qtext || s.header == qtext)
        else {
            continue;
        };
        let field = &questions.fields[idx];
        let mapped: Vec<String> = selected
            .iter()
            .map(|l| {
                field
                    .value_by_label
                    .get(l)
                    .cloned()
                    .unwrap_or_else(|| l.clone())
            })
            .collect();

        let value = match field.kind {
            FieldKind::Text => mapped.into_iter().next().map(Value::String),
            FieldKind::MultiSelect => Some(Value::Array(
                mapped.into_iter().map(Value::String).collect(),
            )),
            FieldKind::Boolean => mapped.first().and_then(|v| parse_bool(v)).map(Value::Bool),
            FieldKind::Number => mapped
                .first()
                .and_then(|v| v.trim().parse::<f64>().ok())
                .filter(|f| f.is_finite())
                .and_then(|f| serde_json::Number::from_f64(f).map(Value::Number)),
            FieldKind::Integer => mapped
                .first()
                .and_then(|v| v.trim().parse::<i64>().ok())
                .map(|i| Value::Number(i.into())),
        };
        if let Some(v) = value {
            content.insert(field.id.clone(), v);
        }
    }

    if content.is_empty() {
        return elicitation_decline();
    }
    json!({ "action": "accept", "content": Value::Object(content) })
}

fn parse_bool(v: &str) -> Option<bool> {
    match v.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "y" => Some(true),
        "false" | "no" | "n" => Some(false),
        _ => None,
    }
}

/// UI questions array for `question/prompt`.
pub fn questions_for_ui(q: &ElicitationQuestions) -> Value {
    let arr: Vec<Value> = q
        .specs
        .iter()
        .map(|s| {
            let options: Vec<Value> = s
                .options
                .iter()
                .map(|(label, desc)| {
                    let mut o = Map::new();
                    o.insert("label".into(), json!(label));
                    if !desc.is_empty() {
                        o.insert("description".into(), json!(desc));
                    }
                    Value::Object(o)
                })
                .collect();
            json!({
                "question": s.question,
                "header": s.header,
                "multiSelect": s.multi_select,
                "options": options,
            })
        })
        .collect();
    Value::Array(arr)
}

/// UI permission options for approval-style elicitation.
pub fn approval_options_for_ui(a: &ElicitationApproval) -> Vec<Value> {
    a.options
        .iter()
        .map(|o| {
            json!({
                "optionId": o.option_id,
                "name": o.label,
                "kind": o.kind,
            })
        })
        .collect()
}

/// Serialize plan fields needed to rebuild response (stored with pending).
pub fn questions_to_stored(q: &ElicitationQuestions) -> Value {
    let fields: Vec<Value> = q
        .fields
        .iter()
        .map(|f| {
            let map: BTreeMap<String, String> = f.value_by_label.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            json!({
                "id": f.id,
                "kind": match f.kind {
                    FieldKind::Text => "text",
                    FieldKind::MultiSelect => "multi",
                    FieldKind::Boolean => "bool",
                    FieldKind::Number => "number",
                    FieldKind::Integer => "integer",
                },
                "valueByLabel": map,
            })
        })
        .collect();
    let specs: Vec<Value> = q
        .specs
        .iter()
        .map(|s| {
            json!({
                "id": s.id,
                "question": s.question,
                "header": s.header,
                "multiSelect": s.multi_select,
                "options": s.options.iter().map(|(l, d)| json!({"label": l, "description": d})).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({
        "message": q.message,
        "toolCallId": q.tool_call_id,
        "specs": specs,
        "fields": fields,
    })
}

pub fn questions_from_stored(v: &Value) -> Option<ElicitationQuestions> {
    let specs_arr = v.get("specs")?.as_array()?;
    let fields_arr = v.get("fields")?.as_array()?;
    let mut specs = Vec::new();
    for s in specs_arr {
        let options = s
            .get("options")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|o| {
                        let label = o.get("label")?.as_str()?.to_string();
                        let desc = o
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        Some((label, desc))
                    })
                    .collect()
            })
            .unwrap_or_default();
        specs.push(QuestionSpec {
            id: s.get("id")?.as_str()?.to_string(),
            question: s.get("question")?.as_str()?.to_string(),
            header: s
                .get("header")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            multi_select: s
                .get("multiSelect")
                .or_else(|| s.get("multi_select"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
            options,
        });
    }
    let mut fields = Vec::new();
    for f in fields_arr {
        let kind = match f.get("kind").and_then(Value::as_str).unwrap_or("text") {
            "multi" => FieldKind::MultiSelect,
            "bool" => FieldKind::Boolean,
            "number" => FieldKind::Number,
            "integer" => FieldKind::Integer,
            _ => FieldKind::Text,
        };
        let mut value_by_label = HashMap::new();
        if let Some(obj) = f.get("valueByLabel").and_then(Value::as_object) {
            for (k, v) in obj {
                if let Some(s) = v.as_str() {
                    value_by_label.insert(k.clone(), s.to_string());
                }
            }
        }
        fields.push(ElicitationField {
            id: f.get("id")?.as_str()?.to_string(),
            kind,
            value_by_label,
        });
    }
    Some(ElicitationQuestions {
        specs,
        fields,
        tool_call_id: v
            .get("toolCallId")
            .and_then(Value::as_str)
            .map(str::to_string),
        message: v
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    })
}

pub fn approval_to_stored(a: &ElicitationApproval) -> Value {
    json!({
        "message": a.message,
        "toolCallId": a.tool_call_id,
        "persistInContent": a.persist_in_content,
        "options": a.options.iter().map(|o| json!({
            "optionId": o.option_id,
            "label": o.label,
            "kind": o.kind,
        })).collect::<Vec<_>>(),
    })
}

pub fn approval_from_stored(v: &Value) -> Option<ElicitationApproval> {
    let options = v
        .get("options")?
        .as_array()?
        .iter()
        .filter_map(|o| {
            Some(ApprovalOption {
                option_id: o.get("optionId")?.as_str()?.to_string(),
                label: o.get("label")?.as_str()?.to_string(),
                kind: o
                    .get("kind")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            })
        })
        .collect();
    Some(ElicitationApproval {
        message: v.get("message")?.as_str()?.to_string(),
        tool_call_id: v
            .get("toolCallId")
            .and_then(Value::as_str)
            .map(str::to_string),
        options,
        persist_in_content: v
            .get("persistInContent")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_form_becomes_accept_decline() {
        let raw = json!({
            "message": "Proceed?",
            "mode": {
                "type": "form",
                "requestedSchema": { "type": "object", "properties": {} }
            }
        });
        let plan = classify_elicitation(&raw).unwrap();
        match plan {
            ElicitationPlan::Approval(a) => {
                assert!(a.options.iter().any(|o| o.option_id == "accept"));
                assert!(a.options.iter().any(|o| o.option_id == DECLINE_OPTION_ID));
            }
            _ => panic!("expected approval"),
        }
    }

    #[test]
    fn string_one_of_becomes_questions() {
        let raw = json!({
            "message": "Pick",
            "mode": {
                "type": "form",
                "requestedSchema": {
                    "type": "object",
                    "properties": {
                        "color": {
                            "type": "string",
                            "title": "Color",
                            "description": "Which color?",
                            "oneOf": [
                                { "const": "blue", "title": "Blue" },
                                { "const": "red", "title": "Red" }
                            ]
                        }
                    }
                },
                "scope": { "type": "session", "toolCallId": "call-1" }
            }
        });
        let plan = classify_elicitation(&raw).unwrap();
        match plan {
            ElicitationPlan::Questions(q) => {
                assert_eq!(q.specs.len(), 1);
                assert_eq!(q.tool_call_id.as_deref(), Some("call-1"));
                let resp = build_questions_response(
                    &q,
                    &json!([{ "question": "Which color?", "selected": ["Blue"] }]),
                    false,
                );
                assert_eq!(resp["action"], "accept");
                assert_eq!(resp["content"]["color"], "blue");
            }
            _ => panic!("expected questions"),
        }
    }

    #[test]
    fn mcp_approval_uses_persist() {
        let raw = json!({
            "message": "Allow tool?",
            "mode": {
                "type": "form",
                "requestedSchema": {
                    "type": "object",
                    "properties": {
                        "persist": {
                            "type": "string",
                            "oneOf": [
                                { "const": "once", "title": "Once" },
                                { "const": "always", "title": "Always" }
                            ]
                        }
                    }
                }
            },
            "_meta": { "codex_approval_kind": "mcp_tool_call" }
        });
        let plan = classify_elicitation(&raw).unwrap();
        match plan {
            ElicitationPlan::Approval(a) => {
                assert!(a.persist_in_content);
                let resp = build_approval_response(&a, "once");
                assert_eq!(resp["action"], "accept");
                assert_eq!(resp["content"]["persist"], "once");
            }
            _ => panic!("expected approval"),
        }
    }
}
