//! codex source: `$CODEX_HOME/sessions/YYYY/MM/DD/rollout-*.jsonl`.
//!
//! Verified against this machine's store (2026-11-09):
//! - Lines are `{timestamp (ISO), type, payload}`; `session_meta` carries
//!   `payload.id` (== the rollout filename uuid == the codex thread id, which
//!   is what the ACP adapter reports as `sessionId`), `payload.parent_thread_id`
//!   (resume/review parent → `parent_native_id`), `payload.cwd`, and the
//!   session's `payload.timestamp` as a creation bound. Local proof for the
//!   id choice: the Marionette-spawned main session 01a1afd4-… has
//!   `id == session_id == filename`; its guardian-review child file is named
//!   after its own `id` with `session_id == parent_thread_id`.
//! - Two usage formats, co-present in the same file describing the SAME
//!   requests (local file: 19 token_usage_record == 19 token_count):
//!   1. `type:"token_usage_record"` rows — per-response `payload.usage` with
//!      a unique `response_id` (dedupe key). Preferred; when a file has any,
//!      `event_msg/token_count` is ignored entirely (they double-report).
//!   2. `event_msg` `payload.type:"token_count"` — `info.last_token_usage`
//!      (per request) + `info.total_token_usage` (cumulative). The cumulative
//!      total is diffed to drop re-reports (diff 0 → duplicate); a FALLING
//!      cumulative value means a reset, so the row itself is taken whole.
//!   - Buckets: `input_tokens` INCLUDES `cached_input_tokens` and
//!     `cache_write_input_tokens` (OpenAI style; local proof:
//!     28 341 + 418 == 28 759 == total_tokens with cached 21 248 → fresh =
//!     input − cached − cache_write). `output_tokens` includes reasoning
//!     (reasoning_output_tokens is the subset) — same as the live ledger's
//!     Codex path (usage.ts reads `_meta.quota.token_count` and nets fresh
//!     input only).
//!   - Old files (May 2026) carry only `total_tokens` with every bucket 0 —
//!     nothing is invented for them: the request/turn still counts, buckets
//!     stay 0.
//! - Models: `turn_context.payload.model`, replaced by
//!   `thread_settings_applied.payload.thread_settings.model` when it appears
//!   later (model switches mid-session).
//! - Turns: `response_item` rows with `payload.type:"message"`,
//!   `role:"user"`, excluding injected context (text starting with
//!   `# AGENTS.md instructions`, `<environment_context>`,
//!   `<user_instructions>`, `<external_codex_apps_open_page>`).
//! - Compactions: top-level `type:"compacted"` rows.
//! - Context: `info.model_context_window` (also on `task_started`) is the
//!   WINDOW; the billed prompt (input incl. cache) is the per-request ctx
//!   reading.

use super::cache::{stamp_files, Cache};
use super::facts::{iso_ms, json_str, json_u64, SessionCollector, SessionFacts};
use super::SourceOutcome;
use serde_json::Value;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

pub(crate) const AGENT: &str = "codex";

pub(crate) fn discover_root() -> Option<PathBuf> {
    Some(
        crate::parsers::util::env_dir("CODEX_HOME")
            .or_else(|| crate::parsers::util::home_join(".codex"))?
            .join("sessions"),
    )
}

#[derive(Default, Clone, Copy)]
struct Totals {
    input: u64,
    cached: u64,
    cache_write: u64,
    output: u64,
    reasoning: u64,
    total: u64,
}

impl Totals {
    fn from_usage(v: &Value) -> Self {
        Totals {
            input: json_u64(v, "input_tokens").unwrap_or(0),
            cached: json_u64(v, "cached_input_tokens").unwrap_or(0),
            cache_write: json_u64(v, "cache_write_input_tokens").unwrap_or(0),
            output: json_u64(v, "output_tokens").unwrap_or(0),
            reasoning: json_u64(v, "reasoning_output_tokens").unwrap_or(0),
            total: json_u64(v, "total_tokens").unwrap_or(0),
        }
    }

    fn is_zero(&self) -> bool {
        self.input == 0
            && self.cached == 0
            && self.cache_write == 0
            && self.output == 0
            && self.reasoning == 0
            && self.total == 0
    }
}

/// Fill one request record from codex's OpenAI-style buckets. `input_tokens`
/// includes both cache legs (local proof: input + output == total_tokens), so
/// fresh = input − cached − cache_write (clamped; a cache sum above the input
/// leaves the field as-is). Context = the full billed prompt.
fn fill_record(record: &mut super::facts::RequestRecord, t: &Totals) {
    if t.is_zero() {
        return;
    }
    let cache_sum = t.cached.saturating_add(t.cache_write);
    record.input = if cache_sum > 0 && cache_sum <= t.input {
        t.input - cache_sum
    } else {
        t.input
    };
    record.output = t.output;
    record.cached = t.cached;
    record.cache_creation = t.cache_write;
    record.reasoning = t.reasoning;
    record.ctx_tokens = Some(t.input);
}

fn is_injected_user_text(text: &str) -> bool {
    let t = text.trim_start();
    t.starts_with("# AGENTS.md instructions")
        || t.starts_with("<environment_context>")
        || t.starts_with("<user_instructions>")
        || t.starts_with("<external_codex_apps_open_page>")
        || t.starts_with("<turn_context>")
}

pub(crate) fn parse_rollout(path: &Path, keep_from_ms: i64) -> Option<SessionFacts> {
    let file = fs::File::open(path).ok()?;
    let reader = BufReader::with_capacity(1 << 20, file);
    let mut collector: Option<SessionCollector> = None;
    let mut model: Option<String> = None;
    let mut ctx_window: Option<u64> = None;
    let mut totals_seen = Totals::default();
    // Both usage formats can appear in one file describing the same requests;
    // accumulate them separately and keep only token_usage_record when the
    // file has any (they double-report the same responses).
    let mut from_records: Vec<super::facts::RequestRecord> = Vec::new();
    let mut from_counts: Vec<super::facts::RequestRecord> = Vec::new();
    let mut seen_response_ids: std::collections::HashSet<String> = std::collections::HashSet::new();

    for raw in reader.lines() {
        let Ok(line) = raw else { break };
        // Prefilter by type markers before paying for JSON.
        if !line.contains("session_meta")
            && !line.contains("token_usage_record")
            && !line.contains("token_count")
            && !line.contains("turn_context")
            && !line.contains("thread_settings_applied")
            && !line.contains("compacted")
            && !line.contains("\"user\"")
            && !line.contains("task_started")
        {
            continue;
        }
        let Ok(row) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let t_ms = json_str(&row, "timestamp").and_then(iso_ms).unwrap_or(0);
        match json_str(&row, "type") {
            Some("session_meta") => {
                let payload = row.get("payload").unwrap_or(&Value::Null);
                let id = json_str(payload, "id")
                    .or_else(|| json_str(payload, "session_id"))
                    .unwrap_or_default()
                    .to_string();
                if id.is_empty() {
                    return None;
                }
                let mut c = SessionCollector::new(id);
                if let Some(parent) = json_str(payload, "parent_thread_id").filter(|p| !p.is_empty()) {
                    c.parent_native_id = Some(parent.to_string());
                }
                c.cwd = json_str(payload, "cwd").map(|s| s.to_string());
                c.hint_bounds(
                    json_str(payload, "timestamp").and_then(iso_ms),
                    None,
                );
                collector = Some(c);
            }
            Some("token_usage_record") => {
                let payload = row.get("payload").unwrap_or(&Value::Null);
                let response_id = json_str(payload, "response_id").unwrap_or("").to_string();
                if !response_id.is_empty() && !seen_response_ids.insert(response_id) {
                    continue; // same response written twice
                }
                let usage = payload.get("usage").unwrap_or(&Value::Null);
                let mut record = super::facts::RequestRecord {
                    t_ms,
                    model: model.clone(),
                    requests: 1,
                    ..Default::default()
                };
                fill_record(&mut record, &Totals::from_usage(usage));
                from_records.push(record);
            }
            Some("event_msg") => {
                let payload = row.get("payload").unwrap_or(&Value::Null);
                match json_str(payload, "type") {
                    Some("token_count") => {
                        let Some(info) = payload.get("info") else { continue };
                        if let Some(window) = json_u64(info, "model_context_window").filter(|w| *w > 0) {
                            ctx_window = Some(window);
                        }
                        let Some(total) = info.get("total_token_usage") else {
                            continue;
                        };
                        let totals = Totals::from_usage(total);
                        // Cumulative diff: falling totals = reset → take the
                        // row whole; equal totals = duplicate re-report → skip.
                        let reset = totals.input < totals_seen.input
                            || totals.output < totals_seen.output
                            || totals.total < totals_seen.total;
                        let delta = if reset {
                            totals
                        } else {
                            Totals {
                                input: totals.input.saturating_sub(totals_seen.input),
                                cached: totals.cached.saturating_sub(totals_seen.cached),
                                cache_write: totals
                                    .cache_write
                                    .saturating_sub(totals_seen.cache_write),
                                output: totals.output.saturating_sub(totals_seen.output),
                                reasoning: totals
                                    .reasoning
                                    .saturating_sub(totals_seen.reasoning),
                                total: totals.total.saturating_sub(totals_seen.total),
                            }
                        };
                        if !delta.is_zero() {
                            let mut record = super::facts::RequestRecord {
                                t_ms,
                                model: model.clone(),
                                requests: 1,
                                ..Default::default()
                            };
                            fill_record(&mut record, &delta);
                            from_counts.push(record);
                        }
                        totals_seen = totals;
                    }
                    Some("thread_settings_applied") => {
                        if let Some(m) = payload
                            .get("thread_settings")
                            .and_then(|s| json_str(s, "model"))
                            .filter(|m| !m.is_empty())
                        {
                            model = Some(m.to_string());
                        }
                    }
                    Some("task_started") => {
                        if let Some(window) =
                            json_u64(payload, "model_context_window").filter(|w| *w > 0)
                        {
                            ctx_window = Some(window);
                        }
                    }
                    _ => {}
                }
            }
            Some("turn_context") => {
                let payload = row.get("payload").unwrap_or(&Value::Null);
                if let Some(m) = json_str(payload, "model").filter(|m| !m.is_empty()) {
                    model = Some(m.to_string());
                }
            }
            Some("compacted") => {
                if let Some(c) = collector.as_mut() {
                    c.push_compaction(t_ms);
                }
            }
            Some("response_item") => {
                let payload = row.get("payload").unwrap_or(&Value::Null);
                if json_str(payload, "type") == Some("message")
                    && json_str(payload, "role") == Some("user")
                {
                    let text = payload
                        .get("content")
                        .and_then(|c| c.as_array())
                        .and_then(|items| {
                            items
                                .iter()
                                .find(|b| json_str(b, "type") == Some("input_text"))
                                .and_then(|b| json_str(b, "text"))
                        })
                        .unwrap_or("");
                    if !text.trim().is_empty() && !is_injected_user_text(text) {
                        if let Some(c) = collector.as_mut() {
                            c.push_turn(t_ms);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    let mut collector = collector?;
    collector.ctx_window = ctx_window;
    // token_usage_record wins when present (same responses, cleaner shape).
    for record in if from_records.is_empty() {
        from_counts
    } else {
        from_records
    } {
        collector.push_request(record);
    }
    collector.finish(AGENT, keep_from_ms)
}

pub(crate) fn scan(root: &Path, cache: &mut Cache, force: bool, keep_from_ms: i64) -> SourceOutcome {
    let mut outcome = SourceOutcome::new(AGENT, Some(root.to_string_lossy().into_owned()));
    let mut live_keys: Vec<String> = Vec::new();
    let mut sessions: Vec<SessionFacts> = Vec::new();
    if !root.is_dir() {
        outcome.found = false;
        outcome.error = Some("codex sessions root not found".to_string());
        return outcome;
    }
    outcome.found = true;
    let mut files: Vec<PathBuf> = Vec::new();
    collect_rollouts(root, 0, &mut files);
    files.sort();
    outcome.files = files.len() as u32;
    for file in files {
        let fp = stamp_files(&file, &[]);
        let key = format!("{AGENT}::{}", file.to_string_lossy());
        if !force {
            if let Some(entry) = cache.entries.get(&key) {
                if entry.fingerprint == fp {
                    sessions.extend(entry.sessions.iter().cloned());
                    live_keys.push(key);
                    continue;
                }
            }
        }
        let parsed = parse_rollout(&file, keep_from_ms);
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

fn collect_rollouts(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > 4 {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rollouts(&path, depth + 1, out);
        } else {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("rollout-") && name.ends_with(".jsonl") {
                out.push(path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_tree(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "marionette-backfill-codex-{name}-{}-{:x}",
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

    fn meta_line(id: &str, parent: Option<&str>, cwd: &str) -> String {
        json!({
            "timestamp": "2026-10-05T05:35:00.000Z", "ordinal": 0,
            "type": "session_meta",
            "payload": {"id": id, "session_id": id, "parent_thread_id": parent,
                        "timestamp": "2026-10-05T05:34:52.000Z", "cwd": cwd}
        })
        .to_string()
    }

    fn token_count_line(t: &str, totals: Value) -> String {
        json!({
            "timestamp": t, "type": "event_msg",
            "payload": {"type": "token_count",
                "info": {"total_token_usage": totals.clone(),
                         "last_token_usage": totals,
                         "model_context_window": 258400}}
        })
        .to_string()
    }

    #[test]
    fn token_count_cumulative_diff_drops_duplicates_and_handles_reset() {
        let root = temp_tree("diff");
        let f = root.join("rollout-2026-10-05T14-35-52-01a10a8f-b31e-7852-b6e5-1e732bbada33.jsonl");
        // Real numbers from this machine: three cumulative readings (1st == 2nd
        // = duplicate re-report), then a falling total (reset) with fresh
        // values, then a user turn + injected context row.
        let rows = format!(
            "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n",
            meta_line("01a10a8f-b31e-7852-b6e5-1e732bbada33", None, "D:\\Projects\\Marionette"),
            json!({"timestamp": "2026-10-05T05:36:00.000Z", "type": "turn_context",
                   "payload": {"turn_id": "t1", "model": "gpt-6-luna", "cwd": "D:\\x"}}),
            token_count_line("2026-10-05T05:36:08.587Z", json!({
                "input_tokens": 28341, "cached_input_tokens": 21248,
                "cache_write_input_tokens": 0, "output_tokens": 418,
                "reasoning_output_tokens": 175, "total_tokens": 28759})),
            token_count_line("2026-10-05T05:36:09.000Z", json!({
                "input_tokens": 28341, "cached_input_tokens": 21248,
                "cache_write_input_tokens": 0, "output_tokens": 418,
                "reasoning_output_tokens": 175, "total_tokens": 28759})),
            token_count_line("2026-10-05T05:36:18.460Z", json!({
                "input_tokens": 69256, "cached_input_tokens": 48640,
                "cache_write_input_tokens": 0, "output_tokens": 749,
                "reasoning_output_tokens": 364, "total_tokens": 70005})),
            token_count_line("2026-10-05T05:37:00.000Z", json!({
                "input_tokens": 500, "cached_input_tokens": 100,
                "cache_write_input_tokens": 50, "output_tokens": 20,
                "reasoning_output_tokens": 5, "total_tokens": 570})),
            json!({"timestamp": "2026-10-05T05:35:30.000Z", "type": "response_item",
                   "payload": {"type": "message", "role": "user",
                       "content": [{"type": "input_text", "text": "帮我看看这个仓库"}]}}),
            json!({"timestamp": "2026-10-05T05:35:31.000Z", "type": "response_item",
                   "payload": {"type": "message", "role": "user",
                       "content": [{"type": "input_text",
                                    "text": "# AGENTS.md instructions for D:\\x\n<INSTRUCTIONS>…"}]}}),
        );
        fs::write(&f, rows).unwrap();
        let facts = parse_rollout(&f, 0).unwrap();
        assert_eq!(facts.native_id, "01a10a8f-b31e-7852-b6e5-1e732bbada33");
        assert_eq!(facts.ctx_window, Some(258400));
        assert_eq!(facts.cwd.as_deref(), Some("D:\\Projects\\Marionette"));
        let h = &facts.hours[0];
        // duplicate dropped, reset taken whole, second delta diffed
        assert_eq!(h.requests, 3);
        assert_eq!(h.turns, 1);
        // request 1: fresh = 28341-21248
        // request 2 (delta): input 40915, cached 27392
        // request 3 (reset row): input 500, cached 100, write 50
        assert_eq!(h.input, (28341 - 21248) + (40915 - 27392) + (500 - 150));
        assert_eq!(h.cached, 21248 + 27392 + 100);
        assert_eq!(h.cache_creation, 50);
        assert_eq!(h.output, 418 + 331 + 20);
        assert_eq!(h.reasoning, 175 + 189 + 5);
        assert_eq!(facts.ctx_series.len(), 3);
        assert_eq!(facts.ctx_series[0][1], 28341);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn token_usage_record_wins_over_token_count() {
        let root = temp_tree("tur");
        let f = root.join("rollout-2026-10-05T14-40-00-01a10a8f-0000-0000-000000000001.jsonl");
        let usage = json!({"input_tokens": 28341, "cached_input_tokens": 21248,
                           "cache_write_input_tokens": 0, "output_tokens": 418,
                           "reasoning_output_tokens": 175, "total_tokens": 28759});
        let rows = format!(
            "{}\n{}\n{}\n{}\n",
            meta_line("01a10a8f-0000-0000-0000-000000000001", Some("01a10a8f-aaaa-0000-000000000000"), "D:\\y"),
            json!({"timestamp": "2026-10-05T05:36:08.000Z", "type": "token_usage_record",
                   "payload": {"response_id": "resp_0088", "usage": usage.clone()}}),
            json!({"timestamp": "2026-10-05T05:36:08.100Z", "type": "token_usage_record",
                   "payload": {"response_id": "resp_0088", "usage": usage.clone()}}),
            token_count_line("2026-10-05T05:36:08.587Z", usage),
        );
        fs::write(&f, rows).unwrap();
        let facts = parse_rollout(&f, 0).unwrap();
        assert_eq!(
            facts.parent_native_id.as_deref(),
            Some("01a10a8f-aaaa-0000-000000000000")
        );
        let h = &facts.hours[0];
        // ONE request: dedup by response_id, token_count ignored entirely.
        assert_eq!(h.requests, 1);
        assert_eq!(h.input, 28341 - 21248);
        assert_eq!(h.output, 418);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn total_only_rows_are_counted_without_invented_buckets() {
        let root = temp_tree("totalonly");
        let f = root.join("rollout-2026-05-18T10-35-02-019e38b8-e81c-7f00-000000000002.jsonl");
        let rows = format!(
            "{}\n{}\n{}\n",
            meta_line("019e38b8-e81c-7f00-000000000002", None, "D:\\z"),
            json!({"timestamp": "2026-05-18T02:35:02.000Z", "type": "response_item",
                   "payload": {"type": "message", "role": "user",
                       "content": [{"type": "input_text", "text": "hi"}]}}),
            token_count_line("2026-05-18T02:35:06.000Z", json!({
                "input_tokens": 0, "cached_input_tokens": 0,
                "cache_write_input_tokens": 0, "output_tokens": 0,
                "reasoning_output_tokens": 0, "total_tokens": 53})),
        );
        fs::write(&f, rows).unwrap();
        let facts = parse_rollout(&f, 0).unwrap();
        let h = &facts.hours[0];
        assert_eq!((h.turns, h.requests), (1, 1));
        assert_eq!(h.input + h.output + h.cached + h.cache_creation, 0);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn compacted_rows_mark_compactions() {
        let root = temp_tree("compact");
        let f = root.join("rollout-2026-05-21T10-22-23-019e4820-6600-000000000000.jsonl");
        let rows = format!(
            "{}\n{}\n",
            meta_line("019e4820-6600-000000000000", None, "D:\\w"),
            json!({"timestamp": "2026-05-21T02:02:11.753Z", "type": "compacted",
                   "payload": {"message": ""}}),
        );
        fs::write(&f, rows).unwrap();
        let facts = parse_rollout(&f, 0).unwrap();
        assert_eq!(facts.compactions.len(), 1);
        assert_eq!(facts.compactions[0], iso_ms("2026-05-21T02:02:11.753Z").unwrap());
        fs::remove_dir_all(&root).ok();
    }
}
