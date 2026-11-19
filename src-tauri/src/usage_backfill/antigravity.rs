//! Antigravity source: `<GEMINI_HOME>/antigravity-acp/conversations/*.db`
//! (same store `parsers/antigravity.rs` reads; that module owns the proto
//! subset — this one only folds usage).
//!
//! Per-step facts (verified against 1.5.0 payloads, 2026-11-19):
//! - `metadata.created` (field 1, Timestamp s+ns) is the step clock;
//! - `metadata.usage` (field 9) carries input(2)/output(3)/cache_write(4)/
//!   cache_read(5)/thought(9) — one record per model request;
//! - `metadata.model_info.model_name` (24.8) is the family+level id
//!   (`gemini-3.8-flash-high`);
//! - user steps (arm 19) are turns; every step with usage is a request.
//!
//! The zcode-style fresh-input question does not arise here: `usage.input`
//! is already the fresh prompt (cache read/write arrive as separate fields).

use super::cache::{file_stamp, stamp_files, Cache, CacheEntry};
use super::facts::{RequestRecord, SessionCollector, SessionFacts};
use super::SourceOutcome;
use std::path::{Path, PathBuf};

pub(crate) const AGENT: &str = "antigravity";

pub(crate) fn discover_root() -> Option<PathBuf> {
    let dir = crate::parsers::antigravity::conversations_dir();
    dir.is_dir().then_some(dir)
}

pub(crate) fn scan(root: &Path, cache: &mut Cache, force: bool, keep_from_ms: i64) -> SourceOutcome {
    let mut outcome = SourceOutcome::new(AGENT, Some(root.to_string_lossy().into_owned()));
    let mut live_keys: Vec<String> = Vec::new();
    let mut sessions: Vec<SessionFacts> = Vec::new();
    if !root.is_dir() {
        outcome.found = false;
        outcome.error = Some("antigravity conversations root not found".to_string());
        return outcome;
    }
    outcome.found = true;
    let Ok(entries) = std::fs::read_dir(root) else {
        return outcome;
    };
    let mut dbs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("db"))
        .collect();
    dbs.sort();
    outcome.files = dbs.len() as u32;
    for db in dbs {
        // The -wal sidecar can change while the db length does not; the .meta
        // sidecar carries the cwd.
        let fp = stamp_files(&db, &[&db.with_extension("wal"), &db.with_extension("meta")]);
        let key = format!("{AGENT}::{}", db.to_string_lossy());
        if !force {
            if let Some(entry) = cache.entries.get(&key) {
                if entry.fingerprint == fp {
                    sessions.extend(entry.sessions.iter().cloned());
                    live_keys.push(key);
                    continue;
                }
            }
        }
        let parsed = parse_db(&db, keep_from_ms);
        sessions.extend(parsed.iter().cloned());
        cache.entries.insert(
            key.clone(),
            CacheEntry {
                fingerprint: fp,
                sessions: parsed,
            },
        );
        live_keys.push(key);
    }
    outcome.sessions = sessions;
    outcome.live_keys = live_keys;
    outcome
}

fn parse_db(db: &Path, keep_from_ms: i64) -> Vec<SessionFacts> {
    // The db filename IS the ACP sessionId — the dedup anchor
    // (Session.native_session_ids) matches for free. A 0-byte db (schema not
    // written yet) reads as empty and is skipped.
    let Some(native_id) = db.file_stem().and_then(|s| s.to_str()) else {
        return Vec::new();
    };
    if file_stamp(db).0 == 0 {
        return Vec::new();
    }
    let mut collector = SessionCollector::new(native_id.to_string());
    collector.cwd = crate::parsers::antigravity::meta_cwd(db);
    let steps = match crate::parsers::antigravity::read_steps(db) {
        Ok(steps) => steps,
        Err(error) => {
            crate::debug_log::append(
                "usage",
                "warn",
                "-",
                "antigravity backfill: db parse failed",
                Some(&error),
            );
            return Vec::new();
        }
    };
    for step in steps {
        let Some(t_ms) = step.created_ms else { continue };
        if let crate::parsers::antigravity::StepKind::User(ref text) = step.kind {
            if !text.trim().is_empty() {
                if collector.title.is_none() {
                    collector.title = Some(crate::parsers::util::clip_chars(text.trim(), 80));
                }
                collector.push_turn(t_ms);
            }
        }
        if let Some(usage) = step.usage {
            collector.push_request(RequestRecord {
                t_ms,
                model: step.model.clone(),
                input: usage.input,
                output: usage.output,
                cached: usage.cache_read,
                cache_creation: usage.cache_write,
                reasoning: usage.thought,
                reported_usd: None,
                ctx_tokens: None,
                requests: 1,
            });
        }
        collector.hint_bounds(Some(t_ms), Some(t_ms));
    }
    collector.finish(AGENT, keep_from_ms).into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "marionette-agy-backfill-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Build a one-session conversations dir with real SQLite + protobuf
    /// bytes assembled at the verified field numbers, then fold it.
    #[test]
    fn folds_turns_requests_and_buckets() {
        let dir = tempdir("fold");
        let db = dir.join("11111111-2222-3333-4444-555555555555.db");
        {
            let conn = rusqlite::Connection::open(&db).unwrap();
            conn.execute_batch(
                "CREATE TABLE steps (idx INTEGER PRIMARY KEY, step_payload BLOB);",
            )
            .unwrap();
            // user step (type=14, arm 19) at t0
            let user = wire_user_step(1795063081_000, "hello antigravity");
            // planner step (type=15, arm 20) at t0+3s with usage + model
            let planner = wire_planner_step_with_usage(
                1795063084_000,
                1200,
                45,
                30,
                0,
                0,
                "gemini-3.8-flash-high",
            );
            conn.execute(
                "INSERT INTO steps (idx, step_payload) VALUES (0, ?1), (1, ?2)",
                rusqlite::params![user, planner],
            )
            .unwrap();
        }
        std::fs::write(
            db.with_extension("meta"),
            r#"{"cwd": "D:/tmp/agy-m0-probe", "mode_id": "default"}"#,
        )
        .unwrap();

        let mut cache = Cache::default();
        let outcome = scan(&dir, &mut cache, false, 0);
        assert!(outcome.found);
        assert_eq!(outcome.files, 1);
        assert_eq!(outcome.sessions.len(), 1);
        let facts = &outcome.sessions[0];
        assert_eq!(facts.native_id, "11111111-2222-3333-4444-555555555555");
        assert_eq!(facts.title.as_deref(), Some("hello antigravity"));
        assert_eq!(facts.cwd.as_deref(), Some("D:/tmp/agy-m0-probe"));
        assert_eq!(facts.hours.len(), 1);
        let hour = &facts.hours[0];
        assert_eq!(hour.turns, 1);
        assert_eq!(hour.requests, 1);
        assert_eq!(hour.input, 1200);
        assert_eq!(hour.output, 45);
        assert_eq!(hour.reasoning, 30);
        assert_eq!(hour.model.as_deref(), Some("gemini-3.8-flash-high"));

        // Cached fingerprint: a second scan serves from the cache (1 entry).
        let mut cache2 = cache.clone();
        let again = scan(&dir, &mut cache2, false, 0);
        assert_eq!(again.sessions.len(), 1);
        assert!(again.live_keys.len() == 1);

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

    /// Field header + varint value (wire type 0).
    fn varint_field(field: u32, v: u64) -> Vec<u8> {
        let mut out = varint(((field << 3) | 0) as u64);
        out.extend(varint(v));
        out
    }

    fn wire_field(field: u32, payload: &[u8]) -> Vec<u8> {
        let mut out = varint(((field << 3) | 2) as u64);
        out.extend(varint(payload.len() as u64));
        out.extend_from_slice(payload);
        out
    }

    fn wire_timestamp(ms: i64) -> Vec<u8> {
        let mut ts = varint_field(1, ms.div_euclid(1000) as u64);
        ts.extend(varint_field(2, ms.rem_euclid(1000) as u64 * 1_000_000));
        wire_field(1, &ts)
    }

    fn wire_user_step(ms: i64, text: &str) -> Vec<u8> {
        let mut step = varint_field(1, 14); // user_input
        step.extend_from_slice(&[0x20, 0x03]); // status = 3
        let metadata = wire_timestamp(ms);
        step.extend(varint((5 << 3) | 2));
        step.extend(varint(metadata.len() as u64));
        step.extend_from_slice(&metadata);
        let arm = wire_field(2, text.as_bytes());
        step.extend(varint((19 << 3) | 2));
        step.extend(varint(arm.len() as u64));
        step.extend_from_slice(&arm);
        step
    }

    fn wire_planner_step_with_usage(
        ms: i64,
        input: u64,
        output: u64,
        thought: u64,
        cache_write: u64,
        cache_read: u64,
        model: &str,
    ) -> Vec<u8> {
        let mut usage = varint_field(2, input);
        usage.extend(varint_field(3, output));
        if cache_write > 0 {
            usage.extend(varint_field(4, cache_write));
        }
        if cache_read > 0 {
            usage.extend(varint_field(5, cache_read));
        }
        usage.extend(varint_field(9, thought));
        let usage_field = wire_field(9, &usage);
        let model_field = wire_field(24, &wire_field(8, model.as_bytes()));
        let mut metadata = wire_timestamp(ms);
        metadata.extend_from_slice(&usage_field);
        metadata.extend_from_slice(&model_field);

        let mut step = varint_field(1, 15); // planner_response
        step.extend(varint((5 << 3) | 2));
        step.extend(varint(metadata.len() as u64));
        step.extend_from_slice(&metadata);
        let arm = wire_field(1, b"done");
        step.extend(varint((20 << 3) | 2));
        step.extend(varint(arm.len() as u64));
        step.extend_from_slice(&arm);
        step
    }
}

/// Opt-in oracle against the user's real store:
/// `cargo test --bin marionette antigravity_backfill_oracle -- --ignored --nocapture`
#[cfg(test)]
mod oracle {
    use super::*;

    #[test]
    #[ignore = "depends on this machine's real ~/.gemini store"]
    fn antigravity_backfill_oracle_real_store() {
        let Some(root) = discover_root() else {
            panic!("no real conversations dir");
        };
        let mut cache = Cache::default();
        let outcome = scan(&root, &mut cache, false, 0);
        println!(
            "files={} sessions={}",
            outcome.files,
            outcome.sessions.len()
        );
        for facts in outcome.sessions.iter().take(4) {
            println!(
                "  {} | turns+h={:?} | title={:?}",
                &facts.native_id[..8],
                facts.hours.iter().map(|h| (h.turns, h.requests, h.input, h.output)).collect::<Vec<_>>(),
                facts.title
            );
        }
        // The probe sessions on this machine each carry one user turn and at
        // least one usage-bearing step.
        assert!(outcome.sessions.iter().all(|s| !s.hours.is_empty()));
    }
}
