//! Cline 3.x: `~/.cline/data/sessions/<id>/<id>.json` + `<id>.messages.json`.

use super::path_norm::cwd_matches_project;
use super::util::{
    assistant_event, env_dir, extract_text, home_join, json_str, read_json_file, read_subdirs,
    thought_event, tool_event, user_event,
};
use super::{AgentParser, ExternalConversation};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub struct ClineParser;

fn cline_data_dir() -> Option<PathBuf> {
    env_dir("CLINE_DIR")
        .or_else(|| home_join(".cline"))
        .map(|h| h.join("data"))
}

impl AgentParser for ClineParser {
    fn source(&self) -> &'static str {
        "cline"
    }
    fn agent_id(&self) -> &'static str {
        "cline"
    }

    fn list(&self, project_root: &str) -> Result<Vec<ExternalConversation>, String> {
        let Some(data) = cline_data_dir() else {
            return Ok(Vec::new());
        };
        let sessions = data.join("sessions");
        if !sessions.is_dir() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for dir in read_subdirs(&sessions) {
            let id = dir
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            if id.is_empty() {
                continue;
            }
            let manifest = dir.join(format!("{id}.json"));
            let Some(v) = read_json_file(&manifest) else {
                continue;
            };
            let cwd = json_str(&v, "cwd")
                .or_else(|| json_str(&v, "workspacePath"))
                .or_else(|| json_str(&v, "directory"))
                .unwrap_or("")
                .to_string();
            if cwd.is_empty() || !cwd_matches_project(&cwd, project_root) {
                continue;
            }
            let native_id = json_str(&v, "session_id").unwrap_or(&id).to_string();
            let title = json_str(&v, "title")
                .or_else(|| json_str(&v, "task"))
                .unwrap_or("Cline session")
                .to_string();
            out.push(ExternalConversation {
                id: format!("cline:{native_id}"),
                source: "cline".into(),
                agent_id: "cline".into(),
                title,
                cwd,
                started_at: json_str(&v, "started_at").map(str::to_string),
                last_active_at: json_str(&v, "ended_at")
                    .or_else(|| json_str(&v, "updated_at"))
                    .map(str::to_string),
                native_id,
                locator: dir.to_string_lossy().into_owned(),
            });
        }
        Ok(out)
    }

    fn load(&self, locator: &str) -> Result<Vec<Value>, String> {
        let dir = Path::new(locator);
        let id = dir
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("cline");
        let messages_path = dir.join(format!("{id}.messages.json"));
        let Some(v) = read_json_file(&messages_path).or_else(|| read_json_file(&dir.join("messages.json")))
        else {
            return Err(format!("Cline messages not found in {locator}"));
        };
        let arr = v
            .as_array()
            .cloned()
            .or_else(|| v.get("messages").and_then(|m| m.as_array()).cloned())
            .unwrap_or_default();
        let mut events = Vec::new();
        for (i, msg) in arr.iter().enumerate() {
            let role = msg
                .get("role")
                .or_else(|| msg.get("type"))
                .and_then(|x| x.as_str())
                .unwrap_or("");
            let created = json_str(msg, "timestamp")
                .or_else(|| json_str(msg, "ts"))
                .map(str::to_string)
                .unwrap_or_else(|| format!("cline-{i:06}"));
            let text = extract_text(msg.get("content").or(msg.get("text")));
            match role {
                "user" | "human" => {
                    if !text.trim().is_empty() {
                        events.push(user_event(id, &text, &created));
                    }
                }
                "assistant" | "ai" => {
                    if let Some(thinking) = json_str(msg, "thinking") {
                        events.push(thought_event(id, thinking, &format!("{created}-t")));
                    }
                    if !text.trim().is_empty() {
                        events.push(assistant_event(id, &text, &created, "cline"));
                    }
                    if let Some(name) = json_str(msg, "tool") {
                        events.push(tool_event(id, name, name, &created, None));
                    }
                }
                _ => {}
            }
        }
        Ok(events)
    }
}
