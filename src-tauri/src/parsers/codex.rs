//! Codex: `$CODEX_HOME/sessions/YYYY/MM/DD/rollout-*.jsonl` + `session_index.jsonl`.

use super::path_norm::cwd_matches_project;
use super::util::{
    assistant_event, clip_chars, env_dir, extract_text, home_join, thought_event, tool_event,
    user_event, walk_files,
};
use super::{AgentParser, ExternalConversation};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

pub struct CodexParser;

fn codex_home() -> Option<PathBuf> {
    env_dir("CODEX_HOME").or_else(|| home_join(".codex"))
}

struct SessionMeta {
    id: String,
    cwd: String,
    started_at: Option<String>,
}

fn read_session_meta(path: &Path) -> Option<SessionMeta> {
    let file = fs::File::open(path).ok()?;
    for (i, line) in BufReader::new(file).lines().enumerate() {
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
        let id = payload.get("id").and_then(|x| x.as_str()).unwrap_or("");
        if id.is_empty() {
            return None;
        }
        return Some(SessionMeta {
            id: id.to_string(),
            cwd: payload
                .get("cwd")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string(),
            started_at: payload
                .get("timestamp")
                .or_else(|| v.get("timestamp"))
                .and_then(|x| x.as_str())
                .map(str::to_string),
        });
    }
    None
}

fn load_index_titles(codex: &Path) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let Ok(text) = fs::read_to_string(codex.join("session_index.jsonl")) else {
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
    for (i, line) in BufReader::new(file).lines().enumerate() {
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
                let t = extract_text(payload.get("content"));
                if !t.trim().is_empty() {
                    return Some(clip_chars(&t.replace('\n', " "), 80));
                }
            }
        }
        if ty == "event_msg" {
            let payload = v.get("payload")?;
            if payload.get("type").and_then(|x| x.as_str()) == Some("user_message") {
                if let Some(m) = payload.get("message").and_then(|x| x.as_str()) {
                    if !m.trim().is_empty() {
                        return Some(clip_chars(&m.replace('\n', " "), 80));
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
    fn agent_id(&self) -> &'static str {
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
        for path in walk_files(&sessions_root, 6) {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !name.starts_with("rollout-") || !name.ends_with(".jsonl") {
                continue;
            }
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
            let last_active = fs::metadata(&path)
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis().to_string());
            out.push(ExternalConversation {
                id: format!("codex:{}", meta.id),
                source: "codex".into(),
                agent_id: "codex".into(),
                title,
                cwd: meta.cwd,
                started_at: meta.started_at,
                last_active_at: last_active,
                native_id: meta.id,
                locator: path.to_string_lossy().into_owned(),
            });
        }
        Ok(out)
    }

    fn load(&self, locator: &str) -> Result<Vec<Value>, String> {
        let path = Path::new(locator);
        if !path.is_file() {
            return Err(format!("Codex session not found: {locator}"));
        }
        let file = fs::File::open(path).map_err(|e| format!("read codex: {e}"))?;
        let mut session_id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("codex")
            .to_string();
        let mut events = Vec::new();
        let mut idx: u64 = 0;
        for line in BufReader::new(file).lines() {
            let Ok(line) = line else { continue };
            let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
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
                    let Some(payload) = v.get("payload") else { continue };
                    let ptype = payload.get("type").and_then(|x| x.as_str()).unwrap_or("");
                    if ptype == "message" {
                        let role = payload.get("role").and_then(|x| x.as_str()).unwrap_or("");
                        let text = extract_text(payload.get("content"));
                        if text.trim().is_empty() {
                            continue;
                        }
                        idx += 1;
                        let created = ts.clone().unwrap_or_else(|| format!("codex-{idx:06}"));
                        if role == "user" {
                            events.push(user_event(&session_id, &text, &created));
                        } else if role == "assistant" {
                            events.push(assistant_event(&session_id, &text, &created, "codex"));
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
                        events.push(tool_event(
                            &session_id,
                            id,
                            name,
                            &ts.clone().unwrap_or_else(|| format!("codex-tc-{idx:06}")),
                            None,
                        ));
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
                            events.push(thought_event(
                                &session_id,
                                &text,
                                &ts.unwrap_or_else(|| format!("codex-t-{idx:06}")),
                            ));
                        }
                    }
                }
                "event_msg" => {
                    let Some(payload) = v.get("payload") else { continue };
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
                            events.push(thought_event(&session_id, text, &created));
                        } else {
                            events.push(assistant_event(&session_id, text, &created, "codex"));
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(events)
    }
}
