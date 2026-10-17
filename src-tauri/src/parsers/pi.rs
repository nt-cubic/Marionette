//! Pi: `PI_CODING_AGENT_SESSION_DIR` > settings.json sessionDir > `~/.pi/agent/sessions`.
//! Layout: `--<dashed-cwd>--/<ts>_<uuid>.jsonl`.

use super::path_norm::{cwd_matches_project, encode_pi_cwd_bucket};
use super::util::{
    assistant_event, clip_chars, env_dir, extract_text, home_join, json_str, read_json_file,
    thought_event, tool_event, user_event,
};
use super::{AgentParser, ExternalConversation};
use serde_json::Value;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

pub struct PiParser;

fn pi_sessions_dir() -> Option<PathBuf> {
    if let Some(p) = env_dir("PI_CODING_AGENT_SESSION_DIR") {
        return Some(p);
    }
    let agent_dir = env_dir("PI_CODING_AGENT_DIR").or_else(|| {
        let home = super::util::home_dir()?;
        Some(home.join(".pi").join("agent"))
    })?;
    if let Some(settings) = read_json_file(&agent_dir.join("settings.json")) {
        if let Some(dir) = json_str(&settings, "sessionDir") {
            let p = PathBuf::from(dir);
            if p.is_absolute() {
                return Some(p);
            }
        }
    }
    Some(agent_dir.join("sessions"))
}

fn candidate_dirs(root: &Path, project_root: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let encoded = encode_pi_cwd_bucket(project_root);
    let direct = root.join(&encoded);
    if direct.is_dir() {
        out.push(direct);
    }
    // Custom sessionDir may be flat (jsonl at root) — also scan the root itself.
    out.push(root.to_path_buf());
    if let Ok(rd) = fs::read_dir(root) {
        let enc_l = encoded.to_ascii_lowercase();
        for ent in rd.flatten() {
            let p = ent.path();
            if !p.is_dir() {
                continue;
            }
            let name = ent.file_name().to_string_lossy().to_ascii_lowercase();
            if name == enc_l || name.starts_with(&enc_l) {
                if !out.iter().any(|x| x == &p) {
                    out.push(p);
                }
            }
        }
    }
    out
}

struct PiMeta {
    id: String,
    cwd: String,
    started_at: Option<String>,
    last_active_at: Option<String>,
    title: String,
}

fn scan_pi_meta(path: &Path) -> Option<PiMeta> {
    let file = fs::File::open(path).ok()?;
    let mut cwd = String::new();
    let mut id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("pi")
        .to_string();
    if let Some((_, rest)) = id.split_once('_') {
        if rest.len() >= 8 {
            id = rest.to_string();
        }
    }
    let mut started = None;
    let mut last = None;
    let mut title = None;
    for (i, line) in BufReader::new(file).lines().enumerate() {
        if i > 120 && title.is_some() && !cwd.is_empty() {
            break;
        }
        let Ok(line) = line else { continue };
        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        let ty = v.get("type").and_then(|x| x.as_str()).unwrap_or("");
        if let Some(ts) = v.get("timestamp").and_then(|x| x.as_str()) {
            if started.is_none() {
                started = Some(ts.to_string());
            }
            last = Some(ts.to_string());
        }
        if ty == "session" {
            if let Some(s) = json_str(&v, "id") {
                id = s.to_string();
            }
            if let Some(c) = json_str(&v, "cwd") {
                cwd = c.to_string();
            }
        }
        if title.is_none() && ty == "message" {
            if v.pointer("/message/role").and_then(|x| x.as_str()) == Some("user") {
                let text = extract_text(v.pointer("/message/content"));
                if !text.trim().is_empty() {
                    title = Some(clip_chars(&text.replace('\n', " "), 80));
                }
            }
        }
    }
    Some(PiMeta {
        title: title.unwrap_or_else(|| format!("Pi {id}")),
        id,
        cwd,
        started_at: started,
        last_active_at: last,
    })
}

impl AgentParser for PiParser {
    fn source(&self) -> &'static str {
        "pi"
    }
    fn agent_id(&self) -> &'static str {
        "pi"
    }

    fn list(&self, project_root: &str) -> Result<Vec<ExternalConversation>, String> {
        list_pi_like(self.source(), self.agent_id(), &pi_sessions_dir(), project_root)
    }

    fn load(&self, locator: &str) -> Result<Vec<Value>, String> {
        load_pi_jsonl(Path::new(locator), "pi")
    }
}

pub fn list_pi_like(
    source: &str,
    agent_id: &str,
    sessions_dir: &Option<PathBuf>,
    project_root: &str,
) -> Result<Vec<ExternalConversation>, String> {
    let Some(root) = sessions_dir.clone().filter(|p| p.is_dir()) else {
        return Ok(Vec::new());
    };
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for dir in candidate_dirs(&root, project_root) {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for ent in rd.flatten() {
            let path = ent.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            if !seen.insert(path.clone()) {
                continue;
            }
            let Some(meta) = scan_pi_meta(&path) else {
                continue;
            };
            if !meta.cwd.is_empty() && !cwd_matches_project(&meta.cwd, project_root) {
                continue;
            }
            if meta.cwd.is_empty() && dir != root {
                // Bucket name matched this project; keep it.
            } else if meta.cwd.is_empty() {
                continue;
            }
            out.push(ExternalConversation {
                id: format!("{source}:{}", meta.id),
                source: source.into(),
                agent_id: agent_id.into(),
                title: meta.title,
                cwd: if meta.cwd.is_empty() {
                    project_root.to_string()
                } else {
                    meta.cwd
                },
                started_at: meta.started_at,
                last_active_at: meta.last_active_at,
                native_id: meta.id,
                locator: path.to_string_lossy().into_owned(),
            });
        }
    }
    Ok(out)
}

pub fn load_pi_jsonl(path: &Path, agent_id: &str) -> Result<Vec<Value>, String> {
    if !path.is_file() {
        return Err(format!("session not found: {}", path.display()));
    }
    let file = fs::File::open(path).map_err(|e| format!("read jsonl: {e}"))?;
    let mut session_id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(agent_id)
        .to_string();
    if let Some((_, rest)) = session_id.split_once('_') {
        if rest.len() >= 8 {
            session_id = rest.to_string();
        }
    }
    let mut events = Vec::new();
    let mut idx: u64 = 0;
    for line in BufReader::new(file).lines() {
        let Ok(line) = line else { continue };
        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        let ty = v.get("type").and_then(|x| x.as_str()).unwrap_or("");
        if ty == "session" {
            if let Some(id) = json_str(&v, "id") {
                session_id = id.to_string();
            }
            continue;
        }
        if ty != "message" {
            continue;
        }
        let msg = match v.get("message") {
            Some(m) => m,
            None => continue,
        };
        let role = msg.get("role").and_then(|x| x.as_str()).unwrap_or("");
        let created = v
            .get("timestamp")
            .and_then(|x| x.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| {
                idx += 1;
                format!("{agent_id}-{idx:06}")
            });
        match role {
            "user" => {
                let text = extract_text(msg.get("content"));
                if !text.trim().is_empty() {
                    events.push(user_event(&session_id, &text, &created));
                }
            }
            "assistant" => {
                if let Some(arr) = msg.get("content").and_then(|x| x.as_array()) {
                    for (i, item) in arr.iter().enumerate() {
                        let ity = item.get("type").and_then(|x| x.as_str()).unwrap_or("");
                        match ity {
                            "thinking" => {
                                if let Some(t) = item
                                    .get("thinking")
                                    .or_else(|| item.get("text"))
                                    .and_then(|x| x.as_str())
                                {
                                    if !t.trim().is_empty() {
                                        events.push(thought_event(
                                            &session_id,
                                            t,
                                            &format!("{created}-t{i}"),
                                        ));
                                    }
                                }
                            }
                            "toolCall" | "tool_use" | "toolUse" => {
                                let name = item
                                    .get("name")
                                    .or_else(|| item.get("toolName"))
                                    .and_then(|x| x.as_str())
                                    .unwrap_or("tool");
                                let id = item
                                    .get("id")
                                    .or_else(|| item.get("toolCallId"))
                                    .and_then(|x| x.as_str())
                                    .unwrap_or("tool");
                                events.push(tool_event(
                                    &session_id,
                                    id,
                                    name,
                                    &format!("{created}-tc{i}"),
                                    None,
                                ));
                            }
                            "text" => {
                                if let Some(t) = item.get("text").and_then(|x| x.as_str()) {
                                    if !t.trim().is_empty() {
                                        events.push(assistant_event(
                                            &session_id,
                                            t,
                                            &format!("{created}-a{i}"),
                                            agent_id,
                                        ));
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                } else {
                    let text = extract_text(msg.get("content"));
                    if !text.trim().is_empty() {
                        events.push(assistant_event(&session_id, &text, &created, agent_id));
                    }
                }
            }
            "toolResult" | "tool_result" => {
                let id = msg
                    .get("toolCallId")
                    .or_else(|| msg.get("tool_call_id"))
                    .and_then(|x| x.as_str())
                    .unwrap_or("tool");
                let name = msg
                    .get("toolName")
                    .and_then(|x| x.as_str())
                    .unwrap_or("tool");
                let detail = extract_text(msg.get("content"));
                events.push(tool_event(
                    &session_id,
                    id,
                    name,
                    &created,
                    Some(&detail),
                ));
            }
            _ => {}
        }
    }
    Ok(events)
}

/// OMP (Oh My Pi) — same jsonl shape; look in a few extra homes.
pub struct OmpParser;

fn omp_sessions_dir() -> Option<PathBuf> {
    env_dir("OMP_HOME")
        .map(|p| p.join("sessions"))
        .filter(|p| p.is_dir())
        .or_else(|| home_join(".omp").map(|p| p.join("sessions")).filter(|p| p.is_dir()))
        .or_else(|| {
            home_join(".oh-my-pi")
                .map(|p| p.join("sessions"))
                .filter(|p| p.is_dir())
        })
        .or_else(pi_sessions_dir)
}

impl AgentParser for OmpParser {
    fn source(&self) -> &'static str {
        "omp"
    }
    fn agent_id(&self) -> &'static str {
        "omp"
    }
    fn list(&self, project_root: &str) -> Result<Vec<ExternalConversation>, String> {
        list_pi_like(self.source(), self.agent_id(), &omp_sessions_dir(), project_root)
    }
    fn load(&self, locator: &str) -> Result<Vec<Value>, String> {
        load_pi_jsonl(Path::new(locator), "omp")
    }
}
