//! ZCode: `~/.zcode/cli/db/db.sqlite`（SQLite，只读）。
//!
//! 会话主库一张 `session` 表：`directory` 是 cwd（项目过滤），
//! `sess_subagent_*` / 带 `parent_id` 的是子代理会话，`time_archived` 非空是
//! 归档，都不进导入列表。转录是 `message`（时间线事件：role +
//! semantics.kind）+ `part`（内容分片，按 message_id 关联、sequence 排序）
//! 两表；只导出真实对话轮（user/user_prompt、assistant/assistant_response），
//! system_reminder / todo_reminder / timeline_event 是合成事件，跳过。

use super::path_norm::cwd_matches_project;
use super::util::{assistant_event, env_dir, home_join, thought_event, tool_event, user_event};
use super::{AgentParser, ExternalConversation};
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use std::path::PathBuf;

pub struct ZcodeParser;

/// `ZCODE_HOME` 替换整个数据根（适配器同款语义：指向 `.zcode` 目录本身）。
fn zcode_db() -> Option<PathBuf> {
    let root = env_dir("ZCODE_HOME").or_else(|| home_join(".zcode"))?;
    let db = root.join("cli").join("db").join("db.sqlite");
    db.is_file().then_some(db)
}

fn open_ro(path: &PathBuf) -> Result<Connection, String> {
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| format!("open zcode db.sqlite: {e}"))
}

struct SessionRow {
    id: String,
    directory: Option<String>,
    title: Option<String>,
    time_created: Option<i64>,
    time_updated: Option<i64>,
    parent_id: Option<String>,
    time_archived: Option<i64>,
}

/// message.data → 导出角色 + 创建时间；None = 合成事件（reminder/timeline）。
fn message_kind(data: &Value) -> Option<(&'static str, Option<i64>)> {
    let role = data.get("role")?.as_str()?;
    let kind = data
        .pointer("/semantics/kind")
        .and_then(Value::as_str)
        .unwrap_or("");
    let exported = match (role, kind) {
        ("user", "user_prompt") => "user",
        ("assistant", "assistant_response") => "assistant",
        _ => return None,
    };
    let created = data.pointer("/time/created").and_then(Value::as_i64);
    Some((exported, created))
}

/// part.data → Marionette 事件；None = step-*/timeline 等非内容分片。
fn part_event(
    session_id: &str,
    message_role: &str,
    data: &Value,
    created: Option<i64>,
) -> Option<Value> {
    let ts = created.map(|t| t.to_string()).unwrap_or_default();
    match data.get("type").and_then(Value::as_str)? {
        "text" => {
            let text = data.get("text").and_then(Value::as_str).unwrap_or("").trim();
            if text.is_empty() {
                return None;
            }
            Some(if message_role == "user" {
                user_event(session_id, text, &ts)
            } else {
                assistant_event(session_id, text, &ts, "zcode")
            })
        }
        "reasoning" => {
            let text = data.get("text").and_then(Value::as_str).unwrap_or("").trim();
            if text.is_empty() {
                return None;
            }
            Some(thought_event(session_id, text, &ts))
        }
        "tool" => {
            let name = data.get("tool").and_then(Value::as_str).unwrap_or("tool");
            let call_id = data.get("callID").and_then(Value::as_str).unwrap_or("zcode-tool");
            let detail = tool_detail(data.get("state"));
            Some(tool_event(session_id, call_id, name, &ts, detail.as_deref()))
        }
        _ => None,
    }
}

/// 工具调用的紧凑详情：入参 JSON + 输出首段（tool_event 自己截 4000 字）。
fn tool_detail(state: Option<&Value>) -> Option<String> {
    let state = state?;
    let mut out = String::new();
    if let Some(input) = state.get("input") {
        let input = serde_json::to_string(input).unwrap_or_default();
        if !input.is_empty() && input != "{}" {
            out.push_str(&input);
        }
    }
    if let Some(output) = state.get("output").and_then(Value::as_str) {
        let output = output.trim();
        if !output.is_empty() {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(output);
        }
    }
    (!out.is_empty()).then_some(out)
}

/// 会话列表（纯函数，测试喂内存库）：子代理 / 归档 / 别的项目全滤掉。
fn list_from_conn(conn: &Connection, project_root: &str) -> Vec<ExternalConversation> {
    // 表/列缺失（schema 升级）→ 当作没有会话，别让整个 refresh 失败。
    let mut stmt = match conn.prepare(
        "SELECT id, directory, title, time_created, time_updated, parent_id, time_archived
         FROM session",
    ) {
        Ok(stmt) => stmt,
        Err(_) => return Vec::new(),
    };
    let Ok(rows) = stmt.query_map([], |row| {
        Ok(SessionRow {
            id: row.get(0)?,
            directory: row.get(1)?,
            title: row.get(2)?,
            time_created: row.get(3)?,
            time_updated: row.get(4)?,
            parent_id: row.get(5)?,
            time_archived: row.get(6)?,
        })
    }) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for row in rows.flatten() {
        if row.id.starts_with("sess_subagent") {
            continue;
        }
        if row
            .parent_id
            .as_deref()
            .map(|p| !p.is_empty())
            .unwrap_or(false)
        {
            continue;
        }
        if row.time_archived.unwrap_or(0) > 0 {
            continue;
        }
        let Some(directory) = row
            .directory
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty())
        else {
            continue;
        };
        if !cwd_matches_project(directory, project_root) {
            continue;
        }
        let title = row
            .title
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("ZCode {}", row.id));
        out.push(ExternalConversation {
            id: format!("zcode:{}", row.id),
            source: "zcode".into(),
            agent_id: "zcode".into(),
            title,
            cwd: directory.to_string(),
            started_at: row.time_created.filter(|t| *t > 0).map(|t| t.to_string()),
            last_active_at: row.time_updated.filter(|t| *t > 0).map(|t| t.to_string()),
            native_id: row.id.clone(),
            locator: row.id,
        });
    }
    out
}

/// 事件导出（纯函数，测试喂内存库）。
fn export_events(conn: &Connection, session_id: &str) -> Result<Vec<Value>, String> {
    let mut out = Vec::new();
    let mut stmt = conn
        .prepare("SELECT id, data FROM message WHERE session_id = ?1 ORDER BY sequence")
        .map_err(|e| format!("query zcode messages: {e}"))?;
    let messages: Vec<(String, String)> = stmt
        .query_map([session_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
            ))
        })
        .map_err(|e| format!("query zcode messages: {e}"))?
        .flatten()
        .collect();
    drop(stmt);

    let mut part_stmt = conn
        .prepare("SELECT data FROM part WHERE message_id = ?1 ORDER BY sequence")
        .map_err(|e| format!("prepare zcode parts: {e}"))?;
    for (message_id, data) in messages {
        let Ok(data) = serde_json::from_str::<Value>(&data) else {
            continue;
        };
        let Some((role, created)) = message_kind(&data) else {
            continue;
        };
        let parts: Vec<String> = part_stmt
            .query_map([&message_id], |row| row.get::<_, String>(0))
            .map_err(|e| format!("query zcode parts: {e}"))?
            .flatten()
            .collect();
        for part in parts {
            let Ok(part) = serde_json::from_str::<Value>(&part) else {
                continue;
            };
            if let Some(event) = part_event(session_id, role, &part, created) {
                out.push(event);
            }
        }
    }
    Ok(out)
}

impl AgentParser for ZcodeParser {
    fn source(&self) -> &'static str {
        "zcode"
    }
    fn agent_id(&self) -> &'static str {
        "zcode"
    }

    fn list(&self, project_root: &str) -> Result<Vec<ExternalConversation>, String> {
        let Some(db) = zcode_db() else {
            return Ok(Vec::new());
        };
        let conn = open_ro(&db)?;
        Ok(list_from_conn(&conn, project_root))
    }

    fn load(&self, locator: &str) -> Result<Vec<Value>, String> {
        let Some(db) = zcode_db() else {
            return Err("zcode db.sqlite not found".into());
        };
        let conn = open_ro(&db)?;
        export_events(&conn, locator)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 与真库同构的最小 schema + 样本（形状取自 2026-10-23 实机采样）。
    fn fixture() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE session (
                id TEXT PRIMARY KEY, project_id TEXT, workspace_id TEXT, parent_id TEXT,
                slug TEXT, directory TEXT, path TEXT, title TEXT, version TEXT,
                share_url TEXT, summary_additions TEXT, summary_deletions TEXT,
                summary_files TEXT, summary_diffs TEXT, revert TEXT, permission TEXT,
                time_created INTEGER, time_updated INTEGER, time_compacting INTEGER,
                time_archived INTEGER, task_type TEXT, title_source TEXT,
                title_message_id TEXT, time_title_updated INTEGER, trace_id TEXT);
             CREATE TABLE message (
                id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER,
                time_updated INTEGER, data TEXT, sequence INTEGER);
             CREATE TABLE part (
                id TEXT PRIMARY KEY, message_id TEXT, session_id TEXT,
                time_created INTEGER, time_updated INTEGER, data TEXT, sequence INTEGER);",
        )
        .unwrap();
        conn
    }

    fn add_session(conn: &Connection, id: &str, directory: &str, parent: Option<&str>, archived: bool) {
        conn.execute(
            "INSERT INTO session (id, directory, title, parent_id, time_archived, time_created, time_updated)
             VALUES (?1, ?2, ?3, ?4, ?5, 1000, 2000)",
            rusqlite::params![
                id,
                directory,
                format!("title-{id}"),
                parent,
                if archived { 1 } else { 0 },
            ],
        )
        .unwrap();
    }

    fn add_message(conn: &Connection, id: &str, session: &str, sequence: i64, data: &Value) {
        conn.execute(
            "INSERT INTO message (id, session_id, time_created, time_updated, data, sequence)
             VALUES (?1, ?2, 1000, 1000, ?3, ?4)",
            rusqlite::params![id, session, data.to_string(), sequence],
        )
        .unwrap();
    }

    fn add_part(conn: &Connection, message: &str, sequence: i64, data: &Value) {
        conn.execute(
            "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data, sequence)
             VALUES (?1, ?2, 's1', 1000, 1000, ?3, ?4)",
            rusqlite::params![format!("{message}-p{sequence}"), message, data.to_string(), sequence],
        )
        .unwrap();
    }

    #[test]
    fn list_filters_subagents_archived_and_other_projects() {
        let conn = fixture();
        let root = "D:\\Proj\\App";
        add_session(&conn, "sess_main", root, None, false);
        add_session(&conn, "sess_subagent_agent_1", root, None, false);
        add_session(&conn, "sess_child", root, Some("sess_main"), false);
        add_session(&conn, "sess_archived", root, None, true);
        add_session(&conn, "sess_elsewhere", "D:\\Other", None, false);

        let rows = list_from_conn(&conn, root);
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].native_id, "sess_main");
        assert_eq!(rows[0].id, "zcode:sess_main");
        assert_eq!(rows[0].agent_id, "zcode");
        assert_eq!(rows[0].cwd, root);
        assert_eq!(rows[0].started_at.as_deref(), Some("1000"));
        assert_eq!(rows[0].last_active_at.as_deref(), Some("2000"));
    }

    #[test]
    fn export_renders_real_turns_and_skips_synthetic() {
        let conn = fixture();
        add_session(&conn, "s1", "D:\\Proj\\App", None, false);

        add_message(
            &conn, "m1", "s1", 0,
            &serde_json::json!({
                "role": "user",
                "time": { "created": 1792719504970i64 },
                "semantics": { "kind": "user_prompt" },
            }),
        );
        add_part(&conn, "m1", 0, &serde_json::json!({ "type": "text", "text": "研究下怎么接入" }));

        add_message(
            &conn, "m2", "s1", 1,
            &serde_json::json!({
                "role": "user",
                "semantics": { "kind": "system_reminder" },
            }),
        );
        add_part(&conn, "m2", 0, &serde_json::json!({ "type": "text", "text": "agent listing delta" }));

        add_message(
            &conn, "m3", "s1", 2,
            &serde_json::json!({
                "role": "assistant",
                "time": { "created": 1792719510000i64 },
                "semantics": { "kind": "assistant_response" },
            }),
        );
        add_part(&conn, "m3", 0, &serde_json::json!({ "type": "reasoning", "text": "think" }));
        add_part(&conn, "m3", 1, &serde_json::json!({ "type": "text", "text": "结论" }));
        add_part(&conn, "m3", 2, &serde_json::json!({ "type": "tool", "callID": "call_1", "tool": "Bash",
            "state": { "status": "completed", "input": { "command": "ls" }, "output": "ok" } }));
        add_part(&conn, "m3", 3, &serde_json::json!({ "type": "step-start" }));

        let events = export_events(&conn, "s1").unwrap();
        let kinds: Vec<&str> = events.iter().map(|e| e["type"].as_str().unwrap_or("")).collect();
        assert_eq!(kinds, vec!["user_message", "thought", "assistant_message", "tool_call"]);
        assert_eq!(events[0]["text"], "研究下怎么接入");
        assert_eq!(events[0]["createdAt"], "1792719504970");
        assert_eq!(events[1]["text"], "think");
        assert_eq!(events[2]["text"], "结论");
        assert_eq!(events[2]["agentId"], "zcode");
        assert_eq!(events[3]["toolName"], "Bash");
        assert_eq!(events[3]["toolCallId"], "call_1");
        assert!(events[3]["detail"].as_str().unwrap().contains("ls"));
    }

    /// 实机冒烟（`cargo test zcode -- --ignored`）：对真 ~/.zcode 库跑 list +
    /// load 第一条会话，验证 schema 没漂。平时跳过——依赖本机 ZCode 数据。
    #[test]
    #[ignore = "needs a real ~/.zcode/cli/db/db.sqlite"]
    fn live_smoke_real_store() {
        let parser = ZcodeParser;
        let rows = parser.list("D:\\Myself\\Marionette").expect("list");
        assert!(!rows.is_empty(), "this machine has zcode sessions");
        println!("zcode sessions for project: {}", rows.len());
        for row in rows.iter().take(5) {
            println!("  {} | {}", row.native_id, row.title);
        }
        let events = parser.load(&rows[0].locator).expect("load");
        println!("events in first session: {}", events.len());
        assert!(!events.is_empty());
        assert!(events[0]["type"].is_string());
    }
}
