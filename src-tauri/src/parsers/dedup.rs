//! Match CLI conversations to dialogs Marionette already has.
//!
//! Refresh must not clone a conversation that was opened here: the agent CLI
//! store is a side effect of `session/new`, so those folders show up in the
//! scan even though the shelf already lists them.

use super::ExternalConversation;
use crate::models::Session;
use serde_json::Value;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

const HISTORY_MARK: &str = "[Marionette — prior conversation in this dialog]";
const TITLE_PREFIX_MIN: usize = 12;
const FIRST_USER_MIN: usize = 16;

#[derive(Clone, Debug)]
pub struct ExistingRow {
    pub id: String,
    pub agent_id: String,
    pub label: String,
    pub origin: Option<String>,
    pub native_ids: Vec<String>,
    pub first_user: String,
    pub live: bool,
    #[allow(dead_code)]
    pub resume_injection: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchKind {
    SameId,
    NativeId,
    Title,
    FirstUser,
}

pub fn row_from_session(session: &Session, live: bool) -> ExistingRow {
    let mut native_ids = session.native_session_ids.clone();
    if let Some(native) = native_id_from_import_id(&session.id) {
        if !native_ids.iter().any(|id| id == &native) {
            native_ids.push(native);
        }
    }
    let peek = peek_transcript(Path::new(&session.transcript_path));
    ExistingRow {
        id: session.id.clone(),
        agent_id: session.agent_id.clone(),
        label: session.label.clone(),
        origin: session.origin.clone(),
        native_ids,
        first_user: peek.first_user,
        live,
        resume_injection: peek.resume_injection,
    }
}

struct TranscriptPeek {
    first_user: String,
    resume_injection: bool,
}

fn peek_transcript(path: &Path) -> TranscriptPeek {
    let mut peek = TranscriptPeek {
        first_user: String::new(),
        resume_injection: false,
    };
    let Ok(file) = File::open(path) else {
        return peek;
    };
    for line in BufReader::new(file).lines() {
        let Ok(line) = line else { continue };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if event.get("type").and_then(Value::as_str) != Some("user_message") {
            continue;
        }
        let Some(text) = event.get("text").and_then(Value::as_str) else {
            continue;
        };
        if text.contains(HISTORY_MARK) {
            peek.resume_injection = true;
            continue;
        }
        if peek.first_user.is_empty() {
            peek.first_user = fingerprint(text);
        }
        if peek.resume_injection && !peek.first_user.is_empty() {
            break;
        }
    }
    peek
}

pub fn native_id_from_import_id(id: &str) -> Option<String> {
    let rest = id.strip_prefix("ext-")?;
    let (_source, native) = rest.split_once('-')?;
    if _source.is_empty() || native.is_empty() {
        return None;
    }
    Some(native.to_string())
}

pub fn is_import_row(row: &ExistingRow) -> bool {
    row.origin.as_deref() == Some("import") || row.id.starts_with("ext-")
}

pub fn events_are_resume_injection(events: &[Value]) -> bool {
    events.iter().any(|event| {
        event.get("type").and_then(Value::as_str) == Some("user_message")
            && event
                .get("text")
                .and_then(Value::as_str)
                .is_some_and(|text| text.contains(HISTORY_MARK))
    })
}

pub fn first_user_from_events(events: &[Value]) -> String {
    for event in events {
        if event.get("type").and_then(Value::as_str) != Some("user_message") {
            continue;
        }
        let Some(text) = event.get("text").and_then(Value::as_str) else {
            continue;
        };
        if text.contains(HISTORY_MARK) {
            continue;
        }
        let key = fingerprint(text);
        if !key.is_empty() {
            return key;
        }
    }
    String::new()
}

/// Find a shelf row that is the same conversation as this CLI scan hit.
pub fn find_match<'a>(
    rows: &'a [ExistingRow],
    conv: &ExternalConversation,
    import_id: &str,
    cli_first_user: Option<&str>,
) -> Option<(&'a ExistingRow, MatchKind)> {
    if let Some(row) = rows.iter().find(|row| row.id == import_id) {
        return Some((row, MatchKind::SameId));
    }
    if rows.iter().any(|row| native_hits(row, &conv.native_id)) {
        let row = rows.iter().find(|row| native_hits(row, &conv.native_id))?;
        return Some((row, MatchKind::NativeId));
    }
    if let Some(row) = rows.iter().find(|row| {
        row.agent_id == conv.agent_id && titles_match(&row.label, &conv.title)
    }) {
        return Some((row, MatchKind::Title));
    }
    let first = cli_first_user.unwrap_or("").trim();
    if first.chars().count() >= FIRST_USER_MIN {
        if let Some(row) = rows.iter().find(|row| {
            row.agent_id == conv.agent_id
                && row.first_user.chars().count() >= FIRST_USER_MIN
                && row.first_user == first
        }) {
            return Some((row, MatchKind::FirstUser));
        }
    }
    None
}

/// Import copies that duplicate a dialog opened in Marionette.
/// Each pair is `(import id to drop, local dialog to keep)`.
pub fn import_ids_to_drop(rows: &[ExistingRow]) -> Vec<(String, String)> {
    let mut drop_ids = Vec::new();
    for row in rows {
        if !is_import_row(row) || row.live {
            continue;
        }
        let twin = rows.iter().find(|other| {
            other.id != row.id
                && other.agent_id == row.agent_id
                && !is_import_row(other)
                && (titles_match(&other.label, &row.label)
                    || first_users_match(&other.first_user, &row.first_user)
                    || row
                        .native_ids
                        .iter()
                        .any(|native| native_hits(other, native)))
        });
        if let Some(twin) = twin {
            drop_ids.push((row.id.clone(), twin.id.clone()));
        }
    }
    drop_ids
}

pub fn titles_match(a: &str, b: &str) -> bool {
    let Some(left) = title_key(a) else {
        return false;
    };
    let Some(right) = title_key(b) else {
        return false;
    };
    if left == right {
        return true;
    }
    let (shorter, longer) = if left.chars().count() <= right.chars().count() {
        (left.as_str(), right.as_str())
    } else {
        (right.as_str(), left.as_str())
    };
    shorter.chars().count() >= TITLE_PREFIX_MIN && longer.starts_with(shorter)
}

fn first_users_match(a: &str, b: &str) -> bool {
    a.chars().count() >= FIRST_USER_MIN && a == b
}

fn native_hits(row: &ExistingRow, native_id: &str) -> bool {
    let want = sanitize_native(native_id);
    row.native_ids
        .iter()
        .any(|stored| stored == native_id || sanitize_native(stored) == want)
}

fn sanitize_native(raw: &str) -> String {
    raw.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

fn title_key(raw: &str) -> Option<String> {
    let trimmed = raw
        .trim()
        .trim_end_matches('…')
        .trim_end_matches("...")
        .trim();
    let key = fingerprint(trimmed);
    if key.is_empty() || is_generic(&key) {
        None
    } else {
        Some(key)
    }
}

fn is_generic(key: &str) -> bool {
    matches!(
        key,
        "new session"
            | "imported session"
            | "grok session"
            | "claude session"
            | "claude code session"
            | "untitled"
            | "untitled session"
            | "delegate"
            | "新对话"
            | "新会话"
    )
}

fn fingerprint(raw: &str) -> String {
    let mut out = String::new();
    let mut gap = false;
    for ch in raw.trim().chars() {
        if ch.is_whitespace() {
            gap = true;
            continue;
        }
        if gap && !out.is_empty() {
            out.push(' ');
        }
        gap = false;
        out.extend(ch.to_lowercase());
        if out.chars().count() >= 240 {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsers::imported_session_id;
    use serde_json::json;

    fn conv(title: &str, native: &str, agent: &str) -> ExternalConversation {
        ExternalConversation {
            id: format!("grok:{native}"),
            source: "grok".into(),
            agent_id: agent.into(),
            title: title.into(),
            cwd: r"D:\Projects\Marionette".into(),
            started_at: None,
            last_active_at: None,
            native_id: native.into(),
            locator: "/tmp/x".into(),
        }
    }

    fn row(id: &str, agent: &str, label: &str, origin: &str, native: &[&str], first: &str) -> ExistingRow {
        ExistingRow {
            id: id.into(),
            agent_id: agent.into(),
            label: label.into(),
            origin: Some(origin.into()),
            native_ids: native.iter().map(|s| (*s).to_string()).collect(),
            first_user: first.into(),
            live: false,
            resume_injection: false,
        }
    }

    #[test]
    fn same_import_id_and_native_id_match() {
        let native = "01a149ea-a17f-7d21-9398-5e505eb46ae3";
        let import_id = imported_session_id("grok", native);
        let rows = vec![row(
            &import_id,
            "grok-build",
            "Fix path",
            "import",
            &[native],
            "",
        )];
        let c = conv("Fix path", native, "grok-build");
        assert_eq!(
            find_match(&rows, &c, &import_id, None).map(|(_, k)| k),
            Some(MatchKind::SameId)
        );

        let rows = vec![row(
            "session-1",
            "grok-build",
            "New session",
            "user",
            &[native],
            "",
        )];
        assert_eq!(
            find_match(&rows, &c, &import_id, None).map(|(r, k)| (r.id.as_str(), k)),
            Some(("session-1", MatchKind::NativeId))
        );
    }

    #[test]
    fn title_match_ignores_generic_and_ellipsis() {
        let import_id = imported_session_id("grok", "abc");
        let rows = vec![row(
            "session-1",
            "grok-build",
            "修复 Marionette.exe 更新失败：中文路径与日语系统兼容问题",
            "user",
            &[],
            "",
        )];
        let c = conv(
            "修复 Marionette.exe 更新失败：中文路径与日语系统兼容问题",
            "abc",
            "grok-build",
        );
        assert_eq!(
            find_match(&rows, &c, &import_id, None).map(|(_, k)| k),
            Some(MatchKind::Title)
        );

        let truncated = row(
            "session-2",
            "grok-build",
            "现在有个bug，我们做了这个跨用功能嘛 然后我在Opencode、Grokbuild里面都自…",
            "user",
            &[],
            "",
        );
        let long = conv(
            "现在有个bug，我们做了这个跨用功能嘛 然后我在Opencode、Grokbuild里面都自己配了key",
            "def",
            "grok-build",
        );
        assert_eq!(
            find_match(std::slice::from_ref(&truncated), &long, &import_id, None).map(|(_, k)| k),
            Some(MatchKind::Title)
        );

        let generic = vec![row("session-3", "grok-build", "New session", "user", &[], "")];
        let unnamed = conv("New session", "zzz", "grok-build");
        assert!(find_match(&generic, &unnamed, &import_id, None).is_none());
    }

    #[test]
    fn first_user_match_needs_enough_text() {
        let import_id = imported_session_id("grok", "abc");
        let rows = vec![row(
            "session-1",
            "grok-build",
            "New session",
            "user",
            &[],
            "帮我研究下codeg，我们的参考对象。",
        )];
        let c = conv("Grok session", "abc", "grok-build");
        assert_eq!(
            find_match(
                &rows,
                &c,
                &import_id,
                Some("帮我研究下codeg，我们的参考对象。")
            )
            .map(|(_, k)| k),
            Some(MatchKind::FirstUser)
        );
        assert!(find_match(&rows, &c, &import_id, Some("你好")).is_none());
    }

    #[test]
    fn resume_injection_is_detected() {
        let events = vec![json!({
            "type": "user_message",
            "text": "[Marionette — prior conversation in this dialog]\nUser:\nhello"
        })];
        assert!(events_are_resume_injection(&events));
        assert!(first_user_from_events(&events).is_empty());
    }

    #[test]
    fn drops_import_twin_of_local_dialog() {
        let rows = vec![
            row(
                "session-1",
                "grok-build",
                "修复 Marionette.exe 更新失败：中文路径与日语系统兼容问题",
                "user",
                &[],
                "",
            ),
            row(
                "ext-grok-01a149ea-a17f-7d21-9398-5e505eb46ae3",
                "grok-build",
                "修复 Marionette.exe 更新失败：中文路径与日语系统兼容问题",
                "import",
                &["01a149ea-a17f-7d21-9398-5e505eb46ae3"],
                "",
            ),
            row(
                "ext-grok-only-cli",
                "grok-build",
                "Windows 音频链诊断：显示器外放音质发平排查",
                "import",
                &[],
                "",
            ),
        ];
        let drop = import_ids_to_drop(&rows);
        assert_eq!(
            drop,
            vec![(
                "ext-grok-01a149ea-a17f-7d21-9398-5e505eb46ae3".to_string(),
                "session-1".to_string()
            )]
        );
    }
}
