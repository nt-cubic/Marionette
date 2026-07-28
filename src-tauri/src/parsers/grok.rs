//! Grok Build session store: `~/.grok/sessions/<encoded-cwd>/<uuid>/`.

use super::path_norm::cwd_matches_project;
use super::{AgentParser, ExternalConversation};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

pub struct GrokParser;

fn grok_sessions_root() -> Option<PathBuf> {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))?;
    Some(PathBuf::from(home).join(".grok").join("sessions"))
}

fn read_summary(path: &Path) -> Option<Value> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn summary_title(v: &Value) -> String {
    v.get("generated_title")
        .or_else(|| v.get("session_summary"))
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("Grok session")
        .to_string()
}

fn summary_cwd(v: &Value) -> String {
    v.pointer("/info/cwd")
        .and_then(|x| x.as_str())
        .or_else(|| v.get("git_root_dir").and_then(|x| x.as_str()))
        .unwrap_or("")
        .to_string()
}

fn summary_id(v: &Value, fallback_dir: &str) -> String {
    v.pointer("/info/id")
        .and_then(|x| x.as_str())
        .unwrap_or(fallback_dir)
        .to_string()
}

impl AgentParser for GrokParser {
    fn source(&self) -> &'static str {
        "grok"
    }

    fn list(&self, project_root: &str) -> Result<Vec<ExternalConversation>, String> {
        let root = match grok_sessions_root() {
            Some(p) if p.is_dir() => p,
            _ => return Ok(Vec::new()),
        };

        let mut out = Vec::new();
        // Layout: sessions/<cwd-encoded>/<uuid>/summary.json
        let cwd_dirs = fs::read_dir(&root).map_err(|e| format!("read grok sessions: {e}"))?;
        for cwd_ent in cwd_dirs.flatten() {
            let cwd_path = cwd_ent.path();
            if !cwd_path.is_dir() {
                continue;
            }
            let uuid_dirs = match fs::read_dir(&cwd_path) {
                Ok(d) => d,
                Err(_) => continue,
            };
            for uuid_ent in uuid_dirs.flatten() {
                let session_dir = uuid_ent.path();
                if !session_dir.is_dir() {
                    continue;
                }
                let summary_path = session_dir.join("summary.json");
                if !summary_path.is_file() {
                    continue;
                }
                let Some(summary) = read_summary(&summary_path) else {
                    continue;
                };
                let cwd = summary_cwd(&summary);
                if !cwd_matches_project(&cwd, project_root) {
                    continue;
                }
                let dir_name = session_dir
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("unknown");
                let native_id = summary_id(&summary, dir_name);
                let locator = session_dir.to_string_lossy().to_string();
                out.push(ExternalConversation {
                    id: format!("grok:{native_id}"),
                    source: "grok".into(),
                    title: summary_title(&summary),
                    cwd,
                    started_at: summary
                        .get("created_at")
                        .and_then(|x| x.as_str())
                        .map(str::to_string),
                    last_active_at: summary
                        .get("last_active_at")
                        .or_else(|| summary.get("updated_at"))
                        .and_then(|x| x.as_str())
                        .map(str::to_string),
                    native_id,
                    locator,
                });
            }
        }
        Ok(out)
    }

    fn load(&self, locator: &str) -> Result<Vec<Value>, String> {
        let dir = Path::new(locator);
        if !dir.is_dir() {
            return Err(format!("Grok session dir not found: {locator}"));
        }
        let session_id = dir
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("grok");
        // Prefer structured chat history when present.
        let history = dir.join("chat_history.jsonl");
        if history.is_file() {
            return load_chat_history(&history, session_id);
        }
        let updates = dir.join("updates.jsonl");
        if updates.is_file() {
            return load_updates_jsonl(&updates, session_id);
        }
        Err("No chat_history.jsonl or updates.jsonl in Grok session".into())
    }
}

fn load_chat_history(path: &Path, session_id: &str) -> Result<Vec<Value>, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("read chat_history: {e}"))?;
    let mut events = Vec::new();
    let mut idx: u64 = 0;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let ty = v.get("type").and_then(|x| x.as_str()).unwrap_or("");
        idx += 1;
        let created = format!("grok-{idx:06}");
        match ty {
            "system" => continue,
            "user" => {
                let text = extract_text_content(v.get("content"));
                if text.trim().is_empty() {
                    continue;
                }
                // Skip huge system-ish blobs wrapped as user (optional soft cap still show)
                events.push(json!({
                    "type": "user_message",
                    "sessionId": session_id,
                    "text": text,
                    "createdAt": created,
                }));
            }
            "assistant" => {
                let text = extract_text_content(v.get("content"));
                if !text.trim().is_empty() {
                    events.push(json!({
                        "type": "assistant_message",
                        "sessionId": session_id,
                        "text": text,
                        "createdAt": created,
                        "agentId": "grok",
                    }));
                }
                // tool_calls on assistant message
                if let Some(calls) = v.get("tool_calls").and_then(|x| x.as_array()) {
                    for (i, call) in calls.iter().enumerate() {
                        let name = call
                            .get("name")
                            .or_else(|| call.pointer("/function/name"))
                            .and_then(|x| x.as_str())
                            .unwrap_or("tool");
                        let id = call
                            .get("id")
                            .and_then(|x| x.as_str())
                            .unwrap_or("tool");
                        events.push(json!({
                            "type": "tool_call",
                            "sessionId": session_id,
                            "toolCallId": id,
                            "title": name,
                            "toolName": name,
                            "status": "completed",
                            "createdAt": format!("{created}-tc{i}"),
                        }));
                    }
                }
            }
            "reasoning" => {
                let text = extract_reasoning(&v);
                if text.trim().is_empty() {
                    continue;
                }
                events.push(json!({
                    "type": "thought",
                    "sessionId": session_id,
                    "text": text,
                    "createdAt": created,
                }));
            }
            "tool_result" => {
                let id = v
                    .get("tool_call_id")
                    .and_then(|x| x.as_str())
                    .unwrap_or("tool");
                let detail = extract_text_content(v.get("content"));
                let clipped = clip(&detail, 4000);
                events.push(json!({
                    "type": "tool_call",
                    "sessionId": session_id,
                    "toolCallId": id,
                    "title": "tool_result",
                    "toolName": "tool_result",
                    "status": "completed",
                    "detail": clipped,
                    "createdAt": created,
                }));
            }
            _ => {}
        }
    }
    Ok(events)
}

/// Fallback: coalesce ACP-style session/update chunks from updates.jsonl.
fn load_updates_jsonl(path: &Path, session_id: &str) -> Result<Vec<Value>, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("read updates: {e}"))?;
    let mut events = Vec::new();
    let mut user_buf = String::new();
    let mut thought_buf = String::new();
    let mut assistant_buf = String::new();
    let mut idx: u64 = 0;

    let flush_user = |buf: &mut String, events: &mut Vec<Value>, idx: &mut u64| {
        if buf.trim().is_empty() {
            return;
        }
        *idx += 1;
        events.push(json!({
            "type": "user_message",
            "sessionId": session_id,
            "text": std::mem::take(buf),
            "createdAt": format!("grok-u-{idx:06}"),
        }));
    };
    let flush_thought = |buf: &mut String, events: &mut Vec<Value>, idx: &mut u64| {
        if buf.trim().is_empty() {
            return;
        }
        *idx += 1;
        events.push(json!({
            "type": "thought",
            "sessionId": session_id,
            "text": std::mem::take(buf),
            "createdAt": format!("grok-t-{idx:06}"),
        }));
    };
    let flush_assistant = |buf: &mut String, events: &mut Vec<Value>, idx: &mut u64| {
        if buf.trim().is_empty() {
            return;
        }
        *idx += 1;
        events.push(json!({
            "type": "assistant_message",
            "sessionId": session_id,
            "text": std::mem::take(buf),
            "createdAt": format!("grok-a-{idx:06}"),
            "agentId": "grok",
        }));
    };

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let update = v
            .pointer("/params/update")
            .cloned()
            .unwrap_or(Value::Null);
        let kind = update
            .get("sessionUpdate")
            .and_then(|x| x.as_str())
            .unwrap_or("");
        match kind {
            "user_message_chunk" => {
                flush_thought(&mut thought_buf, &mut events, &mut idx);
                flush_assistant(&mut assistant_buf, &mut events, &mut idx);
                if let Some(t) = chunk_text(&update) {
                    user_buf.push_str(&t);
                }
            }
            "agent_thought_chunk" => {
                flush_user(&mut user_buf, &mut events, &mut idx);
                flush_assistant(&mut assistant_buf, &mut events, &mut idx);
                if let Some(t) = chunk_text(&update) {
                    thought_buf.push_str(&t);
                }
            }
            "agent_message_chunk" => {
                flush_user(&mut user_buf, &mut events, &mut idx);
                flush_thought(&mut thought_buf, &mut events, &mut idx);
                if let Some(t) = chunk_text(&update) {
                    assistant_buf.push_str(&t);
                }
            }
            "tool_call" | "tool_call_update" => {
                flush_user(&mut user_buf, &mut events, &mut idx);
                flush_thought(&mut thought_buf, &mut events, &mut idx);
                flush_assistant(&mut assistant_buf, &mut events, &mut idx);
                let id = update
                    .get("toolCallId")
                    .and_then(|x| x.as_str())
                    .unwrap_or("tool");
                let title = update
                    .get("title")
                    .or_else(|| update.get("kind"))
                    .and_then(|x| x.as_str())
                    .unwrap_or("tool");
                if kind == "tool_call" {
                    idx += 1;
                    events.push(json!({
                        "type": "tool_call",
                        "sessionId": session_id,
                        "toolCallId": id,
                        "title": title,
                        "toolName": title,
                        "status": update.get("status").and_then(|x| x.as_str()).unwrap_or("completed"),
                        "createdAt": format!("grok-tc-{idx:06}"),
                    }));
                }
            }
            "turn_completed" => {
                flush_user(&mut user_buf, &mut events, &mut idx);
                flush_thought(&mut thought_buf, &mut events, &mut idx);
                flush_assistant(&mut assistant_buf, &mut events, &mut idx);
            }
            _ => {}
        }
    }
    flush_user(&mut user_buf, &mut events, &mut idx);
    flush_thought(&mut thought_buf, &mut events, &mut idx);
    flush_assistant(&mut assistant_buf, &mut events, &mut idx);
    Ok(events)
}

fn chunk_text(update: &Value) -> Option<String> {
    update
        .pointer("/content/text")
        .and_then(|x| x.as_str())
        .map(str::to_string)
}

fn extract_text_content(content: Option<&Value>) -> String {
    let Some(content) = content else {
        return String::new();
    };
    if let Some(s) = content.as_str() {
        return s.to_string();
    }
    if let Some(arr) = content.as_array() {
        let mut parts = Vec::new();
        for item in arr {
            if let Some(t) = item.get("text").and_then(|x| x.as_str()) {
                parts.push(t.to_string());
            } else if item.get("type").and_then(|x| x.as_str()) == Some("text") {
                if let Some(t) = item.get("text").and_then(|x| x.as_str()) {
                    parts.push(t.to_string());
                }
            }
        }
        return parts.join("\n");
    }
    String::new()
}

fn extract_reasoning(v: &Value) -> String {
    if let Some(s) = v.get("content").and_then(|x| x.as_str()) {
        return s.to_string();
    }
    if let Some(arr) = v.get("summary").and_then(|x| x.as_array()) {
        let mut parts = Vec::new();
        for item in arr {
            if let Some(t) = item.get("text").and_then(|x| x.as_str()) {
                parts.push(t.to_string());
            }
        }
        return parts.join("\n");
    }
    extract_text_content(v.get("content"))
}

fn clip(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max])
    }
}
