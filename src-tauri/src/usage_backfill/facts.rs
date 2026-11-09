//! Shared intermediate shape every source folds into before it becomes the
//! contract types: per-request usage records + turn markers, aggregated by the
//! [`SessionFacts`] builder into per-(UTC-hour, model) rows.
//!
//! Bucket semantics are the live ledger's (`src/lib/usageLedger.ts` +
//! `recordTurnUsageInto`), so backfill and real-time numbers can be summed:
//! - `input`  = fresh (uncached) prompt tokens;
//! - `cached` = cache-read prompt tokens;
//! - `cache_creation` = cache-write prompt tokens;
//! - `output` = everything generated INCLUDING reasoning;
//! - `reasoning` = the part of `output` that was reasoning (0 when unknown).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Hours since the Unix epoch, UTC: `floor(t_ms / 3_600_000)`.
pub(crate) fn hour_of(t_ms: i64) -> i64 {
    t_ms.div_euclid(3_600_000)
}

/// One model request's usage, already converted to the ledger's buckets.
/// `requests` is a multiplier: stores that only keep turn totals but also a
/// per-model call count (Grok) fold N identical calls into one record.
#[derive(Debug, Clone, Default)]
pub(crate) struct RequestRecord {
    pub t_ms: i64,
    pub model: Option<String>,
    pub input: u64,
    pub output: u64,
    pub cached: u64,
    pub cache_creation: u64,
    pub reasoning: u64,
    pub reported_usd: Option<f64>,
    /// Tokens in context for this request, when the store lets us reconstruct
    /// it (Claude: input + cache_read + cache_creation; Codex: billed prompt).
    pub ctx_tokens: Option<u64>,
    /// How many API requests this record accounts for (default 1).
    pub requests: u32,
}

/// One aggregated (hour, model) row. Mirrors `BackfillUsageHour` but stays
/// private so the contract types can evolve independently.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct HourFacts {
    pub hour: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub turns: u32,
    pub requests: u32,
    pub input: u64,
    pub output: u64,
    pub cached: u64,
    pub cache_creation: u64,
    pub reasoning: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reported_usd: Option<f64>,
}

/// A folded session, cache-serializable.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct SessionFacts {
    pub agent: String,
    pub native_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_native_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    pub first_at: i64,
    pub last_at: i64,
    #[serde(default)]
    pub hours: Vec<HourFacts>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ctx_window: Option<u64>,
    #[serde(default)]
    pub ctx_series: Vec<[i64; 2]>,
    #[serde(default)]
    pub compactions: Vec<i64>,
    /// Conversation composition when the store has the raw request material
    /// (grok). Cache-serialized, additive to older cache files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub composition: Option<super::composition::CompositionFacts>,
}

/// Accumulator each source parser feeds, then converts with [`Self::finish`].
pub(crate) struct SessionCollector {
    pub native_id: String,
    pub parent_native_id: Option<String>,
    pub title: Option<String>,
    pub cwd: Option<String>,
    pub ctx_window: Option<u64>,
    requests: Vec<RequestRecord>,
    /// Turn start times (ms). Model attribution is borrowed from the first
    /// request at/after the turn, matching how the live ledger stamps a turn
    /// with the model that answered it.
    turns: Vec<i64>,
    compactions: Vec<i64>,
    ctx_points: Vec<(i64, u64)>,
    /// Session-level time bounds from the store's metadata (e.g. Grok
    /// `summary.json` created_at) — widen the computed bounds when they go
    /// beyond what requests/turns cover.
    first_hint_ms: Option<i64>,
    last_hint_ms: Option<i64>,
}

/// Downsample cap for `ctx_series` (contract).
pub(crate) const CTX_SERIES_MAX: usize = 120;

impl SessionCollector {
    pub fn new(native_id: impl Into<String>) -> Self {
        Self {
            native_id: native_id.into(),
            parent_native_id: None,
            title: None,
            cwd: None,
            ctx_window: None,
            requests: Vec::new(),
            turns: Vec::new(),
            compactions: Vec::new(),
            ctx_points: Vec::new(),
            first_hint_ms: None,
            last_hint_ms: None,
        }
    }

    /// Widen the session's time bounds with store metadata timestamps.
    pub fn hint_bounds(&mut self, first: Option<i64>, last: Option<i64>) {
        if let Some(t) = first {
            self.first_hint_ms = Some(self.first_hint_ms.map_or(t, |p| p.min(t)));
        }
        if let Some(t) = last {
            self.last_hint_ms = Some(self.last_hint_ms.map_or(t, |p| p.max(t)));
        }
    }

    /// Latest metadata timestamp seen via [`Self::hint_bounds`], if any —
    /// lets sources anchor a session-final context reading.
    pub fn last_hint(&self) -> Option<i64> {
        self.last_hint_ms
    }

    pub fn push_request(&mut self, record: RequestRecord) {
        if let Some(tokens) = record.ctx_tokens {
            self.ctx_points.push((record.t_ms, tokens));
        }
        self.requests.push(record);
    }

    pub fn push_turn(&mut self, t_ms: i64) {
        self.turns.push(t_ms);
    }

    pub fn push_compaction(&mut self, t_ms: i64) {
        self.compactions.push(t_ms);
    }

    /// Merge another collector's raw contents into this one (used when one
    /// logical session is split across several collectors — e.g. Claude
    /// sidechain replays inside a subagent file).
    pub fn absorb(&mut self, other: SessionCollector) {
        self.requests.extend(other.requests);
        self.turns.extend(other.turns);
        self.compactions.extend(other.compactions);
        self.ctx_points.extend(other.ctx_points);
        if self.title.is_none() {
            self.title = other.title;
        }
        if self.cwd.is_none() {
            self.cwd = other.cwd;
        }
        if self.ctx_window.is_none() {
            self.ctx_window = other.ctx_window;
        }
        self.hint_bounds(other.first_hint_ms, other.last_hint_ms);
    }

    pub fn push_ctx_point(&mut self, t_ms: i64, tokens: u64) {
        self.ctx_points.push((t_ms, tokens));
    }

    pub fn is_empty(&self) -> bool {
        self.requests.is_empty() && self.turns.is_empty() && self.compactions.is_empty()
    }

    /// Fold everything into cache-ready facts. `keep_from_ms` drops hour rows
    /// older than the 190-day window (contract); a session left with nothing
    /// inside the window returns `None`.
    pub fn finish(self, agent: &str, keep_from_ms: i64) -> Option<SessionFacts> {
        if self.is_empty() {
            return None;
        }
        // Sort requests chronologically so turn→model attribution sees them in
        // order.
        let mut requests = self.requests;
        requests.sort_by_key(|r| r.t_ms);
        let mut sorted_turns = self.turns;
        sorted_turns.sort_unstable();

        // (hour, model) → row.
        let mut rows: BTreeMap<(i64, Option<String>), HourFacts> = BTreeMap::new();
        let mut first_at = i64::MAX;
        let mut last_at = i64::MIN;

        for record in &requests {
            if record.t_ms < keep_from_ms {
                continue;
            }
            let key = (hour_of(record.t_ms), record.model.clone());
            let row = rows.entry(key).or_default();
            row.hour = hour_of(record.t_ms);
            row.model = record.model.clone();
            row.requests += record.requests.max(1);
            row.input += record.input;
            row.output += record.output;
            row.cached += record.cached;
            row.cache_creation += record.cache_creation;
            row.reasoning += record.reasoning;
            if let Some(usd) = record.reported_usd {
                row.reported_usd = Some(row.reported_usd.unwrap_or(0.0) + usd);
            }
            first_at = first_at.min(record.t_ms);
            last_at = last_at.max(record.t_ms);
        }

        // Attribute each turn to the model of the first request at/after it
        // (the model that answered); when none follows, reuse the previous
        // request's model. Falls back to a model-less row.
        let mut req_iter = requests.iter().peekable();
        let mut prev_model: Option<String> = None;
        for turn_ms in sorted_turns {
            if turn_ms < keep_from_ms {
                continue;
            }
            while let Some(next) = req_iter.peek() {
                if next.t_ms < turn_ms {
                    prev_model = next.model.clone();
                    req_iter.next();
                } else {
                    break;
                }
            }
            let model = req_iter
                .peek()
                .and_then(|r| r.model.clone())
                .or(prev_model.clone());
            let key = (hour_of(turn_ms), model);
            let row = rows.entry(key.clone()).or_default();
            row.hour = key.0;
            row.model = key.1;
            row.turns += 1;
            first_at = first_at.min(turn_ms);
            last_at = last_at.max(turn_ms);
        }

        let mut compactions: Vec<i64> = self
            .compactions
            .into_iter()
            .filter(|t| *t >= keep_from_ms)
            .collect();
        compactions.sort_unstable();
        compactions.dedup();
        for t in &compactions {
            first_at = first_at.min(*t);
            last_at = last_at.max(*t);
        }

        let mut ctx_series: Vec<[i64; 2]> = self
            .ctx_points
            .into_iter()
            .filter(|(t, _)| *t >= keep_from_ms)
            .map(|(t, tokens)| [t, i64::try_from(tokens).unwrap_or(i64::MAX)])
            .collect();
        downsample_ctx(&mut ctx_series);

        if rows.is_empty() && compactions.is_empty() && ctx_series.is_empty() {
            return None;
        }
        if first_at == i64::MAX {
            first_at = 0;
        }
        if last_at == i64::MIN {
            last_at = first_at;
        }
        // Metadata bounds only ever widen (and stay inside the window).
        if let Some(hint) = self.first_hint_ms.filter(|t| *t >= keep_from_ms) {
            first_at = first_at.min(hint);
        }
        if let Some(hint) = self.last_hint_ms.filter(|t| *t >= keep_from_ms) {
            last_at = last_at.max(hint);
        }

        let mut hours: Vec<HourFacts> = rows.into_values().collect();
        hours.sort_by(|a, b| a.hour.cmp(&b.hour).then(a.model.cmp(&b.model)));

        Some(SessionFacts {
            agent: agent.to_string(),
            native_id: self.native_id,
            parent_native_id: self.parent_native_id,
            title: self.title,
            cwd: self.cwd,
            first_at,
            last_at,
            hours,
            ctx_window: self.ctx_window,
            ctx_series,
            compactions,
            composition: None,
        })
    }
}

/// Keep at most [`CTX_SERIES_MAX`] points: always the first and last, evenly
/// spaced in between. Input must be time-ascending.
pub(crate) fn downsample_ctx(points: &mut Vec<[i64; 2]>) {
    if points.len() <= CTX_SERIES_MAX {
        points.sort_by_key(|p| p[0]);
        points.dedup_by_key(|p| p[0]);
        return;
    }
    points.sort_by_key(|p| p[0]);
    let n = points.len();
    let mut kept: Vec<[i64; 2]> = Vec::with_capacity(CTX_SERIES_MAX);
    kept.push(points[0]);
    // stride so we land exactly on the last element.
    let step = (n - 1) as f64 / (CTX_SERIES_MAX - 1) as f64;
    let mut last_index = 0usize;
    let mut k = 1usize;
    while k < CTX_SERIES_MAX - 1 {
        let idx = (k as f64 * step).round() as usize;
        let idx = idx.max(last_index + 1).min(n - 2);
        kept.push(points[idx]);
        last_index = idx;
        k += 1;
    }
    kept.push(points[n - 1]);
    *points = kept;
}

/// ISO-8601 timestamp (UTC `Z`) → epoch ms. Handles the fractional seconds
/// Claude/Codex write (`2026-11-06T08:18:51.541Z`, `…51.5412001Z`).
pub(crate) fn iso_ms(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.len() < 19 {
        return None;
    }
    let year: i64 = s.get(0..4)?.parse().ok()?;
    let month: i64 = s.get(5..7)?.parse().ok()?;
    let day: i64 = s.get(8..10)?.parse().ok()?;
    let hour: i64 = s.get(11..13)?.parse().ok()?;
    let min: i64 = s.get(14..16)?.parse().ok()?;
    let sec: i64 = s.get(17..19)?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    // Civil-from-days algorithm (Howard Hinnant) — no chrono dependency.
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let mut ms = (days * 86_400 + hour * 3_600 + min * 60 + sec) * 1000;
    // Optional fractional part.
    if let Some(rest) = s.get(19..) {
        if let Some(frac) = rest.strip_prefix('.') {
            let digits: String = frac.chars().take_while(|c| c.is_ascii_digit()).collect();
            if !digits.is_empty() {
                let millis: f64 = format!("0.{digits}").parse().unwrap_or(0.0);
                ms += (millis * 1000.0).floor() as i64;
            }
        }
    }
    Some(ms)
}

/// JSON u64 getter (accepts floats that are whole numbers, negative → None).
pub(crate) fn json_u64(v: &serde_json::Value, key: &str) -> Option<u64> {
    match v.get(key) {
        Some(serde_json::Value::Number(n)) => {
            if let Some(u) = n.as_u64() {
                Some(u)
            } else {
                n.as_f64().filter(|f| *f >= 0.0).map(|f| f as u64)
            }
        }
        Some(serde_json::Value::String(s)) => s.trim().parse::<u64>().ok(),
        _ => None,
    }
}

pub(crate) fn json_i64(v: &serde_json::Value, key: &str) -> Option<i64> {
    match v.get(key) {
        Some(serde_json::Value::Number(n)) => n.as_i64(),
        Some(serde_json::Value::String(s)) => s.trim().parse::<i64>().ok(),
        _ => None,
    }
}

pub(crate) fn json_str<'a>(v: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(|x| x.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hour_bucketing_is_floor_division() {
        assert_eq!(hour_of(0), 0);
        assert_eq!(hour_of(3_599_999), 0);
        assert_eq!(hour_of(3_600_000), 1);
        // Pre-epoch timestamps must floor, not truncate toward zero.
        assert_eq!(hour_of(-1), -1);
        assert_eq!(hour_of(-3_600_000), -1);
    }

    #[test]
    fn iso_ms_parses_fractional_seconds() {
        // Cross-checked with Python datetime (2026-11-09).
        assert_eq!(iso_ms("2026-11-06T08:18:51.541Z"), Some(1_793_953_131_541));
        assert_eq!(iso_ms("2026-11-06T08:18:51Z"), Some(1_793_953_131_000));
        assert_eq!(iso_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(iso_ms("bogus"), None);
    }

    #[test]
    fn ctx_series_downsamples_to_cap() {
        let mut points: Vec<[i64; 2]> =
            (0..500).map(|i| [i as i64 * 1000, i as i64 * 10]).collect();
        downsample_ctx(&mut points);
        assert!(points.len() <= CTX_SERIES_MAX);
        assert_eq!(points.first().unwrap()[0], 0);
        assert_eq!(points.last().unwrap()[0], 499_000);
        // Ascending.
        for w in points.windows(2) {
            assert!(w[0][0] < w[1][0]);
        }
    }

    #[test]
    fn ctx_series_keeps_small_series_intact() {
        let mut points = vec![[5, 50], [1, 10], [3, 30]];
        downsample_ctx(&mut points);
        assert_eq!(points, vec![[1, 10], [3, 30], [5, 50]]);
    }

    #[test]
    fn collector_buckets_requests_and_turns_by_hour_and_model() {
        let mut c = SessionCollector::new("s1");
        c.push_request(RequestRecord {
            t_ms: 3_600_000 * 10 + 1000,
            model: Some("model-a".into()),
            input: 100,
            output: 40,
            cached: 10,
            cache_creation: 5,
            reasoning: 4,
            reported_usd: Some(0.5),
            ctx_tokens: Some(115),
            requests: 1,
        });
        c.push_request(RequestRecord {
            t_ms: 3_600_000 * 10 + 2000,
            model: Some("model-b".into()),
            input: 7,
            output: 3,
            cached: 0,
            cache_creation: 0,
            reasoning: 0,
            reported_usd: None,
            ctx_tokens: None,
            requests: 1,
        });
        c.push_turn(3_600_000 * 10 + 500);
        c.push_turn(3_600_000 * 12);
        let facts = c.finish("agent", 0).unwrap();
        assert_eq!(facts.hours.len(), 3);
        let a = facts.hours.iter().find(|h| h.model.as_deref() == Some("model-a")).unwrap();
        assert_eq!((a.turns, a.requests, a.input, a.output), (1, 1, 100, 40));
        assert_eq!(a.reported_usd, Some(0.5));
        // Late turn with no following request inherits the last request's
        // model (the model that had been answering) — its own (hour, model)
        // row, separate from model-b's request row in hour 10.
        let late = facts.hours.last().unwrap();
        assert_eq!(
            (late.hour, late.turns, late.requests, late.model.as_deref()),
            (12, 1, 0, Some("model-b"))
        );
        assert_eq!(facts.first_at, 3_600_000 * 10 + 500);
        assert_eq!(facts.last_at, 3_600_000 * 12);
    }

    #[test]
    fn collector_drops_hours_before_cutoff() {
        let mut c = SessionCollector::new("s1");
        c.push_request(RequestRecord {
            t_ms: 1_000_000,
            model: None,
            input: 1,
            requests: 1,
            ..Default::default()
        });
        c.push_request(RequestRecord {
            t_ms: 5_000_000,
            model: None,
            input: 2,
            requests: 1,
            ..Default::default()
        });
        let facts = c.finish("agent", 4_000_000).unwrap();
        assert_eq!(facts.hours.len(), 1);
        assert_eq!(facts.hours[0].input, 2);
        // Everything outside the window → session dropped.
        let mut c2 = SessionCollector::new("s2");
        c2.push_request(RequestRecord {
            t_ms: 1_000_000,
            model: None,
            input: 1,
            requests: 1,
            ..Default::default()
        });
        assert!(c2.finish("agent", 4_000_000).is_none());
    }
}
