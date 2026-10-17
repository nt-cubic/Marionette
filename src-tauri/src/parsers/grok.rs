//! Grok Build: `$GROK_HOME/sessions/<encoded-cwd>/<uuid>/{summary.json,updates.jsonl}`.

use super::path_norm::cwd_matches_project;
use super::util::{
    assistant_event, clip_chars, env_dir, extract_text, home_join, json_str, pointer_str,
    read_json_file, read_subdirs, thought_event, tool_event, user_event,
};
use super::{AgentParser, ExternalConversation};
use serde_json::Value;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

pub struct GrokParser;

fn grok_sessions_root() -> Option<PathBuf> {
    Some(env_dir("GROK_HOME").or_else(|| home_join(".grok"))?.join("sessions"))
}

fn summary_title(v: &Value) -> String {
    json_str(v, "generated_title")
        .or_else(|| json_str(v, "session_summary"))
        .unwrap_or("Grok session")
        .to_string()
}

fn summary_cwd(v: &Value) -> String {
    pointer_str(v, "/info/cwd")
        .or_else(|| json_str(v, "git_root_dir"))
        .unwrap_or("")
        .to_string()
}

impl AgentParser for GrokParser {
    fn source(&self) -> &'static str {
        "grok"
    }
    fn agent_id(&self) -> &'static str {
        "grok-build"
    }

    fn list(&self, project_root: &str) -> Result<Vec<ExternalConversation>, String> {
        let root = match grok_sessions_root() {
            Some(p) if p.is_dir() => p,
            _ => return Ok(Vec::new()),
        };
        list_from(&root, project_root, self.agent_id())
    }

    fn load(&self, locator: &str) -> Result<Vec<Value>, String> {
        load_from(Path::new(locator), self.agent_id())
    }
}

pub fn list_from(
    root: &Path,
    project_root: &str,
    agent_id: &str,
) -> Result<Vec<ExternalConversation>, String> {
    let mut out = Vec::new();
    for cwd_path in read_subdirs(root) {
        for session_dir in read_subdirs(&cwd_path) {
            let summary_path = session_dir.join("summary.json");
            if !summary_path.is_file() {
                continue;
            }
            let Some(summary) = read_json_file(&summary_path) else {
                continue;
            };
            if json_str(&summary, "session_kind") == Some("subagent") {
                continue;
            }
            if summary.get("num_messages").and_then(|x| x.as_u64()) == Some(0) {
                continue;
            }
            let cwd = summary_cwd(&summary);
            if cwd.is_empty() || !cwd_matches_project(&cwd, project_root) {
                continue;
            }
            let dir_name = session_dir
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown");
            let native_id = pointer_str(&summary, "/info/id").unwrap_or(dir_name).to_string();
            out.push(ExternalConversation {
                id: format!("grok:{native_id}"),
                source: "grok".into(),
                agent_id: agent_id.into(),
                title: summary_title(&summary),
                cwd,
                started_at: json_str(&summary, "created_at").map(str::to_string),
                last_active_at: json_str(&summary, "last_active_at")
                    .or_else(|| json_str(&summary, "updated_at"))
                    .map(str::to_string),
                native_id,
                locator: session_dir.to_string_lossy().into_owned(),
            });
        }
    }
    Ok(out)
}

fn load_from(dir: &Path, agent_id: &str) -> Result<Vec<Value>, String> {
    if !dir.is_dir() {
        return Err(format!("Grok session dir not found: {}", dir.display()));
    }
    let session_id = dir
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("grok");
    let updates = dir.join("updates.jsonl");
    if updates.is_file() {
        return load_updates_jsonl(&updates, session_id, agent_id);
    }
    let history = dir.join("chat_history.jsonl");
    if history.is_file() {
        return load_chat_history(&history, session_id, agent_id);
    }
    Err("No updates.jsonl or chat_history.jsonl in Grok session".into())
}

fn load_updates_jsonl(path: &Path, session_id: &str, agent_id: &str) -> Result<Vec<Value>, String> {
    let file = fs::File::open(path).map_err(|e| format!("read updates: {e}"))?;
    let reader = BufReader::new(file);
    let mut events = Vec::new();
    let mut user_buf = String::new();
    let mut thought_buf = String::new();
    let mut assistant_buf = String::new();
    let mut idx: u64 = 0;

    let flush = |buf: &mut String, kind: &str, events: &mut Vec<Value>, idx: &mut u64| {
        if buf.trim().is_empty() {
            return;
        }
        *idx += 1;
        let created = format!("grok-{kind}-{idx:06}");
        let text = std::mem::take(buf);
        events.push(match kind {
            "u" => user_event(session_id, &text, &created),
            "t" => thought_event(session_id, &text, &created),
            _ => assistant_event(session_id, &text, &created, agent_id),
        });
    };

    for line in reader.lines() {
        let Ok(line) = line else { continue };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let update = v.pointer("/params/update").cloned().unwrap_or(Value::Null);
        let kind = update
            .get("sessionUpdate")
            .and_then(|x| x.as_str())
            .unwrap_or("");
        match kind {
            "user_message_chunk" => {
                flush(&mut thought_buf, "t", &mut events, &mut idx);
                flush(&mut assistant_buf, "a", &mut events, &mut idx);
                if let Some(t) = update.pointer("/content/text").and_then(|x| x.as_str()) {
                    user_buf.push_str(t);
                }
            }
            "agent_thought_chunk" => {
                flush(&mut user_buf, "u", &mut events, &mut idx);
                flush(&mut assistant_buf, "a", &mut events, &mut idx);
                if let Some(t) = update.pointer("/content/text").and_then(|x| x.as_str()) {
                    thought_buf.push_str(t);
                }
            }
            "agent_message_chunk" => {
                flush(&mut user_buf, "u", &mut events, &mut idx);
                flush(&mut thought_buf, "t", &mut events, &mut idx);
                if let Some(t) = update.pointer("/content/text").and_then(|x| x.as_str()) {
                    assistant_buf.push_str(t);
                }
            }
            "tool_call" | "tool_call_update" => {
                flush(&mut user_buf, "u", &mut events, &mut idx);
                flush(&mut thought_buf, "t", &mut events, &mut idx);
                flush(&mut assistant_buf, "a", &mut events, &mut idx);
                if kind == "tool_call" {
                    let id = update
                        .get("toolCallId")
                        .and_then(|x| x.as_str())
                        .unwrap_or("tool");
                    let title = update
                        .get("title")
                        .or_else(|| update.get("kind"))
                        .and_then(|x| x.as_str())
                        .unwrap_or("tool");
                    idx += 1;
                    events.push(tool_event(
                        session_id,
                        id,
                        title,
                        &format!("grok-tc-{idx:06}"),
                        None,
                    ));
                }
            }
            "turn_completed" => {
                flush(&mut user_buf, "u", &mut events, &mut idx);
                flush(&mut thought_buf, "t", &mut events, &mut idx);
                flush(&mut assistant_buf, "a", &mut events, &mut idx);
            }
            _ => {}
        }
    }
    flush(&mut user_buf, "u", &mut events, &mut idx);
    flush(&mut thought_buf, "t", &mut events, &mut idx);
    flush(&mut assistant_buf, "a", &mut events, &mut idx);
    Ok(events)
}

fn load_chat_history(path: &Path, session_id: &str, agent_id: &str) -> Result<Vec<Value>, String> {
    let file = fs::File::open(path).map_err(|e| format!("read chat_history: {e}"))?;
    let reader = BufReader::new(file);
    let mut events = Vec::new();
    let mut idx: u64 = 0;
    for line in reader.lines() {
        let Ok(line) = line else { continue };
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
            "user" => {
                let text = extract_text(v.get("content"));
                if !text.trim().is_empty() {
                    events.push(user_event(session_id, &text, &created));
                }
            }
            "assistant" => {
                let text = extract_text(v.get("content"));
                if !text.trim().is_empty() {
                    events.push(assistant_event(session_id, &text, &created, agent_id));
                }
                if let Some(calls) = v.get("tool_calls").and_then(|x| x.as_array()) {
                    for (i, call) in calls.iter().enumerate() {
                        let name = call
                            .get("name")
                            .or_else(|| call.pointer("/function/name"))
                            .and_then(|x| x.as_str())
                            .unwrap_or("tool");
                        let id = call.get("id").and_then(|x| x.as_str()).unwrap_or("tool");
                        events.push(tool_event(
                            session_id,
                            id,
                            name,
                            &format!("{created}-tc{i}"),
                            None,
                        ));
                    }
                }
            }
            "reasoning" => {
                let text = extract_text(v.get("content"));
                if !text.trim().is_empty() {
                    events.push(thought_event(session_id, &text, &created));
                }
            }
            "tool_result" => {
                let id = v
                    .get("tool_call_id")
                    .and_then(|x| x.as_str())
                    .unwrap_or("tool");
                let detail = extract_text(v.get("content"));
                events.push(tool_event(
                    session_id,
                    id,
                    "tool_result",
                    &created,
                    Some(&clip_chars(&detail, 4000)),
                ));
            }
            _ => {}
        }
    }
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn lists_only_matching_project_and_skips_subagents() {
        let tmp = std::env::temp_dir().join(format!("marionette-grok-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let sess = tmp
            .join("sessions")
            .join("D%3A%5CProjects%5CMarionette")
            .join("01aaa");
        fs::create_dir_all(&sess).unwrap();
        fs::write(
            sess.join("summary.json"),
            r#"{
              "info": {"id": "01aaa", "cwd": "D:\\Projects\\Marionette"},
              "generated_title": "Fix the freeze",
              "created_at": "2026-08-24T15:52:48Z",
              "last_active_at": "2026-08-24T16:10:48Z",
              "num_messages": 12
            }"#,
        )
        .unwrap();
        let sub = tmp
            .join("sessions")
            .join("D%3A%5CProjects%5CMarionette")
            .join("01sub");
        fs::create_dir_all(&sub).unwrap();
        fs::write(
            sub.join("summary.json"),
            r#"{"info":{"id":"01sub","cwd":"D:\\Projects\\Marionette"},"session_kind":"subagent","num_messages":3}"#,
        )
        .unwrap();
        let other = tmp
            .join("sessions")
            .join("D%3A%5Ctmp")
            .join("01bbb");
        fs::create_dir_all(&other).unwrap();
        fs::write(
            other.join("summary.json"),
            r#"{"info":{"id":"01bbb","cwd":"D:\\tmp"},"generated_title":"noise","num_messages":4}"#,
        )
        .unwrap();

        let rows = list_from(
            &tmp.join("sessions"),
            r"D:\Projects\Marionette",
            "grok-build",
        )
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].native_id, "01aaa");
        assert_eq!(rows[0].title, "Fix the freeze");
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn live_marionette_project_is_not_polluted_by_other_cwds() {
        let Some(home) = crate::parsers::util::home_dir() else {
            return;
        };
        let root = home.join(".grok").join("sessions");
        if !root.is_dir() {
            return;
        }
        let rows = list_from(&root, r"D:\Projects\Marionette", "grok-build").unwrap();
        for row in &rows {
            assert!(
                crate::parsers::path_norm::cwd_matches_project(&row.cwd, r"D:\Projects\Marionette"),
                "leaked cwd {}",
                row.cwd
            );
        }
    }
}
