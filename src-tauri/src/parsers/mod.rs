//! Read-only parsers for external ACP agent session stores.
//!
//! Scan is filtered to a single Marionette project root. Unknown directories
//! never become projects.

mod claude;
mod cline;
mod codex;
mod cursor;
mod deepseek;
mod gemini;
mod grok;
mod hermes;
pub(crate) mod jsonl_agents;
mod opencode;
pub(crate) mod path_norm;
mod pi;
pub(crate) mod util;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// List-row for an external conversation (camelCase for the frontend).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalConversation {
    /// Stable id: `{source}:{native_id}`
    pub id: String,
    pub source: String,
    /// Marionette agent id (e.g. `grok-build`) for icons / labels.
    pub agent_id: String,
    pub title: String,
    pub cwd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_active_at: Option<String>,
    pub native_id: String,
    /// Opaque locator for `load` (path or db key).
    pub locator: String,
}

/// Windows-safe Marionette session id for an imported CLI conversation.
pub fn imported_session_id(source: &str, native_id: &str) -> String {
    let safe: String = native_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let source_safe: String = source
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    format!("ext-{source_safe}-{safe}")
}

pub trait AgentParser: Send + Sync {
    fn source(&self) -> &'static str;
    fn agent_id(&self) -> &'static str;
    fn list(&self, project_root: &str) -> Result<Vec<ExternalConversation>, String>;
    fn load(&self, locator: &str) -> Result<Vec<Value>, String>;
}

fn make_parser(source: &str) -> Option<Box<dyn AgentParser>> {
    Some(match source {
        "grok" => Box::new(grok::GrokParser),
        "claude" => Box::new(claude::ClaudeParser),
        "codex" => Box::new(codex::CodexParser),
        "opencode" => Box::new(opencode::OpenCodeParser),
        "pi" => Box::new(pi::PiParser),
        "omp" => Box::new(pi::OmpParser),
        "kimi" => Box::new(jsonl_agents::KimiParser),
        "codebuddy" => Box::new(jsonl_agents::CodeBuddyParser),
        "openclaw" => Box::new(jsonl_agents::OpenClawParser),
        "cline" => Box::new(cline::ClineParser),
        "gemini" => Box::new(gemini::GeminiParser),
        "hermes" => Box::new(hermes::HermesParser),
        "cursor" => Box::new(cursor::CursorParser),
        "deepseek" => Box::new(deepseek::DeepSeekParser),
        _ => return None,
    })
}

const SOURCES: &[&str] = &[
    "grok",
    "claude",
    "codex",
    "opencode",
    "pi",
    "omp",
    "kimi",
    "codebuddy",
    "openclaw",
    "cline",
    "gemini",
    "hermes",
    "cursor",
    "deepseek",
];

/// Each parser runs on its own thread. One source failing does not fail the rest.
pub fn list_all(project_root: &str) -> Vec<ExternalConversation> {
    let root = project_root.to_string();
    let handles: Vec<_> = SOURCES
        .iter()
        .filter_map(|source| {
            let parser = make_parser(source)?;
            let root = root.clone();
            let source = *source;
            Some(std::thread::spawn(move || (source, parser.list(&root))))
        })
        .collect();

    let mut out = Vec::new();
    for handle in handles {
        match handle.join() {
            Ok((_source, Ok(mut rows))) => out.append(&mut rows),
            Ok((source, Err(e))) => {
                crate::debug_log::append(
                    "external",
                    "warn",
                    "",
                    &format!("{source} list failed"),
                    Some(&e),
                );
            }
            Err(_) => {
                crate::debug_log::append(
                    "external",
                    "warn",
                    "",
                    "external parser thread panicked",
                    None,
                );
            }
        }
    }
    out.sort_by(|a, b| {
        let ta = b
            .last_active_at
            .as_deref()
            .or(b.started_at.as_deref())
            .unwrap_or("");
        let tb = a
            .last_active_at
            .as_deref()
            .or(a.started_at.as_deref())
            .unwrap_or("");
        ta.cmp(tb)
    });
    out
}

pub fn load_one(source: &str, locator: &str) -> Result<Vec<Value>, String> {
    let source = source.trim().to_ascii_lowercase();
    match make_parser(&source) {
        Some(parser) => parser.load(locator),
        None => Err(format!("Unknown external source: {source}")),
    }
}

#[cfg(test)]
mod tests {
    use super::path_norm::cwd_matches_project;
    use super::*;

    #[test]
    fn live_scan_keeps_only_the_named_project() {
        let root = r"D:\Projects\Marionette";
        if !std::path::Path::new(root).is_dir() {
            return;
        }
        let rows = list_all(root);
        for row in &rows {
            assert!(
                cwd_matches_project(&row.cwd, root),
                "{} leaked cwd {}",
                row.id,
                row.cwd
            );
        }
    }
}
