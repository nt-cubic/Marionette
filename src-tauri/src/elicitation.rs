//! ACP `elicitation/create` (form mode) — Codex Plan / request_user_input / OpenCode questions.
//!
//! Two wire shapes share this path:
//! - Codex (Codeg): `mode` is an object `{ type: "form", requestedSchema, scope }`
//! - ACP spec / OpenCode v2: `mode` is the string `"form"`, `requestedSchema` +
//!   `toolCallId` sit on the request params themselves
//!
//! Pure `serde_json` — no sacp dependency. UI reuses permission + ask events.

use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap, HashSet};

const MAX_QUESTIONS: usize = 8;
const MAX_OPTIONS: usize = 16;
const MAX_TEXT: usize = 4096;
pub const DECLINE_OPTION_ID: &str = "decline";
/// Codex sends this sentinel option (`isOther` questions) to say "the real
/// answer is in the `_note` field". It is not a choice: selecting it alone
/// yields an empty answer in codex-acp's own `userInputAnswerText`.
const CODEX_OTHER_OPTION: &str = "None of the above";
/// ZCode injects this sentinel into single-select enums ("Skip this question").
const SKIP_OPTION_VALUE: &str = "__skip__";

#[derive(Debug, Clone)]
pub struct ElicitationChoice {
    pub label: String,
    pub value: String,
    pub description: String,
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
    /// OpenCode `{id}_custom` (or Codex/DSH/ZCode `{id}__other` / `{id}_other`)
    /// free-text companion: the "other" (replacement) answer.
    pub custom_id: Option<String>,
    /// Codex `{id}_note` ("Additional answer or note"): a real note channel
    /// that rides *beside* a pick instead of replacing it.
    pub note_id: Option<String>,
    /// Whether the UI should offer "other" (a free-text replacement answer).
    pub other_allowed: bool,
    /// Whether the UI should offer a separate "note" (codex `_note`, or
    /// Claude's `_custom` when a pick is also present).
    pub note_allowed: bool,
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
    id.ends_with("__other") || id.ends_with("_other") || id.ends_with("_custom")
}

fn codex_meta(prop: &Value) -> Option<&Value> {
    prop.get("_meta").and_then(|m| m.get("codex"))
}

/// Owner question of a Codex note companion.
///
/// `codex-acp` names it `{questionId}_note` and appends a counter when the id
/// is already taken (`_note1`, `_note2`), so the suffix alone is not enough.
/// The authoritative mapping lives in `_meta.codex.questionId`; the suffix is
/// only a fallback for adapters that omit the meta block.
fn note_owner(id: &str, prop: &Value, props: &Map<String, Value>) -> Option<String> {
    if let Some(owner) = codex_meta(prop)
        .and_then(|c| c.get("questionId"))
        .and_then(Value::as_str)
    {
        if props.contains_key(owner) {
            return Some(owner.to_string());
        }
    }
    let idx = id.rfind("_note")?;
    let (base, tail) = id.split_at(idx);
    let tail = &tail["_note".len()..];
    if (tail.is_empty() || tail.chars().all(|c| c.is_ascii_digit())) && props.contains_key(base) {
        return Some(base.to_string());
    }
    None
}

fn elicitation_mode_type(mode: &Value) -> &str {
    if let Some(s) = mode.as_str() {
        return s;
    }
    mode.get("type")
        .or_else(|| mode.get("mode"))
        .and_then(Value::as_str)
        .unwrap_or("form")
}

/// ACP spec puts `requestedSchema` on params; Codex nests it under `mode`.
fn requested_schema<'a>(mode: &'a Value, raw: &'a Value) -> Option<&'a Value> {
    raw.get("requestedSchema")
        .or_else(|| raw.get("requested_schema"))
        .or_else(|| mode.get("requestedSchema"))
        .or_else(|| mode.get("requested_schema"))
}

fn tool_call_id_of(mode: &Value, raw: &Value) -> Option<String> {
    raw.get("toolCallId")
        .or_else(|| raw.get("tool_call_id"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            mode.get("scope")
                .and_then(|s| {
                    s.get("toolCallId")
                        .or_else(|| s.get("tool_call_id"))
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .filter(|s| !s.is_empty())
        })
}

fn companion_id_for(id: &str, props: &Map<String, Value>) -> Option<String> {
    for suffix in ["_custom", "__other", "_other"] {
        let key = format!("{id}{suffix}");
        if props.contains_key(&key) {
            return Some(key);
        }
    }
    None
}

/// Classify form elicitation params into permission-style or ask-style plan.
///
/// `claude_like` is true for the Claude Code adapter: its per-question
/// `_custom` field is a *note* when an option is also picked (and the answer
/// only when nothing was picked). Other adapters use the same suffix as a
/// plain "other" replacement, so the distinction cannot come from the schema.
pub fn classify_elicitation(raw: &Value, claude_like: bool) -> Result<ElicitationPlan, String> {
    let mode = raw
        .get("mode")
        .ok_or_else(|| "elicitation missing mode".to_string())?;
    let mode_type = elicitation_mode_type(mode);
    if mode_type != "form" {
        return Err(format!("elicitation mode {mode_type:?} not supported"));
    }

    let message = raw
        .get("message")
        .and_then(Value::as_str)
        .map(|s| clip(s, MAX_TEXT))
        .unwrap_or_default();

    let tool_call_id = tool_call_id_of(mode, raw);

    if is_mcp_tool_call_approval(raw) {
        return Ok(ElicitationPlan::Approval(approval_from_form(
            mode,
            raw,
            message,
            tool_call_id,
        )));
    }

    let questions = parse_form_questions(
        mode,
        raw,
        message.clone(),
        tool_call_id.clone(),
        claude_like,
    );
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
    raw: &Value,
    message: String,
    tool_call_id: Option<String>,
) -> ElicitationApproval {
    let props = requested_schema(mode, raw)
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

fn const_value(item: &Value) -> Option<String> {
    item.get("const")
        .and_then(|v| match v {
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            Value::Bool(b) => Some(b.to_string()),
            _ => None,
        })
        .or_else(|| {
            item.get("enum")
                .and_then(Value::as_array)
                .and_then(|a| a.first())
                .and_then(|v| match v {
                    Value::String(s) => Some(s.clone()),
                    Value::Number(n) => Some(n.to_string()),
                    Value::Bool(b) => Some(b.to_string()),
                    _ => None,
                })
        })
        .filter(|s| !s.is_empty())
}

fn choices_from_union(arr: &[Value]) -> Vec<ElicitationChoice> {
    let mut out = Vec::new();
    for item in arr {
        let Some(value) = const_value(item) else {
            continue;
        };
        let label = item
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or(value.as_str())
            .to_string();
        let description = item
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        out.push(ElicitationChoice {
            label,
            value,
            description,
        });
    }
    out
}

fn string_prop_choices(prop: &Value) -> Vec<ElicitationChoice> {
    // oneOf / anyOf: [{ const, title, description }]
    // OpenCode multi-select uses items.anyOf; Codex uses oneOf.
    for key in ["oneOf", "anyOf"] {
        if let Some(arr) = prop.get(key).and_then(Value::as_array) {
            let out = choices_from_union(arr);
            if !out.is_empty() {
                return out;
            }
        }
    }
    // enum: ["a","b"] with optional enumNames
    let mut out = Vec::new();
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
            out.push(ElicitationChoice {
                label,
                value,
                description: String::new(),
            });
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
    claude_like: bool,
) -> ElicitationQuestions {
    let props = requested_schema(mode, raw)
        .and_then(|s| s.get("properties"))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    // Companions are not questions: they carry the free-text channels of the
    // question they belong to. `_note` (Codex) used to fall through and became
    // a second, mandatory question ("Additional answer or note").
    let mut note_by_owner: HashMap<String, String> = HashMap::new();
    let mut note_ids: HashSet<String> = HashSet::new();
    for (id, prop) in &props {
        if let Some(owner) = note_owner(id, prop, &props) {
            note_by_owner.entry(owner).or_insert_with(|| id.clone());
            note_ids.insert(id.clone());
        }
    }

    let mut specs = Vec::new();
    let mut fields = Vec::new();

    for (id, prop) in &props {
        if specs.len() >= MAX_QUESTIONS {
            break;
        }
        if is_other_companion(id) || note_ids.contains(id) {
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
                (FieldKind::MultiSelect, true, multi_select_choices(&items))
            }
            "boolean" => (
                FieldKind::Boolean,
                false,
                vec![
                    ElicitationChoice {
                        label: "Yes".into(),
                        value: "true".into(),
                        description: String::new(),
                    },
                    ElicitationChoice {
                        label: "No".into(),
                        value: "false".into(),
                        description: String::new(),
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

        let note_id = note_by_owner.get(id).cloned();
        let mut options = Vec::new();
        let mut value_by_label = HashMap::new();
        let mut seen = HashSet::new();
        for c in &choices {
            if options.len() >= MAX_OPTIONS {
                break;
            }
            // Sentinels are protocol control values, not choices: ZCode's
            // "__skip__" and Codex's "None of the above" must never render as
            // ordinary options (picking the latter alone means "no answer").
            if c.value == SKIP_OPTION_VALUE {
                continue;
            }
            if c.label == CODEX_OTHER_OPTION && note_id.is_some() {
                continue;
            }
            let label = clip(&c.label, MAX_TEXT);
            if label.is_empty() || !seen.insert(label.clone()) {
                continue;
            }
            value_by_label.insert(label.clone(), c.value.clone());
            options.push((label, clip(&c.description, MAX_TEXT)));
        }

        let has_options = !options.is_empty();
        let custom_id = companion_id_for(id, &props);
        let is_other_flag = codex_meta(prop)
            .and_then(|c| c.get("isOther"))
            .and_then(Value::as_bool);
        // Free text must have a wire slot: Codex declares it with
        // `_meta.codex.isOther`, companion-style adapters carry it as a field,
        // and an option-less field is a plain text input by definition.
        let other_allowed = if has_options {
            custom_id.is_some() || is_other_flag == Some(true)
        } else {
            true
        };
        // A note is either Codex's dedicated `_note` field, or Claude's
        // `_custom` on a single-select question (there the adapter turns it
        // into `annotations[q].notes` when a pick is also present).
        let note_allowed =
            note_id.is_some() || (claude_like && !multi_select && custom_id.is_some());

        // Free-text only fields: still one question with empty options (the
        // answer field itself is the free text).
        let header = title
            .as_deref()
            .map(|h| clip(h, 32))
            .unwrap_or_else(|| clip(&question, 32));

        let _is_secret = codex_meta(prop)
            .and_then(|c| c.get("isSecret"))
            .and_then(Value::as_bool)
            .unwrap_or(false);

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
            custom_id,
            note_id,
            other_allowed,
            note_allowed,
        });
    }

    // Claude's single-question form puts the real question in `message` and only
    // a short header (or nothing) on the field — the card would otherwise show
    // the internal id `question_0`. Do it on the spec so the answer lookup back
    // from the UI (which echoes this text) still matches.
    if specs.len() == 1 && !message.is_empty() {
        if let Some(s) = specs.first_mut() {
            if s.question == s.id || s.question == s.header {
                s.question = message.clone();
                if s.header == s.id {
                    s.header = clip(&message, 32);
                }
            }
        }
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
    let accepted =
        option_id != DECLINE_OPTION_ID && approval.options.iter().any(|o| o.option_id == option_id);
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

/// Build Accept response from UI ask answers
/// `[{ question, selected: string[], typed?: string, note?: string }]`.
///
/// 「其他」(typed) replaces the answer; 「补充」(note) rides beside it. Each
/// protocol has its own slots:
/// - Codex: `{id}_note` is a real note field, and typed answers are sent as the
///   `None of the above` sentinel + note text (codex-acp's own mapping).
/// - Claude: `_custom` is the answer when nothing is picked and
///   `annotations[q].notes` when an option is picked — one field, two meanings.
/// - OpenCode / ZCode / DSH: `_custom` / `_other` / `__other` is a replacement
///   answer; there is no note slot, so a note is never sent.
/// - Free text answers go to the companion field only (writing both the main
///   field and the companion made Claude duplicate the answer and multi-selects
///   read "x, x").
pub fn build_questions_response(
    questions: &ElicitationQuestions,
    answers: &Value,
    declined: bool,
) -> Value {
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

        let mut known = Vec::new();
        let mut legacy_custom = Vec::new();
        for label in &selected {
            if let Some(v) = field.value_by_label.get(label) {
                known.push(v.clone());
            } else if !label.trim().is_empty() {
                legacy_custom.push(label.clone());
            }
        }
        // Older clients (and the Telegram buttons) only send `selected`; an
        // unknown label there is a typed answer.
        let typed = item
            .get("typed")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .or_else(|| {
                if legacy_custom.is_empty() {
                    None
                } else {
                    Some(legacy_custom.join(", "))
                }
            });
        let note = item
            .get("note")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);

        let codex_note = field.note_id.is_some();
        let claude_note = field.note_allowed && field.custom_id.is_some();
        let merge_note = |t: &str| match &note {
            Some(n) => format!("{t}\n\n{n}"),
            None => t.to_string(),
        };

        let mut main: Option<Value> = None;
        let mut custom_out: Option<String> = None;
        let mut note_out: Option<String> = None;

        match field.kind {
            FieldKind::MultiSelect => {
                if let Some(t) = &typed {
                    main = Some(Value::Array(Vec::new()));
                    if field.custom_id.is_some() {
                        custom_out = Some(t.clone());
                    } else if codex_note {
                        note_out = Some(merge_note(t));
                    } else {
                        main = Some(Value::Array(vec![Value::String(t.clone())]));
                    }
                } else if !known.is_empty() {
                    main = Some(Value::Array(
                        known.iter().cloned().map(Value::String).collect(),
                    ));
                    if codex_note {
                        note_out = note.clone();
                    }
                }
            }
            FieldKind::Text => {
                if let Some(t) = &typed {
                    if codex_note {
                        main = Some(Value::String(CODEX_OTHER_OPTION.to_string()));
                        note_out = Some(merge_note(t));
                    } else if claude_note {
                        main = Some(Value::String(t.clone()));
                        note_out = note.clone();
                    } else if field.custom_id.is_some() {
                        custom_out = Some(t.clone());
                    } else {
                        main = Some(Value::String(t.clone()));
                    }
                } else if let Some(v) = known.first() {
                    main = Some(Value::String(v.clone()));
                    if codex_note || claude_note {
                        note_out = note.clone();
                    }
                }
            }
            FieldKind::Boolean => {
                let raw = known.first().or(typed.as_ref());
                if let Some(v) = raw.and_then(|v| parse_bool(v)) {
                    main = Some(Value::Bool(v));
                }
            }
            FieldKind::Number => {
                let raw = known.first().or(typed.as_ref());
                if let Some(f) = raw
                    .and_then(|v| v.trim().parse::<f64>().ok())
                    .filter(|f| f.is_finite())
                {
                    if let Some(n) = serde_json::Number::from_f64(f) {
                        main = Some(Value::Number(n));
                    }
                }
            }
            FieldKind::Integer => {
                let raw = known.first().or(typed.as_ref());
                if let Some(i) = raw.and_then(|v| v.trim().parse::<i64>().ok()) {
                    main = Some(Value::Number(i.into()));
                }
            }
        }

        if let Some(v) = main {
            content.insert(field.id.clone(), v);
        }
        if let Some(t) = custom_out {
            if let Some(cid) = &field.custom_id {
                content.insert(cid.clone(), Value::String(t));
            }
        }
        if let Some(n) = note_out {
            if let Some(nid) = &field.note_id {
                content.insert(nid.clone(), Value::String(n));
            } else if claude_note {
                if let Some(cid) = &field.custom_id {
                    content.insert(cid.clone(), Value::String(n));
                }
            }
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
///
/// Carries the agent-declared free-text capabilities (`other` / `note`) so the
/// card can render the right shape per question instead of guessing per agent.
pub fn questions_for_ui(q: &ElicitationQuestions) -> Value {
    let arr: Vec<Value> = q
        .specs
        .iter()
        .zip(q.fields.iter())
        .map(|(s, f)| {
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
                "other": f.other_allowed,
                "note": f.note_allowed,
                // There is no native ACP attachment field. Attachments ride in a
                // separate follow-up message, and a question may only offer them
                // when it accepts a custom text answer (`other` includes the
                // option-less text question).
                "attachmentsAllowed": f.other_allowed,
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
            let map: BTreeMap<String, String> = f
                .value_by_label
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
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
                "customId": f.custom_id,
                "noteId": f.note_id,
                "otherAllowed": f.other_allowed,
                "noteAllowed": f.note_allowed,
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
            custom_id: f
                .get("customId")
                .or_else(|| f.get("custom_id"))
                .and_then(Value::as_str)
                .map(str::to_string)
                .filter(|s| !s.is_empty()),
            note_id: f
                .get("noteId")
                .or_else(|| f.get("note_id"))
                .and_then(Value::as_str)
                .map(str::to_string)
                .filter(|s| !s.is_empty()),
            other_allowed: f
                .get("otherAllowed")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            note_allowed: f
                .get("noteAllowed")
                .and_then(Value::as_bool)
                .unwrap_or(false),
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
        let plan = classify_elicitation(&raw, false).unwrap();
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
        let plan = classify_elicitation(&raw, false).unwrap();
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
        let plan = classify_elicitation(&raw, false).unwrap();
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

    /// OpenCode v2 question tool: ACP spec shape (`mode: "form"` string,
    /// `requestedSchema` on params, `{id}_custom` companion).
    #[test]
    fn opencode_question_form_becomes_ask_card() {
        let raw = json!({
            "mode": "form",
            "sessionId": "ses_abc",
            "toolCallId": "call_question_1",
            "message": "Questions",
            "requestedSchema": {
                "type": "object",
                "properties": {
                    "q0": {
                        "type": "string",
                        "title": "编辑器卡死，怎么恢复",
                        "description": "编辑器卡住了。要不要我直接结束 Unity 进程？",
                        "oneOf": [
                            {
                                "const": "你来重启（推荐）",
                                "title": "你来重启（推荐）",
                                "description": "我 kill 掉卡住的 Unity 进程"
                            },
                            {
                                "const": "先别动，我自己重启",
                                "title": "先别动，我自己重启",
                                "description": "你自己在任务管理器结束 Unity"
                            }
                        ]
                    },
                    "q0_custom": {
                        "type": "string",
                        "title": "编辑器卡死，怎么恢复 (other)",
                        "description": "Type your own answer"
                    }
                },
                "required": []
            }
        });
        let plan = classify_elicitation(&raw, false).unwrap();
        match plan {
            ElicitationPlan::Questions(q) => {
                assert_eq!(
                    q.specs.len(),
                    1,
                    "custom companion must not be a second question"
                );
                assert_eq!(q.tool_call_id.as_deref(), Some("call_question_1"));
                assert_eq!(q.specs[0].header, "编辑器卡死，怎么恢复");
                assert!(q.specs[0].question.contains("结束 Unity"));
                assert_eq!(q.specs[0].options.len(), 2);
                assert_eq!(q.specs[0].options[0].1, "我 kill 掉卡住的 Unity 进程");
                assert_eq!(q.fields[0].custom_id.as_deref(), Some("q0_custom"));
                assert!(q.fields[0].other_allowed);
                assert!(!q.fields[0].note_allowed);

                let picked = build_questions_response(
                    &q,
                    &json!([{
                        "question": q.specs[0].question,
                        "selected": ["你来重启（推荐）"]
                    }]),
                    false,
                );
                assert_eq!(picked["action"], "accept");
                assert_eq!(picked["content"]["q0"], "你来重启（推荐）");
                assert!(picked["content"].get("q0_custom").is_none());

                // 「其他」只写伴生字段：同时写主字段会让 Claude 重复答案、
                // 多选读成 "x, x"。
                let typed = build_questions_response(
                    &q,
                    &json!([{
                        "question": q.specs[0].question,
                        "selected": [],
                        "typed": "先等等，我自己看一眼"
                    }]),
                    false,
                );
                assert_eq!(typed["action"], "accept");
                assert!(typed["content"].get("q0").is_none());
                assert_eq!(typed["content"]["q0_custom"], "先等等，我自己看一眼");
            }
            other => panic!("expected questions, got {other:?}"),
        }
    }

    #[test]
    fn opencode_multiselect_any_of_items() {
        let raw = json!({
            "mode": "form",
            "message": "Questions",
            "requestedSchema": {
                "type": "object",
                "properties": {
                    "q0": {
                        "type": "array",
                        "title": "Pick files",
                        "description": "Which files should I touch?",
                        "items": {
                            "anyOf": [
                                { "const": "a.rs", "title": "a.rs" },
                                { "const": "b.rs", "title": "b.rs" }
                            ]
                        }
                    }
                }
            }
        });
        let plan = classify_elicitation(&raw, false).unwrap();
        match plan {
            ElicitationPlan::Questions(q) => {
                assert!(q.specs[0].multi_select);
                assert_eq!(q.specs[0].options.len(), 2);
                let resp = build_questions_response(
                    &q,
                    &json!([{ "question": "Which files should I touch?", "selected": ["a.rs", "b.rs"] }]),
                    false,
                );
                assert_eq!(resp["content"]["q0"], json!(["a.rs", "b.rs"]));
            }
            other => panic!("expected questions, got {other:?}"),
        }
    }

    #[test]
    fn url_mode_is_rejected() {
        let raw = json!({
            "mode": "url",
            "message": "Open this",
            "url": "https://example.com"
        });
        let err = classify_elicitation(&raw, false).unwrap_err();
        assert!(err.contains("url"));
    }

    /// Codex `isOther` question: `None of the above` is a sentinel, and the
    /// `_note` companion is a note channel (it used to become a second,
    /// mandatory question).
    #[test]
    fn codex_note_companion_is_not_a_question() {
        let raw = json!({
            "mode": "form",
            "message": "Codex needs your input to continue.",
            "requestedSchema": {
                "type": "object",
                "properties": {
                    "q0": {
                        "type": "string",
                        "title": "Proceed?",
                        "description": "What should I do?",
                        "_meta": { "codex": { "isOther": true, "isSecret": false } },
                        "oneOf": [
                            { "const": "A", "title": "Option A" },
                            {
                                "const": "None of the above",
                                "title": "None of the above",
                                "description": "Provide a different answer in the note field."
                            }
                        ]
                    },
                    "q0_note": {
                        "type": "string",
                        "title": "Additional answer or note",
                        "_meta": { "codex": { "questionId": "q0", "role": "user_note" } }
                    }
                }
            }
        });
        let plan = classify_elicitation(&raw, false).unwrap();
        match plan {
            ElicitationPlan::Questions(q) => {
                assert_eq!(q.specs.len(), 1, "note companion must not be a question");
                assert_eq!(q.specs[0].options.len(), 1, "sentinel must be hidden");
                assert_eq!(q.fields[0].note_id.as_deref(), Some("q0_note"));
                assert!(q.fields[0].other_allowed);
                assert!(q.fields[0].note_allowed);

                // 其他 → sentinel + note text (codex-acp's own mapping).
                let typed = build_questions_response(
                    &q,
                    &json!([{
                        "question": q.specs[0].question,
                        "selected": [],
                        "typed": "我自己来"
                    }]),
                    false,
                );
                assert_eq!(typed["content"]["q0"], "None of the above");
                assert_eq!(typed["content"]["q0_note"], "我自己来");

                // 选项 + 补充 → pick + note.
                let picked = build_questions_response(
                    &q,
                    &json!([{
                        "question": q.specs[0].question,
                        "selected": ["Option A"],
                        "note": "还有一件事"
                    }]),
                    false,
                );
                assert_eq!(picked["content"]["q0"], "A");
                assert_eq!(picked["content"]["q0_note"], "还有一件事");

                let ui = questions_for_ui(&q);
                assert_eq!(ui[0]["other"], json!(true));
                assert_eq!(ui[0]["note"], json!(true));
                assert_eq!(ui[0]["options"].as_array().unwrap().len(), 1);
                // Codex single-question message is generic — the field's own
                // description must win.
                assert_eq!(ui[0]["question"], "What should I do?");
            }
            other => panic!("expected questions, got {other:?}"),
        }
    }

    #[test]
    fn codex_is_other_false_hides_free_text() {
        let raw = json!({
            "mode": "form",
            "message": "Pick",
            "requestedSchema": {
                "type": "object",
                "properties": {
                    "q0": {
                        "type": "string",
                        "description": "Pick one",
                        "_meta": { "codex": { "isOther": false } },
                        "oneOf": [{ "const": "A", "title": "A" }]
                    }
                }
            }
        });
        match classify_elicitation(&raw, false).unwrap() {
            ElicitationPlan::Questions(q) => {
                assert!(!q.fields[0].other_allowed);
                assert!(!q.fields[0].note_allowed);
            }
            other => panic!("expected questions, got {other:?}"),
        }
    }

    /// Claude: one `_custom` field is the answer with no pick, and
    /// `annotations[q].notes` when a pick is present.
    #[test]
    fn claude_custom_is_other_or_note() {
        let raw = json!({
            "mode": "form",
            "message": "Which color?",
            "requestedSchema": {
                "type": "object",
                "properties": {
                    "question_0": {
                        "type": "string",
                        "title": "Color",
                        "oneOf": [
                            { "const": "Red", "title": "Red" },
                            { "const": "Blue", "title": "Blue" }
                        ]
                    },
                    "question_0_custom": {
                        "type": "string",
                        "title": "Other",
                        "description": "Type your own answer, or add a note to the option you chose above (optional)."
                    }
                }
            }
        });
        match classify_elicitation(&raw, true).unwrap() {
            ElicitationPlan::Questions(q) => {
                assert!(q.fields[0].other_allowed);
                assert!(q.fields[0].note_allowed);
                let qtext = &q.specs[0].question;

                let typed = build_questions_response(
                    &q,
                    &json!([{ "question": qtext, "selected": [], "typed": "Teal" }]),
                    false,
                );
                assert_eq!(typed["content"]["question_0"], "Teal");
                assert!(typed["content"].get("question_0_custom").is_none());

                let noted = build_questions_response(
                    &q,
                    &json!([{ "question": qtext, "selected": ["Red"], "note": "偏亮" }]),
                    false,
                );
                assert_eq!(noted["content"]["question_0"], "Red");
                assert_eq!(noted["content"]["question_0_custom"], "偏亮");

                let both = build_questions_response(
                    &q,
                    &json!([{
                        "question": qtext,
                        "selected": [],
                        "typed": "Teal",
                        "note": "偏亮"
                    }]),
                    false,
                );
                assert_eq!(both["content"]["question_0"], "Teal");
                assert_eq!(both["content"]["question_0_custom"], "偏亮");

                let ui = questions_for_ui(&q);
                assert_eq!(ui[0]["question"], "Which color?");
                assert_eq!(ui[0]["note"], json!(true));
            }
            other => panic!("expected questions, got {other:?}"),
        }
    }

    /// ZCode injects `__skip__` into single-select enums and pairs a `_other`
    /// free-text field with it; neither the sentinel nor the companion is a
    /// user-visible choice.
    #[test]
    fn zcode_skip_and_other_companion() {
        let raw = json!({
            "mode": "form",
            "message": "Pick",
            "requestedSchema": {
                "type": "object",
                "properties": {
                    "q_0": {
                        "type": "string",
                        "title": "Pick",
                        "description": "Which one?",
                        "oneOf": [
                            { "const": "a", "title": "A" },
                            { "const": "b", "title": "B" },
                            { "const": "__skip__", "title": "跳过此问题" }
                        ]
                    },
                    "q_0_other": {
                        "type": "string",
                        "title": "or type a custom value"
                    }
                }
            }
        });
        match classify_elicitation(&raw, false).unwrap() {
            ElicitationPlan::Questions(q) => {
                assert_eq!(q.specs.len(), 1);
                assert_eq!(q.specs[0].options.len(), 2, "skip sentinel must be hidden");
                assert!(q.fields[0].other_allowed);
                assert!(!q.fields[0].note_allowed);

                let typed = build_questions_response(
                    &q,
                    &json!([{
                        "question": q.specs[0].question,
                        "selected": [],
                        "typed": "custom"
                    }]),
                    false,
                );
                assert!(typed["content"].get("q_0").is_none());
                assert_eq!(typed["content"]["q_0_other"], "custom");
            }
            other => panic!("expected questions, got {other:?}"),
        }
    }
}
