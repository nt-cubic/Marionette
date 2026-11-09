//! usage_backfill — read each agent CLI's own on-disk session store and fold it
//! into per-session, per-hour usage facts for the Token usage page ("history
//! backfill"; research: docs/research-context-transparency.md §3).
//!
//! Contract with the frontend (`src/lib/usageBackfill.ts` mirrors these types,
//! serde camelCase):
//! - Token buckets use the ledger's semantics: `input` = FRESH (uncached) input,
//!   `cached` = cache read, `cache_creation` = cache write, `output` = everything
//!   generated INCLUDING reasoning, `reasoning` = the part of `output` that was
//!   reasoning (0 when the store does not say).
//! - Time is bucketed by UTC epoch hour (`floor(t_ms / 3_600_000)`); the frontend
//!   turns that into local days / hours and DeepSeek's peak window, so this side
//!   never deals with time zones.
//! - `native_id` must be the exact string the agent's ACP adapter reports as
//!   `sessionId` (Marionette stores it in `Session.nativeSessionIds`), so the
//!   page can drop sessions the live ledger already counted.
//! - Nothing is invented: a store that does not record a figure leaves it 0 /
//!   None / empty. Non-fatal problems go into `BackfillSource.error`.
//!
//! Red lines: never write into an agent's own directories (read-only, SQLite
//! opened read-only), never hold the storage lock, stream files line by line
//! (Claude logs reach 80 MB, Codex rollouts 190 MB), and run off the main
//! thread. The only file this module ever writes is
//! `~/.marionette/usage/backfill-cache.json` (via `storage::write_atomic`).

mod cache;
mod claude;
mod codex;
mod composition;
mod dsh;
mod facts;
mod grok;
mod opencode;
mod zcode;

use facts::SessionFacts;
use serde::Serialize;
use std::panic::AssertUnwindSafe;
use std::time::Instant;

/// Hours kept per session (contract: "only the last 190 days of hour rows").
const KEEP_DAYS: i64 = 190;
/// Session cap across all sources, newest `last_at` first.
const SESSION_CAP: usize = 1500;

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackfillUsageHour {
    /// Hours since the Unix epoch, UTC: `floor(t_ms / 3_600_000)`.
    pub hour: i64,
    pub model: Option<String>,
    /// User prompt turns that started in this hour.
    pub turns: u32,
    /// Model API requests (usage records) in this hour.
    pub requests: u32,
    pub input: u64,
    pub output: u64,
    pub cached: u64,
    pub cache_creation: u64,
    pub reasoning: u64,
    /// The agent's own cost for this hour's requests (USD), when the store has one.
    pub reported_usd: Option<f64>,
}

/// Conversation composition (what the context is made of). Grok-only for now:
/// its session directory stores the exact request material (system prompt,
/// tool schemas, every message), so every figure but `measured_prompt` is a
/// chars/4 estimate over real ingredients, scaled by the UI to the anchor.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackfillComposition {
    pub system: u64,
    pub tool_defs: u64,
    pub inject: u64,
    pub user: u64,
    pub attach: u64,
    pub assistant: u64,
    pub reasoning: u64,
    pub tool_calls: u64,
    pub tool_results: u64,
    /// Agent-reported prompt total of the last turn (measured anchor); null in
    /// sessions without usage.
    pub measured_prompt: Option<u64>,
}

impl From<composition::CompositionFacts> for BackfillComposition {
    fn from(f: composition::CompositionFacts) -> Self {
        BackfillComposition {
            system: f.system,
            tool_defs: f.tool_defs,
            inject: f.inject,
            user: f.user,
            attach: f.attach,
            assistant: f.assistant,
            reasoning: f.reasoning,
            tool_calls: f.tool_calls,
            tool_results: f.tool_results,
            measured_prompt: f.measured_prompt,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackfillSession {
    /// Marionette agent id: grok-build | claude-code | codex | deepseek | opencode | zcode.
    pub agent: String,
    pub native_id: String,
    pub parent_native_id: Option<String>,
    pub title: Option<String>,
    pub cwd: Option<String>,
    pub first_at: i64,
    pub last_at: i64,
    /// Ascending by `hour`, one row per (hour, model).
    pub hours: Vec<BackfillUsageHour>,
    /// Context window in tokens, when the store records it.
    pub ctx_window: Option<u64>,
    /// `[t_ms, tokens in context]`, ascending, downsampled to at most 120 points.
    pub ctx_series: Vec<[i64; 2]>,
    /// Timestamps (ms) of context compactions found in the store.
    pub compactions: Vec<i64>,
    /// Conversation composition (grok today); absent for stores without the
    /// raw request material.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub composition: Option<BackfillComposition>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackfillSource {
    pub agent: String,
    pub root: Option<String>,
    pub found: bool,
    pub files: u32,
    pub sessions: u32,
    pub turns: u64,
    pub error: Option<String>,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackfillReport {
    pub version: u32,
    pub generated_at: i64,
    pub sources: Vec<BackfillSource>,
    pub sessions: Vec<BackfillSession>,
}

/// Per-source scan result (internal).
pub(crate) struct SourceOutcome {
    pub agent: &'static str,
    pub root: Option<String>,
    pub found: bool,
    pub files: u32,
    pub sessions: Vec<SessionFacts>,
    pub error: Option<String>,
    pub elapsed_ms: u64,
    /// Cache keys this source still owns — the driver prunes the rest once,
    /// after every source ran (a per-source prune would wipe the others).
    pub live_keys: Vec<String>,
}

impl SourceOutcome {
    pub(crate) fn new(agent: &'static str, root: Option<String>) -> Self {
        Self {
            agent,
            root,
            found: false,
            files: 0,
            sessions: Vec::new(),
            error: None,
            elapsed_ms: 0,
            live_keys: Vec::new(),
        }
    }
}

/// Where each store lives. `None` = "not installed here" (source reports
/// found=false); tests build their own with temp dirs so the real user
/// directories are never touched.
#[derive(Default)]
pub(crate) struct SourceRoots {
    pub grok_sessions: Option<std::path::PathBuf>,
    pub claude_projects: Option<std::path::PathBuf>,
    pub codex_sessions: Option<std::path::PathBuf>,
    pub dsh_sessions: Option<std::path::PathBuf>,
    pub opencode_db: Option<std::path::PathBuf>,
    pub zcode_db: Option<std::path::PathBuf>,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// One source, timed and panic-isolated: a parser crash must not take the
/// other sources down. Errors land in `BackfillSource.error` + one dev.log
/// warn line (no conversation content in the detail).
fn run_isolated(agent: &'static str, job: impl FnOnce() -> SourceOutcome) -> SourceOutcome {
    let started = Instant::now();
    match std::panic::catch_unwind(AssertUnwindSafe(job)) {
        Ok(mut outcome) => {
            outcome.agent = agent;
            outcome.elapsed_ms = started.elapsed().as_millis() as u64;
            outcome
        }
        Err(payload) => {
            let detail = panic_message(&payload);
            crate::debug_log::append(
                "usage",
                "warn",
                "-",
                &format!("backfill source {agent} panicked"),
                Some(&detail),
            );
            SourceOutcome {
                agent,
                root: None,
                found: false,
                files: 0,
                sessions: Vec::new(),
                error: Some(format!("source panicked: {detail}")),
                elapsed_ms: started.elapsed().as_millis() as u64,
                live_keys: Vec::new(),
            }
        }
    }
}

fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}

/// Scan every supported agent store (incremental: unchanged files come from the
/// cache). `force` ignores the cache.
pub fn scan(force: bool) -> BackfillReport {
    let roots = SourceRoots {
        grok_sessions: grok::discover_root(),
        claude_projects: claude::discover_root(),
        codex_sessions: codex::discover_root(),
        dsh_sessions: dsh::discover_root(),
        opencode_db: opencode::discover_db(),
        zcode_db: zcode::discover_db(),
    };
    scan_with_roots(&roots, force)
}

pub(crate) fn scan_with_roots(roots: &SourceRoots, force: bool) -> BackfillReport {
    let keep_from_ms = now_ms() - KEEP_DAYS * 24 * 3_600_000;

    // No cache root (tests without an override) → scan without persistence.
    let cache_path = cache::cache_root()
        .ok()
        .map(|dir| dir.join("backfill-cache.json"));
    let mut cache = cache_path
        .as_ref()
        .map(|path| cache::load_at(path))
        .unwrap_or_default();

    let mut outcomes: Vec<SourceOutcome> = Vec::new();
    if let Some(root) = roots.grok_sessions.clone() {
        outcomes.push(run_isolated(grok::AGENT, || {
            grok::scan(&root, &mut cache, force, keep_from_ms)
        }));
    }
    if let Some(root) = roots.claude_projects.clone() {
        outcomes.push(run_isolated(claude::AGENT, || {
            claude::scan(&root, &mut cache, force, keep_from_ms)
        }));
    }
    if let Some(root) = roots.codex_sessions.clone() {
        outcomes.push(run_isolated(codex::AGENT, || {
            codex::scan(&root, &mut cache, force, keep_from_ms)
        }));
    }
    if let Some(root) = roots.dsh_sessions.clone() {
        outcomes.push(run_isolated(dsh::AGENT, || {
            dsh::scan(&root, &mut cache, force, keep_from_ms)
        }));
    }
    {
        let db = roots.opencode_db.clone();
        outcomes.push(run_isolated(opencode::AGENT, || {
            opencode::scan(db.as_deref(), &mut cache, force, keep_from_ms)
        }));
    }
    {
        let db = roots.zcode_db.clone();
        outcomes.push(run_isolated(zcode::AGENT, || {
            zcode::scan(db.as_deref(), &mut cache, force, keep_from_ms)
        }));
    }

    if let Some(path) = &cache_path {
        let live_keys: Vec<String> = outcomes.iter().flat_map(|o| o.live_keys.clone()).collect();
        cache::prune_to(&mut cache, &live_keys);
        cache.version = 1;
        if let Err(error) = cache::save_at(path, &cache) {
            crate::debug_log::append(
                "usage",
                "warn",
                "-",
                "backfill cache save failed",
                Some(&error),
            );
        }
    }

    assemble(outcomes)
}

/// Merge facts into the contract shape: dedupe by (agent, native_id), apply the
/// 1500-session cap, sort hours ascending.
fn assemble(outcomes: Vec<SourceOutcome>) -> BackfillReport {
    let mut sources: Vec<BackfillSource> = Vec::new();
    let mut merged: std::collections::HashMap<(String, String), SessionFacts> =
        std::collections::HashMap::new();
    for outcome in outcomes {
        let turns: u64 = outcome
            .sessions
            .iter()
            .map(|s| s.hours.iter().map(|h| h.turns as u64).sum::<u64>())
            .sum();
        sources.push(BackfillSource {
            agent: outcome.agent.to_string(),
            root: outcome.root,
            found: outcome.found,
            files: outcome.files,
            sessions: outcome.sessions.len() as u32,
            turns,
            error: outcome.error,
            elapsed_ms: outcome.elapsed_ms,
        });
        for session in outcome.sessions {
            let key = (session.agent.clone(), session.native_id.clone());
            match merged.get_mut(&key) {
                // Same native id seen from two files (e.g. a store that
                // re-exports): union the facts rather than dropping rows.
                Some(existing) => {
                    existing.hours.extend(session.hours);
                    existing.compactions.extend(session.compactions);
                    existing.ctx_series.extend(session.ctx_series);
                    existing.first_at = existing.first_at.min(session.first_at);
                    existing.last_at = existing.last_at.max(session.last_at);
                    if existing.title.is_none() {
                        existing.title = session.title;
                    }
                    if existing.cwd.is_none() {
                        existing.cwd = session.cwd;
                    }
                    if existing.ctx_window.is_none() {
                        existing.ctx_window = session.ctx_window;
                    }
                    if existing.parent_native_id.is_none() {
                        existing.parent_native_id = session.parent_native_id;
                    }
                    if existing.composition.is_none() {
                        existing.composition = session.composition;
                    }
                }
                None => {
                    merged.insert(key, session);
                }
            }
        }
    }

    let mut facts: Vec<SessionFacts> = merged.into_values().collect();
    facts.sort_by(|a, b| b.last_at.cmp(&a.last_at));
    facts.truncate(SESSION_CAP);

    let sessions: Vec<BackfillSession> = facts
        .into_iter()
        .map(|f| {
            // The cross-file merge above concatenated hour rows; re-collapse
            // identical (hour, model) keys so the contract's "one row per
            // (hour, model)" holds.
            let mut by_key: std::collections::BTreeMap<(i64, Option<String>), BackfillUsageHour> =
                std::collections::BTreeMap::new();
            for h in f.hours {
                let key = (h.hour, h.model.clone());
                let row = by_key.entry(key).or_default();
                row.hour = h.hour;
                row.model = h.model;
                row.turns += h.turns;
                row.requests += h.requests;
                row.input += h.input;
                row.output += h.output;
                row.cached += h.cached;
                row.cache_creation += h.cache_creation;
                row.reasoning += h.reasoning;
                if let Some(usd) = h.reported_usd {
                    row.reported_usd = Some(row.reported_usd.unwrap_or(0.0) + usd);
                }
            }
            let mut hours: Vec<BackfillUsageHour> = by_key.into_values().collect();
            hours.sort_by(|a, b| a.hour.cmp(&b.hour).then(a.model.cmp(&b.model)));
            let mut compactions = f.compactions;
            compactions.sort_unstable();
            compactions.dedup();
            BackfillSession {
                agent: f.agent,
                native_id: f.native_id,
                parent_native_id: f.parent_native_id,
                title: f.title,
                cwd: f.cwd,
                first_at: f.first_at,
                last_at: f.last_at,
                hours,
                ctx_window: f.ctx_window,
                ctx_series: f.ctx_series,
                compactions,
                composition: f.composition.map(Into::into),
            }
        })
        .collect();

    BackfillReport {
        version: 1,
        generated_at: now_ms(),
        sources,
        sessions,
    }
}

#[tauri::command]
pub async fn usage_backfill_scan(force: Option<bool>) -> Result<BackfillReport, String> {
    let force = force.unwrap_or(false);
    tauri::async_runtime::spawn_blocking(move || scan(force))
        .await
        .map_err(|error| format!("backfill scan task failed: {error}"))
}

/// On-demand composition of one Grok conversation, for the live session detail
/// panel: `native_id` is the sessionId the adapter reports (Marionette stores
/// it in `Session.nativeSessionIds`). Reads the session's own files only.
#[tauri::command]
pub async fn usage_grok_composition(
    native_id: String,
) -> Result<Option<BackfillComposition>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let root = grok::discover_root().ok_or_else(|| "grok sessions root not found".to_string())?;
        let dir = grok::find_session_dir(&root, &native_id)
            .ok_or_else(|| "session directory not found".to_string())?;
        let measured = composition::signals_context_used(&dir);
        Ok(composition::grok_session_composition(&dir, measured).map(Into::into))
    })
    .await
    .map_err(|error| format!("grok composition task failed: {error}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assemble_caps_sessions_and_sorts_newest_first() {
        let mut o = SourceOutcome::new(grok::AGENT, None);
        o.found = true;
        o.files = 1;
        for i in 0..1600i64 {
            let mut c = facts::SessionCollector::new(format!("s{i}"));
            c.push_turn(i * 1000);
            if let Some(f) = c.finish(grok::AGENT, 0) {
                o.sessions.push(f);
            }
        }
        let report = assemble(vec![o]);
        assert_eq!(report.sessions.len(), SESSION_CAP);
        assert!(report.sessions[0].last_at >= report.sessions[1].last_at);
        // Newest sessions survived the cap.
        assert!(report.sessions.iter().all(|s| s.last_at >= 100_000));
        assert_eq!(report.sources[0].turns, 1600);
    }

    #[test]
    fn assemble_merges_duplicate_native_ids() {
        let mut o = SourceOutcome::new(codex::AGENT, None);
        let mut a = facts::SessionCollector::new("dup");
        a.push_turn(5_000);
        a.push_request(facts::RequestRecord {
            t_ms: 6_000,
            input: 10,
            ..Default::default()
        });
        let mut b = facts::SessionCollector::new("dup");
        b.push_request(facts::RequestRecord {
            t_ms: 3_600_000 * 5,
            input: 7,
            ..Default::default()
        });
        // Same (hour, model) as a's request — must collapse into one row.
        let mut c = facts::SessionCollector::new("dup");
        c.push_request(facts::RequestRecord {
            t_ms: 6_500,
            input: 3,
            ..Default::default()
        });
        b.cwd = Some("D:/x".into());
        o.sessions.push(a.finish(codex::AGENT, 0).unwrap());
        o.sessions.push(b.finish(codex::AGENT, 0).unwrap());
        o.sessions.push(c.finish(codex::AGENT, 0).unwrap());
        let report = assemble(vec![o]);
        assert_eq!(report.sessions.len(), 1);
        let s = &report.sessions[0];
        assert_eq!(s.cwd.as_deref(), Some("D:/x"));
        assert_eq!(s.hours.len(), 2);
        let first = &s.hours[0];
        assert_eq!((first.requests, first.input, first.turns), (2, 13, 1));
    }

    /// Real-machine smoke: scans the actual user stores, prints per-source
    /// numbers and timings, then scans again to show the cache hit.
    /// `cargo test --bin marionette usage_backfill -- --ignored --nocapture`
    #[test]
    #[ignore = "touches the real ~/.grok, ~/.claude, ~/.codex, ~/.dsh, opencode.db, zcode db and writes the real backfill cache"]
    fn real_machine_smoke() {
        cache::set_root_override_for_tests(
            crate::app_paths::global_dir().ok().map(|p| p.join("usage")),
        );
        let t0 = Instant::now();
        let cold = scan(false);
        let cold_elapsed = t0.elapsed();
        println!("── cold scan: {:.2?}", cold_elapsed);
        for source in &cold.sources {
            println!(
                "  {:<12} found={} files={} sessions={} turns={} elapsed={}ms error={:?}",
                source.agent, source.found, source.files, source.sessions, source.turns,
                source.elapsed_ms, source.error
            );
        }
        let mut totals: std::collections::BTreeMap<String, [u64; 8]> = Default::default();
        for s in &cold.sessions {
            let e = totals.entry(s.agent.clone()).or_insert([0; 8]);
            for h in &s.hours {
                e[0] += h.turns as u64;
                e[1] += h.requests as u64;
                e[2] += h.input;
                e[3] += h.output;
                e[4] += h.cached;
                e[5] += h.cache_creation;
                e[6] += h.reasoning as u64;
                if let Some(usd) = h.reported_usd {
                    e[7] += (usd * 1e4).round() as u64; // 0.1 milli-USD units
                }
            }
        }
        for (agent, v) in &totals {
            println!(
                "  Σ {agent:<12} turns={} requests={} input={} output={} cached={} cacheWrite={} reasoning={} usd={:.4}",
                v[0], v[1], v[2], v[3], v[4], v[5], v[6], v[7] as f64 / 1e4
            );
        }
        println!("  sessions total: {}", cold.sessions.len());

        let t1 = Instant::now();
        let warm = scan(false);
        let warm_elapsed = t1.elapsed();
        println!("── warm scan (cache): {:.2?}", warm_elapsed);
        for source in &warm.sources {
            println!(
                "  {:<12} sessions={} elapsed={}ms",
                source.agent, source.sessions, source.elapsed_ms
            );
        }
        assert_eq!(warm.sessions.len(), cold.sessions.len());
        cache::set_root_override_for_tests(None);
    }
}
