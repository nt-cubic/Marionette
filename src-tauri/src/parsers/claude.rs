//! Claude Code: `$CLAUDE_CONFIG_DIR/projects/<encoded>/*.jsonl`

use super::path_norm::{cwd_matches_project, encode_dashed_windows_path};
use super::util::{
    assistant_event, env_dir, extract_text, home_join, thought_event, tool_event, user_event,
};
use super::{AgentParser, ExternalConversation};
use serde_json::Value;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

pub struct ClaudeParser;

fn claude_projects_root() -> Option<PathBuf> {
    Some(
        env_dir("CLAUDE_CONFIG_DIR")
            .or_else(|| home_join(".claude"))?
            .join("projects"),
    )
}

fn candidate_project_dirs(root: &Path, project_root: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let encoded = encode_dashed_windows_path(project_root);
    let direct = root.join(&encoded);
    if direct.is_dir() {
        out.push(direct);
    }
    if let Ok(rd) = fs::read_dir(root) {
        let enc_l = encoded.to_ascii_lowercase();
        for ent in rd.flatten() {
            let p = ent.path();
            if !p.is_dir() {
                continue;
            }
            let name = ent.file_name().to_string_lossy().to_ascii_lowercase();
            if name == enc_l || name.starts_with(&format!("{enc_l}-")) {
                if !out.iter().any(|x| x == &p) {
                    out.push(p);
                }
            }
        }
    }
    out
}

struct ListMeta {
    title: Option<String>,
    cwd: Option<String>,
    started_at: Option<String>,
    last_active_at: Option<String>,
    session_id: Option<String>,
}

fn scan_list_meta(path: &Path) -> ListMeta {
    let mut meta = ListMeta {
        title: None,
        cwd: None,
        started_at: None,
        last_active_at: None,
        session_id: None,
    };
    let Ok(file) = fs::File::open(path) else {
        return meta;
    };
    for (i, line) in BufReader::new(file).lines().enumerate() {
        if i > 400 {
            break;
        }
        let Ok(line) = line else { continue };
        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        let ty = v.get("type").and_then(|x| x.as_str()).unwrap_or("");
        if meta.session_id.is_none() {
            meta.session_id = v
                .get("sessionId")
                .and_then(|x| x.as_str())
                .map(str::to_string);
        }
        if let Some(ts) = v.get("timestamp").and_then(|x| x.as_str()) {
            if meta.started_at.is_none() {
                meta.started_at = Some(ts.to_string());
            }
            meta.last_active_at = Some(ts.to_string());
        }
        if ty == "custom-title" || ty == "ai-title" {
            if let Some(t) = v
                .get("customTitle")
                .or_else(|| v.get("aiTitle"))
                .or_else(|| v.get("title"))
                .and_then(|x| x.as_str())
            {
                if !t.trim().is_empty() && (meta.title.is_none() || ty == "custom-title") {
                    meta.title = Some(t.to_string());
                }
            }
        }
        if meta.cwd.is_none() {
            if let Some(c) = v.get("cwd").and_then(|x| x.as_str()) {
                if !c.is_empty() {
                    meta.cwd = Some(c.to_string());
                }
            }
        }
        if ty == "user" && meta.title.is_none() {
            let text = extract_claude_message_text(v.get("message"));
            if !text.trim().is_empty()
                && !text.starts_with("<command-")
                && !text.starts_with("Caveat:")
            {
                meta.title = Some(super::util::clip_chars(&text.replace('\n', " "), 80));
            }
        }
    }
    meta
}

impl AgentParser for ClaudeParser {
    fn source(&self) -> &'static str {
        "claude"
    }
    fn agent_id(&self) -> &'static str {
        "claude-code"
    }

    fn list(&self, project_root: &str) -> Result<Vec<ExternalConversation>, String> {
        let root = match claude_projects_root() {
            Some(p) if p.is_dir() => p,
            _ => return Ok(Vec::new()),
        };
        let mut out = Vec::new();
        for dir in candidate_project_dirs(&root, project_root) {
            let Ok(rd) = fs::read_dir(&dir) else { continue };
            for ent in rd.flatten() {
                let path = ent.path();
                if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                    continue;
                }
                let meta = scan_list_meta(&path);
                let cwd = meta.cwd.unwrap_or_default();
                if !cwd.is_empty() && !cwd_matches_project(&cwd, project_root) {
                    continue;
                }
                let native_id = meta
                    .session_id
                    .or_else(|| path.file_stem().and_then(|s| s.to_str()).map(str::to_string))
                    .unwrap_or_else(|| "unknown".into());
                let title = meta
                    .title
                    .filter(|t| !t.trim().is_empty())
                    .unwrap_or_else(|| format!("Claude {native_id}"));
                out.push(ExternalConversation {
                    id: format!("claude:{native_id}"),
                    source: "claude".into(),
                    agent_id: self.agent_id().into(),
                    title,
                    cwd: if cwd.is_empty() {
                        project_root.to_string()
                    } else {
                        cwd
                    },
                    started_at: meta.started_at,
                    last_active_at: meta.last_active_at,
                    native_id,
                    locator: path.to_string_lossy().into_owned(),
                });
            }
        }
        Ok(out)
    }

    fn load(&self, locator: &str) -> Result<Vec<Value>, String> {
        let path = Path::new(locator);
        if !path.is_file() {
            return Err(format!("Claude session not found: {locator}"));
        }
        let file = fs::File::open(path).map_err(|e| format!("read claude jsonl: {e}"))?;
        let session_id = path.file_stem().and_then(|s| s.to_str()).unwrap_or("claude");
        let mut events = Vec::new();
        let mut idx: u64 = 0;
        for line in BufReader::new(file).lines() {
            let Ok(line) = line else { continue };
            let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
                continue;
            };
            let ty = v.get("type").and_then(|x| x.as_str()).unwrap_or("");
            match ty {
                "user" => {
                    let text = extract_claude_message_text(v.get("message"));
                    if text.trim().is_empty()
                        || text.starts_with("<command-")
                        || text.starts_with("Caveat:")
                    {
                        continue;
                    }
                    idx += 1;
                    let created = v
                        .get("timestamp")
                        .and_then(|x| x.as_str())
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("claude-u-{idx:06}"));
                    events.push(user_event(session_id, &text, &created));
                }
                "assistant" => {
                    let (text, thoughts, tools) = extract_assistant_parts(v.get("message"));
                    let created = v
                        .get("timestamp")
                        .and_then(|x| x.as_str())
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("claude-a-{idx:06}"));
                    for (i, th) in thoughts.into_iter().enumerate() {
                        if th.trim().is_empty() {
                            continue;
                        }
                        events.push(thought_event(session_id, &th, &format!("{created}-t{i}")));
                    }
                    for (i, (name, id)) in tools.into_iter().enumerate() {
                        events.push(tool_event(
                            session_id,
                            &id,
                            &name,
                            &format!("{created}-tc{i}"),
                            None,
                        ));
                    }
                    if !text.trim().is_empty() {
                        events.push(assistant_event(
                            session_id,
                            &text,
                            &created,
                            self.agent_id(),
                        ));
                    }
                }
                _ => {}
            }
        }
        Ok(events)
    }
}

fn extract_claude_message_text(message: Option<&Value>) -> String {
    extract_text(message.and_then(|m| m.get("content")).or(message))
}

fn extract_assistant_parts(message: Option<&Value>) -> (String, Vec<String>, Vec<(String, String)>) {
    let mut text_parts = Vec::new();
    let mut thoughts = Vec::new();
    let mut tools = Vec::new();
    let Some(message) = message else {
        return (String::new(), thoughts, tools);
    };
    let Some(arr) = message.get("content").and_then(|x| x.as_array()) else {
        if let Some(s) = message.get("content").and_then(|x| x.as_str()) {
            return (s.to_string(), thoughts, tools);
        }
        return (String::new(), thoughts, tools);
    };
    for item in arr {
        let ty = item.get("type").and_then(|x| x.as_str()).unwrap_or("");
        match ty {
            "text" => {
                if let Some(t) = item.get("text").and_then(|x| x.as_str()) {
                    text_parts.push(t.to_string());
                }
            }
            "thinking" => {
                if let Some(t) = item
                    .get("thinking")
                    .or_else(|| item.get("text"))
                    .and_then(|x| x.as_str())
                {
                    thoughts.push(t.to_string());
                }
            }
            "tool_use" => {
                let name = item
                    .get("name")
                    .and_then(|x| x.as_str())
                    .unwrap_or("tool")
                    .to_string();
                let id = item
                    .get("id")
                    .and_then(|x| x.as_str())
                    .unwrap_or("tool")
                    .to_string();
                tools.push((name, id));
            }
            _ => {}
        }
    }
    (text_parts.join("\n"), thoughts, tools)
}
