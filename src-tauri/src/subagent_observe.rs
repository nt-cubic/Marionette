//! Phase 0 / P0-1: five-hop observability counters for delegated workers.
//!
//! This module is **measurement only**: it never changes how events are parsed,
//! stored, replayed or rendered. The pipeline a worker's events travel looks
//! like this:
//!
//! 1. agent emits (the agent's own session store; external, never invented here)
//! 2. `acp.rs` parses stdout into `session/update` events (counted by kind)
//! 3. `chief::tee` enqueues + writes them (bounded queue, drops are counted)
//! 4. `storage` appends the transcript (line count + kind distribution)
//! 5. the frontend replays the tee into the rail (`backfillChiefTee`)
//!
//! Each hop writes **one** structured line to the developer diary with
//! `source=subagent-observe` and a single-line JSON `detail`; the session id is
//! the join key. `scripts/report-subagent-pipeline.mts` reads those lines (plus
//! the tee/transcript files) and prints a "same session, five hops" table so a
//! hop whose count collapses is visible instead of guessed at.
//!
//! Red lines:
//! - Hot paths only bump relaxed atomics; the summary log is written once per
//!   turn/close, never per event.
//! - Nothing here blocks the ACP read thread (`debug_log::append` is a bounded
//!   non-blocking channel for `info`).
//! - No prompt/body/tool-output content is logged; counts and ids only.

use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// Diary source for every line this module emits.
pub const SOURCE: &str = "subagent-observe";

/// Fixed kind slots keep per-session memory bounded: an adversarial agent
/// cannot grow the counter array by emitting novel `sessionUpdate` strings.
const ACP_KIND_NAMES: [&str; 12] = [
    "tool_call",
    "tool_call_update",
    "agent_message_chunk",
    "agent_thought_chunk",
    "user_message_chunk",
    "plan",
    "available_commands_update",
    "current_mode_update",
    "config_option_update",
    "session_info_update",
    "compaction_update",
    "other",
];
const ACP_KIND_OTHER: usize = ACP_KIND_NAMES.len() - 1;

fn classify_acp_kind(kind: &str) -> usize {
    match kind {
        "tool_call" => 0,
        "tool_call_update" => 1,
        "agent_message_chunk" => 2,
        "agent_thought_chunk" => 3,
        "user_message_chunk" => 4,
        "plan" => 5,
        "available_commands_update" => 6,
        "current_mode_update" => 7,
        "config_option_update" => 8,
        "session_info_update" => 9,
        "compaction_update" => 10,
        _ => ACP_KIND_OTHER,
    }
}

/// Per-session ACP-parse counters. Cumulative for the app's lifetime (a worker
/// process can restart on retry; the totals must not reset with it).
#[derive(Debug)]
pub struct AcpCounters {
    total: AtomicU64,
    kinds: [AtomicU64; ACP_KIND_NAMES.len()],
}

impl AcpCounters {
    fn new() -> Self {
        Self {
            total: AtomicU64::new(0),
            kinds: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }

    /// Hot path: one relaxed increment. Called once per parsed `session/update`.
    pub fn note(&self, kind: &str) {
        self.total.fetch_add(1, Ordering::Relaxed);
        self.kinds[classify_acp_kind(kind)].fetch_add(1, Ordering::Relaxed);
    }

    fn snapshot(&self) -> (u64, Vec<(&'static str, u64)>) {
        let total = self.total.load(Ordering::Relaxed);
        let kinds = ACP_KIND_NAMES
            .iter()
            .enumerate()
            .map(|(index, name)| (*name, self.kinds[index].load(Ordering::Relaxed)))
            .filter(|(_, count)| *count > 0)
            .collect();
        (total, kinds)
    }

    /// Emit one cumulative summary line. `reason` says what boundary triggered
    /// it (`turn_complete` / `stdout_eof`) so a torn-off run is obvious.
    pub fn flush(&self, session_id: &str, reason: &str) {
        let (total, kinds) = self.snapshot();
        if total == 0 {
            return;
        }
        let mut map = Map::new();
        for (name, count) in kinds {
            map.insert(name.to_string(), json!(count));
        }
        let detail = json!({
            "stage": 2,
            "hop": "acp_parse",
            "reason": reason,
            "total": total,
            "kinds": Value::Object(map),
        });
        crate::debug_log::append(
            SOURCE,
            "info",
            session_id,
            "stage=2 hop=acp_parse",
            Some(&detail.to_string()),
        );
    }
}

/// Upper bound on remembered sessions. Entries are only pruned when they have
/// no live owner (`Arc` strong count == 1); a pruned session that later restarts
/// simply starts a fresh cumulative count, which the report tolerates.
const ACP_REGISTRY_CAP: usize = 512;

static ACP_COUNTERS: OnceLock<Mutex<HashMap<String, Arc<AcpCounters>>>> = OnceLock::new();

fn acp_registry() -> &'static Mutex<HashMap<String, Arc<AcpCounters>>> {
    ACP_COUNTERS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Counters owned by one ACP process (shared across restarts of the same
/// logical session). Called once at spawn; never on the event hot path.
pub fn acp_counters(session_id: &str) -> Arc<AcpCounters> {
    let mut map = match acp_registry().lock() {
        Ok(map) => map,
        // Poisoned registry: measurement must never take the app down.
        Err(poisoned) => poisoned.into_inner(),
    };
    if let Some(existing) = map.get(session_id) {
        return Arc::clone(existing);
    }
    if map.len() >= ACP_REGISTRY_CAP {
        // Keep only counters still held by a live process; anything else is
        // reconstructible on demand.
        map.retain(|_, counters| Arc::strong_count(counters) > 1);
    }
    let counters = Arc::new(AcpCounters::new());
    map.insert(session_id.to_string(), Arc::clone(&counters));
    counters
}

/// Test seam: drop every remembered counter. Never used by product code.
#[cfg(test)]
pub fn reset_acp_counters_for_tests() {
    if let Ok(mut map) = acp_registry().lock() {
        map.clear();
    }
}

/// Stage 3 summary: tee enqueued/written/dropped plus the file's line count.
pub fn log_tee_summary(
    session_id: &str,
    enqueued: u64,
    written: u64,
    dropped: u64,
    file_lines: u64,
) {
    let detail = json!({
        "stage": 3,
        "hop": "tee",
        "enqueued": enqueued,
        "written": written,
        "dropped": dropped,
        "fileLines": file_lines,
    });
    crate::debug_log::append(
        SOURCE,
        "info",
        session_id,
        "stage=3 hop=tee",
        Some(&detail.to_string()),
    );
}

/// Stage 3 marker at register time: how many lines the tee file already had
/// (a resumed worker continues from there).
pub fn log_tee_registered(session_id: &str, seed_lines: u64) {
    let detail = json!({
        "stage": 3,
        "hop": "tee_register",
        "seedLines": seed_lines,
    });
    crate::debug_log::append(
        SOURCE,
        "info",
        session_id,
        "stage=3 hop=tee_register",
        Some(&detail.to_string()),
    );
}

/// Derive the Marionette session id from a transcript path
/// (`.../transcripts/<sessionId>.jsonl`).
fn session_id_from_path(path: &Path) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default()
        .to_string()
}

/// Stage 4 summary: one line per transcript write with the batch's kind
/// distribution. The report additionally scans the transcript file for the
/// authoritative total; these lines prove the transcript hop actually ran.
pub fn log_transcript_write(path: &Path, mode: &str, events: &[Value]) {
    let mut kinds: BTreeMap<&str, u64> = BTreeMap::new();
    for event in events {
        let kind = event
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        *kinds.entry(kind).or_insert(0) += 1;
    }
    let mut map = Map::new();
    for (name, count) in kinds {
        map.insert(name.to_string(), json!(count));
    }
    let session_id = session_id_from_path(path);
    let detail = json!({
        "stage": 4,
        "hop": "transcript",
        "mode": mode,
        "batch": events.len(),
        "kinds": Value::Object(map),
    });
    crate::debug_log::append(
        SOURCE,
        "info",
        &session_id,
        "stage=4 hop=transcript",
        Some(&detail.to_string()),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_known_and_unknown_kinds_into_bounded_slots() {
        assert_eq!(classify_acp_kind("tool_call"), 0);
        assert_eq!(classify_acp_kind("tool_call_update"), 1);
        assert_eq!(classify_acp_kind("agent_message_chunk"), 2);
        assert_eq!(classify_acp_kind("agent_thought_chunk"), 3);
        assert_eq!(classify_acp_kind("some_future_kind"), ACP_KIND_OTHER);
        assert_eq!(ACP_KIND_NAMES.len(), 12);
    }

    #[test]
    fn counters_snapshot_only_reports_nonzero_kinds() {
        let counters = AcpCounters::new();
        counters.note("tool_call");
        counters.note("tool_call");
        counters.note("tool_call_update");
        counters.note("mystery");
        let (total, kinds) = counters.snapshot();
        assert_eq!(total, 4);
        let map: HashMap<&str, u64> = kinds.into_iter().collect();
        assert_eq!(map.get("tool_call"), Some(&2));
        assert_eq!(map.get("tool_call_update"), Some(&1));
        assert_eq!(map.get("other"), Some(&1));
        assert!(!map.contains_key("plan"));
    }

    #[test]
    fn registry_is_cumulative_for_the_same_session() {
        reset_acp_counters_for_tests();
        let first = acp_counters("session_observe_registry");
        first.note("tool_call");
        let second = acp_counters("session_observe_registry");
        assert!(Arc::ptr_eq(&first, &second), "same session, same counters");
        assert_eq!(first.snapshot().0, 1);
        reset_acp_counters_for_tests();
    }

    #[test]
    fn transcript_kinds_are_counted_from_event_type() {
        // The logger itself is a silent no-op under `cfg(test)` (the dev-log sink
        // is disabled in tests), so reproduce the same fold it performs.
        let events = vec![
            json!({"type": "assistant_message"}),
            json!({"type": "tool_call"}),
            json!({"type": "tool_call"}),
            json!({"type": "thought"}),
            json!({}),
        ];
        let mut kinds: BTreeMap<&str, u64> = BTreeMap::new();
        for event in &events {
            let kind = event
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            *kinds.entry(kind).or_insert(0) += 1;
        }
        assert_eq!(kinds.get("tool_call"), Some(&2));
        assert_eq!(kinds.get("assistant_message"), Some(&1));
        assert_eq!(kinds.get("thought"), Some(&1));
        assert_eq!(kinds.get("unknown"), Some(&1));
    }

    #[test]
    fn session_id_is_derived_from_transcript_file_stem() {
        assert_eq!(
            session_id_from_path(Path::new(r"C:\x\transcripts\session_123.jsonl")),
            "session_123"
        );
    }
}
