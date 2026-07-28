//! Codex sessions: `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`

use super::path_norm::cwd_matches_project;
use super::{AgentParser, ExternalConversation};
use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

pub struct CodexParser;

fn codex_home() -> Option<PathBuf> {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
    Some(PathBuf::from(home).join(".codex"))
}

struct SessionMeta {
    id: String,
    cwd: String,
    started_at: Option<String>,
}

fn read_session_meta(path: &Path) -> Option<SessionMeta> {
    let file = fs::File::open(path).ok()?;
    let reader = BufReader::new(file);
    for (i, line) in reader.lines().enumerate() {
        if i > 30 {
            break;
        }
        let Ok(line) = line else { continue };
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if v.get("type").and_then(|x| x.as_str()) != Some("session_meta") {
            continue;
        }
        let payload = v.get("payload")?;
        let id = payload
            .get("id")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        if id.is_empty() {
            return None;
        }
        let cwd = payload
            .get("cwd")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let started_at = payload
            .get("timestamp")
            .or_else(|| v.get("timestamp"))
            .and_then(|x| x.as_str())
            .map(str::to_string);
        return Some(SessionMeta {
            id,
            cwd,
            started_at,
        });
    }
    None
}

fn load_index_titles(codex: &Path) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    let index = codex.join("session_index.jsonl");
    let Ok(text) = fs::read_to_string(index) else {
        return map;
    };
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("");
        let name = v
            .get("thread_name")
            .or_else(|| v.get("title"))
            .and_then(|x| x.as_str())
            .unwrap_or("");
        if !id.is_empty() && !name.is_empty() {
            map.insert(id.to_string(), name.to_string());
        }
    }
    map
}

fn first_user_text(path: &Path) -> Option<String> {
    let file = fs::File::open(path).ok()?;
    let reader = BufReader::new(file);
    for (i, line) in reader.lines().enumerate() {
        if i > 80 {
            break;
        }
        let Ok(line) = line else { continue };
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let ty = v.get("type").and_then(|x| x.as_str()).unwrap_or("");
        if ty == "response_item" {
            let payload = v.get("payload")?;
            if payload.get("type").and_then(|x| x.as_str()) == Some("message")
                && payload.get("role").and_then(|x| x.as_str()) == Some("user")
            {
                let t = extract_codex_content(payload.get("content"));
                if !t.trim().is_empty() {
                    return Some(clip(&t, 80));
                }
            }
        }
        if ty == "event_msg" {
            let payload = v.get("payload")?;
            if payload.get("type").and_then(|x| x.as_str()) == Some("user_message") {
                if let Some(m) = payload.get("message").and_then(|x| x.as_str()) {
                    if !m.trim().is_empty() {
                        return Some(clip(m, 80));
                    }
                }
            }
        }
    }
    None
}

impl AgentParser for CodexParser {
    fn source(&self) -> &'static str {
        "codex"
    }

    fn list(&self, project_root: &str) -> Result<Vec<ExternalConversation>, String> {
        let codex = match codex_home() {
            Some(p) if p.is_dir() => p,
            _ => return Ok(Vec::new()),
        };
        let sessions_root = codex.join("sessions");
        if !sessions_root.is_dir() {
            return Ok(Vec::new());
        }
        let titles = load_index_titles(&codex);
        let mut out = Vec::new();
        // Nested YYYY/MM/DD/rollout-*.jsonl
        let walker = walk_jsonl(&sessions_root);
        for path in walker {
            let Some(meta) = read_session_meta(&path) else {
                continue;
            };
            if !cwd_matches_project(&meta.cwd, project_root) {
                continue;
            }
            let title = titles
                .get(&meta.id)
                .cloned()
                .or_else(|| first_user_text(&path))
                .filter(|t| !t.trim().is_empty())
                .unwrap_or_else(|| format!("Codex {}", &meta.id[..8.min(meta.id.len())]));
            let locator = path.to_string_lossy().to_string();
            // mtime as last_active fallback
            let last_active = fs::metadata(&path)
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis().to_string());
            out.push(ExternalConversation {
                id: format!("codex:{}", meta.id),
                source: "codex".into(),
                title,
                cwd: meta.cwd,
                started_at: meta.started_at,
                last_active_at: last_active,
                native_id: meta.id,
                locator,
            });
        }
        Ok(out)
    }

    fn load(&self, locator: &str) -> Result<Vec<Value>, String> {
        let path = Path::new(locator);
        if !path.is_file() {
            return Err(format!("Codex session not found: {locator}"));
        }
        let text = fs::read_to_string(path).map_err(|e| format!("read codex: {e}"))?;
        let mut session_id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("codex")
            .to_string();
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
            let ts = v
                .get("timestamp")
                .and_then(|x| x.as_str())
                .map(str::to_string);
            match ty {
                "session_meta" => {
                    if let Some(id) = v.pointer("/payload/id").and_then(|x| x.as_str()) {
                        session_id = id.to_string();
                    }
                }
                "response_item" => {
                    let payload = match v.get("payload") {
                        Some(p) => p,
                        None => continue,
                    };
                    let ptype = payload.get("type").and_then(|x| x.as_str()).unwrap_or("");
                    if ptype == "message" {
                        let role = payload.get("role").and_then(|x| x.as_str()).unwrap_or("");
                        let text = extract_codex_content(payload.get("content"));
                        if text.trim().is_empty() {
                            continue;
                        }
                        idx += 1;
                        let created = ts.clone().unwrap_or_else(|| format!("codex-{idx:06}"));
                        if role == "user" {
                            events.push(json!({
                                "type": "user_message",
                                "sessionId": session_id,
                                "text": text,
                                "createdAt": created,
                            }));
                        } else if role == "assistant" {
                            events.push(json!({
                                "type": "assistant_message",
                                "sessionId": session_id,
                                "text": text,
                                "createdAt": created,
                                "agentId": "codex",
                            }));
                        }
                    } else if ptype == "function_call" || ptype == "custom_tool_call" {
                        let name = payload
                            .get("name")
                            .or_else(|| payload.get("tool_name"))
                            .and_then(|x| x.as_str())
                            .unwrap_or("tool");
                        let id = payload
                            .get("call_id")
                            .or_else(|| payload.get("id"))
                            .and_then(|x| x.as_str())
                            .unwrap_or("tool");
                        idx += 1;
                        events.push(json!({
                            "type": "tool_call",
                            "sessionId": session_id,
                            "toolCallId": id,
                            "title": name,
                            "toolName": name,
                            "status": "completed",
                            "createdAt": ts.clone().unwrap_or_else(|| format!("codex-tc-{idx:06}")),
                        }));
                    } else if ptype == "reasoning" {
                        let text = payload
                            .get("summary")
                            .and_then(|x| x.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|i| i.get("text").and_then(|t| t.as_str()))
                                    .collect::<Vec<_>>()
                                    .join("\n")
                            })
                            .unwrap_or_default();
                        if !text.trim().is_empty() {
                            idx += 1;
                            events.push(json!({
                                "type": "thought",
                                "sessionId": session_id,
                                "text": text,
                                "createdAt": ts.unwrap_or_else(|| format!("codex-t-{idx:06}")),
                            }));
                        }
                    }
                }
                "event_msg" => {
                    let payload = match v.get("payload") {
                        Some(p) => p,
                        None => continue,
                    };
                    let et = payload.get("type").and_then(|x| x.as_str()).unwrap_or("");
                    if et == "agent_message" || et == "agent_reasoning" {
                        let text = payload
                            .get("message")
                            .or_else(|| payload.get("text"))
                            .and_then(|x| x.as_str())
                            .unwrap_or("");
                        if text.trim().is_empty() {
                            continue;
                        }
                        idx += 1;
                        let created = ts.unwrap_or_else(|| format!("codex-e-{idx:06}"));
                        if et == "agent_reasoning" {
                            events.push(json!({
                                "type": "thought",
                                "sessionId": session_id,
                                "text": text,
                                "createdAt": created,
                            }));
                        } else {
                            events.push(json!({
                                "type": "assistant_message",
                                "sessionId": session_id,
                                "text": text,
                                "createdAt": created,
                                "agentId": "codex",
                            }));
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(events)
    }
}

fn walk_jsonl(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = fs::read_dir(&dir) else {
            continue;
        };
        for ent in rd.flatten() {
            let p = ent.path();
            if p.is_dir() {
                stack.push(p);
            } else if p
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("rollout-") && n.ends_with(".jsonl"))
                .unwrap_or(false)
            {
                out.push(p);
            }
        }
    }
    out
}

fn extract_codex_content(content: Option<&Value>) -> String {
    let Some(content) = content else {
        return String::new();
    };
    if let Some(s) = content.as_str() {
        return s.to_string();
    }
    let Some(arr) = content.as_array() else {
        return String::new();
    };
    let mut parts = Vec::new();
    for item in arr {
        let ty = item.get("type").and_then(|x| x.as_str()).unwrap_or("");
        if ty == "input_text" || ty == "output_text" || ty == "text" {
            if let Some(t) = item.get("text").and_then(|x| x.as_str()) {
                parts.push(t.to_string());
            }
        }
    }
    parts.join("\n")
}

fn clip(s: &str, max: usize) -> String {
    let t = s.replace('\n', " ").trim().to_string();
    if t.chars().count() <= max {
        t
    } else {
        format!("{}…", t.chars().take(max.saturating_sub(1)).collect::<String>())
    }
}
