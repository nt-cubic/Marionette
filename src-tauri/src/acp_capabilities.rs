//! ACP capability parsing: session/new + set_config_option echo -> CapabilitySnapshot.
//!
//! Pure functions cut out of acp.rs (the ~900-line "Capability parsing" section).
//! Child module of acp: types and helpers it shares with the service live in
//! `super` (CapabilitySnapshot, ModeDef, ModelDef, KnobClaims, …).

use super::*;
use agent_client_protocol_schema::v1 as acp_schema;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

// ─── Capability parsing from session/new response ──────────────────────────

pub(super) fn parse_session_capabilities(session_response: &Value) -> CapabilitySnapshot {
    parse_session_capabilities_after(session_response, None)
}

/// Parse any session-state response: `session/new`, or the `configOptions` echo
/// a `session/set_config_option` returns.
///
/// The echo re-sends only the options it changed, so a knob `session/new`
/// assigned stays assigned. Without that, DeepSeek Harness's `sandbox`
/// (category "mode") takes the plan/build knob on the first model change — the
/// pre-v2 `modes` field it loses to is simply absent from the echo.
pub(super) fn parse_session_capabilities_after(
    session_response: &Value,
    previous: Option<&CapabilitySnapshot>,
) -> CapabilitySnapshot {
    // The pre-v2 `modes` field is the session-mode list, and it keeps a
    // *different* `category: "mode"` option from replacing it.
    // Exception: @openma/deepseek-harness-acp puts *sandbox levels* in
    // `modes.availableModes` (read-only / workspace-write / full access) and
    // the real agent presets (standard / ptc / minimal / cordis) under
    // configOption id `agent`. Sandbox-shaped legacy lists must not claim the
    // mode knob, or the Composer Mode chip shows permissions instead of
    // Standard / Minimal / Creator.
    let legacy_modes = legacy_modes_from_json(session_response);
    let legacy_is_sandbox = legacy_modes
        .as_ref()
        .is_some_and(|(list, _)| looks_like_sandbox_modes(list));
    let held = KnobClaims {
        model: previous.is_some_and(|caps| caps.model_config_id.is_some()),
        effort: previous.is_some_and(|caps| caps.effort_config_id.is_some()),
        mode: (!legacy_is_sandbox && legacy_modes.is_some())
            || previous.is_some_and(|caps| !caps.modes.is_empty()),
    };
    let mut caps = if let Ok(response) =
        serde_json::from_value::<acp_schema::NewSessionResponse>(session_response.clone())
    {
        // Typed ACP NewSessionResponse has no `models` field — Grok (and other
        // pre-v2 agents) put the list under top-level `models.currentModelId` /
        // `availableModels`. Ignoring that left current_model=None and made
        // effort changes fail with "no current model known for this session".
        parse_capabilities_from_typed(response, legacy_modes, held)
    } else {
        parse_capabilities_fallback(session_response, legacy_modes, held)
    };
    merge_legacy_models_field(session_response, &mut caps);
    normalize_combined_model_ids(&mut caps);
    caps
}

/// Codex ACP 2.x encodes effort in the model id: `gpt-5.6-luna[xhigh]`.
/// Grok / Claude ids never use this form, so a miss is a no-op.
pub(super) fn split_combined_model_id(id: &str) -> (String, Option<String>) {
    let id = id.trim();
    if let Some(open) = id.rfind('[') {
        if id.ends_with(']') && open > 0 {
            let base = &id[..open];
            let effort = &id[open + 1..id.len() - 1];
            if !base.is_empty()
                && !effort.is_empty()
                && effort
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            {
                return (base.to_string(), Some(effort.to_string()));
            }
        }
    }
    (id.to_string(), None)
}

pub(super) fn combine_model_effort_id(model_id: &str, effort: Option<&str>) -> String {
    let (base, _) = split_combined_model_id(model_id);
    match effort.map(str::trim).filter(|s| !s.is_empty()) {
        Some(e) => format!("{base}[{e}]"),
        None => base,
    }
}

fn pretty_codex_effort_label(id: &str) -> String {
    match id.to_ascii_lowercase().as_str() {
        "none" => "None".into(),
        "minimal" | "min" => "Minimal".into(),
        "low" => "Low".into(),
        "medium" | "mid" | "default" => "Medium".into(),
        "high" => "High".into(),
        "xhigh" | "extra-high" | "extra_high" => "Extra High".into(),
        "max" => "Max".into(),
        _ => id.to_string(),
    }
}

fn strip_effort_decoration(label: &str, effort: Option<&str>) -> String {
    let mut s = label.trim().to_string();
    if s.ends_with(']') {
        if let Some(open) = s.rfind('[') {
            if open > 0 {
                s.truncate(open);
                s = s.trim_end().to_string();
            }
        }
    }
    if let Some(effort) = effort {
        let suffix = format!(" ({effort})");
        if s.len() > suffix.len()
            && s.as_bytes()[s.len() - suffix.len()..].eq_ignore_ascii_case(suffix.as_bytes())
        {
            s.truncate(s.len() - suffix.len());
            s = s.trim_end().to_string();
        }
    }
    s
}

/// Peel `model[effort]` off current_model / the catalog so Composer sees a
/// base model plus an Effort chip. A single bracketed current id is only the
/// current level — not a menu — so we leave `effort_options` empty there and
/// let the Codex supplement fill the list.
pub(super) fn normalize_combined_model_ids(caps: &mut CapabilitySnapshot) {
    let mut saw_combined = false;
    if let Some(current) = caps.current_model.clone() {
        let (base, effort) = split_combined_model_id(&current);
        if let Some(effort) = effort {
            saw_combined = true;
            caps.current_model = Some(base);
            // The suffix on the live model id is the wire's current level.
            caps.current_effort_id = Some(effort);
        }
    }

    let mut seen = HashSet::new();
    let mut next_models = Vec::new();
    let mut from_suffix: Vec<ModeDef> = Vec::new();
    for model in caps.models.drain(..) {
        let (base, effort) = split_combined_model_id(&model.id);
        if let Some(ref e) = effort {
            saw_combined = true;
            if !from_suffix.iter().any(|o| o.id == *e) {
                from_suffix.push(ModeDef {
                    id: e.clone(),
                    label: pretty_codex_effort_label(e),
                    description: None,
                });
            }
        }
        if !seen.insert(base.clone()) {
            continue;
        }
        let label = if effort.is_some() {
            let stripped = strip_effort_decoration(&model.label, effort.as_deref());
            if stripped.is_empty() || stripped == model.id {
                base.clone()
            } else {
                stripped
            }
        } else {
            model.label
        };
        next_models.push(ModelDef {
            id: base,
            label,
            description: model.description,
        });
    }
    caps.models = next_models;
    if caps.effort_options.is_empty() && from_suffix.len() >= 2 {
        caps.effort_options = from_suffix;
    }
    if saw_combined {
        caps.effort_in_model_id = true;
    }
    if let Some(current) = &caps.current_model {
        if !caps.models.iter().any(|m| &m.id == current) {
            caps.models.insert(
                0,
                ModelDef {
                    id: current.clone(),
                    label: current.clone(),
                    description: None,
                },
            );
        }
    }
}

/// Sandbox / file-permission levels DSH (and Codex-shaped agents) advertise.
pub(super) fn looks_like_sandbox_modes(modes: &[ModeDef]) -> bool {
    if modes.is_empty() {
        return false;
    }
    modes.iter().all(|m| {
        matches!(
            m.id.trim().to_ascii_lowercase().as_str(),
            "read-only"
                | "readonly"
                | "workspace-write"
                | "workspace_write"
                | "danger-full-access"
                | "danger_full_access"
                | "full-access"
                | "full_access"
        )
    })
}

/// Pretty labels for DSH agent-preset ids when the wire name is the raw id.
pub(super) fn pretty_agent_preset_label(id: &str, name: &str) -> String {
    let trimmed = name.trim();
    if !trimmed.is_empty() && !trimmed.eq_ignore_ascii_case(id) {
        return trimmed.to_string();
    }
    match id.trim().to_ascii_lowercase().as_str() {
        "standard" => "Standard".to_string(),
        "ptc" => "PTC".to_string(),
        "minimal" => "Minimal".to_string(),
        "cordis" | "creator" | "creation" => "Creator".to_string(),
        _ => {
            if trimmed.is_empty() {
                id.to_string()
            } else {
                trimmed.to_string()
            }
        }
    }
}

/// The pre-v2 `modes` field of `session/new`: `{ availableModes, currentModeId }`.
///
/// These ids are the ones `session/set_mode` takes; nothing here names a
/// `session/set_config_option` id.
pub(super) fn legacy_modes_from_json(session_response: &Value) -> Option<(Vec<ModeDef>, Option<String>)> {
    let modes = session_response.get("modes")?;
    let available = modes
        .get("availableModes")
        .or_else(|| modes.get("available_modes"))
        .and_then(Value::as_array);
    let list = available
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| {
                    let id = entry.get("id").and_then(Value::as_str)?;
                    let name = entry
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or(id)
                        .to_string();
                    Some(ModeDef {
                        id: id.to_string(),
                        label: name,
                        description: entry
                            .get("description")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let current = modes
        .get("currentModeId")
        .or_else(|| modes.get("current_mode_id"))
        .and_then(Value::as_str)
        .map(str::to_string);
    Some((list, current))
}

/// Which Composer knob a `configOptions` entry drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ConfigKnob {
    Model,
    Mode,
    Effort,
    /// File permission / sandbox level. Never a mode: the two are separate axes
    /// (a session can be in plan mode with full file access).
    Permission,
}

/// Knobs already spoken for: the model/mode/effort control has been found.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct KnobClaims {
    pub(super) model: bool,
    pub(super) mode: bool,
    pub(super) effort: bool,
}

/// The knob an option *is*, from its id.
///
/// ACP ids are the stable identity (`model`, `mode`, `thought_level`); the
/// category is a UX hint that "MUST NOT be required for correctness". Effort
/// words are unambiguous, so a compound id (`reasoning_effort_level`) still
/// lands on the effort knob — while `fast-mode` (a `model_config` toggle) and
/// `collaboration_mode` (Codex's own plan switch) stay out of the mode knob.
/// `sandbox` is a knob of its own, not a mode, however it is categorised.
pub(super) fn knob_by_id(id: &str) -> Option<ConfigKnob> {
    let lower = id.trim().to_ascii_lowercase();
    match lower.as_str() {
        "model" | "model_id" | "modelid" => return Some(ConfigKnob::Model),
        // DSH agent presets (standard / ptc / minimal / cordis) — the Mode chip.
        "agent" | "agent_preset" | "agent-preset" | "preset" => {
            return Some(ConfigKnob::Mode);
        }
        "mode" | "mode_id" | "modeid" | "approval-policy" | "approvalpolicy" => {
            return Some(ConfigKnob::Mode);
        }
        "sandbox" | "sandbox_mode" | "sandbox-mode" | "permission_mode" | "permission-mode" => {
            return Some(ConfigKnob::Permission)
        }
        _ => {}
    }
    if lower.contains("effort")
        || lower.contains("reason")
        || lower.contains("think")
        || lower.contains("thought")
    {
        return Some(ConfigKnob::Effort);
    }
    None
}

/// Reclassify a Mode-claimed option as Permission when its values are sandbox
/// levels. openma dsh-acp names the permission select `id: "mode"` /
/// `name: "Permissions"` while the real presets live under `id: "agent"`.
pub(super) fn reclassify_mode_if_sandbox(
    id: &str,
    knob: Option<ConfigKnob>,
    option_ids: &[String],
) -> Option<ConfigKnob> {
    if knob != Some(ConfigKnob::Mode) {
        return knob;
    }
    let lower = id.trim().to_ascii_lowercase();
    let sandbox_shaped = !option_ids.is_empty()
        && option_ids.iter().all(|oid| {
            matches!(
                oid.trim().to_ascii_lowercase().as_str(),
                "read-only"
                    | "readonly"
                    | "workspace-write"
                    | "workspace_write"
                    | "danger-full-access"
                    | "danger_full_access"
                    | "full-access"
                    | "full_access"
            )
        });
    if sandbox_shaped
        && matches!(
            lower.as_str(),
            "mode" | "mode_id" | "modeid" | "sandbox" | "sandbox_mode" | "sandbox-mode"
        )
    {
        return Some(ConfigKnob::Permission);
    }
    knob
}

/// The knob an option's category claims when its id names none.
///
/// `model_config` is deliberately absent: it labels a parameter *of* the model
/// (Codex's `fast-mode`), not the model catalog.
pub(super) fn knob_by_category(category: Option<&str>) -> Option<ConfigKnob> {
    match category?.trim().to_ascii_lowercase().as_str() {
        "model" => Some(ConfigKnob::Model),
        "mode" => Some(ConfigKnob::Mode),
        "thought_level" => Some(ConfigKnob::Effort),
        _ => None,
    }
}

/// Classify one `configOptions` entry.
///
/// The id decides whenever it names a knob; the category may only claim a knob
/// that no id named and that nothing else holds. DeepSeek Harness is why: it
/// ships its 推理档位 select as `category: "model"` (last-wins per category used
/// to hand the Composer 关闭/低/高/最高 as the model list) and its 文件权限 select
/// as `category: "mode"` (which replaced the plan/build modes).
///
/// No category ever claims `Permission`: "mode" on a sandbox select means "this
/// is a session setting", not "this is the collaboration mode", and the two axes
/// must not collapse into one control.
pub(super) fn config_option_knob(
    id: &str,
    category: Option<&str>,
    held: KnobClaims,
) -> Option<ConfigKnob> {
    if let Some(knob) = knob_by_id(id) {
        return Some(knob);
    }
    match knob_by_category(category)? {
        ConfigKnob::Model if !held.model => Some(ConfigKnob::Model),
        ConfigKnob::Mode if !held.mode => Some(ConfigKnob::Mode),
        ConfigKnob::Effort if !held.effort => Some(ConfigKnob::Effort),
        _ => None,
    }
}

/// Category name as it appears on the wire, for the string-based classifier.
pub(super) fn category_name(category: &acp_schema::SessionConfigOptionCategory) -> Option<String> {
    match serde_json::to_value(category) {
        Ok(Value::String(name)) => Some(name),
        _ => None,
    }
}

/// (value, name, optional description) from a typed select's options.
pub(super) fn extract_typed_options(
    options: &acp_schema::SessionConfigSelectOptions,
) -> Vec<(String, String, Option<String>)> {
    let one = |o: &acp_schema::SessionConfigSelectOption| {
        (
            o.value.0.to_string(),
            o.name.clone(),
            o.description.clone(),
        )
    };
    match options {
        acp_schema::SessionConfigSelectOptions::Ungrouped(list) => list.iter().map(one).collect(),
        acp_schema::SessionConfigSelectOptions::Grouped(groups) => groups
            .iter()
            .flat_map(|g| &g.options)
            .map(one)
            .collect(),
        _ => vec![],
    }
}

/// (value, name, optional description) from raw JSON select options, flattening
/// the `{ group, name, options: [...] }` headers the wire format allows.
pub(super) fn json_select_options(items: &[Value]) -> Vec<(String, String, Option<String>)> {
    let mut out = Vec::new();
    for item in items {
        if let Some(group) = item.get("options").and_then(Value::as_array) {
            out.extend(json_select_options(group));
            continue;
        }
        let Some(value) = json_scalar_string(item.get("value")) else {
            continue;
        };
        let name = item
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(value.as_str())
            .to_string();
        let description = item
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_string);
        out.push((value, name, description));
    }
    out
}

/// A config option's value (or an option value) as text: the wire carries
/// strings, numbers and bools for the same field.
pub(super) fn json_scalar_string(value: Option<&Value>) -> Option<String> {
    let value = value?;
    value
        .as_str()
        .map(str::to_string)
        .or_else(|| value.as_f64().map(|n| n.to_string()))
        .or_else(|| value.as_bool().map(|b| b.to_string()))
}

/// Debug hook: the raw catalog an agent advertised, so a mislabelled
/// `configOptions` entry can be told apart from a missing one.
pub(super) fn log_raw_model_options(path: &str, options: &[(String, String, Option<String>)]) {
    for (id, name, desc) in options {
        crate::debug_log::append(
            "acp",
            "debug",
            "",
            "raw_model_option",
            Some(&format!(
                "path={path} id={id} name={name} desc={:?}",
                desc.as_deref().unwrap_or("(none)")
            )),
        );
    }
}

/// A `session/set_config_option` echo is a *partial* session state: it speaks
/// about the options it changed and omits everything else. Carry forward what it
/// stayed silent about, or the Composer loses controls it still needs — the
/// model catalog at session/new, and (DeepSeek Harness, whose echo carries
/// `configOptions` alone) the plan/build mode list after any model change.
pub(super) fn carry_forward_unsaid(next: &mut CapabilitySnapshot, prev: &CapabilitySnapshot) {
    if next.models.is_empty() {
        next.models = prev.models.clone();
    }
    if next.model_context_sizes.is_empty() {
        next.model_context_sizes = prev.model_context_sizes.clone();
    }
    if next.model_effort_menus.is_empty() {
        next.model_effort_menus = prev.model_effort_menus.clone();
    }
    if next.model_default_efforts.is_empty() {
        next.model_default_efforts = prev.model_default_efforts.clone();
    }
    // The effort-reject latch is learned from a failed write, never from a
    // response — no echo mentions it, so it always carries forward.
    for model in &prev.effort_reject_models {
        if !next.effort_reject_models.contains(model) {
            next.effort_reject_models.push(model.clone());
        }
    }
    if prev.effort_in_model_id {
        next.effort_in_model_id = true;
    }
    if next.effort_options.is_empty() && next.current_effort_id.is_none() {
        next.effort_options = prev.effort_options.clone();
        next.current_effort_id = prev.current_effort_id.clone();
        next.current_effort = prev.current_effort;
        next.thinking_effort = prev.thinking_effort.clone();
        if next.effort_config_id.is_none() {
            next.effort_config_id = prev.effort_config_id.clone();
        }
    }
    if next.modes.is_empty() && next.current_mode.is_none() {
        next.modes = prev.modes.clone();
        next.current_mode = prev.current_mode.clone();
    }
    // File permission is a config option, so an echo that mentions other options
    // may still have left it out — an empty list is "not mentioned", not
    // "the agent removed it".
    if next.permission_options.is_empty() {
        next.permission_options = prev.permission_options.clone();
        next.permission_config_id = prev.permission_config_id.clone();
        next.permission_label = prev.permission_label.clone();
        if next.current_permission.is_none() {
            next.current_permission = prev.current_permission.clone();
        }
    }
}

/// Grok / pre-v2 ACP: `session/new` returns
/// `{ models: { currentModelId, availableModels: [{ modelId, name, _meta }] } }`.
pub(super) fn merge_legacy_models_field(session_response: &Value, caps: &mut CapabilitySnapshot) {
    let Some(models_obj) = session_response.get("models") else {
        return;
    };

    // Ceilings are published only in this catalog — collect them for every
    // model, not just the current one, so the meter can follow a later switch.
    if caps.model_context_sizes.is_empty() {
        caps.model_context_sizes = advertised_model_context_sizes(session_response);
    }
    if caps.model_effort_menus.is_empty() {
        let (menus, defaults) = advertised_model_effort_catalog(session_response);
        caps.model_effort_menus = menus;
        if caps.model_default_efforts.is_empty() {
            caps.model_default_efforts = defaults;
        }
    }

    if caps.current_model.is_none() {
        if let Some(id) = models_obj
            .get("currentModelId")
            .or_else(|| models_obj.get("current_model_id"))
            .and_then(Value::as_str)
        {
            caps.current_model = Some(id.to_string());
        }
    }

    let available = models_obj
        .get("availableModels")
        .or_else(|| models_obj.get("available_models"))
        .and_then(Value::as_array);

    if let Some(list) = available {
        if caps.models.is_empty() {
            for entry in list {
                let id = entry
                    .get("modelId")
                    .or_else(|| entry.get("model_id"))
                    .or_else(|| entry.get("id"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if id.is_empty() {
                    continue;
                }
                let name = entry
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or(id.as_str())
                    .to_string();
                let description = entry
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                caps.models.push(ModelDef {
                    id: id.clone(),
                    label: model_display_label(&name, description.as_deref()),
                    description,
                });
            }
        }

        // Effort levels live on each model's `_meta` for Grok. Composer still
        // reads the session-level list, so copy the current model's menu there.
        let current = caps.current_model.clone();
        let pick = list.iter().find(|m| {
            let id = m
                .get("modelId")
                .or_else(|| m.get("model_id"))
                .or_else(|| m.get("id"))
                .and_then(Value::as_str);
            current.as_deref().zip(id).is_some_and(|(c, i)| c == i)
        }).or_else(|| list.first());

        if caps.effort_options.is_empty() {
            let menu_key = current
                .clone()
                .or_else(|| pick.and_then(model_entry_id).map(str::to_string));
            if let Some(key) = menu_key {
                if let Some(menu) = caps.model_effort_menus.get(&key) {
                    caps.effort_options = menu.clone();
                }
            }
        }

        if let Some(entry) = pick {
            if caps.current_effort_id.is_none() {
                if let Some(effort) = entry
                    .get("_meta")
                    .and_then(|meta| {
                        meta.get("reasoningEffort")
                            .or_else(|| meta.get("reasoning_effort"))
                    })
                    .and_then(Value::as_str)
                {
                    caps.current_effort_id = Some(effort.to_string());
                }
            }
        }
    }

    // Ensure current is in the list for the UI / set_model path.
    if let Some(current) = &caps.current_model {
        if !caps.models.iter().any(|m| &m.id == current) {
            caps.models.insert(
                0,
                ModelDef {
                    id: current.clone(),
                    label: current.clone(),
                    description: None,
                },
            );
        }
    }
}

pub(super) fn parse_capabilities_from_typed(
    response: acp_schema::NewSessionResponse,
    legacy_modes: Option<(Vec<ModeDef>, Option<String>)>,
    held: KnobClaims,
) -> CapabilitySnapshot {
    use acp_schema::SessionConfigKind as Kind;

    let mut modes: Vec<ModeDef> = Vec::new();
    let mut models: Vec<ModelDef> = Vec::new();
    let mut effort_options: Vec<ModeDef> = Vec::new();
    let mut current_mode: Option<String> = None;
    let mut current_model: Option<String> = None;
    let mut current_effort: Option<f64> = None;
    let mut current_effort_id: Option<String> = None;
    let mut thinking_effort: Option<ThinkingEffort> = None;
    let mut model_config_id: Option<String> = None;
    let mut mode_config_id: Option<String> = None;
    let mut effort_config_id: Option<String> = None;
    let mut permission_options: Vec<ModeDef> = Vec::new();
    let mut permission_config_id: Option<String> = None;
    let mut current_permission: Option<String> = None;
    let mut permission_label: Option<String> = None;

    let mut held = held;
    if let Some((legacy, current)) = legacy_modes {
        if !legacy.is_empty() {
            if looks_like_sandbox_modes(&legacy) {
                // openma dsh-acp: sandbox levels in the pre-v2 modes field.
                if permission_options.is_empty() {
                    permission_options = legacy;
                    current_permission = current;
                }
            } else {
                modes = legacy;
                current_mode = current;
            }
        } else {
            current_mode = current;
        }
    }

    if let Some(config_options) = &response.config_options {
        for opt in config_options {
            let id_str = opt.id.0.to_string();
            let category = opt.category.as_ref().and_then(category_name);
            let select_ids: Vec<String> = match &opt.kind {
                Kind::Select(select) => extract_typed_options(&select.options)
                    .into_iter()
                    .map(|(id, _, _)| id)
                    .collect(),
                _ => Vec::new(),
            };
            let knob = reclassify_mode_if_sandbox(
                &id_str,
                config_option_knob(&id_str, category.as_deref(), held),
                &select_ids,
            );
            match knob {
                Some(ConfigKnob::Mode) => {
                    held.mode = true;
                    mode_config_id = Some(id_str.clone());
                    if let Kind::Select(select) = &opt.kind {
                        let extracted = extract_typed_options(&select.options);
                        if !extracted.is_empty() {
                            let agent_preset = matches!(
                                id_str.trim().to_ascii_lowercase().as_str(),
                                "agent" | "agent_preset" | "agent-preset" | "preset"
                            );
                            modes = extracted
                                .into_iter()
                                .map(|(id, name, description)| ModeDef {
                                    label: if agent_preset {
                                        pretty_agent_preset_label(&id, &name)
                                    } else {
                                        name
                                    },
                                    id,
                                    description,
                                })
                                .collect();
                        }
                        current_mode = Some(select.current_value.0.to_string());
                    }
                }
                Some(ConfigKnob::Model) => {
                    held.model = true;
                    model_config_id = Some(id_str.clone());
                    if let Kind::Select(select) = &opt.kind {
                        let raw = extract_typed_options(&select.options);
                        log_raw_model_options("typed", &raw);
                        models = raw
                            .into_iter()
                            .map(|(id, name, desc)| ModelDef {
                                id,
                                label: model_display_label(&name, desc.as_deref()),
                                description: desc,
                            })
                            .collect();
                        current_model = Some(select.current_value.0.to_string());
                    }
                }
                Some(ConfigKnob::Effort) => {
                    held.effort = true;
                    effort_config_id = Some(id_str.clone());
                    if let Kind::Select(select) = &opt.kind {
                        let extracted = extract_typed_options(&select.options);
                        effort_options = extracted
                            .iter()
                            .map(|(id, name, description)| ModeDef {
                                id: id.clone(),
                                label: name.clone(),
                                description: description.clone(),
                            })
                            .collect();
                        let vals: Vec<f64> = extracted
                            .iter()
                            .filter_map(|(v, _, _)| v.parse::<f64>().ok())
                            .collect();
                        if !vals.is_empty() {
                            let min_val = vals.iter().cloned().fold(f64::INFINITY, f64::min);
                            let max_val = vals.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
                            thinking_effort = Some(ThinkingEffort {
                                min: min_val,
                                max: max_val,
                                default: min_val + (max_val - min_val) * 0.5,
                            });
                        }
                        let cur = select.current_value.0.to_string();
                        current_effort_id = Some(cur.clone());
                        current_effort = cur.parse::<f64>().ok();
                    }
                }
                Some(ConfigKnob::Permission) => {
                    permission_config_id = Some(id_str.clone());
                    permission_label = Some(opt.name.clone());
                    if let Kind::Select(select) = &opt.kind {
                        permission_options = extract_typed_options(&select.options)
                            .into_iter()
                            .map(|(id, name, description)| ModeDef {
                                id,
                                label: name,
                                description,
                            })
                            .collect();
                        current_permission = Some(select.current_value.0.to_string());
                    }
                }
                None => {}
            }
        }
    }

    // Ensure current model appears even if not in options list
    if let Some(current) = &current_model {
        if !models.iter().any(|m| &m.id == current) {
            models.insert(
                0,
                ModelDef {
                    id: current.clone(),
                    label: current.clone(),
                    description: None,
                },
            );
        }
    }

    CapabilitySnapshot {
        modes,
        models,
        thinking_effort,
        effort_options,
        supports_cancel: true,
        current_mode,
        current_model,
        current_effort,
        current_effort_id,
        model_config_id,
        mode_config_id,
        effort_config_id,
        permission_options,
        permission_config_id,
        current_permission,
        permission_label,
        prompt_image: true,
        model_context_sizes: HashMap::new(),
        model_effort_menus: HashMap::new(),
        model_default_efforts: HashMap::new(),
        effort_reject_models: Vec::new(),
        effort_in_model_id: false,
    }
}

pub(super) fn parse_capabilities_fallback(
    session_response: &Value,
    legacy_modes: Option<(Vec<ModeDef>, Option<String>)>,
    held: KnobClaims,
) -> CapabilitySnapshot {
    let config_options = session_response
        .get("configOptions")
        .and_then(|c| c.as_array());

    let mut modes: Vec<ModeDef> = Vec::new();
    let mut models: Vec<ModelDef> = Vec::new();
    let mut effort_options: Vec<ModeDef> = Vec::new();
    let mut thinking_effort: Option<ThinkingEffort> = None;
    let mut current_mode: Option<String> = None;
    let mut current_model: Option<String> = None;
    let mut current_effort: Option<f64> = None;
    let mut current_effort_id: Option<String> = None;
    let mut model_config_id: Option<String> = None;
    let mut mode_config_id: Option<String> = None;
    let mut effort_config_id: Option<String> = None;
    let mut permission_options: Vec<ModeDef> = Vec::new();
    let mut permission_config_id: Option<String> = None;
    let mut current_permission: Option<String> = None;
    let mut permission_label: Option<String> = None;

    let mut held = held;
    if let Some((legacy, current)) = legacy_modes {
        if !legacy.is_empty() {
            if looks_like_sandbox_modes(&legacy) {
                if permission_options.is_empty() {
                    permission_options = legacy;
                    current_permission = current;
                }
            } else {
                modes = legacy;
                current_mode = current;
            }
        } else {
            current_mode = current;
        }
    }

    if let Some(options) = config_options {
        for opt in options {
            let id = opt.get("id").and_then(Value::as_str).unwrap_or("").to_string();
            let category = opt
                .get("category")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let current_value = json_scalar_string(opt.get("currentValue"));

            // (value, name, description)
            let select_options = opt
                .get("options")
                .and_then(|o| o.as_array())
                .map(|arr| json_select_options(arr))
                .unwrap_or_default();
            let select_ids: Vec<String> = select_options.iter().map(|(oid, _, _)| oid.clone()).collect();
            let knob = reclassify_mode_if_sandbox(
                &id,
                config_option_knob(&id, Some(category.as_str()), held),
                &select_ids,
            );

            match knob {
                Some(ConfigKnob::Mode) => {
                    held.mode = true;
                    let agent_preset = matches!(
                        id.trim().to_ascii_lowercase().as_str(),
                        "agent" | "agent_preset" | "agent-preset" | "preset"
                    );
                    mode_config_id = Some(id);
                    if !select_options.is_empty() {
                        modes = select_options
                            .into_iter()
                            .map(|(oid, name, description)| ModeDef {
                                label: if agent_preset {
                                    pretty_agent_preset_label(&oid, &name)
                                } else {
                                    name
                                },
                                id: oid,
                                description,
                            })
                            .collect();
                    }
                    current_mode = current_value;
                }
                Some(ConfigKnob::Model) => {
                    held.model = true;
                    model_config_id = Some(id);
                    if !select_options.is_empty() {
                        log_raw_model_options("fallback", &select_options);
                        models = select_options
                            .into_iter()
                            .map(|(id, name, desc)| ModelDef {
                                id,
                                label: model_display_label(&name, desc.as_deref()),
                                description: desc,
                            })
                            .collect();
                    }
                    current_model = current_value;
                }
                Some(ConfigKnob::Effort) => {
                    held.effort = true;
                    effort_config_id = Some(id);
                    if !select_options.is_empty() {
                        effort_options = select_options
                            .iter()
                            .map(|(id, name, description)| ModeDef {
                                id: id.clone(),
                                label: name.clone(),
                                description: description.clone(),
                            })
                            .collect();
                        let vals: Vec<f64> = select_options
                            .iter()
                            .filter_map(|(v, _, _)| v.parse::<f64>().ok())
                            .collect();
                        if !vals.is_empty() {
                            let min_val = vals.iter().cloned().fold(f64::INFINITY, f64::min);
                            let max_val = vals.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
                            thinking_effort = Some(ThinkingEffort {
                                min: min_val,
                                max: max_val,
                                default: min_val + (max_val - min_val) * 0.5,
                            });
                        }
                    }
                    current_effort_id = current_value.clone();
                    current_effort = current_value.and_then(|v| v.parse().ok());
                }
                Some(ConfigKnob::Permission) => {
                    permission_config_id = Some(id);
                    permission_label = opt
                        .get("name")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    if !select_options.is_empty() {
                        permission_options = select_options
                            .into_iter()
                            .map(|(id, name, description)| ModeDef {
                                id,
                                label: name,
                                description,
                            })
                            .collect();
                    }
                    current_permission = current_value;
                }
                None => {}
            }
        }
    }

    if let Some(current) = &current_model {
        if !models.iter().any(|m| &m.id == current) {
            models.insert(
                0,
                ModelDef {
                    id: current.clone(),
                    label: current.clone(),
                    description: None,
                },
            );
        }
    }

    CapabilitySnapshot {
        modes,
        models,
        thinking_effort,
        effort_options,
        supports_cancel: true,
        current_mode,
        current_model,
        current_effort,
        current_effort_id,
        model_config_id,
        mode_config_id,
        effort_config_id,
        permission_options,
        permission_config_id,
        current_permission,
        permission_label,
        prompt_image: true,
        model_context_sizes: HashMap::new(),
        model_effort_menus: HashMap::new(),
        model_default_efforts: HashMap::new(),
        effort_reject_models: Vec::new(),
        effort_in_model_id: false,
    }
}
