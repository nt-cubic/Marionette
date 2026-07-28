//! OpenCode SQLite store: `~/.local/share/opencode/opencode.db` (read-only).

use super::path_norm::{cwd_matches_project, normalize_path};
use super::{AgentParser, ExternalConversation};
use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};
use std::path::PathBuf;

pub struct OpenCodeParser;

fn opencode_db_path() -> Option<PathBuf> {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
    let home = PathBuf::from(home);
    // Prefer XDG-style path used on this machine; fall back to a few alternatives.
    let candidates = [
        home.join(".local").join("share").join("opencode").join("opencode.db"),
        home.join(".config").join("opencode").join("opencode.db"),
        home.join("AppData")
            .join("Local")
            .join("opencode")
            .join("opencode.db"),
    ];
    candidates.into_iter().find(|p| p.is_file())
}

fn open_ro(path: &PathBuf) -> Result<Connection, String> {
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| format!("open opencode.db: {e}"))
}

fn millis_to_iso(ms: i64) -> String {
    // Compact sortable stamp — frontend only displays, not parsed strictly.
    ms.to_string()
}

impl AgentParser for OpenCodeParser {
    fn source(&self) -> &'static str {
        "opencode"
    }

    fn list(&self, project_root: &str) -> Result<Vec<ExternalConversation>, String> {
        let Some(db_path) = opencode_db_path() else {
            return Ok(Vec::new());
        };
        let conn = open_ro(&db_path)?;
        // Indexed path filter: exact + normalized variants (forward slash).
        let norm = normalize_path(project_root);
        let slash = norm.replace('\\', "/");
        let mut stmt = conn
            .prepare(
                "SELECT id, title, directory, time_created, time_updated
                 FROM session
                 WHERE directory = ?1 OR directory = ?2
                    OR directory LIKE ?3 OR directory LIKE ?4
                 ORDER BY time_updated DESC
                 LIMIT 500",
            )
            .map_err(|e| format!("prepare session query: {e}"))?;
        let like_bs = format!("{norm}\\%");
        let like_sl = format!("{slash}/%");
        let rows = stmt
            .query_map(
                rusqlite::params![norm, slash, like_bs, like_sl],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<i64>>(3)?,
                        row.get::<_, Option<i64>>(4)?,
                    ))
                },
            )
            .map_err(|e| format!("query sessions: {e}"))?;

        let mut out = Vec::new();
        for row in rows.flatten() {
            let (id, title, directory, created, updated) = row;
            let cwd = directory.unwrap_or_default();
            if !cwd.is_empty() && !cwd_matches_project(&cwd, project_root) {
                // LIKE may over-match; double-check.
                continue;
            }
            let title = title
                .filter(|t| !t.trim().is_empty())
                .unwrap_or_else(|| format!("OpenCode {id}"));
            out.push(ExternalConversation {
                id: format!("opencode:{id}"),
                source: "opencode".into(),
                title,
                cwd: if cwd.is_empty() {
                    project_root.to_string()
                } else {
                    cwd
                },
                started_at: created.map(millis_to_iso),
                last_active_at: updated.map(millis_to_iso),
                native_id: id.clone(),
                locator: id,
            });
        }
        Ok(out)
    }

    fn load(&self, locator: &str) -> Result<Vec<Value>, String> {
        let session_id = locator.trim();
        if session_id.is_empty() {
            return Err("empty OpenCode session id".into());
        }
        let Some(db_path) = opencode_db_path() else {
            return Err("opencode.db not found".into());
        };
        let conn = open_ro(&db_path)?;

        // Messages ordered by time
        let mut msg_stmt = conn
            .prepare(
                "SELECT id, time_created, data FROM message
                 WHERE session_id = ?1
                 ORDER BY time_created ASC, id ASC",
            )
            .map_err(|e| format!("prepare message: {e}"))?;
        let messages: Vec<(String, i64, String)> = msg_stmt
            .query_map([session_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(|e| format!("query message: {e}"))?
            .filter_map(|r| r.ok())
            .collect();

        // Parts for whole session (join in memory by message_id)
        let mut part_stmt = conn
            .prepare(
                "SELECT message_id, time_created, data FROM part
                 WHERE session_id = ?1
                 ORDER BY time_created ASC, id ASC",
            )
            .map_err(|e| format!("prepare part: {e}"))?;
        let mut parts_by_msg: std::collections::HashMap<String, Vec<(i64, Value)>> =
            std::collections::HashMap::new();
        {
            let part_rows = part_stmt
                .query_map([session_id], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })
                .map_err(|e| format!("query part: {e}"))?;
            for row in part_rows.flatten() {
                let (mid, t, data) = row;
                let Ok(v) = serde_json::from_str::<Value>(&data) else {
                    continue;
                };
                parts_by_msg.entry(mid).or_default().push((t, v));
            }
        }

        let mut events = Vec::new();
        let mut idx: u64 = 0;
        for (msg_id, time_created, data) in messages {
            let Ok(meta) = serde_json::from_str::<Value>(&data) else {
                continue;
            };
            let role = meta.get("role").and_then(|x| x.as_str()).unwrap_or("");
            let created = millis_to_iso(time_created);
            let parts = parts_by_msg.remove(&msg_id).unwrap_or_default();

            let mut text_buf = String::new();
            let mut thought_buf = String::new();
            for (_t, part) in &parts {
                let pty = part.get("type").and_then(|x| x.as_str()).unwrap_or("");
                match pty {
                    "text" => {
                        if let Some(t) = part.get("text").and_then(|x| x.as_str()) {
                            if !text_buf.is_empty() {
                                text_buf.push('\n');
                            }
                            text_buf.push_str(t);
                        }
                    }
                    "reasoning" | "thinking" => {
                        if let Some(t) = part
                            .get("text")
                            .or_else(|| part.get("reasoning"))
                            .and_then(|x| x.as_str())
                        {
                            if !thought_buf.is_empty() {
                                thought_buf.push('\n');
                            }
                            thought_buf.push_str(t);
                        }
                    }
                    "tool" | "tool-invocation" | "tool_use" => {
                        let name = part
                            .get("tool")
                            .or_else(|| part.get("name"))
                            .and_then(|x| x.as_str())
                            .unwrap_or("tool");
                        let id = part
                            .get("callID")
                            .or_else(|| part.get("id"))
                            .and_then(|x| x.as_str())
                            .unwrap_or("tool");
                        idx += 1;
                        events.push(json!({
                            "type": "tool_call",
                            "sessionId": session_id,
                            "toolCallId": id,
                            "title": name,
                            "toolName": name,
                            "status": "completed",
                            "createdAt": format!("{created}-tc{idx}"),
                        }));
                    }
                    _ => {}
                }
            }

            if !thought_buf.trim().is_empty() {
                idx += 1;
                events.push(json!({
                    "type": "thought",
                    "sessionId": session_id,
                    "text": thought_buf,
                    "createdAt": format!("{created}-th"),
                }));
            }

            if text_buf.trim().is_empty() {
                continue;
            }
            idx += 1;
            if role == "user" {
                events.push(json!({
                    "type": "user_message",
                    "sessionId": session_id,
                    "text": text_buf,
                    "createdAt": created,
                }));
            } else if role == "assistant" || role == "agent" {
                events.push(json!({
                    "type": "assistant_message",
                    "sessionId": session_id,
                    "text": text_buf,
                    "createdAt": created,
                    "agentId": "opencode",
                }));
            }
        }
        Ok(events)
    }
}
