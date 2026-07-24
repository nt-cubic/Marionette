use crate::models::HandoffResult;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Build per-dialog handoff + composer prefill from recent transcript.
/// Writes:
///   `.agentshell/handoff/{sessionId}.md`  (SSOT for this dialog)
///   `.agentshell/handoff.md`              (latest shortcut for CLI / humans)
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
) -> Result<HandoffResult, String> {
    let events = load_transcript_events(transcript_path);
    let user_msgs = recent_texts(&events, "user_message", 8);
    let assistant_msgs = recent_texts(&events, "assistant_message", 4);
    let tool_titles = recent_tool_titles(&events, 6);

    let created_at = iso_now();
    let safe_id = sanitize_session_id(session_id);
    let handoff_dir = project_root.join(".agentshell").join("handoff");
    fs::create_dir_all(&handoff_dir).map_err(|e| format!("Create handoff dir failed: {e}"))?;

    let handoff_path = handoff_dir.join(format!("{safe_id}.md"));
    let latest_path = project_root.join(".agentshell").join("handoff.md");

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
    );

    Ok(HandoffResult {
        project_id: project_id.to_string(),
        target_agent_id: target_agent_id.to_string(),
        handoff_path: handoff_path.to_string_lossy().to_string(),
        prompt,
        created_at,
        summary: first_line_summary(&user_msgs, session_label),
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

pub fn read_handoff_summary(project_root: &Path) -> Option<String> {
    let path = project_root.join(".agentshell").join("handoff.md");
    let text = fs::read_to_string(path).ok()?;
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    // Skip title lines; return a short preview.
    let mut preview = Vec::new();
    for line in lines.by_ref() {
        if line.starts_with('#') {
            continue;
        }
        preview.push(line.trim());
        if preview.len() >= 3 {
            break;
        }
    }
    if preview.is_empty() {
        None
    } else {
        Some(preview.join(" · "))
    }
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
    out.push_str("# AgentShell Handoff\n\n");
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
) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "Continue from an AgentShell handoff (previous agent: {source_agent_label} → you: {target_agent_label}).\n\n"
    ));
    out.push_str(&format!("Project: {project_name} ({})\n", project_root.display()));
    out.push_str(&format!("Session: {session_label}\n"));
    // Prefer relative-looking path under project for agent readability.
    let rel = handoff_path
        .strip_prefix(project_root)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| handoff_path.display().to_string());
    out.push_str(&format!("Full notes: `{rel}` (also `.agentshell/handoff.md` latest)\n\n"));
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
    project_root.join(".agentshell").join("handoff.md")
}
