//! grok-build source: `~/.grok/sessions/<url-encoded-cwd>/<uuid>/` (respects
//! `GROK_HOME`).
//!
//! Verified against this machine's store (2026-11-09):
//! - `native_id` = the session directory's uuid == `summary.json info.id` ==
//!   `params.sessionId` on every `session/update` line — the same string the
//!   ACP adapter reports as `sessionId` (checked against Marionette's
//!   `nativeSessionIds` recording convention; grok sessions here predate the
//!   recording, but the dir/id/sessionId triple is self-consistent).
//! - `updates.jsonl` lines are ACP events
//!   `{timestamp: <seconds>, method: "_x.ai/session/update" | "session/update",
//!     params: {sessionId, update: {sessionUpdate, …}, _meta: {agentTimestampMs}}}`.
//!   - `turn_completed` carries `update.usage` per TURN (turns with no model
//!     calls — elapsed_ms 25–31ms — legitimately have none; verified against
//!     `signals.json turnCount`: 3 turn_completed == turnCount 3):
//!     `{inputTokens, cachedReadTokens, cacheCreationTokens, outputTokens,
//!       reasoningTokens, totalTokens, costUsdTicks, modelCalls,
//!       modelUsage: {<model>: {same fields}}}`.
//!   - Bucket conversion (same as the live ledger's Grok path, usage.ts
//!     `netFreshInput`): `inputTokens` is the BILLED prompt — cache inside.
//!     Local proof: 2 824 236 + 38 862 == 2 863 098 == totalTokens, with
//!     cachedRead 2 692 480. So fresh = inputTokens − cachedRead − cacheCreation
//!     (clamped ≥ 0; when the cache sum exceeds inputTokens we take the field
//!     as already-fresh). `outputTokens` already includes reasoning.
//!   - `costUsdTicks` = 1e-10 USD per tick (docs + research §3).
//!   - `compaction_checkpoint` events mark compactions.
//! - `summary.json`: `session_summary`/`generated_title`, `info.cwd`,
//!   `created_at`/`updated_at` (ISO).
//! - `signals.json`: `contextTokensUsed`/`contextWindowTokens` — session-final
//!   context reading; emitted as one ctx point at `updated_at`.
//! - Known gap: `subagents/<id>/` holds only `meta.json` (parent/child ids)
//!   with no usage on disk, so subagent sessions are not emitted.
//!   Old sessions (pre-usage versions) emit turns only.

use super::cache::{stamp_files, Cache};
use super::facts::{
    iso_ms, json_i64, json_str, json_u64, SessionCollector, SessionFacts,
};
use super::SourceOutcome;
use serde_json::Value;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

pub(crate) const AGENT: &str = "grok-build";

pub(crate) fn discover_root() -> Option<PathBuf> {
    Some(
        crate::parsers::util::env_dir("GROK_HOME")
            .or_else(|| crate::parsers::util::home_join(".grok"))?
            .join("sessions"),
    )
}

fn turn_usage_records(
    usage: &Value,
    t_ms: i64,
    fallback_model: Option<&str>,
) -> Vec<super::facts::RequestRecord> {
    // Per-model split when modelUsage is present (multi-model turns); the
    // top-level totals equal its sum on this machine.
    let mut out = Vec::new();
    if let Some(models) = usage.get("modelUsage").and_then(|m| m.as_object()) {
        for (model, entry) in models {
            if let Some(record) = usage_record(
                entry,
                t_ms,
                Some(model.as_str()),
                json_u64(entry, "modelCalls").unwrap_or(1).max(1) as u32,
            ) {
                out.push(record);
            }
        }
        if !out.is_empty() {
            return out;
        }
    }
    if let Some(record) = usage_record(
        usage,
        t_ms,
        fallback_model,
        json_u64(usage, "modelCalls").unwrap_or(1).max(1) as u32,
    ) {
        out.push(record);
    }
    out
}

fn usage_record(
    v: &Value,
    t_ms: i64,
    model: Option<&str>,
    requests: u32,
) -> Option<super::facts::RequestRecord> {
    let input_billed = json_u64(v, "inputTokens")?;
    let cached = json_u64(v, "cachedReadTokens").unwrap_or(0);
    let cache_creation = json_u64(v, "cacheCreationTokens").unwrap_or(0);
    let output = json_u64(v, "outputTokens").unwrap_or(0);
    if input_billed == 0 && output == 0 && cached == 0 && cache_creation == 0 {
        return None;
    }
    let cache_sum = cached.saturating_add(cache_creation);
    let input = if cache_sum > 0 && cache_sum <= input_billed {
        input_billed - cache_sum
    } else {
        input_billed
    };
    let usd = json_u64(v, "costUsdTicks")
        .map(|ticks| ticks as f64 * 1e-10);
    Some(super::facts::RequestRecord {
        t_ms,
        model: model.map(|s| s.to_string()),
        input,
        output,
        cached,
        cache_creation,
        reasoning: json_u64(v, "reasoningTokens").unwrap_or(0),
        reported_usd: usd,
        ctx_tokens: None,
        requests,
    })
}

/// Parse one session directory into facts (also used by tests with fabricated
/// trees).
pub(crate) fn parse_session_dir(dir: &Path, keep_from_ms: i64) -> Option<SessionFacts> {
    let updates = dir.join("updates.jsonl");
    if !updates.is_file() {
        return None;
    }
    let mut collector = SessionCollector::new(
        dir.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string(),
    );
    let mut primary_model: Option<String> = None;

    let file = fs::File::open(&updates).ok()?;
    let reader = BufReader::new(file);
    for line in reader.lines() {
        let Ok(line) = line else { break };
        // Cheap prefilter: two sessionUpdate kinds carry everything we need.
        if !line.contains("turn_completed") && !line.contains("compaction_checkpoint") {
            continue;
        }
        let Ok(event) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let params = event.get("params").unwrap_or(&Value::Null);
        let update = params.get("update").unwrap_or(&Value::Null);
        let kind = json_str(update, "sessionUpdate").unwrap_or("");
        // ms-precision timestamp when the adapter stamps one, else the line's
        // top-level seconds.
        let t_ms = event
            .get("_meta")
            .and_then(|m| json_i64(m, "agentTimestampMs"))
            .or_else(|| {
                params
                    .get("_meta")
                    .and_then(|m| json_i64(m, "agentTimestampMs"))
            })
            .or_else(|| json_i64(&event, "timestamp").map(|s| s * 1000))
            .unwrap_or(0);
        match kind {
            "turn_completed" => {
                // Turn start = completion − elapsed_ms (sane turns only).
                let elapsed = json_i64(update, "elapsed_ms").unwrap_or(0);
                let start = if (0..3_600_000).contains(&elapsed) {
                    t_ms - elapsed
                } else {
                    t_ms
                };
                collector.push_turn(start);
                if let Some(usage) = update.get("usage") {
                    if primary_model.is_none() {
                        if let Some(models) = usage.get("modelUsage").and_then(|m| m.as_object()) {
                            primary_model = models.keys().next().cloned();
                        }
                    }
                    for record in turn_usage_records(usage, t_ms, primary_model.as_deref()) {
                        collector.push_request(record);
                    }
                }
            }
            "compaction_checkpoint" => collector.push_compaction(t_ms),
            _ => {}
        }
    }

    // summary.json: title / cwd / bounds. (Parsed after the stream so a
    // malformed summary never kills the usage data.)
    if let Ok(text) = fs::read_to_string(dir.join("summary.json")) {
        if let Ok(summary) = serde_json::from_str::<Value>(&text) {
            if collector.title.is_none() {
                collector.title = json_str(&summary, "session_summary")
                    .or_else(|| json_str(&summary, "generated_title"))
                    .map(|s| s.to_string())
                    .filter(|s| !s.is_empty());
            }
            if collector.cwd.is_none() {
                collector.cwd = summary
                    .get("info")
                    .and_then(|info| json_str(info, "cwd"))
                    .map(|s| s.to_string());
            }
            collector.hint_bounds(
                json_str(&summary, "created_at").and_then(iso_ms),
                json_str(&summary, "updated_at")
                    .or_else(|| json_str(&summary, "last_active_at"))
                    .and_then(iso_ms),
            );
        }
    }
    // signals.json: final context reading + window. The reading is also the
    // composition's measured anchor.
    let mut signals_used: Option<u64> = None;
    if let Ok(text) = fs::read_to_string(dir.join("signals.json")) {
        if let Ok(signals) = serde_json::from_str::<Value>(&text) {
            let used = json_u64(&signals, "contextTokensUsed");
            let window = json_u64(&signals, "contextWindowTokens").filter(|w| *w > 0);
            if let Some(window) = window {
                collector.ctx_window = Some(window);
            }
            if let Some(used) = used {
                signals_used = Some(used);
                // Anchor the final context reading at the latest timestamp the
                // metadata knows (summary updated_at).
                let at = collector.last_hint().unwrap_or(0);
                collector.push_ctx_point(at, used);
            }
        }
    }
    let mut facts = collector.finish(AGENT, keep_from_ms)?;
    // Composition comes from the same directory's request material; the
    // measured anchor is the final context reading (signals.json), NOT the
    // last turn's inputTokens (a session-lifetime sum).
    facts.composition =
        super::composition::grok_session_composition(dir, signals_used);
    Some(facts)
}

/// Locate a session directory by native id (the directory name is the uuid the
/// adapter reports as sessionId) — for the on-demand live-session lookup.
pub(crate) fn find_session_dir(root: &Path, native_id: &str) -> Option<PathBuf> {
    let native_id = native_id.trim();
    if native_id.is_empty() {
        return None;
    }
    for encoded in read_dirs(root) {
        for session in read_dirs(&encoded) {
            if session
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.eq_ignore_ascii_case(native_id))
                .unwrap_or(false)
                && session.join("chat_history.jsonl").is_file()
            {
                return Some(session);
            }
        }
    }
    None
}

pub(crate) fn scan(root: &Path, cache: &mut Cache, force: bool, keep_from_ms: i64) -> SourceOutcome {
    let mut outcome = SourceOutcome::new(AGENT, Some(root.to_string_lossy().into_owned()));
    let mut live_keys: Vec<String> = Vec::new();
    let mut sessions: Vec<SessionFacts> = Vec::new();
    if !root.is_dir() {
        outcome.found = false;
        outcome.error = Some("grok sessions root not found".to_string());
        return outcome;
    }
    outcome.found = true;
    let mut dirs: Vec<PathBuf> = Vec::new();
    for encoded in read_dirs(root) {
        for session in read_dirs(&encoded) {
            if session.join("updates.jsonl").is_file() {
                dirs.push(session);
            }
        }
    }
    outcome.files = dirs.len() as u32;
    for dir in dirs {
        let fp = stamp_files(
            &dir.join("updates.jsonl"),
            &[
                &dir.join("summary.json"),
                &dir.join("signals.json"),
                // Composition reads these too; a stale cache must not serve an
                // outdated composition when updates.jsonl did not change.
                &dir.join("chat_history.jsonl"),
                &dir.join("system_prompt.txt"),
                &dir.join("tool_definitions.json"),
            ],
        );
        let key = format!("{AGENT}::{}", dir.to_string_lossy());
        if !force {
            if let Some(entry) = cache.entries.get(&key) {
                if entry.fingerprint == fp {
                    sessions.extend(entry.sessions.iter().cloned());
                    live_keys.push(key);
                    continue;
                }
            }
        }
        let parsed = parse_session_dir(&dir, keep_from_ms);
        let entry_sessions: Vec<SessionFacts> = parsed.into_iter().collect();
        sessions.extend(entry_sessions.iter().cloned());
        cache.entries.insert(
            key.clone(),
            super::cache::CacheEntry {
                fingerprint: fp,
                sessions: entry_sessions,
            },
        );
        live_keys.push(key);
    }
    outcome.sessions = sessions;
    outcome.live_keys = live_keys;
    outcome
}

fn read_dirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_tree(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "marionette-backfill-grok-{name}-{}-{:x}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn turn_usage_splits_buckets_like_the_live_ledger() {
        // Real shape from ~/.grok (2026-11-06): billed input includes cache.
        let usage = json!({
            "inputTokens": 2_824_236i64,
            "outputTokens": 38_862,
            "totalTokens": 2_863_098i64,
            "cachedReadTokens": 2_692_480i64,
            "cacheCreationTokens": 0,
            "reasoningTokens": 28_416,
            "modelCalls": 26,
            "costUsdTicks": 6_265_941_600i64,
            "modelUsage": {
                "grok-4.6-build": {
                    "inputTokens": 2_824_236i64, "outputTokens": 38_862,
                    "cachedReadTokens": 2_692_480i64, "cacheCreationTokens": 0,
                    "reasoningTokens": 28_416, "modelCalls": 26,
                    "costUsdTicks": 6_265_941_600i64
                }
            }
        });
        let records = turn_usage_records(&usage, 1_793_953_129_031, None);
        assert_eq!(records.len(), 1);
        let r = &records[0];
        assert_eq!(r.model.as_deref(), Some("grok-4.6-build"));
        assert_eq!(r.input, 2_824_236 - 2_692_480); // fresh only
        assert_eq!(r.cached, 2_692_480);
        assert_eq!(r.cache_creation, 0);
        assert_eq!(r.output, 38_862);
        assert_eq!(r.reasoning, 28_416);
        assert_eq!(r.requests, 26);
        assert!((r.reported_usd.unwrap() - 0.62659416).abs() < 1e-9);
    }

    #[test]
    fn session_dir_folds_turns_hours_compactions_and_ctx() {
        let root = temp_tree("dir");
        let dir = root.join("01a1afe6-2836-74a1-b5a9-cb2cd2a51169");
        fs::create_dir_all(&dir).unwrap();
        let t0 = 1_793_953_129i64; // seconds
        let mut updates = String::new();
        // Turn 1 (hour H) with usage; elapsed 585988ms puts the start in the
        // previous minute (same hour).
        updates.push_str(&json!({
            "timestamp": t0, "method": "_x.ai/session/update",
            "params": {"sessionId": "01a1afe6-2836-74a1-b5a9-cb2cd2a51169",
                "update": {"sessionUpdate": "turn_completed",
                    "prompt_id": "p1", "stop_reason": "end_turn",
                    "elapsed_ms": 585_988,
                    "usage": {"inputTokens": 1000, "outputTokens": 100,
                              "cachedReadTokens": 600, "cacheCreationTokens": 100,
                              "reasoningTokens": 50, "modelCalls": 2,
                              "costUsdTicks": 1_000_000_000}},
                "_meta": {"agentTimestampMs": t0 * 1000 + 31}}
        }).to_string());
        updates.push('\n');
        // Turn 2 in the next hour, no usage (tiny turn).
        updates.push_str(&json!({
            "timestamp": t0 + 7300, "method": "session/update",
            "params": {"sessionId": "01a1afe6-2836-74a1-b5a9-cb2cd2a51169",
                "update": {"sessionUpdate": "turn_completed",
                    "prompt_id": "p2", "stop_reason": "end_turn",
                    "elapsed_ms": 25},
                "_meta": {"agentTimestampMs": (t0 + 7300) * 1000}}
        }).to_string());
        updates.push('\n');
        updates.push_str(&json!({
            "timestamp": t0 + 100, "method": "session/update",
            "params": {"update": {"sessionUpdate": "compaction_checkpoint"}},
            "_meta": {"agentTimestampMs": (t0 + 100) * 1000}
        }).to_string());
        updates.push('\n');
        fs::write(dir.join("updates.jsonl"), updates).unwrap();
        fs::write(
            dir.join("summary.json"),
            json!({"info": {"id": "01a1afe6-2836-74a1-b5a9-cb2cd2a51169",
                            "cwd": "D:\\Projects\\Marionette"},
                   "session_summary": "Backfill test",
                   "created_at": "2026-11-06T08:10:00Z",
                   "updated_at": "2026-11-06T08:18:49Z",
                   "current_model_id": "grok-4.6"}).to_string(),
        )
        .unwrap();
        fs::write(
            dir.join("signals.json"),
            json!({"contextTokensUsed": 130_386, "contextWindowTokens": 256_000}).to_string(),
        )
        .unwrap();

        let facts = parse_session_dir(&dir, 0).unwrap();
        assert_eq!(facts.native_id, "01a1afe6-2836-74a1-b5a9-cb2cd2a51169");
        assert_eq!(facts.title.as_deref(), Some("Backfill test"));
        assert_eq!(facts.cwd.as_deref(), Some("D:\\Projects\\Marionette"));
        assert_eq!(facts.ctx_window, Some(256_000));
        assert_eq!(facts.hours.len(), 2);
        let h0 = &facts.hours[0];
        assert_eq!((h0.turns, h0.requests), (1, 2));
        assert_eq!(h0.input, 1000 - 700);
        assert_eq!((h0.cached, h0.cache_creation, h0.output), (600, 100, 100));
        let h1 = &facts.hours[1];
        assert_eq!(h1.turns, 1);
        assert_eq!(h1.requests, 0);
        assert_eq!(facts.compactions.len(), 1);
        assert_eq!(facts.ctx_series.len(), 1);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn scan_uses_cache_until_fingerprint_changes() {
        let root = temp_tree("scan");
        // Real layout: <encoded-cwd>/<uuid>/.
        let dir = root
            .join("D%3A%5CProjects%5CMarionette")
            .join("01a1afe6-0000-0000-0000-000000000001");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("updates.jsonl"),
            format!(
                "{}\n",
                json!({"timestamp": 1_793_953_129, "method": "session/update",
                    "params": {"update": {"sessionUpdate": "turn_completed",
                        "prompt_id": "p1", "elapsed_ms": 10,
                        "usage": {"inputTokens": 10, "outputTokens": 5, "modelCalls": 1}}}})
            ),
        )
        .unwrap();
        let mut cache = Cache::default();
        let out1 = scan(&root, &mut cache, false, 0);
        assert_eq!(out1.sessions.len(), 1);
        assert_eq!(cache.entries.len(), 1);
        // Cached path: same result, and a sentinel mutation of the cache entry
        // is reflected (proves we did not re-parse).
        let key = cache.entries.keys().next().unwrap().clone();
        cache.entries.get_mut(&key).unwrap().sessions[0].title = Some("cached".into());
        let out2 = scan(&root, &mut cache, false, 0);
        assert_eq!(out2.sessions[0].title.as_deref(), Some("cached"));
        // force re-parses and the sentinel disappears.
        let out3 = scan(&root, &mut cache, true, 0);
        assert_ne!(out3.sessions[0].title.as_deref(), Some("cached"));
        // Touching the file invalidates.
        fs::write(dir.join("updates.jsonl"), "{}\n").unwrap();
        let out4 = scan(&root, &mut cache, false, 0);
        assert!(out4.sessions.is_empty());
        fs::remove_dir_all(&root).ok();
    }
}
