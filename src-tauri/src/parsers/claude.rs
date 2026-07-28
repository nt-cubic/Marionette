//! Claude Code sessions: `~/.claude/projects/<encoded-path>/*.jsonl`

use super::path_norm::{cwd_matches_project, normalize_path};
use super::{AgentParser, ExternalConversation};
use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

pub struct ClaudeParser;

fn claude_projects_root() -> Option<PathBuf> {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
    Some(PathBuf::from(home).join(".claude").join("projects"))
}

/// Claude encodes `D:\Myself\Foo` as `D--Myself-Foo` (drive + double-dash, then `-` for separators).
fn encode_project_dir(project_root: &str) -> String {
    let n = normalize_path(project_root);
    // `D:\path` → `D--path` with `\` → `-`
    if n.len() >= 2 && n.as_bytes()[1] == b':' {
        let drive = n.chars().next().unwrap();
        let rest = n[2..].trim_start_matches(['\\', '/']);
        let rest = rest.replace(['\\', '/'], "-");
        format!("{drive}--{rest}")
    } else {
        n.replace(['\\', '/'], "-")
    }
}

fn candidate_project_dirs(root: &Path, project_root: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let encoded = encode_project_dir(project_root);
    let direct = root.join(&encoded);
    if direct.is_dir() {
        out.push(direct);
    }
    // Also match dirs that encode this path as a prefix (subfolders) or case variants.
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
    let reader = BufReader::new(file);
    for (i, line) in reader.lines().enumerate() {
        if i > 400 {
            break;
        }
        let Ok(line) = line else { continue };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
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
        if ty == "custom-title" {
            if let Some(t) = v
                .get("customTitle")
                .or_else(|| v.get("title"))
                .and_then(|x| x.as_str())
            {
                if !t.trim().is_empty() {
                    meta.title = Some(t.to_string());
                }
            }
        }
        if ty == "ai-title" && meta.title.is_none() {
            if let Some(t) = v
                .get("title")
                .or_else(|| v.get("aiTitle"))
                .and_then(|x| x.as_str())
            {
                if !t.trim().is_empty() {
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
        // Fast path: have enough for list row.
        if meta.title.is_some() && meta.cwd.is_some() && i > 20 {
            // keep scanning a bit for title if only cwd so far
        }
    }
    meta
}

impl AgentParser for ClaudeParser {
    fn source(&self) -> &'static str {
        "claude"
    }

    fn list(&self, project_root: &str) -> Result<Vec<ExternalConversation>, String> {
        let root = match claude_projects_root() {
            Some(p) if p.is_dir() => p,
            _ => return Ok(Vec::new()),
        };
        let dirs = candidate_project_dirs(&root, project_root);
        let mut out = Vec::new();
        for dir in dirs {
            let rd = match fs::read_dir(&dir) {
                Ok(d) => d,
                Err(_) => continue,
            };
            for ent in rd.flatten() {
                let path = ent.path();
                if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                    continue;
                }
                let meta = scan_list_meta(&path);
                let cwd = meta.cwd.unwrap_or_default();
                // If cwd missing, still allow when dir encoding matched this project.
                if !cwd.is_empty() && !cwd_matches_project(&cwd, project_root) {
                    continue;
                }
                let native_id = meta
                    .session_id
                    .or_else(|| {
                        path.file_stem()
                            .and_then(|s| s.to_str())
                            .map(str::to_string)
                    })
                    .unwrap_or_else(|| "unknown".into());
                let title = meta
                    .title
                    .filter(|t| !t.trim().is_empty())
                    .unwrap_or_else(|| format!("Claude {native_id}"));
                let locator = path.to_string_lossy().to_string();
                out.push(ExternalConversation {
                    id: format!("claude:{native_id}"),
                    source: "claude".into(),
                    title,
                    cwd: if cwd.is_empty() {
                        project_root.to_string()
                    } else {
                        cwd
                    },
                    started_at: meta.started_at,
                    last_active_at: meta.last_active_at,
                    native_id,
                    locator,
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
        let text = fs::read_to_string(path).map_err(|e| format!("read claude jsonl: {e}"))?;
        let session_id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("claude");
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
            match ty {
                "user" => {
                    let text = extract_claude_message_text(v.get("message"));
                    if text.trim().is_empty() {
                        continue;
                    }
                    // Skip system-ish / meta noise that is huge
                    if text.starts_with("<command-") || text.starts_with("Caveat:") {
                        continue;
                    }
                    idx += 1;
                    let created = v
                        .get("timestamp")
                        .and_then(|x| x.as_str())
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("claude-u-{idx:06}"));
                    events.push(json!({
                        "type": "user_message",
                        "sessionId": session_id,
                        "text": text,
                        "createdAt": created,
                    }));
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
                        idx += 1;
                        events.push(json!({
                            "type": "thought",
                            "sessionId": session_id,
                            "text": th,
                            "createdAt": format!("{created}-t{i}"),
                        }));
                    }
                    for (i, (name, id)) in tools.into_iter().enumerate() {
                        idx += 1;
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
                    if !text.trim().is_empty() {
                        idx += 1;
                        events.push(json!({
                            "type": "assistant_message",
                            "sessionId": session_id,
                            "text": text,
                            "createdAt": created,
                            "agentId": "claude-code",
                        }));
                    }
                }
                _ => {}
            }
        }
        Ok(events)
    }
}

fn extract_claude_message_text(message: Option<&Value>) -> String {
    let Some(message) = message else {
        return String::new();
    };
    match message.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(arr)) => {
            let mut parts = Vec::new();
            for item in arr {
                let ty = item.get("type").and_then(|x| x.as_str()).unwrap_or("");
                if ty == "text" {
                    if let Some(t) = item.get("text").and_then(|x| x.as_str()) {
                        parts.push(t.to_string());
                    }
                }
            }
            parts.join("\n")
        }
        _ => String::new(),
    }
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

#[cfg(test)]
mod tests {
    use super::encode_project_dir;

    #[test]
    fn encodes_windows_path() {
        assert_eq!(
            encode_project_dir(r"D:\Myself\AgentsShell"),
            "D--Myself-AgentsShell"
        );
    }
}
