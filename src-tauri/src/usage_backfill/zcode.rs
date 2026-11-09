//! zcode source: `~/.zcode/cli/db/db.sqlite` (respects `ZCODE_HOME`; same
//! location `parsers/zcode.rs` reads).
//!
//! Verified against this machine's store (2026-11-09) — unlike the research
//! doc's cautious "usage fields not yet probed", this db DOES record usage, in
//! dedicated tables:
//! - `model_usage`: one row per model request with the full five-bucket split
//!   (`input_tokens` INCLUDES cache — local proof: input + output ==
//!   computed_total_tokens, e.g. 144 799 + 436 == 145 235 with cache_read
//!   143 872 → fresh = input − cache_creation − cache_read). `reasoning_tokens`
//!   is inside `output_tokens` (rows with reasoning: input+output still equals
//!   the total). `provider_id`/`model_id` name the model, `started_at`/`
//!   completed_at` are epoch ms, `query_source` is `main_turn`/`subagent`/
//!   `session_title` (title-generation requests still burn tokens, so they
//!   count as requests but never as turns).
//! - `turn_usage`: one row per user turn (`started_at`, `session_id`) —
//!   includes subagent sessions' turns.
//! - `session`: `id` (`sess_…` — the id zcode-acp-server reports as the ACP
//!   `sessionId`), `parent_id` (subagent linking: local rows like
//!   `sess_subagent_agent_<uuid>` with parent `sess_<uuid>`), `directory`,
//!   `title`, `time_created`/`time_updated`. `time_compacting` exists but is
//!   NULL everywhere locally — no compaction timestamps available.
//! - Known gap: no context-window column anywhere → `ctx_window` stays None;
//!   the per-request billed prompt still feeds `ctx_series`.

use super::cache::{stamp_files, Cache};
use super::facts::{SessionCollector, SessionFacts};
use super::SourceOutcome;
use rusqlite::{Connection, OpenFlags};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub(crate) const AGENT: &str = "zcode";

pub(crate) fn discover_db() -> Option<PathBuf> {
    Some(
        crate::parsers::util::env_dir("ZCODE_HOME")
            .or_else(|| crate::parsers::util::home_join(".zcode"))?
            .join("cli")
            .join("db")
            .join("db.sqlite"),
    )
    .filter(|p| p.is_file())
}

fn open_ro(path: &Path) -> Result<Connection, String> {
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| format!("open zcode db.sqlite: {e}"))
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

/// Fold the db. `Ok((sessions, note))` — `note` is a non-fatal gap message
/// (e.g. this db predates the usage tables → sessions/turns only, tokens 0).
pub(crate) fn parse_db(
    db: &Path,
    keep_from_ms: i64,
) -> Result<(Vec<SessionFacts>, Option<String>), String> {
    let conn = open_ro(db)?;
    if !has_table(&conn, "session") {
        return Err("zcode db has no session table".into());
    }
    let has_usage = has_table(&conn, "model_usage");
    let has_turns = has_table(&conn, "turn_usage");
    let note = (!has_usage).then(|| "该库不记录 token 用量（无 model_usage 表）".to_string());

    let mut collectors: HashMap<String, SessionCollector> = HashMap::new();
    let mut order: Vec<String> = Vec::new();

    {
        let mut stmt = conn
            .prepare("SELECT id, parent_id, directory, title, time_created, time_updated FROM session")
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
                ))
            })
            .map_err(|e| format!("scan sessions: {e}"))?;
        for entry in rows.flatten() {
            let (id, parent, directory, title, created, updated) = entry;
            let mut collector = SessionCollector::new(id.clone());
            collector.parent_native_id = parent.filter(|p| !p.is_empty());
            collector.cwd = directory.filter(|d| !d.is_empty());
            collector.title = title.filter(|t| !t.is_empty());
            collector.hint_bounds(created, updated);
            order.push(id.clone());
            collectors.insert(id, collector);
        }
    }

    if has_usage {
        let mut stmt = conn
            .prepare(
                "SELECT session_id, provider_id, model_id, input_tokens, output_tokens, \
                 reasoning_tokens, cache_creation_input_tokens, cache_read_input_tokens, started_at \
                 FROM model_usage ORDER BY started_at",
            )
            .map_err(|e| format!("query model_usage: {e}"))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                    row.get::<_, Option<i64>>(6)?,
                    row.get::<_, Option<i64>>(7)?,
                    row.get::<_, Option<i64>>(8)?,
                ))
            })
            .map_err(|e| format!("scan model_usage: {e}"))?;
        for entry in rows.flatten() {
            let (
                session_id,
                provider,
                model,
                input_billed,
                output,
                reasoning,
                cache_creation,
                cached,
                started_at,
            ) = entry;
            let input_billed = input_billed.unwrap_or(0).max(0) as u64;
            let output = output.unwrap_or(0).max(0) as u64;
            let reasoning = reasoning.unwrap_or(0).max(0) as u64;
            let cache_creation = cache_creation.unwrap_or(0).max(0) as u64;
            let cached = cached.unwrap_or(0).max(0) as u64;
            if input_billed == 0 && output == 0 && cached == 0 && cache_creation == 0 {
                continue;
            }
            let t_ms = started_at.unwrap_or(0);
            let model_label = match (provider.as_deref(), model.as_deref()) {
                (Some(p), Some(m)) if !p.is_empty() && !m.is_empty() => {
                    // model_id often already carries the provider
                    // (opencode-go/deepseek-…); join only when it doesn't.
                    if m.contains('/') {
                        Some(m.to_string())
                    } else {
                        Some(format!("{p}/{m}"))
                    }
                }
                (_, Some(m)) if !m.is_empty() => Some(m.to_string()),
                (Some(p), _) if !p.is_empty() => Some(p.to_string()),
                _ => None,
            };
            let cache_sum = cached.saturating_add(cache_creation);
            let input = if cache_sum > 0 && cache_sum <= input_billed {
                input_billed - cache_sum
            } else {
                input_billed
            };
            let collector = ensure_collector(&mut collectors, &mut order, session_id.as_deref());
            collector.push_request(super::facts::RequestRecord {
                t_ms,
                model: model_label,
                input,
                output,
                cached,
                cache_creation,
                reasoning,
                reported_usd: None,
                ctx_tokens: Some(input_billed),
                requests: 1,
            });
        }
    }

    if has_turns {
        let mut stmt = conn
            .prepare("SELECT session_id, started_at FROM turn_usage ORDER BY started_at")
            .map_err(|e| format!("query turn_usage: {e}"))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, Option<String>>(0)?, row.get::<_, Option<i64>>(1)?))
            })
            .map_err(|e| format!("scan turn_usage: {e}"))?;
        for (session_id, started_at) in rows.flatten() {
            let t_ms = started_at.unwrap_or(0);
            let collector = ensure_collector(&mut collectors, &mut order, session_id.as_deref());
            collector.push_turn(t_ms);
        }
    }

    let mut out = Vec::new();
    for id in order {
        if let Some(collector) = collectors.remove(&id) {
            if let Some(facts) = collector.finish(AGENT, keep_from_ms) {
                out.push(facts);
            }
        }
    }
    Ok((out, note))
}

/// Usage rows can reference sessions the `session` table no longer lists
/// (compacted away); give them a collector instead of dropping the tokens.
fn ensure_collector<'a>(
    collectors: &'a mut HashMap<String, SessionCollector>,
    order: &mut Vec<String>,
    session_id: Option<&str>,
) -> &'a mut SessionCollector {
    let id = session_id.filter(|s| !s.is_empty()).unwrap_or("__orphan__");
    if !collectors.contains_key(id) {
        order.push(id.to_string());
        collectors.insert(id.to_string(), SessionCollector::new(id.to_string()));
    }
    collectors.get_mut(id).unwrap()
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
        outcome.error = Some("zcode db.sqlite not found".to_string());
        return outcome;
    };
    outcome.found = true;
    outcome.files = 1;
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
        Ok((sessions, note)) => {
            outcome.sessions = sessions.clone();
            cache.entries.insert(
                key.clone(),
                super::cache::CacheEntry {
                    fingerprint: fp,
                    sessions,
                },
            );
            outcome.live_keys.push(key);
            // Non-fatal gap note (db without usage tables) surfaces in the UI.
            outcome.error = note;
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

    fn temp_db(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "marionette-backfill-zc-{name}-{}-{:x}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("db.sqlite")
    }

    fn build(db: &Path) {
        let conn = Connection::open(db).unwrap();
        conn.execute_batch(
            "CREATE TABLE session (id TEXT PRIMARY KEY, parent_id TEXT, directory TEXT,
                title TEXT, time_created INTEGER, time_updated INTEGER);
             CREATE TABLE model_usage (id TEXT, session_id TEXT, provider_id TEXT, model_id TEXT,
                input_tokens INTEGER, output_tokens INTEGER, reasoning_tokens INTEGER,
                cache_creation_input_tokens INTEGER, cache_read_input_tokens INTEGER,
                started_at INTEGER, query_source TEXT);
             CREATE TABLE turn_usage (session_id TEXT, started_at INTEGER);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session VALUES ('sess_main', NULL, 'D:\\Projects\\Marionette', '迁移统计页', 1000, 9000)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session VALUES ('sess_subagent_agent_x1', 'sess_main', 'D:\\Projects\\Marionette', NULL, 2000, 8000)",
            [],
        )
        .unwrap();
        // Real shapes from this machine's GLM-5.3 rows: billed input includes
        // cache_read; output includes reasoning.
        conn.execute(
            "INSERT INTO model_usage VALUES ('r1','sess_main','bigmodel-api','GLM-5.3',
                144799, 436, 0, 0, 143872, 1794189000000, 'main_turn')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO model_usage VALUES ('r2','sess_main','opencode-go','opencode-go/deepseek-v4.1-flash',
                22942, 1583, 1330, 0, 22656, 1794189100000, 'session_title')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO model_usage VALUES ('r3','sess_subagent_agent_x1','bigmodel-api','GLM-5.3',
                143058, 462, 0, 0, 142336, 1794189200000, 'subagent')",
            [],
        )
        .unwrap();
        // Orphan: usage for a session the session table no longer lists.
        conn.execute(
            "INSERT INTO model_usage VALUES ('r4','sess_gone','bigmodel-api','GLM-5.3',
                1000, 50, 0, 0, 900, 1794189300000, 'main_turn')",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO turn_usage VALUES ('sess_main', 1794188999000)", [])
            .unwrap();
        conn.execute("INSERT INTO turn_usage VALUES ('sess_subagent_agent_x1', 1794189199000)", [])
            .unwrap();
    }

    #[test]
    fn folds_subagents_orphans_and_title_requests() {
        let db = temp_db("fold");
        build(&db);
        let (sessions, note) = parse_db(&db, 0).unwrap();
        assert!(note.is_none());
        assert_eq!(sessions.len(), 3);

        let main = sessions.iter().find(|s| s.native_id == "sess_main").unwrap();
        assert_eq!(main.title.as_deref(), Some("迁移统计页"));
        assert_eq!(main.cwd.as_deref(), Some("D:\\Projects\\Marionette"));
        assert_eq!(main.compactions.len(), 0);
        // r1 + r2 (title-gen counts as a request, not a turn).
        assert_eq!(main.hours.iter().map(|h| h.requests).sum::<u32>(), 2);
        assert_eq!(main.hours.iter().map(|h| h.turns).sum::<u32>(), 1);
        // r1 fresh = 144799-143872; r2 fresh = 22942-22656.
        assert_eq!(main.hours.iter().map(|h| h.input).sum::<u64>(), 927 + 286);
        // r2's reasoning (1330) is INSIDE output (input+output == computed).
        assert_eq!(main.hours.iter().map(|h| h.output).sum::<u64>(), 436 + 1583);
        assert_eq!(main.hours.iter().map(|h| h.reasoning).sum::<u64>(), 1330);
        // model_id without a provider gets `provider/model`; one already
        // carrying a provider is kept verbatim.
        let models: Vec<&str> = main
            .hours
            .iter()
            .filter_map(|h| h.model.as_deref())
            .collect();
        assert!(models.contains(&"bigmodel-api/GLM-5.3"));
        assert!(models.contains(&"opencode-go/deepseek-v4.1-flash"));

        let sub = sessions
            .iter()
            .find(|s| s.native_id == "sess_subagent_agent_x1")
            .unwrap();
        assert_eq!(sub.parent_native_id.as_deref(), Some("sess_main"));
        assert_eq!(sub.hours.iter().map(|h| h.requests).sum::<u32>(), 1);
        assert_eq!(sub.hours.iter().map(|h| h.turns).sum::<u32>(), 1);

        let orphan = sessions.iter().find(|s| s.native_id == "sess_gone").unwrap();
        assert_eq!(orphan.hours.iter().map(|h| h.input).sum::<u64>(), 100);
        std::fs::remove_dir_all(db.parent().unwrap()).ok();
    }

    #[test]
    fn missing_usage_tables_report_the_gap() {
        let db = temp_db("no Usage");
        {
            let conn = Connection::open(&db).unwrap();
            conn.execute_batch(
                "CREATE TABLE session (id TEXT PRIMARY KEY, parent_id TEXT, directory TEXT,
                    title TEXT, time_created INTEGER, time_updated INTEGER);
                 CREATE TABLE turn_usage (session_id TEXT, started_at INTEGER);",
            )
            .unwrap();
            conn.execute("INSERT INTO session VALUES ('s1', NULL, NULL, 'T', 1, 2)", [])
                .unwrap();
            conn.execute("INSERT INTO turn_usage VALUES ('s1', 1794188999000)", [])
                .unwrap();
        }
        let (sessions, note) = parse_db(&db, 0).unwrap();
        assert_eq!(note.as_deref(), Some("该库不记录 token 用量（无 model_usage 表）"));
        // Turns still counted, tokens all zero.
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].hours[0].turns, 1);
        assert_eq!(sessions[0].hours[0].requests, 0);
        assert_eq!(sessions[0].hours[0].input + sessions[0].hours[0].output, 0);
        std::fs::remove_dir_all(db.parent().unwrap()).ok();
    }

    #[test]
    fn scan_caches_by_db_stamp() {
        let db = temp_db("cache");
        build(&db);
        let mut cache = Cache::default();
        let out1 = scan(Some(&db), &mut cache, false, 0);
        assert_eq!(out1.sessions.len(), 3);
        assert_eq!(cache.entries.len(), 1);
        let key = cache.entries.keys().next().unwrap().clone();
        cache.entries.get_mut(&key).unwrap().sessions[0].title = Some("cached".into());
        let out2 = scan(Some(&db), &mut cache, false, 0);
        assert!(out2.sessions.iter().any(|s| s.title.as_deref() == Some("cached")));
        let out3 = scan(Some(&db), &mut cache, true, 0);
        assert!(!out3.sessions.iter().any(|s| s.title.as_deref() == Some("cached")));
        std::fs::remove_dir_all(db.parent().unwrap()).ok();
    }
}
