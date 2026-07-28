//! Read-only parsers for external agent session stores (Grok / Claude / Codex / OpenCode).

mod claude;
mod codex;
mod grok;
mod opencode;
pub mod path_norm;

use serde::{Deserialize, Serialize};

/// List-row for an external conversation (camelCase for the frontend).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalConversation {
    /// Stable id: `{source}:{native_id}`
    pub id: String,
    pub source: String,
    pub title: String,
    pub cwd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_active_at: Option<String>,
    pub native_id: String,
    /// Opaque locator for `load` (usually an absolute path).
    pub locator: String,
}

pub trait AgentParser: Send + Sync {
    fn source(&self) -> &'static str;
    fn list(&self, project_root: &str) -> Result<Vec<ExternalConversation>, String>;
    fn load(&self, locator: &str) -> Result<Vec<serde_json::Value>, String>;
}

/// Concurrent-ish scan: each parser runs on its own; failures are logged and skipped.
pub fn list_all(project_root: &str) -> Vec<ExternalConversation> {
    let parsers: Vec<Box<dyn AgentParser>> = vec![
        Box::new(grok::GrokParser),
        Box::new(claude::ClaudeParser),
        Box::new(codex::CodexParser),
        Box::new(opencode::OpenCodeParser),
    ];

    let mut out = Vec::new();
    for p in parsers {
        match p.list(project_root) {
            Ok(mut rows) => out.append(&mut rows),
            Err(e) => {
                crate::debug_log::append(
                    "external",
                    "warn",
                    "",
                    &format!("{} list failed", p.source()),
                    Some(&e),
                );
            }
        }
    }
    // Newest first when timestamps exist
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

pub fn load_one(source: &str, locator: &str) -> Result<Vec<serde_json::Value>, String> {
    let source = source.trim().to_ascii_lowercase();
    match source.as_str() {
        "grok" => grok::GrokParser.load(locator),
        "claude" => claude::ClaudeParser.load(locator),
        "codex" => codex::CodexParser.load(locator),
        "opencode" => opencode::OpenCodeParser.load(locator),
        other => Err(format!("Unknown external source: {other}")),
    }
}
