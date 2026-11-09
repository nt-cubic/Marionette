//! Conversation composition — what one Grok session's context is made of.
//!
//! Grok writes the exact request material to the session directory:
//! `system_prompt.txt` (rendered system prompt), `tool_definitions.json`
//! (tool schemas) and `chat_history.jsonl` (every message sent to the model,
//! including reasoning items with their encrypted blobs). That makes Grok the
//! one agent whose composition needs no guessing about ingredients — only the
//! usual chars/4 density estimate to turn text into tokens (the same density
//! dsh-context uses; every figure here is an estimate except `measured_prompt`,
//! the agent-reported prompt total of the last turn, which the UI uses as an
//! anchor to scale the shares).
//!
//! Categories mirror the UI rows: system / tool definitions / injected context
//! (`<user_info>`, `<system-reminder>` … inside user messages) / your input /
//! image attachments / replies / reasoning / tool calls / tool results.

use super::facts::json_str;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;

/// ceil(chars / 4) — the flat text density (dsh-context's CHARS_PER_TOKEN).
fn est_tokens(chars: usize) -> u64 {
    (chars as u64).div_ceil(4)
}

/// Flat per-image estimate (providers bill 0.5–1.6k for a typical screenshot).
const IMAGE_TOKENS_EST: u64 = 1000;

/// Per-message overhead (dsh-context's ROLE_OVERHEAD).
const MSG_OVERHEAD: u64 = 4;

/// User-message wrappers that carry machine context, not what the human typed.
const INJECT_PREFIXES: [&str; 8] = [
    "<user_info>",
    "<system-reminder>",
    "<environment_context>",
    "<rules",
    "<skills",
    "<attachments",
    "<system_warning",
    "<command-name>",
];

/// Composition of one conversation (all fields token estimates; the UI scales
/// them to `measured_prompt` when present). Cache-serializable.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct CompositionFacts {
    pub system: u64,
    pub tool_defs: u64,
    pub inject: u64,
    pub user: u64,
    pub attach: u64,
    pub assistant: u64,
    pub reasoning: u64,
    pub tool_calls: u64,
    pub tool_results: u64,
    /// Tokens in context at the end of the conversation (grok signals'
    /// `contextTokensUsed`) — the one measured figure; the UI scales the
    /// estimates to it. None in sessions without the reading.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub measured_prompt: Option<u64>,
}

impl CompositionFacts {
    /// Sum of the estimated parts (the UI's scaling denominator).
    pub fn estimated_total(&self) -> u64 {
        self.system
            .saturating_add(self.tool_defs)
            .saturating_add(self.inject)
            .saturating_add(self.user)
            .saturating_add(self.attach)
            .saturating_add(self.assistant)
            .saturating_add(self.reasoning)
            .saturating_add(self.tool_calls)
            .saturating_add(self.tool_results)
    }
}

/// Estimate one text-carrying JSON value (string, or array of text blocks).
fn text_tokens(v: &serde_json::Value) -> u64 {
    match v {
        serde_json::Value::String(s) => est_tokens(s.len()).saturating_add(MSG_OVERHEAD),
        serde_json::Value::Array(blocks) => {
            let mut sum = MSG_OVERHEAD;
            for block in blocks {
                if let serde_json::Value::String(s) = block {
                    sum = sum.saturating_add(est_tokens(s.len()));
                }
            }
            sum
        }
        _ => 0,
    }
}

/// Reasoning items carry a visible summary plus the encrypted hidden chain.
/// Base64 inflates bytes by 4/3, so len/5 ≈ decoded bytes / 4 ≈ tokens.
fn reasoning_tokens(item: &serde_json::Value) -> u64 {
    let mut sum = MSG_OVERHEAD;
    if let Some(serde_json::Value::String(enc)) = item.get("encrypted_content") {
        sum = sum.saturating_add((enc.len() as u64) / 5);
    }
    if let Some(summary) = item.get("summary").and_then(|s| s.as_array()) {
        for part in summary {
            if let Some(serde_json::Value::String(s)) = part.get("text") {
                sum = sum.saturating_add(est_tokens(s.len()));
            }
        }
    }
    sum
}

/// Fold one chat_history line into the composition.
///
/// `skip_system`: chat_history repeats the system prompt as its first line —
/// when system_prompt.txt was read, counting both would double it.
fn fold_line(line: &str, out: &mut CompositionFacts, skip_system: bool) {
    let Ok(event) = serde_json::from_str::<serde_json::Value>(line) else {
        return;
    };
    match json_str(&event, "type") {
        Some("system") if !skip_system => {
            out.system = out.system.saturating_add(text_tokens(&event["content"]));
        }
        Some("system") => {}
        Some("user") => {
            let blocks = match event.get("content") {
                Some(serde_json::Value::Array(blocks)) => blocks.as_slice(),
                // A bare string is real user input (no wrapper).
                Some(serde_json::Value::String(_)) => {
                    out.user = out.user.saturating_add(text_tokens(&event["content"]));
                    return;
                }
                _ => return,
            };
            for block in blocks {
                match json_str(block, "type") {
                    Some("text") => {
                        let text = json_str(block, "text").unwrap_or("");
                        let inject = INJECT_PREFIXES.iter().any(|p| text.starts_with(p));
                        let tokens = est_tokens(text.len());
                        if inject {
                            out.inject = out.inject.saturating_add(tokens);
                        } else {
                            out.user = out.user.saturating_add(tokens);
                        }
                    }
                    Some("image") => {
                        out.attach = out.attach.saturating_add(IMAGE_TOKENS_EST);
                    }
                    _ => {}
                }
            }
        }
        Some("assistant") => {
            out.assistant = out.assistant.saturating_add(text_tokens(&event["content"]));
            if let Some(calls) = event.get("tool_calls").and_then(|c| c.as_array()) {
                for call in calls {
                    let mut tokens = MSG_OVERHEAD;
                    if let Some(name) = call.get("function").and_then(|f| json_str(f, "name")) {
                        tokens = tokens.saturating_add(est_tokens(name.len()));
                    }
                    if let Some(args) = call
                        .get("function")
                        .and_then(|f| json_str(f, "arguments"))
                    {
                        tokens = tokens.saturating_add(est_tokens(args.len()));
                    }
                    out.tool_calls = out.tool_calls.saturating_add(tokens);
                }
            }
        }
        Some("reasoning") => {
            out.reasoning = out.reasoning.saturating_add(reasoning_tokens(&event));
        }
        Some("tool_result") => {
            out.tool_results = out
                .tool_results
                .saturating_add(text_tokens(&event["content"]));
        }
        _ => {}
    }
}

/// Composition of one grok session directory. `measured_prompt` comes from the
/// caller (grok.rs already walks updates.jsonl for usage). Returns None when
/// none of the three ingredient files exist.
pub(crate) fn grok_session_composition(dir: &Path, measured_prompt: Option<u64>) -> Option<CompositionFacts> {
    let mut out = CompositionFacts {
        measured_prompt,
        ..CompositionFacts::default()
    };
    let mut system_from_file = false;

    if let Ok(text) = fs::read_to_string(dir.join("system_prompt.txt")) {
        out.system = est_tokens(text.len());
        system_from_file = true;
    }
    if let Ok(text) = fs::read_to_string(dir.join("tool_definitions.json")) {
        // Per-tool JSON length (dsh-context's estimateToolsTotal), not the whole
        // file: the array's brackets would otherwise double-count nothing useful.
        if let Ok(defs) = serde_json::from_str::<serde_json::Value>(&text) {
            if let Some(list) = defs.as_array() {
                for def in list {
                    let encoded = def.to_string();
                    out.tool_defs = out
                        .tool_defs
                        .saturating_add(est_tokens(encoded.len()).saturating_add(MSG_OVERHEAD));
                }
            }
        }
    }

    let history = dir.join("chat_history.jsonl");
    let file = fs::File::open(&history).ok()?;
    for line in BufReader::new(file).lines() {
        let Ok(line) = line else { break };
        fold_line(&line, &mut out, system_from_file);
    }

    if out.estimated_total() == 0 {
        return None;
    }
    Some(out)
}

/// Tokens in context at the end of the conversation — grok `signals.json`
/// `contextTokensUsed`. This is the measured anchor the UI scales estimates to
/// (the last turn's `inputTokens` is a session-lifetime sum, NOT the context
/// size, and must not be used as the anchor).
pub(crate) fn signals_context_used(dir: &Path) -> Option<u64> {
    let text = fs::read_to_string(dir.join("signals.json")).ok()?;
    let signals = serde_json::from_str::<serde_json::Value>(&text).ok()?;
    signals
        .get("contextTokensUsed")
        .and_then(|v| v.as_u64())
        .filter(|v| *v > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("marionette-comp-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn categories_split_injections_from_user_text() {
        let dir = tmp_dir("cats");
        fs::write(dir.join("system_prompt.txt"), "a".repeat(400)).unwrap();
        fs::write(
            dir.join("tool_definitions.json"),
            r#"[{"type":"function","function":{"name":"read","description":"dddd"}}]"#,
        )
        .unwrap();
        let history = [
            r#"{"type":"system","content":""}"#.to_string(),
            format!(
                r#"{{"type":"user","content":[{{"type":"text","text":"{}"}}]}}"#,
                "<user_info>\\nwindows\\n</user_info>"
            ),
            format!(r#"{{"type":"user","content":[{{"type":"text","text":"{}"}},{{"type":"image","url":"x"}}]}}"#, "把按钮改成胶片格"),
            format!(r#"{{"type":"assistant","content":"{}","tool_calls":[{{"function":{{"name":"read","arguments":"{{\"path\":\"a.ts\"}}"}}}}]}}"#, "好的，我看下布局"),
            format!(r#"{{"type":"reasoning","summary":[{{"type":"summary_text","text":"{}"}}],"encrypted_content":"{}"}}"#, "先查样式", "Z".repeat(500)),
            format!(r#"{{"type":"tool_result","content":"{}"}}"#, "width: 28px"),
        ];
        fs::write(dir.join("chat_history.jsonl"), history.join("\n") + "\n").unwrap();
        fs::write(dir.join("updates.jsonl"), "").unwrap();

        let comp = grok_session_composition(&dir, Some(12345)).unwrap();
        // system_prompt.txt wins; the history's duplicate system line is skipped.
        assert_eq!(comp.system, 100); // 400 chars / 4
        assert!(comp.tool_defs > 0);
        assert_eq!(comp.inject, est_tokens("<user_info>\nwindows\n</user_info>".len()));
        assert_eq!(comp.user, est_tokens("把按钮改成胶片格".len()));
        assert_eq!(comp.attach, IMAGE_TOKENS_EST);
        assert!(comp.assistant > 0 && comp.tool_calls > 0);
        assert_eq!(comp.reasoning, 4 + 100 + est_tokens("先查样式".len())); // overhead + 500/5 + summary
        assert!(comp.tool_results > 0);
        assert_eq!(comp.measured_prompt, Some(12345));
        assert!(comp.estimated_total() > 0);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn bare_string_user_message_counts_as_user() {
        let dir = tmp_dir("bare");
        fs::write(dir.join("chat_history.jsonl"), "{\"type\":\"user\",\"content\":\"继续\"}\n").unwrap();
        let comp = grok_session_composition(&dir, None).unwrap();
        // Bare-string user content: est("继续") + per-message overhead.
        assert_eq!(comp.user, est_tokens("继续".len()) + MSG_OVERHEAD);
        assert_eq!(comp.inject, 0);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn anchor_is_the_final_context_reading_not_the_turn_sum() {
        let dir = tmp_dir("anchor");
        fs::write(
            dir.join("signals.json"),
            r#"{"contextTokensUsed":130386,"contextWindowTokens":256000}"#,
        )
        .unwrap();
        assert_eq!(signals_context_used(&dir), Some(130386));
        // Zero / missing readings are None, never an anchor of 0.
        fs::write(dir.join("signals.json"), r#"{"contextTokensUsed":0}"#).unwrap();
        assert_eq!(signals_context_used(&dir), None);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn no_ingredients_is_none() {
        let dir = tmp_dir("empty");
        assert!(grok_session_composition(&dir, None).is_none());
        let _ = fs::remove_dir_all(dir);
    }

    /// Real machine: composition of the newest grok session under `~/.grok`.
    #[test]
    #[ignore = "reads the user's real ~/.grok session store"]
    fn real_grok_composition_smoke() {
        let root = super::super::grok::discover_root().expect("grok sessions root");
        let mut newest: Option<(std::time::SystemTime, std::path::PathBuf)> = None;
        for encoded in fs::read_dir(&root).into_iter().flatten().flatten() {
            for session in fs::read_dir(encoded.path()).into_iter().flatten().flatten() {
                let path = session.path();
                if path.join("chat_history.jsonl").is_file() {
                    let mtime = session
                        .metadata()
                        .and_then(|m| m.modified())
                        .unwrap_or(std::time::UNIX_EPOCH);
                    if newest.as_ref().map(|(t, _)| mtime > *t).unwrap_or(true) {
                        newest = Some((mtime, path));
                    }
                }
            }
        }
        let (_, dir) = newest.expect("at least one grok session with chat history");
        let comp =
            grok_session_composition(&dir, signals_context_used(&dir)).expect("composition");
        println!(
            "grok composition of {}:\n  system={} tool_defs={} inject={} user={} attach={} \
             assistant={} reasoning={} tool_calls={} tool_results={} est_total={} measured={:?}",
            dir.display(),
            comp.system,
            comp.tool_defs,
            comp.inject,
            comp.user,
            comp.attach,
            comp.assistant,
            comp.reasoning,
            comp.tool_calls,
            comp.tool_results,
            comp.estimated_total(),
            comp.measured_prompt,
        );
    }
}
