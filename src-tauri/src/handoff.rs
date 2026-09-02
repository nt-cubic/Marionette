use crate::models::HandoffResult;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Build per-dialog handoff + composer prefill from recent transcript.
/// Writes the per-dialog handoff and the latest shortcut under the supplied
/// metadata directory. Projects pass their `.marionette` directory; Chat
/// passes the global Chat directory.
/// Returns `Ok(None)` — writing nothing — when the dialog has no messages yet,
/// so switching agents on a fresh dialog does not create an empty handoff.
/// Does not auto-send; does not read arbitrary repo files.
pub fn generate_handoff(
    project_id: &str,
    project_root: &Path,
    project_name: &str,
    session_id: &str,
    session_label: &str,
    source_agent_id: &str,
    source_agent_label: &str,
    target_agent_id: &str,
    target_agent_label: &str,
    transcript_path: &Path,
) -> Result<Option<HandoffResult>, String> {
    generate_handoff_with_storage(
        project_id,
        project_root,
        project_name,
        session_id,
        session_label,
        source_agent_id,
        source_agent_label,
        target_agent_id,
        target_agent_label,
        transcript_path,
        &crate::app_paths::project_dir(project_root),
    )
}

/// Same handoff generation with an explicit metadata directory. Chat sessions
/// use this to keep handoff files global instead of creating `.marionette` in
/// the Chat session's working folder.
pub fn generate_handoff_with_storage(
    project_id: &str,
    project_root: &Path,
    project_name: &str,
    session_id: &str,
    session_label: &str,
    source_agent_id: &str,
    source_agent_label: &str,
    target_agent_id: &str,
    target_agent_label: &str,
    transcript_path: &Path,
    storage_root: &Path,
) -> Result<Option<HandoffResult>, String> {
    let events = load_transcript_events(transcript_path);
    if !has_transcript_content(&events) {
        return Ok(None);
    }
    let user_msgs = recent_texts(&events, "user_message", 8);
    let assistant_msgs = recent_texts(&events, "assistant_message", 4);
    let tool_titles = recent_tool_titles(&events, 6);

    let created_at = iso_now();
    let safe_id = sanitize_session_id(session_id);
    let handoff_dir = storage_root.join("handoff");
    fs::create_dir_all(&handoff_dir).map_err(|e| format!("Create handoff dir failed: {e}"))?;

    let handoff_path = handoff_dir.join(format!("{safe_id}.md"));
    let latest_path = storage_root.join("handoff.md");

    let body = render_markdown(
        &created_at,
        project_name,
        project_root,
        session_id,
        session_label,
        source_agent_id,
        source_agent_label,
        target_agent_id,
        target_agent_label,
        &user_msgs,
        &assistant_msgs,
        &tool_titles,
    );
    fs::write(&handoff_path, &body).map_err(|e| format!("Write session handoff failed: {e}"))?;
    // Latest pointer for humans / CLI (overwritten each switch).
    let _ = fs::write(
        &latest_path,
        format!(
            "<!-- latest handoff for session `{session_id}` — also at handoff/{safe_id}.md -->\n\n{body}"
        ),
    );

    let prompt = render_prefill(
        project_name,
        project_root,
        session_label,
        source_agent_label,
        target_agent_label,
        &user_msgs,
        &assistant_msgs,
        &handoff_path,
        &latest_path,
    );

    Ok(Some(HandoffResult {
        project_id: project_id.to_string(),
        target_agent_id: target_agent_id.to_string(),
        handoff_path: handoff_path.to_string_lossy().to_string(),
        prompt,
        created_at,
        summary: first_line_summary(&user_msgs, session_label),
    }))
}

/// True when the dialog has anything worth handing off: a user message, an
/// assistant reply, a thought, or a tool call. A `handoff_prepared` event from
/// a previous switch does not count — switching agents twice on a fresh dialog
/// must not start producing notes.
fn has_transcript_content(events: &[Value]) -> bool {
    events.iter().any(|e| {
        match e.get("type").and_then(Value::as_str) {
            Some("user_message" | "assistant_message" | "thought") => e
                .get("text")
                .and_then(Value::as_str)
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false),
            Some("tool_call") => true,
            _ => false,
        }
    })
}

fn sanitize_session_id(session_id: &str) -> String {
    session_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn load_transcript_events(path: &Path) -> Vec<Value> {
    let Ok(raw) = fs::read_to_string(path) else {
        return Vec::new();
    };
    raw.lines()
        .filter_map(|line| {
            let t = line.trim();
            if t.is_empty() {
                return None;
            }
            serde_json::from_str::<Value>(t).ok()
        })
        .collect()
}

fn recent_texts(events: &[Value], event_type: &str, limit: usize) -> Vec<String> {
    events
        .iter()
        .rev()
        .filter(|e| e.get("type").and_then(Value::as_str) == Some(event_type))
        .filter_map(|e| {
            e.get("text")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| truncate(s, 600))
        })
        .take(limit)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

fn recent_tool_titles(events: &[Value], limit: usize) -> Vec<String> {
    events
        .iter()
        .rev()
        .filter(|e| e.get("type").and_then(Value::as_str) == Some("tool_call"))
        .filter_map(|e| {
            e.get("title")
                .and_then(Value::as_str)
                .or_else(|| e.get("text").and_then(Value::as_str))
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| truncate(s, 120))
        })
        .take(limit)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

fn render_markdown(
    created_at: &str,
    project_name: &str,
    project_root: &Path,
    session_id: &str,
    session_label: &str,
    source_agent_id: &str,
    source_agent_label: &str,
    target_agent_id: &str,
    target_agent_label: &str,
    user_msgs: &[String],
    assistant_msgs: &[String],
    tool_titles: &[String],
) -> String {
    let mut out = String::new();
    out.push_str("# Marionette Handoff\n\n");
    out.push_str(&format!("- Generated: `{created_at}`\n"));
    out.push_str(&format!("- Project: **{project_name}** (`{}`)\n", project_root.display()));
    out.push_str(&format!("- Session: **{session_label}** (`{session_id}`)\n"));
    out.push_str(&format!(
        "- From: **{source_agent_label}** (`{source_agent_id}`)\n"
    ));
    out.push_str(&format!(
        "- To: **{target_agent_label}** (`{target_agent_id}`)\n\n"
    ));
    out.push_str("## Recent user messages\n\n");
    if user_msgs.is_empty() {
        out.push_str("_No user messages in transcript yet._\n\n");
    } else {
        for (i, msg) in user_msgs.iter().enumerate() {
            out.push_str(&format!("{}. {}\n\n", i + 1, msg));
        }
    }
    out.push_str("## Recent assistant replies\n\n");
    if assistant_msgs.is_empty() {
        out.push_str("_None yet._\n\n");
    } else {
        for msg in assistant_msgs {
            out.push_str(&format!("- {}\n", first_paragraph(msg, 400)));
        }
        out.push('\n');
    }
    if !tool_titles.is_empty() {
        out.push_str("## Recent tools\n\n");
        for t in tool_titles {
            out.push_str(&format!("- `{t}`\n"));
        }
        out.push('\n');
    }
    out.push_str("## Instructions for next agent\n\n");
    out.push_str("Continue from the context above. Prefer reading project files over guessing.\n");
    out.push_str("Do not repeat completed work. Confirm before destructive actions.\n");
    out
}

fn render_prefill(
    project_name: &str,
    project_root: &Path,
    session_label: &str,
    source_agent_label: &str,
    target_agent_label: &str,
    user_msgs: &[String],
    assistant_msgs: &[String],
    handoff_path: &Path,
    latest_path: &Path,
) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "Continue from a Marionette handoff (previous agent: {source_agent_label} → you: {target_agent_label}).\n\n"
    ));
    out.push_str(&format!("Project: {project_name} ({})\n", project_root.display()));
    out.push_str(&format!("Session: {session_label}\n"));
    // Prefer relative-looking path under project for agent readability.
    let rel = handoff_path
        .strip_prefix(project_root)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| handoff_path.display().to_string());
    let latest = latest_path
        .strip_prefix(project_root)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| latest_path.display().to_string());
    out.push_str(&format!("Full notes: `{rel}` (latest shortcut: `{latest}`)\n\n"));
    if !user_msgs.is_empty() {
        out.push_str("Recent user asks:\n");
        let start = user_msgs.len().saturating_sub(3);
        for (i, msg) in user_msgs.iter().enumerate().skip(start) {
            out.push_str(&format!("{}. {}\n", i + 1, truncate(msg, 280)));
        }
        out.push('\n');
    }
    if let Some(last) = assistant_msgs.last() {
        out.push_str("Last assistant summary:\n");
        out.push_str(&first_paragraph(last, 360));
        out.push_str("\n\n");
    }
    out.push_str("Please continue the work. Ask only if a critical decision is blocked.\n");
    out
}

fn first_line_summary(user_msgs: &[String], session_label: &str) -> String {
    user_msgs
        .last()
        .map(|s| truncate(s, 100))
        .unwrap_or_else(|| session_label.to_string())
}

fn first_paragraph(text: &str, max: usize) -> String {
    let para = text.split("\n\n").next().unwrap_or(text).trim();
    truncate(para, max)
}

fn truncate(s: &str, max: usize) -> String {
    let t = s.trim();
    if t.chars().count() <= max {
        return t.to_string();
    }
    let clipped: String = t.chars().take(max.saturating_sub(1)).collect();
    format!("{clipped}…")
}

fn iso_now() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("{millis}")
}

#[allow(dead_code)]
pub fn handoff_path_for(project_root: &Path) -> PathBuf {
    crate::app_paths::project_dir(project_root).join("handoff.md")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    /// Fresh temp project dir; returns (project_root, transcript_path).
    fn temp_project(transcript: Option<&str>) -> (PathBuf, PathBuf) {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let tmp = std::env::temp_dir().join(format!(
            "marionette-handoff-test-{}-{}",
            std::process::id(),
            n
        ));
        let root = tmp.join("proj");
        fs::create_dir_all(&root).unwrap();
        let transcript_path = tmp.join("transcript.jsonl");
        if let Some(body) = transcript {
            fs::write(&transcript_path, body).unwrap();
        }
        (root, transcript_path)
    }

    fn cleanup(root: &Path) {
        let _ = fs::remove_dir_all(root.parent().expect("parent dir"));
    }

    fn gen(root: &Path, transcript: &Path) -> Result<Option<HandoffResult>, String> {
        generate_handoff(
            "p-test",
            root,
            "TestProj",
            "s-test",
            "New session",
            "agent-a",
            "Agent A",
            "agent-b",
            "Agent B",
            transcript,
        )
    }

    #[test]
    fn fresh_dialog_skips_handoff() {
        let (root, transcript) = temp_project(None);
        let res = gen(&root, &transcript).expect("generate");
        assert!(res.is_none(), "empty dialog must not produce a handoff");
        assert!(!root.join(".marionette").join("handoff.md").exists());
        assert!(!root.join(".marionette").join("handoff").join("s-test.md").exists());
        cleanup(&root);
    }

    #[test]
    fn dialog_with_only_prior_handoff_still_skips() {
        let body = r#"{"type":"handoff_prepared","sessionId":"s-test","targetAgentId":"agent-b","handoffPath":"x.md","prompt":"p","createdAt":"2026-01-01T00:00:00.000Z"}"#;
        let (root, transcript) = temp_project(Some(body));
        assert!(gen(&root, &transcript).expect("generate").is_none());
        assert!(!root.join(".marionette").join("handoff.md").exists());
        cleanup(&root);
    }

    #[test]
    fn dialog_with_user_message_generates_handoff() {
        let body = "{\"type\":\"user_message\",\"sessionId\":\"s-test\",\"text\":\"hello\",\"createdAt\":\"2026-01-01T00:00:00.000Z\"}\n";
        let (root, transcript) = temp_project(Some(body));
        let res = gen(&root, &transcript).expect("generate");
        let handoff = res.expect("dialog with messages must produce a handoff");
        assert!(!handoff.handoff_path.is_empty());
        assert!(root.join(".marionette").join("handoff.md").exists());
        cleanup(&root);
    }

    #[test]
    fn explicit_storage_root_keeps_chat_handoff_out_of_workspace() {
        let body = "{\"type\":\"user_message\",\"sessionId\":\"s-chat\",\"text\":\"hello\",\"createdAt\":\"2026-01-01T00:00:00.000Z\"}\n";
        let (root, transcript) = temp_project(Some(body));
        let storage_root = root.parent().unwrap().join("chat-data");
        let result = generate_handoff_with_storage(
            "project-chat",
            &root,
            "聊天",
            "s-chat",
            "New chat",
            "agent-a",
            "Agent A",
            "agent-b",
            "Agent B",
            &transcript,
            &storage_root,
        )
        .expect("generate")
        .expect("message produces handoff");
        assert!(Path::new(&result.handoff_path).starts_with(&storage_root));
        assert!(storage_root.join("handoff.md").is_file());
        assert!(!root.join(".marionette").exists());
        cleanup(&root);
    }

    #[test]
    fn whitespace_only_messages_skip_handoff() {
        let body = "{\"type\":\"user_message\",\"sessionId\":\"s-test\",\"text\":\"   \",\"createdAt\":\"2026-01-01T00:00:00.000Z\"}\n";
        let (root, transcript) = temp_project(Some(body));
        assert!(gen(&root, &transcript).expect("generate").is_none());
        cleanup(&root);
    }
}
