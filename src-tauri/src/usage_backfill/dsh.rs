//! deepseek (DSH) source: `~/.dsh/sessions/<encoded-cwd>/<uuid>/session.v4.jsonl.zstd`
//! (respects `DSH_SESSION_ROOT`, then `DSH_HOME`, then `~/.dsh` — the same
//! chain Marionette's spawn env pins, see AGENTS.md "DSH 接入").
//!
//! Verified against this machine's store (2026-11-09):
//! - `native_id` = the session directory's uuid. Cross-checked with
//!   Marionette's own `nativeSessionIds`: every uuid recorded for the two
//!   local deepseek sessions (`7fc4ec84-…`, `5b72b1d0-…`, `4d898533-…`,
//!   `a52d3ed3-…`, `0c10cd80-…`, `725f89b2-…`, `4f107c89-…`, `4d3ad779-…`)
//!   exists as a session directory — the dir name IS the ACP sessionId.
//!   (Marionette's list also carries one `ses_*` id from an older adapter
//!   generation; it has no directory and is simply never matched.)
//! - The `.zstd` payload is one or more concatenated zstd frames (append-style
//!   archives); ruzstd's `StreamingDecoder` decodes exactly ONE frame (upstream
//!   caveat), so [`decode_all_frames`] chains them: decode a frame over a
//!   cursor of the remaining bytes, continue at the next frame magic. Each
//!   line: `{type, seq, time (epoch ms), data}`.
//! - `assistant/message` rows carry `data.usage` per API request:
//!   `{inputTokens (FRESH — local proof: input + cacheRead + cacheWrite +
//!     output == totalTokens, e.g. 11915 + 384 + 0 + 595 == 12894),
//!    cacheReadTokens, cacheWriteTokens, outputTokens, reasoningTokens?}`.
//!   `reasoningTokens` was absent from every local session (deepseek-flash);
//!   when present, a `totalTokens` that only adds up WITH the reasoning field
//!   means reasoning sits outside `outputTokens`, so it is added back to the
//!   ledger's `output` (everything generated) — mirrors the live ledger's
//!   `generatedOutputTokens` heuristic.
//! - `user/message` rows: `data.source.kind` — only `""`/null/`"user"` are
//!   user turns; the rest (`user-approval`, `agent-instructions`,
//!   `runtime-context`, `skill-catalog`, …) are injections.
//! - `request/header` rows set the current model
//!   (`data.config.provider` + `data.config.model`, joined `provider::model`
//!   — the same id shape dsh-acp's live catalog uses per AGENTS.md).
//! - `request/context` rows carry `data.contextWindow`.
//! - `compaction/summary` / `compaction/prune` are compactions (none locally).
//! - Titles: `session/title` rows (`data.title`).
//! - cwd is NOT recoverable (the directory name is a lossy dash encoding);
//!   left `None`.

use super::cache::{stamp_files, Cache};
use super::facts::{json_i64, json_str, json_u64, SessionCollector, SessionFacts};
use super::SourceOutcome;
use serde_json::Value;
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

pub(crate) const AGENT: &str = "deepseek";

pub(crate) fn discover_root() -> Option<PathBuf> {
    if let Some(root) = crate::parsers::util::env_dir("DSH_SESSION_ROOT") {
        return Some(root);
    }
    let home = crate::parsers::util::env_dir("DSH_HOME")
        .or_else(|| crate::parsers::util::home_join(".dsh"))?;
    Some(home.join("sessions"))
}

/// Decode every zstd frame in the archive. ruzstd's `StreamingDecoder` decodes
/// exactly ONE frame (upstream caveat), so chained frames need an explicit
/// loop: decode a frame over a cursor of the remaining bytes, look at how many
/// bytes it consumed, continue at the next frame magic. Skippable frames and
/// trailing junk end the walk. DSH archives are hundreds of KB compressed
/// locally, so holding the compressed bytes (and decoded text) in memory is
/// fine; the JSONL itself is then processed line by line.
pub(crate) fn decode_all_frames(compressed: &[u8]) -> Option<Vec<u8>> {
    const MAGIC: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];
    let mut out = Vec::new();
    let mut rest = compressed;
    loop {
        while matches!(rest.first(), Some(b'\n') | Some(b'\r') | Some(b' ')) {
            rest = &rest[1..];
        }
        if rest.len() < 4 || rest[..4] != MAGIC {
            break;
        }
        let mut cursor = std::io::Cursor::new(rest);
        let consumed = {
            let mut decoder = ruzstd::StreamingDecoder::new(&mut cursor).ok()?;
            decoder.read_to_end(&mut out).ok()?;
            cursor.position() as usize
        };
        if consumed == 0 {
            break; // defensive: no progress
        }
        rest = &rest[consumed.min(rest.len())..];
    }
    Some(out)
}

/// Decode the zstd archive (possibly several concatenated frames) and fold it.
/// Split from [`parse_session_dir`] so the multi-frame loop is unit-testable.
pub(crate) fn fold_decoded(
    compressed: &[u8],
    native_id: &str,
    keep_from_ms: i64,
) -> Option<SessionFacts> {
    let text = decode_all_frames(compressed)?;
    let mut collector = SessionCollector::new(native_id.to_string());
    let mut model: Option<String> = None;
    let mut ctx_window: Option<u64> = None;
    let mut first_user_text: Option<String> = None;
    for line in BufReader::new(text.as_slice()).lines() {
        let Ok(line) = line else { break };
        // Prefilter on the few event types we consume.
        if !line.contains("assistant/message")
            && !line.contains("user/message")
            && !line.contains("request/header")
            && !line.contains("request/context")
            && !line.contains("compaction/")
            && !line.contains("session/title")
        {
            continue;
        }
        let Ok(event) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let data = event.get("data").unwrap_or(&Value::Null);
        let t_ms = json_i64(&event, "time").unwrap_or(0);
        match json_str(&event, "type") {
            Some("assistant/message") => {
                let Some(usage) = data.get("usage") else { continue };
                let input = json_u64(usage, "inputTokens").unwrap_or(0);
                let cached = json_u64(usage, "cacheReadTokens").unwrap_or(0);
                let cache_creation = json_u64(usage, "cacheWriteTokens").unwrap_or(0);
                let output = json_u64(usage, "outputTokens").unwrap_or(0);
                if input == 0 && output == 0 && cached == 0 && cache_creation == 0 {
                    continue;
                }
                let reasoning = json_u64(usage, "reasoningTokens").unwrap_or(0);
                // Is reasoning inside outputTokens? Prefer the total's
                // arithmetic; without a total, DeepSeek's API folds reasoning
                // into completion tokens, so keep output as-is.
                let output_all = if reasoning > 0 {
                    let total = json_u64(usage, "totalTokens");
                    match total {
                        Some(total) => {
                            let with = input + cached + cache_creation + output + reasoning;
                            let without = input + cached + cache_creation + output;
                            if total.abs_diff(with) <= total.abs_diff(without) {
                                output + reasoning
                            } else {
                                output
                            }
                        }
                        None => output,
                    }
                } else {
                    output
                };
                collector.push_request(super::facts::RequestRecord {
                    t_ms,
                    model: model.clone(),
                    input,
                    output: output_all,
                    cached,
                    cache_creation,
                    reasoning,
                    reported_usd: None,
                    ctx_tokens: Some(input + cached + cache_creation),
                    requests: 1,
                });
            }
            Some("user/message") => {
                let kind = data
                    .get("source")
                    .and_then(|s| json_str(s, "kind"))
                    .unwrap_or("");
                if !kind.is_empty() && kind != "user" {
                    continue; // injection (approval/instructions/context/skill)
                }
                if first_user_text.is_none() {
                    if let Some(text) = data
                        .get("content")
                        .and_then(|c| c.as_array())
                        .and_then(|items| {
                            items
                                .iter()
                                .find(|b| json_str(b, "type") == Some("text"))
                                .and_then(|b| json_str(b, "text"))
                        })
                    {
                        first_user_text = Some(text.chars().take(80).collect());
                    }
                }
                collector.push_turn(t_ms);
            }
            Some("request/header") => {
                let config = data.get("header").and_then(|h| h.get("config"));
                let model_name = config.and_then(|c| json_str(c, "model")).filter(|m| !m.is_empty());
                let provider = config
                    .and_then(|c| json_str(c, "provider"))
                    .filter(|p| !p.is_empty());
                model = match (provider, model_name) {
                    (Some(p), Some(m)) => Some(format!("{p}::{m}")),
                    (None, Some(m)) => Some(m.to_string()),
                    (_, None) => model.take(),
                };
            }
            Some("request/context") => {
                if let Some(window) = json_u64(data, "contextWindow").filter(|w| *w > 0) {
                    ctx_window = Some(window);
                }
            }
            Some(t) if t.starts_with("compaction/") => {
                collector.push_compaction(t_ms);
            }
            Some("session/title") => {
                if collector.title.is_none() {
                    if let Some(title) = json_str(data, "title").filter(|t| !t.is_empty()) {
                        collector.title = Some(title.to_string());
                    }
                }
            }
            _ => {}
        }
    }
    if collector.title.is_none() {
        collector.title = first_user_text.filter(|t| !t.is_empty());
    }
    collector.ctx_window = ctx_window;
    collector.finish(AGENT, keep_from_ms)
}

fn parse_session_dir(dir: &Path, keep_from_ms: i64) -> Option<SessionFacts> {
    let path = dir.join("session.v4.jsonl.zstd");
    let native_id = dir.file_name()?.to_str()?.to_string();
    let compressed = fs::read(&path).ok()?;
    fold_decoded(&compressed, &native_id, keep_from_ms)
}

pub(crate) fn scan(root: &Path, cache: &mut Cache, force: bool, keep_from_ms: i64) -> SourceOutcome {
    let mut outcome = SourceOutcome::new(AGENT, Some(root.to_string_lossy().into_owned()));
    let mut live_keys: Vec<String> = Vec::new();
    let mut sessions: Vec<SessionFacts> = Vec::new();
    if !root.is_dir() {
        outcome.found = false;
        outcome.error = Some("dsh sessions root not found".to_string());
        return outcome;
    }
    outcome.found = true;
    let mut dirs: Vec<PathBuf> = Vec::new();
    for encoded in list_dirs(root) {
        for session in list_dirs(&encoded) {
            if session.join("session.v4.jsonl.zstd").is_file() {
                dirs.push(session);
            }
        }
    }
    dirs.sort();
    outcome.files = dirs.len() as u32;
    for dir in dirs {
        let zstd_path = dir.join("session.v4.jsonl.zstd");
        let fp = stamp_files(&zstd_path, &[]);
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

fn list_dirs(dir: &Path) -> Vec<PathBuf> {
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
            "marionette-backfill-dsh-{name}-{}-{:x}",
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

    fn v4_lines() -> String {
        // Real shapes from ~/.dsh (2026-10-23 session 7fc4ec84).
        format!(
            "{}\n{}\n{}\n{}\n{}\n{}\n{}\n",
            json!({"type": "session/title", "seq": 1, "time": 1792743240000i64,
                   "data": {"title": "GLM broke ZCode", "source": {"kind": "llm"}}}),
            json!({"type": "user/message", "seq": 13, "time": 1792743239952i64,
                   "data": {"content": [{"type": "text",
                       "text": "[Marionette — prior conversation in this dialog] …"}],
                       "source": {"kind": "user"}, "role": "user"}}),
            json!({"type": "user/message", "seq": 12, "time": 1792743239000i64,
                   "data": {"content": [{"type": "text",
                       "text": "The approval policy changed from \"ask\" to \"never\"."}],
                       "source": {"kind": "user-approval"}}}),
            json!({"type": "request/header", "seq": 18, "time": 1792743239954i64,
                   "data": {"header": {"config": {"provider": "deepseek-official",
                        "model": "deepseek-flash", "reasoningEffort": "max"}}}}),
            json!({"type": "request/context", "seq": 19, "time": 1792743239955i64,
                   "data": {"provider": "deepseek-official", "model": "deepseek-flash",
                            "contextWindow": 1000000}}),
            json!({"type": "assistant/message", "seq": 25, "time": 1792743244187i64,
                   "data": {"turn": 1, "step": 1, "usage": {
                        "inputTokens": 11915, "outputTokens": 595,
                        "cacheReadTokens": 384, "cacheWriteTokens": 0,
                        "totalTokens": 12894}}}),
            json!({"type": "assistant/message", "seq": 30, "time": 1792743250000i64,
                   "data": {"turn": 1, "step": 2, "usage": {
                        "inputTokens": 3582, "outputTokens": 578,
                        "cacheReadTokens": 12800, "cacheWriteTokens": 100,
                        "reasoningTokens": 300, "totalTokens": 17360}}})
        )
    }

    #[test]
    fn folds_usage_turns_model_and_ctx() {
        let compressed = zstd_raw::encode_frames(v4_lines().as_bytes(), 10_000);
        let facts = fold_decoded(&compressed, "7fc4ec84", 0).unwrap();
        assert_eq!(facts.native_id, "7fc4ec84");
        assert_eq!(facts.title.as_deref(), Some("GLM broke ZCode"));
        assert_eq!(facts.ctx_window, Some(1_000_000));
        // Only the kind:"user" row is a turn; the approval notice is injected.
        assert_eq!(facts.hours[0].turns, 1);
        assert_eq!(facts.hours[0].requests, 2);
        // Fresh input kept as-is; buckets 1:1.
        assert_eq!(facts.hours[0].input, 11915 + 3582);
        assert_eq!(facts.hours[0].cached, 384 + 12800);
        assert_eq!(facts.hours[0].cache_creation, 100);
        // Row 2 has reasoning and only adds up WITH it (3582+12800+100+578+300
        // == 17360), so output gains the reasoning leg.
        assert_eq!(facts.hours[0].output, 595 + 578 + 300);
        assert_eq!(facts.hours[0].reasoning, 300);
        assert_eq!(facts.hours[0].model.as_deref(), Some("deepseek-official::deepseek-flash"));
        assert_eq!(facts.ctx_series.len(), 2);
        assert_eq!(facts.ctx_series[0][1], 11915 + 384);
    }

    #[test]
    fn multi_frame_zstd_stream_decodes_to_end() {
        // Build a real multi-frame archive: one raw-block frame per 200-byte
        // chunk of the decoded payload, concatenated (the same shape as DSH's
        // append-style archives).
        let payload = v4_lines();
        let compressed = zstd_raw::encode_frames(payload.as_bytes(), 200);
        assert!(compressed.len() > payload.len()); // many frames indeed
        let decoder_text = decompress_all(&compressed);
        assert_eq!(decoder_text, payload);

        // And the real on-disk path: parse_session_dir on a fabricated tree.
        let root = temp_tree("frames");
        let dir = root
            .join("--D-Projects-Marionette--")
            .join("7fc4ec84-7be6-499c-b8de-842ec3d8ff80");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("session.v4.jsonl.zstd"), &compressed).unwrap();
        let facts = parse_session_dir(&dir, 0).unwrap();
        assert_eq!(facts.hours[0].requests, 2);
        fs::remove_dir_all(&root).ok();
    }

    /// Minimal zstd frame writer for tests: emits one frame per chunk, made
    /// only of RAW blocks (uncompressed zstd frames are legal and trivial to
    /// emit: magic + single-segment header + 1-byte content size + raw block
    /// headers). Good enough to exercise the decoder's frame-chaining path.
    mod zstd_raw {
        pub fn encode_frames(data: &[u8], frame_chunk_size: usize) -> Vec<u8> {
            let mut out = Vec::new();
            for chunk in data.chunks(frame_chunk_size) {
                out.extend_from_slice(&frame_header(chunk.len()));
                let mut rest = chunk;
                while !rest.is_empty() {
                    let take = rest.len().min(65_521); // raw block size limit
                    let (block, remaining) = rest.split_at(take);
                    rest = remaining;
                    let last = u32::from(rest.is_empty());
                    let header: u32 = last | (BlockType::Raw as u32) << 1 | (block.len() as u32) << 3;
                    out.push((header & 0xff) as u8);
                    out.push((header >> 8) as u8);
                    out.push((header >> 16) as u8);
                    out.extend_from_slice(block);
                }
            }
            out
        }

        enum BlockType {
            Raw = 0,
        }

        fn frame_header(content: usize) -> Vec<u8> {
            let mut out = vec![0x28, 0xb5, 0x2f, 0xfd];
            // descriptor 0x20: single_segment=1 (window = content size, no
            // window descriptor byte), FCS flag=0 → 1-byte content size,
            // no checksum, no dictionary.
            out.push(0x20);
            out.push(content as u8); // chunk sizes stay < 256
            out
        }
    }

    fn decompress_all(bytes: &[u8]) -> String {
        String::from_utf8(decode_all_frames(bytes).unwrap()).unwrap()
    }

    #[test]
    fn reasoning_inside_output_when_totals_say_so() {
        let lines = format!(
            "{}\n{}\n",
            json!({"type": "request/header", "time": 1000,
                   "data": {"header": {"config": {"provider": "p", "model": "m"}}}}),
            json!({"type": "assistant/message", "time": 2000,
                   "data": {"usage": {"inputTokens": 100, "outputTokens": 500,
                        "cacheReadTokens": 0, "cacheWriteTokens": 0,
                        "reasoningTokens": 400, "totalTokens": 600}}})
        );
        let compressed = zstd_raw::encode_frames(lines.as_bytes(), 10_000);
        let facts = fold_decoded(&compressed, "x", 0).unwrap();
        // 100 + 500 + 400 = 1000 != 600; 100 + 500 = 600 → reasoning is
        // already inside output.
        assert_eq!(facts.hours[0].output, 500);
        assert_eq!(facts.hours[0].reasoning, 400);
    }
}
