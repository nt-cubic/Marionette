//! Orb assistant scenario buttons — one-click presets (docs/plan-orb-assistant.md §4 档④).
//!
//! Stored as JSON under the Marionette global dir. The struct mirrors the
//! frontend `Scenario` type (src/lib/scenarios.ts) exactly: camelCase wire
//! names, optional fields omitted. A missing file is seeded with the two
//! factory presets; the frontend re-merges the builtins on top of whatever
//! is stored, so the file may also carry user-edited builtin copies.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Scenario {
    /// Stable id — builtins use the reserved `builtin-` prefix.
    pub id: String,
    pub name: String,
    pub emoji: String,
    /// "capture" | "prompt".
    pub kind: String,
    /// Undefined = app default agent for the target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// Optional per-scenario chips, applied like Telegram /new wizard prefs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode_id: Option<String>,
    /// Template with a `{content}` placeholder; empty = frontend default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_template: Option<String>,
    /// "inbox" | "newChat".
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub builtin: Option<bool>,
}

fn scenarios_path() -> Result<PathBuf, String> {
    Ok(crate::app_paths::global_dir()?.join("scenarios.json"))
}

/// The two factory presets. `promptTemplate` stays empty here — the frontend
/// supplies the default organizing prompt (DEFAULT_CAPTURE_TEMPLATE).
fn builtin_scenarios() -> Vec<Scenario> {
    fn s(id: &str, name: &str, emoji: &str, kind: &str, target: &str) -> Scenario {
        Scenario {
            id: id.into(),
            name: name.into(),
            emoji: emoji.into(),
            kind: kind.into(),
            agent_id: None,
            model_id: None,
            mode_id: None,
            prompt_template: Some(String::new()),
            target: target.into(),
            builtin: Some(true),
        }
    }
    vec![
        s("builtin-capture", "记录收藏", "📥", "capture", "inbox"),
        s("builtin-new-chat", "新对话", "✨", "prompt", "newChat"),
    ]
}

/// Load the stored list. A missing file is seeded with the builtins (persisted
/// best-effort so the file exists for user edits afterwards); an empty file
/// falls back to the builtins without writing.
pub fn list() -> Result<Vec<Scenario>, String> {
    let path = scenarios_path()?;
    if !path.exists() {
        let seeded = builtin_scenarios();
        let _ = save_all(&seeded);
        return Ok(seeded);
    }
    let text = fs::read_to_string(&path)
        .map_err(|e| format!("Read scenarios.json failed: {e}"))?;
    if text.trim().is_empty() {
        return Ok(builtin_scenarios());
    }
    serde_json::from_str(&text).map_err(|e| format!("Parse scenarios.json failed: {e}"))
}

/// Replace the whole list. Ids are trimmed and must be non-empty; duplicates
/// are dropped (first occurrence wins). Returns the list as stored.
pub fn save(scenarios: Vec<Scenario>) -> Result<Vec<Scenario>, String> {
    let normalized = normalize(scenarios)?;
    save_all(&normalized)?;
    Ok(normalized)
}

/// Validation + dedupe half of [`save`] — pure, so tests do not touch the
/// user's real `~/.marionette/scenarios.json`.
fn normalize(mut scenarios: Vec<Scenario>) -> Result<Vec<Scenario>, String> {
    for scenario in &mut scenarios {
        scenario.id = scenario.id.trim().to_string();
    }
    if scenarios.iter().any(|s| s.id.is_empty()) {
        return Err("scenario id must not be empty".into());
    }
    let mut seen = std::collections::HashSet::new();
    scenarios.retain(|s| seen.insert(s.id.clone()));
    Ok(scenarios)
}

fn save_all(list: &[Scenario]) -> Result<(), String> {
    let path = scenarios_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Create storage dir failed: {e}"))?;
    }
    let text = serde_json::to_string_pretty(list)
        .map_err(|e| format!("Serialize scenarios failed: {e}"))?;
    fs::write(&path, text).map_err(|e| format!("Write scenarios.json failed: {e}"))
}

#[cfg(test)]
mod tests {
    use super::{builtin_scenarios, normalize};

    #[test]
    fn builtins_match_the_frontend_contract() {
        let builtins = builtin_scenarios();
        assert_eq!(builtins.len(), 2);
        assert_eq!(builtins[0].id, "builtin-capture");
        assert_eq!(builtins[0].name, "记录收藏");
        assert_eq!(builtins[0].emoji, "📥");
        assert_eq!(builtins[0].kind, "capture");
        assert_eq!(builtins[0].target, "inbox");
        assert_eq!(builtins[0].builtin, Some(true));
        assert_eq!(builtins[1].id, "builtin-new-chat");
        assert_eq!(builtins[1].kind, "prompt");
        assert_eq!(builtins[1].target, "newChat");
        // camelCase wire names, optional fields omitted when None.
        let json = serde_json::to_value(&builtins[0]).unwrap();
        assert!(json.get("promptTemplate").is_some());
        assert!(json.get("agentId").is_none());
    }

    #[test]
    fn normalize_rejects_empty_ids_and_dedupes() {
        let mut a = builtin_scenarios()[0].clone();
        a.id = "  x  ".into();
        let mut dup = a.clone();
        dup.name = "dup".into();
        let mut empty = a.clone();
        empty.id = "   ".into();

        assert!(normalize(vec![empty]).is_err());

        let mut valid = a.clone();
        valid.id = "keep".into();
        let mut dup_of_valid = valid.clone();
        dup_of_valid.name = "later copy".into();
        let stored = normalize(vec![valid, dup, dup_of_valid]).unwrap();
        assert_eq!(stored.len(), 2);
        assert_eq!(stored[0].id, "keep");
        assert_eq!(stored[0].name, "记录收藏");
        assert_eq!(stored[1].id, "x");
        assert_eq!(stored[1].name, "dup");
    }
}
