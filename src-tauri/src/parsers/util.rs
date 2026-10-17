//! Shared helpers for read-only external session parsers.

use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

/// Sortable activity time in epoch milliseconds. Accepts ISO-8601, epoch ms,
/// or epoch seconds (OpenCode stores millis as a decimal string).
pub fn recency_ms(raw: &str) -> u64 {
    let t = raw.trim();
    if t.is_empty() {
        return 0;
    }
    if let Ok(n) = t.parse::<u64>() {
        if n > 1_000_000_000_000 {
            return n;
        }
        if n > 1_000_000_000 {
            return n * 1000;
        }
        return n;
    }
    parse_iso_ms(t).unwrap_or(0)
}

fn parse_iso_ms(s: &str) -> Option<u64> {
    let s = s.trim().trim_end_matches('Z');
    if s.len() < 19 {
        return None;
    }
    let year: i64 = s.get(0..4)?.parse().ok()?;
    let month: i64 = s.get(5..7)?.parse().ok()?;
    let day: i64 = s.get(8..10)?.parse().ok()?;
    let hour: i64 = s.get(11..13)?.parse().ok()?;
    let min: i64 = s.get(14..16)?.parse().ok()?;
    let sec: i64 = s.get(17..19)?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let mut days = 0i64;
    for y in 1970..year {
        days += if is_leap(y) { 366 } else { 365 };
    }
    const CUM: [i64; 12] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
    days += CUM[(month as usize).saturating_sub(1)];
    if month > 2 && is_leap(year) {
        days += 1;
    }
    days += day - 1;
    let secs = days * 86400 + hour * 3600 + min * 60 + sec;
    if secs < 0 {
        return None;
    }
    Some((secs as u64).saturating_mul(1000))
}

fn is_leap(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

#[cfg(test)]
mod tests {
    use super::recency_ms;

    #[test]
    fn recency_parses_iso_and_epoch_millis() {
        let iso = recency_ms("2026-08-24T16:10:48Z");
        assert!(iso > 1_000_000_000_000);
        assert_eq!(recency_ms("1724500000000"), 1724500000000);
        assert!(recency_ms("2026-08-24T16:10:48Z") > recency_ms("2026-08-24T15:52:48Z"));
        assert_eq!(recency_ms(""), 0);
    }
}

pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

/// Non-empty env var as a path. Whitespace-only counts as unset.
pub fn env_dir(var: &str) -> Option<PathBuf> {
    let raw = std::env::var_os(var)?;
    if raw.is_empty() {
        return None;
    }
    let text = raw.to_string_lossy().into_owned();
    if text.trim().is_empty() {
        return None;
    }
    let mut p = PathBuf::from(raw);
    if let Some(stripped) = text
        .strip_prefix("~/")
        .or_else(|| text.strip_prefix("~\\"))
    {
        if let Some(home) = home_dir() {
            p = home.join(stripped);
        }
    }
    Some(p)
}

pub fn home_join(rel: &str) -> Option<PathBuf> {
    Some(home_dir()?.join(rel))
}

pub fn first_existing_file(paths: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    paths.into_iter().find(|p| p.is_file())
}

pub fn read_json_file(path: &Path) -> Option<Value> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn clip_chars(s: &str, max: usize) -> String {
    let t = s.trim();
    if t.chars().count() <= max {
        t.to_string()
    } else {
        let mut out: String = t.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

pub fn read_subdirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = fs::read_dir(dir) else {
        return Vec::new();
    };
    rd.flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect()
}

pub fn walk_files(root: &Path, max_depth: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(rd) = fs::read_dir(&dir) else {
            continue;
        };
        for ent in rd.flatten() {
            let p = ent.path();
            if p.is_dir() {
                if depth < max_depth {
                    stack.push((p, depth + 1));
                }
            } else {
                out.push(p);
            }
        }
    }
    out
}

pub fn json_str<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(|x| x.as_str()).map(str::trim).filter(|s| !s.is_empty())
}

pub fn pointer_str<'a>(v: &'a Value, pointer: &str) -> Option<&'a str> {
    v.pointer(pointer)
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// Pull concatenated text out of a string / `{text}` / content-block array.
pub fn extract_text(content: Option<&Value>) -> String {
    let Some(content) = content else {
        return String::new();
    };
    if let Some(s) = content.as_str() {
        return s.to_string();
    }
    if let Some(arr) = content.as_array() {
        let mut parts = Vec::new();
        for item in arr {
            let ty = item.get("type").and_then(|x| x.as_str()).unwrap_or("");
            if ty == "text" || ty == "input_text" || ty == "output_text" || ty.is_empty() {
                if let Some(t) = item.get("text").and_then(|x| x.as_str()) {
                    if !t.is_empty() {
                        parts.push(t.to_string());
                    }
                }
            } else if ty == "thinking" {
                if let Some(t) = item
                    .get("thinking")
                    .or_else(|| item.get("text"))
                    .and_then(|x| x.as_str())
                {
                    if !t.is_empty() {
                        parts.push(t.to_string());
                    }
                }
            }
        }
        return parts.join("\n");
    }
    if let Some(t) = content.get("text").and_then(|x| x.as_str()) {
        return t.to_string();
    }
    String::new()
}

pub fn user_event(session_id: &str, text: &str, created_at: &str) -> Value {
    json!({
        "type": "user_message",
        "sessionId": session_id,
        "text": text,
        "createdAt": created_at,
    })
}

pub fn assistant_event(session_id: &str, text: &str, created_at: &str, agent_id: &str) -> Value {
    json!({
        "type": "assistant_message",
        "sessionId": session_id,
        "text": text,
        "createdAt": created_at,
        "agentId": agent_id,
    })
}

pub fn thought_event(session_id: &str, text: &str, created_at: &str) -> Value {
    json!({
        "type": "thought",
        "sessionId": session_id,
        "text": text,
        "createdAt": created_at,
    })
}

pub fn tool_event(
    session_id: &str,
    tool_id: &str,
    name: &str,
    created_at: &str,
    detail: Option<&str>,
) -> Value {
    let mut v = json!({
        "type": "tool_call",
        "sessionId": session_id,
        "text": name,
        "toolCallId": tool_id,
        "title": name,
        "toolName": name,
        "status": "completed",
        "createdAt": created_at,
    });
    if let Some(d) = detail {
        if !d.is_empty() {
            v["detail"] = json!(clip_chars(d, 4000));
        }
    }
    v
}
