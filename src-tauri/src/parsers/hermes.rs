//! Hermes: `~/.hermes/state.db` (SQLite, read-only). cwd may live in `model_config` JSON.

use super::path_norm::cwd_matches_project;
use super::util::{
    assistant_event, env_dir, home_join, thought_event, tool_event, user_event,
};
use super::{AgentParser, ExternalConversation};
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use std::path::PathBuf;

pub struct HermesParser;

fn hermes_db() -> Option<PathBuf> {
    Some(env_dir("HERMES_HOME").or_else(|| home_join(".hermes"))?.join("state.db"))
        .filter(|p| p.is_file())
}

fn open_ro(path: &PathBuf) -> Result<Connection, String> {
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| format!("open hermes state.db: {e}"))
}

fn epoch_to_iso(secs: f64) -> String {
    if secs <= 0.0 {
        return String::new();
    }
    format!("{secs:.3}")
}

fn cwd_from_row(cwd: Option<String>, model_config: Option<String>) -> String {
    if let Some(c) = cwd {
        if !c.trim().is_empty() {
            return c;
        }
    }
    if let Some(raw) = model_config {
        if let Ok(v) = serde_json::from_str::<Value>(&raw) {
            if let Some(c) = v.get("cwd").and_then(|x| x.as_str()) {
                return c.to_string();
            }
        }
    }
    String::new()
}

impl AgentParser for HermesParser {
    fn source(&self) -> &'static str {
        "hermes"
    }
    fn agent_id(&self) -> &'static str {
        "hermes"
    }

    fn list(&self, project_root: &str) -> Result<Vec<ExternalConversation>, String> {
        let Some(db) = hermes_db() else {
            return Ok(Vec::new());
        };
        let conn = open_ro(&db)?;
        let mut stmt = match conn.prepare(
            "SELECT id, title, cwd, started_at, ended_at, model_config, parent_session_id
             FROM sessions",
        ) {
            Ok(s) => s,
            Err(_) => return Ok(Vec::new()),
        };
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<f64>>(3)?,
                    row.get::<_, Option<f64>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                ))
            })
            .map_err(|e| format!("query hermes sessions: {e}"))?;
        let mut out = Vec::new();
        for row in rows.flatten() {
            let (id, title, cwd, started, ended, model_config, parent) = row;
            if parent.as_deref().map(|s| !s.is_empty()).unwrap_or(false) {
                continue;
            }
            let cwd = cwd_from_row(cwd, model_config);
            if cwd.is_empty() || !cwd_matches_project(&cwd, project_root) {
                continue;
            }
            out.push(ExternalConversation {
                id: format!("hermes:{id}"),
                source: "hermes".into(),
                agent_id: "hermes".into(),
                title: title
                    .filter(|t| !t.trim().is_empty())
                    .unwrap_or_else(|| format!("Hermes {id}")),
                cwd,
                started_at: started.map(epoch_to_iso),
                last_active_at: ended.map(epoch_to_iso),
                native_id: id.clone(),
                locator: id,
            });
        }
        Ok(out)
    }

    fn load(&self, locator: &str) -> Result<Vec<Value>, String> {
        let Some(db) = hermes_db() else {
            return Err("hermes state.db not found".into());
        };
        let conn = open_ro(&db)?;
        let mut stmt = conn
            .prepare(
                "SELECT role, content, reasoning, tool_name, timestamp, active
                 FROM messages WHERE session_id = ?1 ORDER BY id ASC",
            )
            .map_err(|e| format!("prepare hermes messages: {e}"))?;
        let rows = stmt
            .query_map([locator], |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<f64>>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                ))
            })
            .map_err(|e| format!("query hermes messages: {e}"))?;
        let mut events = Vec::new();
        let mut idx: u64 = 0;
        for row in rows.flatten() {
            let (role, content, reasoning, tool_name, ts, active) = row;
            if active == Some(0) {
                continue;
            }
            idx += 1;
            let created = ts.map(epoch_to_iso).unwrap_or_else(|| format!("hermes-{idx:06}"));
            let role = role.unwrap_or_default();
            let text = content.unwrap_or_default();
            if role == "user" && !text.trim().is_empty() {
                events.push(user_event(locator, &text, &created));
            } else if role == "assistant" {
                if let Some(r) = reasoning {
                    if !r.trim().is_empty() {
                        events.push(thought_event(locator, &r, &format!("{created}-t")));
                    }
                }
                if !text.trim().is_empty() {
                    events.push(assistant_event(locator, &text, &created, "hermes"));
                }
            } else if role == "tool" {
                let name = tool_name.unwrap_or_else(|| "tool".into());
                events.push(tool_event(locator, &name, &name, &created, Some(&text)));
            }
        }
        Ok(events)
    }
}
