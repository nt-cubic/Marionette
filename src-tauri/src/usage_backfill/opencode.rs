//! opencode source: `~/.local/share/opencode/opencode.db` (Windows keeps the
//! XDG path too; respects `OPENCODE_DATA_DIR` / `XDG_DATA_HOME`, same
//! candidate chain as `parsers/opencode.rs`).
//!
//! Verified against this machine's store (2026-11-09):
//! - `native_id` = `session.id` (`ses_…`) — the id opencode's ACP adapter
//!   reports (same alphabet as Marionette's `nativeSessionIds` entries).
//! - TWO generations of tables, with the SAME message ids duplicated across
//!   them (local db: 1938 of 1940 `message` ids also in `session_message`) —
//!   never read both:
//!   1. `session_v2` + `session_message` (current): `session_message` has a
//!      `type` column (`user`/`assistant`/`compaction`/`synthetic`/…);
//!      assistant `data.tokens` = `{input, output, reasoning, cache{read,
//!      write}}` and `data.model` = `{id, providerID, variant}`; `time` =
//!      `{created, completed}` epoch ms. `session_v2` covers sessions the
//!      legacy table never saw (141 vs 76 rows locally).
//!   2. `session` + `message` (legacy): `data.tokens` additionally carries
//!      `total`, whose arithmetic (input+output+cache+reasoning == total,
//!      e.g. 102 743+30+183 == 102 956) proves **reasoning is a SEPARATE
//!      bucket** — so the ledger's `output` (all generated) = output +
//!      reasoning, matching the live path's `generatedOutputTokens` treatment
//!      of opencode's `thoughtTokens`.
//! - Buckets otherwise map 1:1 (`input` is fresh: total == input + output +
//!   cache_read + cache_write locally).
//! - Turns: `user`-type messages. Compactions: `compaction`-type rows (v2) /
//!   `session.time_compacting` (legacy; always NULL locally).
//! - Context per request = input + cache.read + cache.write. No context
//!   window column exists anywhere → `ctx_window` stays None.
//! - Read-only open with WAL co-present works (parsers/credentials precedent).

use super::cache::{stamp_files, Cache};
use super::facts::{json_i64, json_str, json_u64, SessionCollector, SessionFacts};
use super::SourceOutcome;
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub(crate) const AGENT: &str = "opencode";

pub(crate) fn discover_db() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    let home = PathBuf::from(home);
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(dir) = crate::parsers::util::env_dir("OPENCODE_DATA_DIR") {
        candidates.push(dir.join("opencode").join("opencode.db"));
    }
    if let Some(dir) = crate::parsers::util::env_dir("XDG_DATA_HOME") {
        candidates.push(dir.join("opencode").join("opencode.db"));
    }
    candidates.push(home.join(".local").join("share").join("opencode").join("opencode.db"));
    candidates.push(home.join(".config").join("opencode").join("opencode.db"));
    candidates.push(
        home.join("AppData")
            .join("Local")
            .join("opencode")
            .join("opencode.db"),
    );
    candidates.push(home.join(".opencode").join("opencode.db"));
    candidates.into_iter().find(|p| p.is_file())
}

fn open_ro(path: &Path) -> Result<Connection, String> {
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| format!("open opencode.db: {e}"))
}

fn has_table(conn: &Connection, table: &str) -> bool {
    conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
        [table],
        |row| row.get::<_, i64>(0),
    )
    .map(|n| n > 0)
    .unwrap_or(false)
}

fn has_column(conn: &Connection, table: &str, column: &str) -> bool {
    let Ok(mut stmt) = conn.prepare(&format!("PRAGMA table_info({table})")) else {
        return false;
    };
    let found = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map(|rows| rows.flatten().any(|name| name == column))
        .unwrap_or(false);
    found
}

pub(crate) fn parse_db(db: &Path, keep_from_ms: i64) -> Result<Vec<SessionFacts>, String> {
    let conn = open_ro(db)?;
    let mut meta: HashMap<String, SessionCollector> = HashMap::new();
    let mut cwd_by_id: HashMap<String, String> = HashMap::new();
    let mut order: Vec<String> = Vec::new();

    // Sessions: prefer v2 (superset locally), else legacy.
    let v2 = has_table(&conn, "session_v2") && has_table(&conn, "session_message");
    let (session_table, message_table) = if v2 {
        ("session_v2", "session_message")
    } else if has_table(&conn, "session") && has_table(&conn, "message") {
        ("session", "message")
    } else {
        return Ok(Vec::new());
    };
    let parent_col = has_column(&conn, session_table, "parent_id");
    let compact_col = has_column(&conn, session_table, "time_compacting");
    {
        // Column order: id, parent(NULL when absent), directory, title,
        // time_created, time_updated, time_compacting(NULL when absent).
        let mut stmt = conn
            .prepare(&format!(
                "SELECT id, {parent}, directory, title, time_created, time_updated, {compact} \
                 FROM {session_table}",
                parent = if parent_col { "parent_id" } else { "NULL" },
                compact = if compact_col { "time_compacting" } else { "NULL" },
            ))
            .map_err(|e| format!("query sessions: {e}"))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                    row.get::<_, Option<i64>>(6)?,
                ))
            })
            .map_err(|e| format!("scan sessions: {e}"))?;
        for entry in rows.flatten() {
            let (id, parent, directory, title, created, updated, compacting) = entry;
            let mut collector = SessionCollector::new(id.clone());
            collector.parent_native_id = parent.filter(|p| !p.is_empty());
            collector.title = title.filter(|t| !t.is_empty());
            collector.hint_bounds(created, updated);
            if let Some(t) = compacting.filter(|t| *t > 0) {
                collector.push_compaction(t);
            }
            if let Some(dir) = directory.filter(|d| !d.is_empty()) {
                cwd_by_id.insert(id.clone(), dir);
            }
            order.push(id.clone());
            meta.insert(id, collector);
        }
    }

    // Messages: one streaming pass, bucket math in Rust.
    let type_col = has_column(&conn, message_table, "type");
    let mut stmt = conn
        .prepare(&format!(
            "SELECT session_id, {}data FROM {message_table} ORDER BY time_created",
            if type_col { "type, " } else { "NULL, " }
        ))
        .map_err(|e| format!("query messages: {e}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|e| format!("scan messages: {e}"))?;
    for entry in rows.flatten() {
        let (session_id, row_type, data) = entry;
        let Some(collector) = meta.get_mut(&session_id) else {
            continue;
        };
        let Ok(data) = serde_json::from_str::<Value>(&data) else {
            continue;
        };
        // The v2 table says the row type; the legacy table only has
        // data.role.
        let role = row_type
            .as_deref()
            .map(|t| t.to_string())
            .or_else(|| json_str(&data, "role").map(|r| r.to_string()));
        let t_ms = data
            .get("time")
            .and_then(|t| json_i64(t, "created"))
            .unwrap_or(0);
        match role.as_deref() {
            Some("user") => collector.push_turn(t_ms),
            Some("assistant") => {
                let Some(tokens) = data.get("tokens") else { continue };
                let cache = tokens.get("cache").unwrap_or(&Value::Null);
                let input = json_u64(tokens, "input").unwrap_or(0);
                let output = json_u64(tokens, "output").unwrap_or(0);
                let reasoning = json_u64(tokens, "reasoning").unwrap_or(0);
                let cached = json_u64(cache, "read").unwrap_or(0);
                let cache_write = json_u64(cache, "write").unwrap_or(0);
                if input == 0 && output == 0 && reasoning == 0 && cached == 0 && cache_write == 0 {
                    continue;
                }
                // Model: v2 nests {id, providerID}; legacy has flat modelID.
                let model = data
                    .get("model")
                    .and_then(|m| {
                        let id = json_str(m, "id").filter(|s| !s.is_empty());
                        let provider = json_str(m, "providerID").filter(|s| !s.is_empty());
                        match (provider, id) {
                            (Some(p), Some(i)) => Some(format!("{p}/{i}")),
                            (None, Some(i)) => Some(i.to_string()),
                            _ => None,
                        }
                    })
                    .or_else(|| {
                        let id = json_str(&data, "modelID").filter(|s| !s.is_empty())?;
                        match json_str(&data, "providerID").filter(|s| !s.is_empty()) {
                            Some(p) => Some(format!("{p}/{id}")),
                            None => Some(id.to_string()),
                        }
                    });
                collector.push_request(super::facts::RequestRecord {
                    t_ms,
                    model,
                    input,
                    // Reasoning is a separate bucket in opencode (proven by the
                    // legacy table's total arithmetic) → add it back so the
                    // ledger's `output` is everything generated.
                    output: output + reasoning,
                    cached,
                    cache_creation: cache_write,
                    reasoning,
                    reported_usd: None,
                    ctx_tokens: Some(input + cached + cache_write),
                    requests: 1,
                });
            }
            // compaction rows live in the v2 message stream with their own type
            Some("compaction") => {
                if t_ms > 0 {
                    collector.push_compaction(t_ms);
                }
            }
            _ => {}
        }
    }

    let mut out = Vec::new();
    for id in order {
        if let Some(mut collector) = meta.remove(&id) {
            collector.cwd = cwd_by_id.get(&id).cloned();
            if let Some(facts) = collector.finish(AGENT, keep_from_ms) {
                out.push(facts);
            }
        }
    }
    Ok(out)
}

pub(crate) fn scan(
    db: Option<&Path>,
    cache: &mut Cache,
    force: bool,
    keep_from_ms: i64,
) -> SourceOutcome {
    let mut outcome = SourceOutcome::new(AGENT, db.map(|p| p.to_string_lossy().into_owned()));
    let Some(db) = db.filter(|p| p.is_file()) else {
        outcome.found = false;
        outcome.error = Some("opencode.db not found".to_string());
        return outcome;
    };
    outcome.found = true;
    outcome.files = 1;
    // Fingerprint the db together with its WAL/SHM: an un-checkpointed WAL
    // changes what a read-only connection sees.
    let wal = {
        let mut s = db.as_os_str().to_os_string();
        s.push("-wal");
        PathBuf::from(s)
    };
    // -shm is deliberately NOT fingerprinted: it is an ephemeral index of
    // the WAL whose mtime moves on plain reads, which would defeat the cache.
    let fp = stamp_files(db, &[&wal]);
    let key = format!("{AGENT}::{}", db.to_string_lossy());
    if !force {
        if let Some(entry) = cache.entries.get(&key) {
            if entry.fingerprint == fp {
                outcome.sessions = entry.sessions.clone();
                outcome.live_keys.push(key);
                return outcome;
            }
        }
    }
    match parse_db(&db, keep_from_ms) {
        Ok(sessions) => {
            outcome.sessions = sessions.clone();
            cache.entries.insert(
                key.clone(),
                super::cache::CacheEntry {
                    fingerprint: fp,
                    sessions,
                },
            );
            outcome.live_keys.push(key);
        }
        Err(error) => {
            outcome.error = Some(error);
        }
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_db(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "marionette-backfill-oc-{name}-{}-{:x}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("opencode.db")
    }

    #[test]
    fn v2_schema_folds_turns_buckets_parent_and_compaction() {
        let db = temp_db("v2");
        {
            let conn = Connection::open(&db).unwrap();
            conn.execute_batch(
                "CREATE TABLE session_v2 (id TEXT PRIMARY KEY, directory TEXT, title TEXT,
                    time_created INTEGER, time_updated INTEGER, parent_id TEXT, time_compacting INTEGER);
                 CREATE TABLE session_message (id TEXT PRIMARY KEY, session_id TEXT, type TEXT,
                    seq INTEGER, time_created INTEGER, time_updated INTEGER, data TEXT);",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO session_v2 VALUES ('ses_child', 'D:/proj', 'Child work', 1000, 9000, 'ses_parent', NULL)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO session_message VALUES ('m1','ses_child','user',1,1000,1000,?1)",
                [json!({"text": "go", "time": {"created": 1000}}).to_string()],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO session_message VALUES ('m2','ses_child','assistant',2,2000,2000,?1)",
                [json!({
                    "time": {"created": 2000, "completed": 2500},
                    "model": {"id": "deepseek-v4.1-flash-free", "providerID": "opencode", "variant": "high"},
                    "tokens": {"input": 16311, "output": 422, "reasoning": 30,
                               "cache": {"read": 22656, "write": 100}}
                }).to_string()],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO session_message VALUES ('m3','ses_child','compaction',3,3000,3000,?1)",
                [json!({"time": {"created": 3000}, "status": "completed", "reason": "manual"}).to_string()],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO session_message VALUES ('m4','ses_child','synthetic',4,4000,4000,?1)",
                [json!({"time": {"created": 4000}}).to_string()],
            )
            .unwrap();
        }
        let sessions = parse_db(&db, 0).unwrap();
        assert_eq!(sessions.len(), 1);
        let s = &sessions[0];
        assert_eq!(s.native_id, "ses_child");
        assert_eq!(s.parent_native_id.as_deref(), Some("ses_parent"));
        assert_eq!(s.cwd.as_deref(), Some("D:/proj"));
        assert_eq!(s.compactions, vec![3000]);
        let h = &s.hours[0];
        assert_eq!((h.turns, h.requests), (1, 1));
        assert_eq!(h.input, 16311);
        assert_eq!(h.cached, 22656);
        assert_eq!(h.cache_creation, 100);
        // reasoning added back into output
        assert_eq!(h.output, 422 + 30);
        assert_eq!(h.reasoning, 30);
        assert_eq!(h.model.as_deref(), Some("opencode/deepseek-v4.1-flash-free"));
        assert_eq!(s.ctx_series[0][1], 16311 + 22656 + 100);
        std::fs::remove_dir_all(db.parent().unwrap()).ok();
    }

    #[test]
    fn legacy_schema_uses_role_from_data() {
        let db = temp_db("legacy");
        {
            let conn = Connection::open(&db).unwrap();
            conn.execute_batch(
                "CREATE TABLE session (id TEXT PRIMARY KEY, parent_id TEXT, directory TEXT,
                    title TEXT, time_created INTEGER, time_updated INTEGER, time_compacting INTEGER);
                 CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT,
                    time_created INTEGER, time_updated INTEGER, data TEXT);",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO session VALUES ('ses_old', NULL, 'D:/x', 'Old', 100, 200, NULL)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO message VALUES ('m1','ses_old',1000,1000,?1)",
                [json!({"role": "user", "time": {"created": 1000},
                        "content": "hi"}).to_string()],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO message VALUES ('m2','ses_old',2000,2000,?1)",
                [json!({
                    "role": "assistant", "modelID": "grok-4.6", "providerID": "opencode",
                    "time": {"created": 2000},
                    "tokens": {"input": 411, "output": 155, "reasoning": 0,
                               "total": 42230, "cache": {"read": 41664, "write": 0}}
                }).to_string()],
            )
            .unwrap();
        }
        let sessions = parse_db(&db, 0).unwrap();
        assert_eq!(sessions.len(), 1);
        let h = &sessions[0].hours[0];
        assert_eq!((h.turns, h.requests), (1, 1));
        assert_eq!(h.input, 411);
        assert_eq!(h.cached, 41664);
        assert_eq!(h.model.as_deref(), Some("opencode/grok-4.6"));
        std::fs::remove_dir_all(db.parent().unwrap()).ok();
    }

    #[test]
    fn scan_caches_by_db_and_wal_stamp() {
        let db = temp_db("cache");
        {
            let conn = Connection::open(&db).unwrap();
            conn.execute_batch(
                "CREATE TABLE session_v2 (id TEXT PRIMARY KEY, directory TEXT, title TEXT,
                    time_created INTEGER, time_updated INTEGER);
                 CREATE TABLE session_message (id TEXT PRIMARY KEY, session_id TEXT, type TEXT,
                    seq INTEGER, time_created INTEGER, time_updated INTEGER, data TEXT);",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO session_v2 VALUES ('ses_a', NULL, NULL, 1, 2)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO session_message VALUES ('m1','ses_a','assistant',1,1000,1000,?1)",
                [json!({"time": {"created": 1000}, "model": {"id": "m1", "providerID": "p"},
                         "tokens": {"input": 5, "output": 2, "reasoning": 0,
                                    "cache": {"read": 0, "write": 0}}}).to_string()],
            )
            .unwrap();
        }
        let mut cache = Cache::default();
        let out1 = scan(Some(&db), &mut cache, false, 0);
        assert_eq!(out1.sessions.len(), 1);
        let key = cache.entries.keys().next().unwrap().clone();
        cache.entries.get_mut(&key).unwrap().sessions[0].title = Some("cached".into());
        let out2 = scan(Some(&db), &mut cache, false, 0);
        assert_eq!(out2.sessions[0].title.as_deref(), Some("cached"));
        // force re-parses.
        let out3 = scan(Some(&db), &mut cache, true, 0);
        assert_ne!(out3.sessions[0].title.as_deref(), Some("cached"));
        std::fs::remove_dir_all(db.parent().unwrap()).ok();
    }
}
