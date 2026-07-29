//! User-registered custom ACP agents (lightweight; no binary_cache / no ring).
//!
//! Stored as JSON under the Marionette global dir. Merged into `list_agents`
//! after built-ins. npm packages optional — when set, Install button works.

use crate::models::{AgentConfig, AgentInstallSpec};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomAgentDef {
    /// Stable id — must not collide with built-ins (`custom-…` recommended).
    pub id: String,
    pub label: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// When set, in-app npm install uses this package (unpinned name preferred).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub npm_package: Option<String>,
    /// Shown in the agent menu “如何安装” when not on PATH.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

fn custom_agents_path() -> Result<PathBuf, String> {
    let dir = crate::app_paths::global_dir()?;
    Ok(dir.join("custom_agents.json"))
}

pub fn load_all() -> Result<Vec<CustomAgentDef>, String> {
    let path = custom_agents_path()?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = fs::read_to_string(&path)
        .map_err(|e| format!("Read custom_agents.json failed: {e}"))?;
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(&text).map_err(|e| format!("Parse custom_agents.json failed: {e}"))
}

fn save_all(list: &[CustomAgentDef]) -> Result<(), String> {
    let path = custom_agents_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Create storage dir failed: {e}"))?;
    }
    let text = serde_json::to_string_pretty(list)
        .map_err(|e| format!("Serialize custom agents failed: {e}"))?;
    fs::write(&path, text).map_err(|e| format!("Write custom_agents.json failed: {e}"))
}

fn is_valid_id(id: &str) -> bool {
    let id = id.trim();
    if id.is_empty() || id.len() > 64 {
        return false;
    }
    id.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Built-in agent ids — custom must not shadow these.
fn builtin_ids() -> Vec<String> {
    AgentConfig::defaults()
        .into_iter()
        .map(|a| a.id)
        .collect()
}

pub fn add(def: CustomAgentDef) -> Result<CustomAgentDef, String> {
    let mut def = def;
    def.id = def.id.trim().to_string();
    def.label = def.label.trim().to_string();
    def.command = def.command.trim().to_string();
    if !is_valid_id(&def.id) {
        return Err("id must be 1–64 chars [A-Za-z0-9_-]".into());
    }
    if def.label.is_empty() {
        return Err("label is required".into());
    }
    if def.command.is_empty() {
        return Err("command is required".into());
    }
    if builtin_ids().iter().any(|b| b == &def.id) {
        return Err(format!("id `{}` is reserved for a built-in agent", def.id));
    }
    let mut list = load_all()?;
    if list.iter().any(|c| c.id == def.id) {
        return Err(format!("custom agent `{}` already exists", def.id));
    }
    list.push(def.clone());
    save_all(&list)?;
    Ok(def)
}

pub fn remove(id: &str) -> Result<(), String> {
    let mut list = load_all()?;
    let before = list.len();
    list.retain(|c| c.id != id);
    if list.len() == before {
        return Err(format!("custom agent `{id}` not found"));
    }
    save_all(&list)
}

pub fn to_agent_config(def: &CustomAgentDef) -> AgentConfig {
    let install = if let Some(pkg) = def.npm_package.as_ref().filter(|p| !p.trim().is_empty()) {
        AgentInstallSpec {
            manager: "npm".into(),
            package: Some(pkg.trim().to_string()),
            requires: Vec::new(),
            note: def.note.clone(),
        }
    } else {
        AgentInstallSpec {
            manager: "manual".into(),
            package: None,
            requires: Vec::new(),
            note: Some(
                def.note
                    .clone()
                    .unwrap_or_else(|| {
                        format!(
                            "Custom agent: put `{}` on PATH (or set npmPackage for in-app install).",
                            def.command
                        )
                    }),
            ),
        }
    };
    AgentConfig {
        id: def.id.clone(),
        label: def.label.clone(),
        command: def.command.clone(),
        args: def.args.clone(),
        cwd_mode: "project-root".into(),
        launch_mode: "pty".into(),
        send_strategy: "stdin".into(),
        parser: "ansi-raw".into(),
        transport: "acp".into(),
        enabled: true,
        install,
    }
}

/// Built-ins first, then custom (same order as file).
pub fn all_agents_merged() -> Vec<AgentConfig> {
    let mut out = AgentConfig::defaults();
    if let Ok(custom) = load_all() {
        for def in custom {
            out.push(to_agent_config(&def));
        }
    }
    out
}
