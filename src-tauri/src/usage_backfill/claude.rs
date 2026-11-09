//! claude-code source: `~/.claude/projects/<encoded-cwd>/*.jsonl` (respects
//! `CLAUDE_CONFIG_DIR`); subagents in `<session-dir>/subagents/agent-*.jsonl`.
//!
//! Verified against this machine's store (2026-11-09):
//! - `native_id` = the transcript file stem == every row's `sessionId` — the
//!   id claude-agent-acp reports as the ACP `sessionId`.
//! - Assistant rows carry `message.usage` per API REQUEST:
//!   `{input_tokens (FRESH — Claude's own semantics), cache_creation_input_tokens,
//!     cache_read_input_tokens, output_tokens, output_tokens_details.thinking_tokens}`.
//!   No conversion needed — this IS the ledger's bucket layout. `output_tokens`
//!   already includes thinking; `thinking_tokens` is the reasoning subset.
//! - **Dedup**: the same API response is written as several assistant lines
//!   repeating the same usage — key `(message.id, requestId)` (ccusage's rule;
//!   local file: 67 usage rows → 31 duplicates). `<synthetic>` models skipped.
//! - `isSidechain:true` rows are subagent traffic. On this machine they live in
//!   `subagents/agent-<hex>.jsonl` (rows still carry the PARENT sessionId);
//!   emitted as their own session, `native_id = "agent-<hex>"` (the file stem),
//!   `parent_native_id` = parent sessionId. Sidechain rows found inside a main
//!   file (older layout) are routed the same way, keyed by `agentId` when
//!   present.
//! - Turns: `type:"user"` rows whose content is a string or has a text block,
//!   excluding `isCompactSummary` / `isMeta` / sidechain (local file: 17).
//! - Compactions: `type:"system", subtype:"compact_boundary"` → timestamp.
//! - Context per request = input + cache_read + cache_creation (research §3;
//!   the billed prompt is exactly what sat in the context window).
//! - Titles: `type:"custom-title"` (`customTitle`) — present locally; fall back
//!   to `type:"summary"`, then the first real user prompt clipped to 80 chars.
//! - Known gap: `cost-state` rows carry session-cumulative `totalCostUSD`
//!   (per-model under `modelUsage`) which cannot be split across hours
//!   honestly, so no `reported_usd` is emitted for Claude.
//! - Files reach 80 MB: streamed line by line, substring-prefiltered before
//!   JSON parsing.

use super::cache::{stamp_files, Cache};
use super::facts::{iso_ms, json_str, json_u64, SessionCollector, SessionFacts};
use super::SourceOutcome;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

pub(crate) const AGENT: &str = "claude-code";

pub(crate) fn discover_root() -> Option<PathBuf> {
    Some(
        crate::parsers::util::env_dir("CLAUDE_CONFIG_DIR")
            .or_else(|| crate::parsers::util::home_join(".claude"))?
            .join("projects"),
    )
}

struct RowSinks<'a> {
    /// Main session collector (keyed by file stem).
    main: &'a mut SessionCollector,
    /// Sidechain collectors discovered inside this file, keyed by a stable
    /// sub-key (`agent-<hex>` or the row's `agentId`).
    sidechains: &'a mut HashMap<String, SessionCollector>,
}

/// Fold one transcript file. Returns whether any usage row was seen.
fn fold_file(path: &Path, session_key: &str, parent_id: Option<&str>, sinks: &mut RowSinks) {
    let Ok(file) = fs::File::open(path) else {
        return;
    };
    let reader = BufReader::with_capacity(1 << 20, file);
    let mut seen_usage: HashSet<(String, String)> = HashSet::new();
    let mut first_user_text: Option<String> = None;
    let mut cwd: Option<String> = None;
    for raw in reader.lines() {
        let line = match raw {
            Ok(text) => text,
            Err(_) => break,
        };
        // Prefilter: only line types that can contribute.
        if !line.contains("\"type\":\"assistant\"")
            && !line.contains("\"type\":\"user\"")
            && !line.contains("\"type\":\"system\"")
            && !line.contains("\"type\":\"custom-title\"")
            && !line.contains("\"type\":\"summary\"")
        {
            continue;
        }
        let Ok(row) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if cwd.is_none() {
            if let Some(c) = json_str(&row, "cwd") {
                cwd = Some(c.to_string());
            }
        }
        match json_str(&row, "type") {
            Some("assistant") => {
                let Some(message) = row.get("message") else { continue };
                let model = json_str(message, "model").unwrap_or("");
                if model.is_empty() || model.contains("<synthetic>") {
                    continue;
                }
                let Some(usage) = message.get("usage") else { continue };
                let msg_id = json_str(message, "id").unwrap_or("").to_string();
                let request_id = json_str(&row, "requestId").unwrap_or("").to_string();
                if !msg_id.is_empty() || !request_id.is_empty() {
                    let key = (msg_id, request_id);
                    if !seen_usage.insert(key) {
                        continue; // duplicate write of the same API response
                    }
                }
                let t_ms = json_str(&row, "timestamp").and_then(iso_ms).unwrap_or(0);
                let input = json_u64(usage, "input_tokens").unwrap_or(0);
                let cache_creation =
                    json_u64(usage, "cache_creation_input_tokens").unwrap_or(0);
                let cached = json_u64(usage, "cache_read_input_tokens").unwrap_or(0);
                let output = json_u64(usage, "output_tokens").unwrap_or(0);
                if input == 0 && output == 0 && cached == 0 && cache_creation == 0 {
                    continue;
                }
                let reasoning = usage
                    .get("output_tokens_details")
                    .and_then(|d| json_u64(d, "thinking_tokens"))
                    .unwrap_or(0);
                let sidechain = row.get("isSidechain").and_then(|v| v.as_bool()).unwrap_or(false);
                let target = if sidechain {
                    let agent_key = json_str(&row, "agentId")
                        .map(|a| format!("agent-{a}"))
                        .unwrap_or_else(|| "agent-sidechain".to_string());
                    sinks.sidechains.entry(agent_key.clone()).or_insert_with(|| {
                        new_sidechain(&agent_key, parent_id.unwrap_or(session_key))
                    })
                } else {
                    &mut *sinks.main
                };
                target.cwd = target.cwd.clone().or_else(|| cwd.clone());
                target.push_request(super::facts::RequestRecord {
                    t_ms,
                    model: Some(model.to_string()),
                    input,
                    output,
                    cached,
                    cache_creation,
                    reasoning,
                    reported_usd: None,
                    ctx_tokens: Some(input + cached + cache_creation),
                    requests: 1,
                });
            }
            Some("user") => {
                let sidechain = row.get("isSidechain").and_then(|v| v.as_bool()).unwrap_or(false);
                let compact_summary = row
                    .get("isCompactSummary")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let is_meta = row.get("isMeta").and_then(|v| v.as_bool()).unwrap_or(false);
                if compact_summary || is_meta {
                    continue;
                }
                let Some(message) = row.get("message") else { continue };
                let text = user_text(message);
                let Some(text) = text else { continue };
                let t_ms = json_str(&row, "timestamp").and_then(iso_ms).unwrap_or(0);
                if sidechain {
                    let agent_key = json_str(&row, "agentId")
                        .map(|a| format!("agent-{a}"))
                        .unwrap_or_else(|| "agent-sidechain".to_string());
                    let collector = sinks
                        .sidechains
                        .entry(agent_key.clone())
                        .or_insert_with(|| {
                            new_sidechain(&agent_key, parent_id.unwrap_or(session_key))
                        });
                    collector.push_turn(t_ms);
                } else {
                    if first_user_text.is_none() {
                        first_user_text = Some(text.chars().take(80).collect());
                    }
                    sinks.main.push_turn(t_ms);
                }
            }
            Some("system") => {
                if json_str(&row, "subtype") == Some("compact_boundary") {
                    if let Some(t) = json_str(&row, "timestamp").and_then(iso_ms) {
                        sinks.main.push_compaction(t);
                    }
                }
            }
            Some("custom-title") => {
                if sinks.main.title.is_none() {
                    if let Some(title) = json_str(&row, "customTitle").filter(|t| !t.is_empty()) {
                        sinks.main.title = Some(title.to_string());
                    }
                }
            }
            Some("summary") => {
                if sinks.main.title.is_none() {
                    if let Some(title) = json_str(&row, "summary").filter(|t| !t.is_empty()) {
                        sinks.main.title = Some(title.to_string());
                    }
                }
            }
            _ => {}
        }
    }
    if sinks.main.cwd.is_none() {
        sinks.main.cwd = cwd;
    }
    if sinks.main.title.is_none() {
        sinks.main.title = first_user_text.filter(|t| !t.is_empty());
    }
}

fn new_sidechain(agent_key: &str, parent_id: &str) -> SessionCollector {
    let mut c = SessionCollector::new(agent_key.to_string());
    c.parent_native_id = Some(parent_id.to_string());
    c
}

/// Real user prompt text: string content, or a text block in a list. Rows
/// whose content is only `tool_result` blocks return None.
fn user_text(message: &Value) -> Option<String> {
    match message.get("content") {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Array(items)) => {
            let mut text = String::new();
            for item in items {
                if json_str(item, "type") == Some("text") {
                    if let Some(t) = json_str(item, "text") {
                        if !text.is_empty() {
                            text.push('\n');
                        }
                        text.push_str(t);
                    }
                }
            }
            if text.is_empty() {
                None
            } else {
                Some(text)
            }
        }
        _ => None,
    }
}

/// Fold one main session file (stem = native id) plus its sibling
/// `subagents/agent-*.jsonl` files.
fn parse_session(
    path: &Path,
    keep_from_ms: i64,
    out: &mut Vec<SessionFacts>,
) {
    let stem = path
        .file_stem()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_string();
    if stem.is_empty() {
        return;
    }
    let mut main = SessionCollector::new(stem.clone());
    let mut sidechains: HashMap<String, SessionCollector> = HashMap::new();
    {
        let mut sinks = RowSinks {
            main: &mut main,
            sidechains: &mut sidechains,
        };
        fold_file(path, &stem, None, &mut sinks);
    }
    if let Some(facts) = main.finish(AGENT, keep_from_ms) {
        out.push(facts);
    }
    // Subagent transcripts live in <stem>/subagents/ next to the main file
    // (rows inside carry the PARENT sessionId — that's how we link them).
    let sub_dir = path.with_file_name(&stem).join("subagents");
    if let Ok(entries) = fs::read_dir(&sub_dir) {
        let mut files: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.extension().and_then(|e| e.to_str()) == Some("jsonl")
                    && p
                        .file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| n.starts_with("agent-"))
                        .unwrap_or(false)
            })
            .collect();
        files.sort();
        for file in files {
            let agent_key = file
                .file_stem()
                .and_then(|n| n.to_str())
                .unwrap_or("agent-unknown")
                .to_string();
            let mut collector = {
                let mut c = SessionCollector::new(agent_key.clone());
                c.parent_native_id = Some(stem.clone());
                c
            };
            let mut side_map: HashMap<String, SessionCollector> = HashMap::new();
            {
                let mut sinks = RowSinks {
                    main: &mut collector,
                    sidechains: &mut side_map,
                };
                fold_file(&file, &agent_key, Some(&stem), &mut sinks);
            }
            // Any nested sidechain rows inside a subagent file fold into the
            // file's own session (they are that agent's replays).
            for extra in side_map.into_values() {
                collector.absorb(extra);
            }
            if let Some(facts) = collector.finish(AGENT, keep_from_ms) {
                out.push(facts);
            }
        }
    }
    for (_key, collector) in sidechains {
        // Sidechain rows found in the MAIN file: already keyed by agent id
        // with parent = this file's session.
        if let Some(facts) = collector.finish(AGENT, keep_from_ms) {
            out.push(facts);
        }
    }
}

pub(crate) fn scan(root: &Path, cache: &mut Cache, force: bool, keep_from_ms: i64) -> SourceOutcome {
    let mut outcome = SourceOutcome::new(AGENT, Some(root.to_string_lossy().into_owned()));
    let mut live_keys: Vec<String> = Vec::new();
    let mut sessions: Vec<SessionFacts> = Vec::new();
    if !root.is_dir() {
        outcome.found = false;
        outcome.error = Some("claude projects root not found".to_string());
        return outcome;
    }
    outcome.found = true;
    let mut files: Vec<PathBuf> = Vec::new();
    collect_transcripts(root, 0, &mut files);
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
        let mut parsed: Vec<SessionFacts> = Vec::new();
        parse_session(&file, keep_from_ms, &mut parsed);
        sessions.extend(parsed.iter().cloned());
        cache.entries.insert(
            key.clone(),
            super::cache::CacheEntry {
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

fn collect_transcripts(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > 3 {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            // `subagents` dirs are handled per-session; `memory` holds notes.
            if name == "subagents" || name == "memory" {
                continue;
            }
            collect_transcripts(&path, depth + 1, out);
        } else if name.ends_with(".jsonl") {
            out.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_tree(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "marionette-backfill-claude-{name}-{}-{:x}",
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

    fn assistant_row(id: &str, request: &str, ts: &str, usage: Value) -> String {
        json!({
            "type": "assistant",
            "sessionId": "sess-1",
            "requestId": request,
            "timestamp": ts,
            "cwd": "D:\\proj",
            "isSidechain": false,
            "message": {
                "id": id,
                "model": "claude-sonnet-5-5",
                "usage": usage
            }
        })
        .to_string()
    }

    #[test]
    fn duplicate_usage_rows_are_deduped_by_message_and_request() {
        let root = temp_tree("dedup");
        let file = root.join("sess-1.jsonl");
        let usage = json!({
            "input_tokens": 2,
            "cache_creation_input_tokens": 66527,
            "cache_read_input_tokens": 48462,
            "output_tokens": 343,
            "output_tokens_details": {"thinking_tokens": 68}
        });
        // Same (message.id, requestId) written twice + a genuinely new row.
        let rows = format!(
            "{}\n{}\n{}\n{}\n",
            assistant_row("msg_1", "req_1", "2026-11-06T08:18:51.541Z", usage.clone()),
            assistant_row("msg_1", "req_1", "2026-11-06T08:18:52.000Z", usage.clone()),
            assistant_row("msg_1", "req_2", "2026-11-06T08:20:00.000Z", usage.clone()),
            json!({
                "type": "user", "sessionId": "sess-1",
                "timestamp": "2026-11-06T08:18:40.000Z",
                "message": {"role": "user", "content": "hello world"}
            })
        );
        fs::write(&file, rows).unwrap();
        let mut out = Vec::new();
        parse_session(&file, 0, &mut out);
        assert_eq!(out.len(), 1);
        let facts = &out[0];
        assert_eq!(facts.native_id, "sess-1");
        assert_eq!(facts.hours.len(), 1);
        let h = &facts.hours[0];
        assert_eq!((h.requests, h.turns), (2, 1));
        assert_eq!(h.input, 4);
        assert_eq!(h.cache_creation, 2 * 66527);
        assert_eq!(h.cached, 2 * 48462);
        assert_eq!(h.output, 2 * 343);
        assert_eq!(h.reasoning, 2 * 68);
        // ctx per request = input + cache_read + cache_creation.
        assert_eq!(facts.ctx_series.len(), 2);
        assert_eq!(facts.ctx_series[0][1], 2 + 48462 + 66527);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn sidechain_and_compact_and_title() {
        let root = temp_tree("side");
        let file = root.join("sess-2.jsonl");
        let rows = format!(
            "{}\n{}\n{}\n{}\n{}\n",
            json!({
                "type": "custom-title", "sessionId": "sess-2",
                "customTitle": "用户消息视觉区分"
            }),
            json!({
                "type": "user", "sessionId": "sess-2",
                "timestamp": "2026-11-06T08:00:00.000Z",
                "message": {"role": "user", "content": "go"}
            }),
            json!({
                "type": "assistant", "sessionId": "sess-2",
                "requestId": "req_9", "timestamp": "2026-11-06T08:00:05.000Z",
                "isSidechain": true, "agentId": "a233ea67c70280dec",
                "message": {"id": "msg_s", "model": "claude-sonnet-5-5",
                    "usage": {"input_tokens": 2, "cache_creation_input_tokens": 7436,
                              "cache_read_input_tokens": 16869, "output_tokens": 1}}
            }),
            json!({
                "type": "system", "subtype": "compact_boundary",
                "timestamp": "2026-11-06T08:01:00.000Z",
                "compactMetadata": {"trigger": "auto", "preTokens": 385456}
            }),
            json!({
                "type": "user", "isCompactSummary": true, "sessionId": "sess-2",
                "timestamp": "2026-11-06T08:01:00.100Z",
                "message": {"role": "user", "content": "compacted summary text"}
            })
        );
        fs::write(&file, rows).unwrap();
        let mut out = Vec::new();
        parse_session(&file, 0, &mut out);
        assert_eq!(out.len(), 2);
        let main = out.iter().find(|s| s.native_id == "sess-2").unwrap();
        // compact-summary user row does NOT count as a turn; the real one does.
        assert_eq!(main.hours[0].turns, 1);
        assert_eq!(main.hours[0].requests, 0);
        assert_eq!(main.compactions.len(), 1);
        assert_eq!(main.title.as_deref(), Some("用户消息视觉区分"));
        let side = out.iter().find(|s| s.native_id != "sess-2").unwrap();
        assert_eq!(side.native_id, "agent-a233ea67c70280dec");
        assert_eq!(side.parent_native_id.as_deref(), Some("sess-2"));
        assert_eq!(side.hours[0].requests, 1);
        assert_eq!(side.hours[0].input, 2);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn subagent_dir_files_become_child_sessions() {
        let root = temp_tree("subdir");
        let file = root.join("sess-3.jsonl");
        fs::write(
            &file,
            format!(
                "{}\n",
                json!({"type": "user", "sessionId": "sess-3",
                    "timestamp": "2026-11-06T08:00:00.000Z",
                    "message": {"role": "user", "content": "research"}})
            ),
        )
        .unwrap();
        let sub = root.join("sess-3").join("subagents");
        fs::create_dir_all(&sub).unwrap();
        fs::write(
            sub.join("agent-beef0000.jsonl"),
            format!(
                "{}\n{}\n",
                json!({"type": "user", "sessionId": "sess-3", "isSidechain": true,
                    "timestamp": "2026-11-06T08:00:01.000Z",
                    "message": {"role": "user", "content": "task"}}),
                json!({"type": "assistant", "sessionId": "sess-3", "isSidechain": true,
                    "requestId": "req_x", "timestamp": "2026-11-06T08:00:10.000Z",
                    "message": {"id": "msg_b", "model": "claude-sonnet-5-5",
                        "usage": {"input_tokens": 10, "output_tokens": 4}}})
            ),
        )
        .unwrap();
        let mut out = Vec::new();
        parse_session(&file, 0, &mut out);
        assert_eq!(out.len(), 2);
        let child = out.iter().find(|s| s.native_id == "agent-beef0000").unwrap();
        assert_eq!(child.parent_native_id.as_deref(), Some("sess-3"));
        assert_eq!(child.hours[0].requests, 1);
        let parent = out.iter().find(|s| s.native_id == "sess-3").unwrap();
        // Parent usage NOT polluted by the sidechain (43 usage rows on disk all
        // belong to the child).
        assert_eq!(parent.hours[0].requests, 0);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn tool_result_user_rows_do_not_count_as_turns() {
        let root = temp_tree("toolres");
        let file = root.join("sess-4.jsonl");
        fs::write(
            &file,
            format!(
                "{}\n{}\n",
                json!({"type": "user", "sessionId": "sess-4",
                    "timestamp": "2026-11-06T08:00:00.000Z",
                    "message": {"role": "user",
                        "content": [{"type": "tool_result", "tool_use_id": "t1"}]}}),
                json!({"type": "user", "sessionId": "sess-4",
                    "timestamp": "2026-11-06T08:00:01.000Z",
                    "message": {"role": "user",
                        "content": [{"type": "text", "text": "actual prompt"}]}})
            ),
        )
        .unwrap();
        let mut out = Vec::new();
        parse_session(&file, 0, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].hours[0].turns, 1);
        assert_eq!(out[0].title.as_deref(), Some("actual prompt"));
        fs::remove_dir_all(&root).ok();
    }
}
