//! Google Antigravity trajectory reader (`agy_acp_server` 1.5.0).
//!
//! LAYOUT (docs/research/research-antigravity.md §五): everything hangs off
//! the Gemini home — `$GEMINI_HOME` (names the `.gemini` directory ITSELF,
//! unlike Gemini CLI's `GEMINI_CLI_HOME`) else `~/.gemini` — under
//! `antigravity-acp/conversations/<sessionId>.db` + a `.meta` sidecar
//! (`{"cwd": …, "mode_id": …}`, no title).
//!
//! The `.db` is SQLite with a `steps(idx, step_payload)` table; each payload
//! is a serialized `gemini_coder.Step` protobuf. A brand-new session's db is
//! pre-created EMPTY (0 bytes) before the Go harness writes the schema, so
//! "no such table: steps" is a normal state, not corruption.
//!
//! WHY A HAND-WRITTEN SUBSET: the real `Step` has ~130 fields behind a oneof
//! whose enum has 118 values — upstream's to renumber. Protobuf lets a
//! decoder declare just the fields it needs and skip the rest by wire type,
//! so the structs below mirror a strict subset at field numbers **verified
//! against real 1.5.0 payloads on this machine** (2026-11-19):
//!
//! ```text
//! Step:            1=type enum (14 user_input, 15 planner_response),
//!                  4=status, 5=metadata, 19=user_input, 20=planner_response
//! Metadata:        1=created Ts, 7/8=Ts (completed-ish), 9=usage, 24=model info
//! Timestamp:       1=seconds, 2=nanos
//! ModelUsageStats: 2=input, 3=output, 4=cache_write, 5=cache_read,
//!                  9=thought (10 seen alongside; semantics unverified → dropped)
//! ModelInfo:       8=model name ("gemini-3.8-flash-high" — family+level id)
//! UserInput:       2=prompt text
//! PlannerResponse: 1=response text (8 carries a duplicate copy), 3=thinking,
//!                  7=tool call announcements (repeated ChatToolCall)
//! ChatToolCall:    1=id, 2=name, 3=arguments JSON
//! ```
//!
//! Dispatch is on WHICH arm is populated, never on the `type` enum numbering
//! beyond the two values above — the arm is self-describing.

use std::path::{Path, PathBuf};

use prost::Message;
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;

use super::path_norm::cwd_matches_project;
use super::util::{assistant_event, clip_chars, thought_event, tool_event, user_event};
use super::{AgentParser, ExternalConversation};

pub struct AntigravityParser {
    conversations_dir: PathBuf,
}

mod proto {
    use prost::Message;

    #[derive(Clone, PartialEq, Message)]
    pub struct Step {
        #[prost(int32, tag = "1")]
        pub r#type: i32,
        #[prost(int32, tag = "4")]
        pub status: i32,
        #[prost(message, optional, tag = "5")]
        pub metadata: Option<CortexStepMetadata>,
        #[prost(message, optional, tag = "19")]
        pub user_input: Option<UserInput>,
        #[prost(message, optional, tag = "20")]
        pub planner_response: Option<PlannerResponse>,
    }

    #[derive(Clone, PartialEq, Message)]
    pub struct CortexStepMetadata {
        #[prost(message, optional, tag = "1")]
        pub created: Option<Timestamp>,
        #[prost(message, optional, tag = "7")]
        pub ts7: Option<Timestamp>,
        #[prost(message, optional, tag = "8")]
        pub ts8: Option<Timestamp>,
        #[prost(message, optional, tag = "9")]
        pub usage: Option<ModelUsageStats>,
        #[prost(message, optional, tag = "24")]
        pub model_info: Option<ModelInfo>,
    }

    #[derive(Clone, PartialEq, Message)]
    pub struct Timestamp {
        #[prost(int64, tag = "1")]
        pub seconds: i64,
        #[prost(int32, tag = "2")]
        pub nanos: i32,
    }

    impl Timestamp {
        pub fn to_ms(&self) -> i64 {
            self.seconds.saturating_mul(1000) + (self.nanos / 1_000_000) as i64
        }
    }

    #[derive(Clone, PartialEq, Message)]
    pub struct ModelUsageStats {
        #[prost(uint64, tag = "2")]
        pub input: u64,
        #[prost(uint64, tag = "3")]
        pub output: u64,
        #[prost(uint64, tag = "4")]
        pub cache_write: u64,
        #[prost(uint64, tag = "5")]
        pub cache_read: u64,
        #[prost(uint64, tag = "9")]
        pub thought: u64,
    }

    #[derive(Clone, PartialEq, Message)]
    pub struct ModelInfo {
        #[prost(string, tag = "8")]
        pub model_name: String,
    }

    #[derive(Clone, PartialEq, Message)]
    pub struct UserInput {
        #[prost(string, tag = "2")]
        pub text: String,
    }

    #[derive(Clone, PartialEq, Message)]
    pub struct PlannerResponse {
        #[prost(string, tag = "1")]
        pub text: String,
        #[prost(string, tag = "3")]
        pub thinking: String,
        #[prost(message, repeated, tag = "7")]
        pub tool_calls: Vec<ChatToolCall>,
    }

    #[derive(Clone, PartialEq, Message)]
    pub struct ChatToolCall {
        #[prost(string, tag = "1")]
        pub id: String,
        #[prost(string, tag = "2")]
        pub name: String,
        #[prost(string, tag = "3")]
        pub arguments_json: String,
    }
}

/// `$GEMINI_HOME/antigravity-acp/conversations` (shared with
/// antigravity_support — that module owns the env semantics).
pub fn conversations_dir() -> PathBuf {
    crate::antigravity_support::antigravity_acp_dir().join("conversations")
}

/// Open read-only; fall back to read-write when WAL recovery is needed
/// (`SQLITE_READONLY_RECOVERY` on a cleanly-killed server), mirroring the
/// house rule for other people's SQLite stores.
fn open_ro(db: &Path) -> Result<Connection, String> {
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    match Connection::open_with_flags(db, flags) {
        Ok(conn) => Ok(conn),
        Err(_) => Connection::open_with_flags(
            db,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_CREATE,
        )
        .map_err(|e| format!("open {}: {e}", db.display())),
    }
}

pub(crate) struct DecodedStep {
    pub created_ms: Option<i64>,
    pub usage: Option<proto::ModelUsageStats>,
    pub model: Option<String>,
    pub kind: StepKind,
}

#[derive(Debug)]
pub(crate) enum StepKind {
    User(String),
    Planner {
        text: String,
        thinking: String,
        tool_calls: Vec<proto::ChatToolCall>,
    },
    Other,
}

/// Decode one step payload; a decode failure reads as `Other` (single bad
/// steps must not sink the session).
pub(crate) fn decode_step(payload: &[u8]) -> DecodedStep {
    let Ok(step) = proto::Step::decode(payload) else {
        return DecodedStep {
            created_ms: None,
            usage: None,
            model: None,
            kind: StepKind::Other,
        };
    };
    let created_ms = step.metadata.as_ref().and_then(|m| m.created.as_ref()).map(|t| t.to_ms());
    let usage = step.metadata.as_ref().and_then(|m| m.usage.clone());
    let model = step
        .metadata
        .as_ref()
        .and_then(|m| m.model_info.as_ref())
        .map(|info| info.model_name.clone())
        .filter(|name| !name.is_empty());
    let kind = if let Some(user) = step.user_input {
        StepKind::User(user.text)
    } else if let Some(planner) = step.planner_response {
        StepKind::Planner {
            text: planner.text,
            thinking: planner.thinking,
            tool_calls: planner.tool_calls,
        }
    } else {
        StepKind::Other
    };
    DecodedStep {
        created_ms,
        usage,
        model,
        kind,
    }
}

/// Read and decode every step of a session db, idx ascending. A 0-byte db or
/// one without the schema yet decodes as an empty conversation.
pub(crate) fn read_steps(db: &Path) -> Result<Vec<DecodedStep>, String> {
    if std::fs::metadata(db).map(|m| m.len() == 0).unwrap_or(true) {
        return Ok(Vec::new());
    }
    let conn = open_ro(db)?;
    let has_steps: bool = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='steps'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map(|n| n > 0)
        .unwrap_or(false);
    if !has_steps {
        return Ok(Vec::new());
    }
    let mut stmt = conn
        .prepare("SELECT step_payload FROM steps ORDER BY idx")
        .map_err(|e| format!("prepare steps query: {e}"))?;
    let payloads: Vec<Vec<u8>> = stmt
        .query_map([], |row| row.get::<_, Vec<u8>>(0))
        .map_err(|e| format!("query steps: {e}"))?
        .flatten()
        .collect();
    Ok(payloads.iter().map(|p| decode_step(p)).collect())
}

/// `.meta` sidecar → cwd (no fallback: the server writes it, we read it).
pub(crate) fn meta_cwd(db: &Path) -> Option<String> {
    let text = std::fs::read_to_string(db.with_extension("meta")).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    value
        .get("cwd")
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|cwd| !cwd.is_empty())
}

/// Event timestamps are epoch-ms strings (house convention — zcode parser).
fn ms_ts(ms: Option<i64>) -> String {
    ms.map(|t| t.to_string()).unwrap_or_default()
}

impl AgentParser for AntigravityParser {
    fn source(&self) -> &'static str {
        "antigravity"
    }
    fn agent_id(&self) -> &'static str {
        crate::antigravity_support::AGENT_ID
    }

    fn list(&self, project_root: &str) -> Result<Vec<ExternalConversation>, String> {
        let dir = &self.conversations_dir;
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("db") {
                continue;
            }
            let Some(native_id) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let Some(cwd) = meta_cwd(&path) else { continue };
            if !cwd_matches_project(&cwd, project_root) {
                continue;
            }
            let steps = match read_steps(&path) {
                Ok(steps) => steps,
                Err(_) => continue,
            };
            // Title = first user prompt (the server itself only synthesizes
            // "Session <first 8 chars>" — not worth mirroring).
            let title = steps
                .iter()
                .find_map(|s| match &s.kind {
                    StepKind::User(text) if !text.trim().is_empty() => {
                        Some(clip_chars(text.trim(), 80))
                    }
                    _ => None,
                })
                .unwrap_or_else(|| format!("Session {}", &native_id[..native_id.len().min(8)]));
            let first = steps.iter().filter_map(|s| s.created_ms).min();
            let last = steps.iter().filter_map(|s| s.created_ms).max();
            out.push(ExternalConversation {
                id: crate::parsers::imported_session_id("antigravity", native_id),
                source: "antigravity".into(),
                agent_id: crate::antigravity_support::AGENT_ID.into(),
                title,
                cwd,
                started_at: first.map(|t| t.to_string()),
                last_active_at: last.map(|t| t.to_string()),
                native_id: native_id.to_string(),
                locator: path.to_string_lossy().to_string(),
            });
        }
        Ok(out)
    }

    fn load(&self, locator: &str) -> Result<Vec<Value>, String> {
        let db = PathBuf::from(locator);
        let session_id = db
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("antigravity");
        let steps = read_steps(&db)?;
        let mut out = Vec::new();
        for step in steps {
            let ts = ms_ts(step.created_ms);
            match step.kind {
                StepKind::User(text) => {
                    if !text.trim().is_empty() {
                        out.push(user_event(session_id, &text, &ts));
                    }
                }
                StepKind::Planner {
                    text,
                    thinking,
                    tool_calls,
                } => {
                    if !thinking.trim().is_empty() {
                        out.push(thought_event(session_id, &thinking, &ts));
                    }
                    for call in tool_calls {
                        let detail = if call.arguments_json.trim().is_empty() {
                            None
                        } else {
                            Some(call.arguments_json.clone())
                        };
                        out.push(tool_event(
                            session_id,
                            &call.id,
                            &call.name,
                            &ts,
                            detail.as_deref(),
                        ));
                    }
                    if !text.trim().is_empty() {
                        out.push(assistant_event(
                            session_id,
                            &text,
                            &ts,
                            crate::antigravity_support::AGENT_ID,
                        ));
                    }
                }
                StepKind::Other => {}
            }
        }
        Ok(out)
    }
}

impl AntigravityParser {
    pub fn new() -> Self {
        Self {
            conversations_dir: conversations_dir(),
        }
    }
}

impl Default for AntigravityParser {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "marionette-agy-parser-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Raw wire pieces below mirror bytes captured from a REAL
    /// agy-acp-server 1.5.0 session db on this machine (2026-11-19) — the
    /// field numbers and shapes are upstream's, only assembled in Rust.
    #[test]
    fn decodes_real_user_step_shape() {
        // Full payload assembled from the verified wire pieces: type=14,
        // status=3, metadata with created Ts, user_input with the prompt.
        let metadata = {
            // created Ts {seconds: 1795063081, nanos: 82628700}
            let ts = {
                let mut b = Vec::new();
                b.extend_from_slice(&[0x08]);
                b.extend(varint(1795063081u64));
                b.extend_from_slice(&[0x10]);
                b.extend(varint(82628700u64));
                b
            };
            let mut b = Vec::new();
            b.extend_from_slice(&[0x0a]);
            b.extend(varint(ts.len() as u64));
            b.extend_from_slice(&ts);
            b
        };
        let user = {
            let text = b"Reply with exactly: M1_E2E_OK4";
            let mut b = vec![0x12]; // field 2, wire 2
            b.extend(varint(text.len() as u64));
            b.extend_from_slice(text);
            b
        };
        let mut step = Vec::new();
        step.extend_from_slice(&[0x08, 0x0e]); // type = 14 (user_input)
        step.extend_from_slice(&[0x20, 0x03]); // status = 3
        step.extend_from_slice(&[0x2a]); // field 5 (metadata)
        step.extend(varint(metadata.len() as u64));
        step.extend_from_slice(&metadata);
        step.extend_from_slice(&[0x9a, 0x01]); // field 19 (user_input)
        step.extend(varint(user.len() as u64));
        step.extend_from_slice(&user);

        let decoded = decode_step(&step);
        assert_eq!(decoded.created_ms, Some(1795063081_082));
        match decoded.kind {
            StepKind::User(text) => assert_eq!(text, "Reply with exactly: M1_E2E_OK4"),
            other => panic!("expected user step, got {other:?}"),
        }
    }

    #[test]
    fn planner_step_decodes_text_thinking_and_tool_call() {
        let planner = {
            let text = b"BLUE_PENGUIN_42";
            let thinking = b"hmm";
            let call = {
                let mut b = Vec::new();
                for (tag, value) in [(1u8, b"call-1".as_slice()), (2u8, b"view_file".as_slice()), (3u8, br#"{"file_path":"x.txt"}"#.as_slice())] {
                    b.push(tag << 3 | 2);
                    b.extend(varint(value.len() as u64));
                    b.extend_from_slice(value);
                }
                b
            };
            let mut b = Vec::new();
            b.push(1 << 3 | 2); // text
            b.extend(varint(text.len() as u64));
            b.extend_from_slice(text);
            b.push(3 << 3 | 2); // thinking
            b.extend(varint(thinking.len() as u64));
            b.extend_from_slice(thinking);
            b.push(7 << 3 | 2); // tool_calls[0]
            b.extend(varint(call.len() as u64));
            b.extend_from_slice(&call);
            b
        };
        let mut step = Vec::new();
        step.extend_from_slice(&[0x08, 0x0f]); // type = 15
        step.extend_from_slice(&[0xa2, 0x01]); // field 20
        step.extend(varint(planner.len() as u64));
        step.extend_from_slice(&planner);

        let decoded = decode_step(&step);
        match decoded.kind {
            StepKind::Planner {
                text,
                thinking,
                tool_calls,
            } => {
                assert_eq!(text, "BLUE_PENGUIN_42");
                assert_eq!(thinking, "hmm");
                assert_eq!(tool_calls.len(), 1);
                assert_eq!(tool_calls[0].name, "view_file");
                assert_eq!(tool_calls[0].arguments_json, r#"{"file_path":"x.txt"}"#);
            }
            other => panic!("expected planner step, got {other:?}"),
        }
    }

    #[test]
    fn zero_byte_db_reads_as_empty() {
        let dir = tempdir("empty");
        let db = dir.join("s.db");
        std::fs::write(&db, b"").unwrap();
        assert!(read_steps(&db).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn varint(mut v: u64) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let byte = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                out.push(byte);
                return out;
            }
            out.push(byte | 0x80);
        }
    }

    /// Opt-in oracle against the user's real conversations dir:
    /// `cargo test --bin marionette antigravity_oracle -- --ignored --nocapture`
    #[test]
    #[ignore = "depends on this machine's real ~/.gemini store"]
    fn antigravity_oracle_real_store() {
        let parser = AntigravityParser::new();
        // Per-db diagnosis (which store root, which cwds, step counts).
        let dir = parser.conversations_dir.clone();
        println!("conversations dir: {} (exists={})", dir.display(), dir.is_dir());
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("db") {
                    continue;
                }
                println!(
                    "  db {} | cwd={:?} | steps={:?}",
                    path.file_stem().and_then(|s| s.to_str()).unwrap_or("?"),
                    meta_cwd(&path),
                    read_steps(&path).map(|s| s.len()),
                );
            }
        }
        let all = parser.list(r"D:\tmp\agy-m0-probe").unwrap();
        println!(r"sessions under D:\tmp\agy-m0-probe: {}", all.len());
        for conversation in all.iter().take(3) {
            println!(
                "  {} | {} | {}",
                conversation.native_id, conversation.title, conversation.cwd
            );
        }
        if let Some(conversation) = all.first() {
            let events = parser.load(&conversation.locator).unwrap();
            println!("first session events: {}", events.len());
            for event in events.iter().take(6) {
                println!(
                    "  {} {}",
                    event.get("type").and_then(Value::as_str).unwrap_or("?"),
                    event
                        .pointer("/message/content/0/text")
                        .or_else(|| event.get("text"))
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .chars()
                        .take(60)
                        .collect::<String>()
                );
            }
            assert!(
                events
                    .iter()
                    .any(|e| e.get("role").and_then(Value::as_str) == Some("user")
                        || e.get("type").and_then(Value::as_str) == Some("user_message")),
                "at least one user event"
            );
        }
    }
}
