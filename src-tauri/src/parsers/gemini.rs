//! Gemini CLI: `$GEMINI_CLI_HOME/.gemini` (or `~/.gemini`) tmp/history + `.project_root`.

use super::path_norm::cwd_matches_project;
use super::util::{
    assistant_event, env_dir, extract_text, json_str, read_json_file, read_subdirs, thought_event,
    user_event, walk_files,
};
use super::{AgentParser, ExternalConversation};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub struct GeminiParser;

fn gemini_base() -> Option<PathBuf> {
    Some(
        env_dir("GEMINI_CLI_HOME")
            .or_else(|| super::util::home_dir())?
            .join(".gemini"),
    )
}

fn alias_cwd(base: &Path, alias: &str) -> Option<String> {
    for rel in ["tmp", "history"] {
        let p = base.join(rel).join(alias).join(".project_root");
        // lossy: 一个坏字节不该让整个 alias 解析失效。
        let s = std::fs::read(&p)
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_default();
        let t = s.trim();
        if !t.is_empty() {
            return Some(t.to_string());
        }
    }
    if let Some(v) = read_json_file(&base.join("projects.json")) {
        if let Some(obj) = v.get("projects").and_then(|x| x.as_object()) {
            for (path, mapped) in obj {
                if mapped.as_str() == Some(alias) {
                    return Some(path.clone());
                }
            }
        }
    }
    None
}

impl AgentParser for GeminiParser {
    fn source(&self) -> &'static str {
        "gemini"
    }
    fn agent_id(&self) -> &'static str {
        "gemini"
    }

    fn list(&self, project_root: &str) -> Result<Vec<ExternalConversation>, String> {
        let Some(base) = gemini_base().filter(|p| p.is_dir()) else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        for bucket in ["tmp", "history"] {
            for alias_dir in read_subdirs(&base.join(bucket)) {
                let alias = alias_dir
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_string();
                let Some(cwd) = alias_cwd(&base, &alias) else {
                    continue;
                };
                if !cwd_matches_project(&cwd, project_root) {
                    continue;
                }
                for path in walk_files(&alias_dir, 2) {
                    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                    if ext != "json" && ext != "jsonl" {
                        continue;
                    }
                    if path.file_name().and_then(|n| n.to_str()) == Some(".project_root") {
                        continue;
                    }
                    let Some(v) = read_json_file(&path) else {
                        continue;
                    };
                    let native_id = json_str(&v, "sessionId")
                        .or_else(|| json_str(&v, "id"))
                        .unwrap_or("")
                        .to_string();
                    let native_id = if native_id.is_empty() {
                        path.file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("gemini")
                            .to_string()
                    } else {
                        native_id
                    };
                    let title = json_str(&v, "title").unwrap_or("Gemini session").to_string();
                    out.push(ExternalConversation {
                        id: format!("gemini:{native_id}"),
                        source: "gemini".into(),
                        agent_id: "gemini".into(),
                        title,
                        cwd: cwd.clone(),
                        started_at: json_str(&v, "startTime")
                            .or_else(|| json_str(&v, "createdAt"))
                            .map(str::to_string),
                        last_active_at: json_str(&v, "lastUpdated").map(str::to_string),
                        native_id,
                        locator: path.to_string_lossy().into_owned(),
                    });
                }
            }
        }
        Ok(out)
    }

    fn load(&self, locator: &str) -> Result<Vec<Value>, String> {
        let path = Path::new(locator);
        let Some(v) = read_json_file(path) else {
            return super::pi::load_pi_jsonl(path, "gemini");
        };
        let session_id = json_str(&v, "sessionId")
            .or_else(|| json_str(&v, "id"))
            .unwrap_or("gemini");
        let messages = v
            .get("messages")
            .or_else(|| v.get("history"))
            .and_then(|x| x.as_array())
            .cloned()
            .unwrap_or_default();
        let mut events = Vec::new();
        for (i, msg) in messages.iter().enumerate() {
            let role = msg.get("role").and_then(|x| x.as_str()).unwrap_or("");
            let created = json_str(msg, "timestamp")
                .map(str::to_string)
                .unwrap_or_else(|| format!("gemini-{i:06}"));
            let text = extract_text(msg.get("content").or(msg.get("text")));
            if role == "user" && !text.trim().is_empty() {
                events.push(user_event(session_id, &text, &created));
            } else if (role == "assistant" || role == "model") && !text.trim().is_empty() {
                if let Some(th) = json_str(msg, "thought") {
                    events.push(thought_event(session_id, th, &format!("{created}-t")));
                }
                events.push(assistant_event(session_id, &text, &created, "gemini"));
            }
        }
        Ok(events)
    }
}
