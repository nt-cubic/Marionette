//! JSONL / summary.json stores: Kimi Code, CodeBuddy, OpenClaw.

use super::path_norm::cwd_matches_project;
use super::pi::load_pi_jsonl;
use super::util::{
    clip_chars, env_dir, extract_text, home_join, json_str, pointer_str, read_json_file,
    read_subdirs, walk_files,
};
use super::{AgentParser, ExternalConversation};
use serde_json::Value;
use std::path::{Path, PathBuf};

fn scan_jsonl_cwd_title(path: &Path) -> (String, String, Option<String>, Option<String>) {
    let mut cwd = String::new();
    let mut id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("session")
        .to_string();
    let mut started = None;
    let mut last = None;
    let mut title = None;
    let Ok(file) = std::fs::File::open(path) else {
        return (id, cwd, started, title);
    };
    use std::io::BufRead;
    for (i, line) in std::io::BufReader::new(file).lines().enumerate() {
        if i > 80 && !cwd.is_empty() && title.is_some() {
            break;
        }
        let Ok(line) = line else { continue };
        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        if let Some(ts) = v
            .get("timestamp")
            .or_else(|| v.get("createdAt"))
            .and_then(|x| x.as_str())
        {
            if started.is_none() {
                started = Some(ts.to_string());
            }
            last = Some(ts.to_string());
        }
        if let Some(s) = json_str(&v, "id").or_else(|| json_str(&v, "sessionId")) {
            id = s.to_string();
        }
        if cwd.is_empty() {
            if let Some(c) = json_str(&v, "cwd")
                .or_else(|| pointer_str(&v, "/info/cwd"))
                .or_else(|| json_str(&v, "workingDirectory"))
            {
                cwd = c.to_string();
            }
        }
        if title.is_none() {
            if let Some(t) = json_str(&v, "title").or_else(|| json_str(&v, "generated_title")) {
                title = Some(t.to_string());
            } else if v.get("type").and_then(|x| x.as_str()) == Some("message")
                && v.pointer("/message/role").and_then(|x| x.as_str()) == Some("user")
            {
                let text = extract_text(v.pointer("/message/content"));
                if !text.trim().is_empty() {
                    title = Some(clip_chars(&text.replace('\n', " "), 80));
                }
            } else if v.get("type").and_then(|x| x.as_str()) == Some("user") {
                let text = extract_text(v.get("message").and_then(|m| m.get("content")).or(v.get("content")));
                if !text.trim().is_empty() {
                    title = Some(clip_chars(&text.replace('\n', " "), 80));
                }
            }
        }
    }
    (id, cwd, started.or(last.clone()), title)
}

pub(crate) fn list_tree_for(
    source: &str,
    agent_id: &str,
    roots: &[PathBuf],
    project_root: &str,
) -> Vec<ExternalConversation> {
    list_jsonl_tree(source, agent_id, roots, project_root)
}

pub(crate) fn load_tree_locator(locator: &str, agent_id: &str) -> Result<Vec<Value>, String> {
    load_locator(locator, agent_id)
}

fn list_jsonl_tree(
    source: &str,
    agent_id: &str,
    roots: &[PathBuf],
    project_root: &str,
) -> Vec<ExternalConversation> {
    let mut out = Vec::new();
    for root in roots {
        if !root.exists() {
            continue;
        }
        // Grok-like: <cwd-encoded>/<uuid>/summary.json
        for group in read_subdirs(root) {
            for session_dir in read_subdirs(&group) {
                let summary = session_dir.join("summary.json");
                if summary.is_file() {
                    if let Some(v) = read_json_file(&summary) {
                        let cwd = pointer_str(&v, "/info/cwd")
                            .or_else(|| json_str(&v, "cwd"))
                            .unwrap_or("")
                            .to_string();
                        if cwd.is_empty() || !cwd_matches_project(&cwd, project_root) {
                            continue;
                        }
                        let native_id = pointer_str(&v, "/info/id")
                            .or_else(|| json_str(&v, "id"))
                            .unwrap_or("")
                            .to_string();
                        let native_id = if native_id.is_empty() {
                            session_dir
                                .file_name()
                                .and_then(|s| s.to_str())
                                .unwrap_or("session")
                                .to_string()
                        } else {
                            native_id
                        };
                        let title = json_str(&v, "generated_title")
                            .or_else(|| json_str(&v, "title"))
                            .or_else(|| json_str(&v, "session_summary"))
                            .unwrap_or("session")
                            .to_string();
                        out.push(ExternalConversation {
                            id: format!("{source}:{native_id}"),
                            source: source.into(),
                            agent_id: agent_id.into(),
                            title,
                            cwd,
                            started_at: json_str(&v, "created_at").map(str::to_string),
                            last_active_at: json_str(&v, "last_active_at")
                                .or_else(|| json_str(&v, "updated_at"))
                                .map(str::to_string),
                            native_id,
                            locator: session_dir.to_string_lossy().into_owned(),
                        });
                    }
                }
            }
        }
        for path in walk_files(root, 5) {
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let (native_id, cwd, started, title) = scan_jsonl_cwd_title(&path);
            if cwd.is_empty() || !cwd_matches_project(&cwd, project_root) {
                continue;
            }
            let title = title.unwrap_or_else(|| format!("{source} {native_id}"));
            out.push(ExternalConversation {
                id: format!("{source}:{native_id}"),
                source: source.into(),
                agent_id: agent_id.into(),
                title,
                cwd,
                started_at: started.clone(),
                last_active_at: started,
                native_id,
                locator: path.to_string_lossy().into_owned(),
            });
        }
    }
    out
}

fn load_locator(locator: &str, agent_id: &str) -> Result<Vec<Value>, String> {
    let path = Path::new(locator);
    if path.is_dir() {
        for name in ["updates.jsonl", "chat_history.jsonl", "session.jsonl", "events.jsonl"] {
            let p = path.join(name);
            if p.is_file() {
                return load_pi_jsonl(&p, agent_id);
            }
        }
        return Err(format!("no transcript in {}", path.display()));
    }
    load_pi_jsonl(path, agent_id)
}

pub struct KimiParser;
impl AgentParser for KimiParser {
    fn source(&self) -> &'static str {
        "kimi"
    }
    fn agent_id(&self) -> &'static str {
        "kimi-code"
    }
    fn list(&self, project_root: &str) -> Result<Vec<ExternalConversation>, String> {
        let home = env_dir("KIMI_CODE_HOME").or_else(|| home_join(".kimi-code"));
        let roots = home
            .map(|h| vec![h.join("sessions"), h])
            .unwrap_or_default();
        Ok(list_jsonl_tree(self.source(), self.agent_id(), &roots, project_root))
    }
    fn load(&self, locator: &str) -> Result<Vec<Value>, String> {
        load_locator(locator, self.agent_id())
    }
}

pub struct CodeBuddyParser;
impl AgentParser for CodeBuddyParser {
    fn source(&self) -> &'static str {
        "codebuddy"
    }
    fn agent_id(&self) -> &'static str {
        "codebuddy"
    }
    fn list(&self, project_root: &str) -> Result<Vec<ExternalConversation>, String> {
        let home = env_dir("CODEBUDDY_CONFIG_DIR").or_else(|| home_join(".codebuddy"));
        let roots = home.map(|h| vec![h.join("sessions"), h]).unwrap_or_default();
        Ok(list_jsonl_tree(self.source(), self.agent_id(), &roots, project_root))
    }
    fn load(&self, locator: &str) -> Result<Vec<Value>, String> {
        load_locator(locator, self.agent_id())
    }
}

pub struct OpenClawParser;
impl AgentParser for OpenClawParser {
    fn source(&self) -> &'static str {
        "openclaw"
    }
    fn agent_id(&self) -> &'static str {
        "openclaw"
    }
    fn list(&self, project_root: &str) -> Result<Vec<ExternalConversation>, String> {
        let home = home_join(".openclaw");
        let roots = home
            .map(|h| vec![h.join("agents"), h.join("sessions"), h])
            .unwrap_or_default();
        Ok(list_jsonl_tree(self.source(), self.agent_id(), &roots, project_root))
    }
    fn load(&self, locator: &str) -> Result<Vec<Value>, String> {
        load_locator(locator, self.agent_id())
    }
}
