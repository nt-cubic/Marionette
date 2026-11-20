//! Host-side record of which snapshot generation the page's refs answer to.
//!
//! The page engine (vendored `agent.bundle.js`) is the only judge of ref
//! freshness: `refsAreCurrent` re-checks the token, the URL, history length,
//! the popstate/hashchange ticks, the navigation entry id and the element's
//! `isConnected` on every action, and its `refsToken` is rewritten only by a
//! new snapshot. Nothing here duplicates that — the lease never says a
//! generation is *valid*, it only remembers when the host already *knows*
//! one is dead (a navigation it saw, the annotate flow's hidden re-snapshot,
//! the page's own stale answer). A known-dead generation answers from the
//! lease without touching CDP, and with the true reason; everything else
//! goes to the page unchanged. That one-sided rule is what makes the
//! fast path safe: it cannot refuse a generation the page would have
//! accepted, because it only fires on deaths the host witnessed.
//!
//! Rollback: `MARIONETTE_BROWSER_REF_LEASE=0` restores the pre-lease host
//! behavior exactly (the per-action generation bump, no fast answers, no
//! response annotations).

use std::cell::RefCell;
use std::sync::OnceLock;

use serde_json::{json, Value};

/// How a lease died. The string is wire-facing (`reason` on stale answers);
/// the sentence is what the model actually reads.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LeaseDeath {
    /// A real main-frame document navigation (`apply_document_navigation`).
    DocumentNavigation,
    /// A same-document route change — pushState, hash, history navigation.
    RouteNavigation,
    /// The user's annotate flow (`pick_resolve`) re-snapshotted the tab;
    /// its hidden full snapshot replaces the refs the agent was using.
    UserAnnotation,
    /// The page refused this generation itself; retrying the same token
    /// without a new snapshot cannot succeed.
    StaleAnswer,
    /// A newer snapshot replaced this one — the token is old news.
    SupersededBySnapshot,
}

impl LeaseDeath {
    pub fn as_str(self) -> &'static str {
        match self {
            LeaseDeath::DocumentNavigation => "document_navigation",
            LeaseDeath::RouteNavigation => "route_navigation",
            LeaseDeath::UserAnnotation => "user_annotation",
            LeaseDeath::StaleAnswer => "stale_answer",
            LeaseDeath::SupersededBySnapshot => "superseded_by_snapshot",
        }
    }

    pub fn guidance(self) -> &'static str {
        match self {
            LeaseDeath::DocumentNavigation => {
                "the tab navigated to a new document after that snapshot; take a new one"
            }
            LeaseDeath::RouteNavigation => {
                "the page changed route (pushState, hash or history) after that snapshot; take a new one"
            }
            LeaseDeath::UserAnnotation => {
                "the user's annotate flow re-snapshotted this tab and replaced the refs; take a new one"
            }
            LeaseDeath::StaleAnswer => {
                "the page already refused that generation as stale; take a new snapshot"
            }
            LeaseDeath::SupersededBySnapshot => {
                "a newer snapshot replaced that one; use the generation from the latest snapshot"
            }
        }
    }
}

/// One snapshot's lease. Lives on the `Surface`; never persisted, never
/// restored across cold starts — a token from a previous app run or tab
/// lifetime means nothing and the page would refuse it anyway.
pub struct RefLease {
    /// The world's four-segment generation string, verbatim from the
    /// snapshot answer (`pick::generation_from_snapshot` checked the shape).
    pub token: String,
    /// `nav_epoch` at adopt time; diagnostic, not a validity check.
    /// (读侧未接线;字段保留供诊断,消费点接上后删 allow。)
    #[allow(dead_code)]
    pub nav_epoch: u64,
    /// Adopt time; diagnostic (why does this tab hold a ref from 2h ago?).
    #[allow(dead_code)]
    pub created_at_ms: u64,
    /// Last time an action answered `ok` against this exact token.
    pub last_used_at_ms: u64,
    /// First cause of death for `token`. First wins: after the fact every
    /// later "cause" is just another way of saying it is still dead.
    invalid: Option<LeaseDeath>,
    /// The token this lease replaced, and why it died — lets a model still
    /// holding the previous snapshot's token get the true story instead of
    /// the page's generic stale detail.
    superseded: Option<(String, LeaseDeath)>,
}

impl RefLease {
    /// Adopt the generation of a fresh snapshot. `replaced` is the death the
    /// previous lease's token died of *by being replaced*; if it had already
    /// been marked dead (a navigation, a stale answer), that earlier cause
    /// is the truer one and is kept.
    pub fn adopt(
        token: String,
        nav_epoch: u64,
        at_ms: u64,
        replaced: LeaseDeath,
        previous: Option<&RefLease>,
    ) -> RefLease {
        let superseded = previous
            .filter(|old| old.token != token)
            .map(|old| (old.token.clone(), old.invalid.unwrap_or(replaced)));
        RefLease {
            token,
            nav_epoch,
            created_at_ms: at_ms,
            last_used_at_ms: at_ms,
            invalid: None,
            superseded,
        }
    }

    pub fn invalidate(&mut self, death: LeaseDeath) {
        if self.invalid.is_none() {
            self.invalid = Some(death);
        }
    }

    /// Refresh the use timestamp, but only for the token the page actually
    /// honored — a lease that lags behind the world must not look fresher
    /// than it is.
    pub fn note_used(&mut self, token: &str, at_ms: u64) {
        if self.token == token {
            self.last_used_at_ms = at_ms;
        }
    }

    /// The death this lease can *prove* for `token`, if any. `None` means
    /// "unknown — the page decides": a token that is neither the current
    /// one nor a recorded predecessor was never seen here, and the world is
    /// the only authority on it.
    pub fn stale_reason_for(&self, token: &str) -> Option<LeaseDeath> {
        if self.token == token {
            return self.invalid;
        }
        self.superseded
            .as_ref()
            .filter(|(old, _)| old == token)
            .map(|(_, death)| *death)
    }
}

fn env_enables(raw: Option<&str>) -> bool {
    raw != Some("0")
}

/// The lease feature flag. `MARIONETTE_BROWSER_REF_LEASE=0` restores the
/// pre-lease host behavior (per-action generation bump, no fast answers,
/// no annotations). Read once per process.
pub fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| env_enables(std::env::var("MARIONETTE_BROWSER_REF_LEASE").ok().as_deref()))
}

/// Answer a known-dead generation without touching CDP. The payload mirrors
/// the page engine's stale shape (`ok:false, url, error:"stale", detail`)
/// plus `reason`, so consumers need no new parsing branch.
pub fn fast_refusal(lease: &RefCell<Option<RefLease>>, url: &str, token: &str) -> Option<Value> {
    if !enabled() {
        return None;
    }
    let lease = lease.borrow();
    let lease = lease.as_ref()?;
    let death = lease.stale_reason_for(token)?;
    Some(json!({
        "ok": false,
        "url": url,
        "error": "stale",
        "reason": death.as_str(),
        "detail": death.guidance(),
    }))
}

/// Fold an action's answer back into the lease and annotate the success so
/// the model knows it can keep acting on the same generation. A stale
/// answer kills the lease (retrying the token cannot succeed); an `ok`
/// answer stamps the token back into the response — nothing today tells
/// the model a generation survives actions, and the re-snapshot habit is
/// exactly what the lease exists to end.
pub fn note_action(lease: &RefCell<Option<RefLease>>, token: &str, answer: &mut Value, at_ms: u64) {
    if !enabled() {
        return;
    }
    let error = answer.get("error").and_then(Value::as_str).map(str::to_string);
    let ok = answer.get("ok").and_then(Value::as_bool).unwrap_or(false);
    if error.as_deref() == Some("stale") {
        if let Some(lease) = lease.borrow_mut().as_mut() {
            lease.invalidate(LeaseDeath::StaleAnswer);
        }
        return;
    }
    if !ok {
        return;
    }
    if let Some(lease) = lease.borrow_mut().as_mut() {
        lease.note_used(token, at_ms);
    }
    if let Some(object) = answer.as_object_mut() {
        object.insert("generation".to_string(), json!(token));
        object.insert("generationStillValid".to_string(), json!(true));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lease(token: &str) -> RefLease {
        RefLease::adopt(token.to_string(), 3, 1000, LeaseDeath::SupersededBySnapshot, None)
    }

    #[test]
    fn a_fresh_lease_judges_nothing() {
        let lease = lease("G.1.4.2");
        // The core invariant: an un-dead lease never claims to know a token
        // is stale — that verdict belongs to the page alone.
        assert_eq!(lease.stale_reason_for("G.1.4.2"), None);
        assert_eq!(lease.stale_reason_for("unknown-token"), None);
    }

    #[test]
    fn a_marked_death_answers_for_the_current_token() {
        let mut lease = lease("G.1.4.2");
        lease.invalidate(LeaseDeath::DocumentNavigation);
        assert_eq!(lease.stale_reason_for("G.1.4.2"), Some(LeaseDeath::DocumentNavigation));
        // Other tokens are still not the lease's business.
        assert_eq!(lease.stale_reason_for("G.2.5.2"), None);
    }

    #[test]
    fn first_death_wins() {
        let mut lease = lease("G.1.4.2");
        lease.invalidate(LeaseDeath::DocumentNavigation);
        lease.invalidate(LeaseDeath::StaleAnswer);
        assert_eq!(lease.stale_reason_for("G.1.4.2"), Some(LeaseDeath::DocumentNavigation));
    }

    #[test]
    fn adopt_remember_why_the_previous_token_died() {
        let old = lease("G.1.4.2");
        // The common path: a plain newer snapshot replaces an alive lease.
        let replaced = RefLease::adopt("G.2.5.2".to_string(), 3, 2000, LeaseDeath::SupersededBySnapshot, Some(&old));
        assert_eq!(replaced.stale_reason_for("G.1.4.2"), Some(LeaseDeath::SupersededBySnapshot));
        assert_eq!(replaced.stale_reason_for("G.2.5.2"), None);

        // The annotate path: pick_resolve adopts with the user-annotation cause.
        let annotated = RefLease::adopt("G.3.6.2".to_string(), 4, 3000, LeaseDeath::UserAnnotation, Some(&replaced));
        assert_eq!(annotated.stale_reason_for("G.2.5.2"), Some(LeaseDeath::UserAnnotation));
        // Memory is one predecessor deep: a grandparent token is the page's
        // business again — its generic stale prose is enough that far back.
        assert_eq!(annotated.stale_reason_for("G.1.4.2"), None);

        // A lease that was already dead keeps its earlier, truer cause.
        let mut dead = lease("G.4.7.2");
        dead.invalidate(LeaseDeath::RouteNavigation);
        let after = RefLease::adopt("G.5.8.2".to_string(), 5, 4000, LeaseDeath::SupersededBySnapshot, Some(&dead));
        assert_eq!(after.stale_reason_for("G.4.7.2"), Some(LeaseDeath::RouteNavigation));
    }

    #[test]
    fn adopting_the_same_token_records_no_predecessor() {
        let old = lease("G.1.4.2");
        let re = RefLease::adopt("G.1.4.2".to_string(), 3, 2000, LeaseDeath::SupersededBySnapshot, Some(&old));
        assert!(re.superseded.is_none());
    }

    #[test]
    fn use_stamps_only_the_honored_token() {
        let mut lease = lease("G.1.4.2");
        lease.note_used("G.9.9.9", 5000);
        assert_eq!(lease.last_used_at_ms, 1000);
        lease.note_used("G.1.4.2", 6000);
        assert_eq!(lease.last_used_at_ms, 6000);
    }

    #[test]
    fn fast_refusal_answers_with_reason_and_detail() {
        let cell = RefCell::new(Some({
            let mut l = lease("G.1.4.2");
            l.invalidate(LeaseDeath::UserAnnotation);
            l
        }));
        let answer = fast_refusal(&cell, "https://example.test/", "G.1.4.2").unwrap();
        assert_eq!(answer["error"], "stale");
        assert_eq!(answer["ok"], false);
        assert_eq!(answer["url"], "https://example.test/");
        assert_eq!(answer["reason"], "user_annotation");
        assert_eq!(answer["detail"], LeaseDeath::UserAnnotation.guidance());
        // Unknown tokens and healthy leases never fast-refuse.
        assert!(fast_refusal(&cell, "https://example.test/", "other").is_none());
        assert!(fast_refusal(&RefCell::new(None), "https://example.test/", "G.1.4.2").is_none());
    }

    #[test]
    fn note_action_kills_the_lease_on_stale_and_stamps_successes() {
        let cell = RefCell::new(Some(lease("G.1.4.2")));
        let mut stale = json!({ "ok": false, "url": "https://x.test/", "error": "stale", "detail": "…" });
        note_action(&cell, "G.1.4.2", &mut stale, 9000);
        assert_eq!(
            cell.borrow().as_ref().unwrap().stale_reason_for("G.1.4.2"),
            Some(LeaseDeath::StaleAnswer)
        );

        let cell = RefCell::new(Some(lease("G.2.5.2")));
        let mut ok = json!({ "ok": true, "url": "https://x.test/" });
        note_action(&cell, "G.2.5.2", &mut ok, 9500);
        assert_eq!(ok["generation"], "G.2.5.2");
        assert_eq!(ok["generationStillValid"], true);
        assert_eq!(cell.borrow().as_ref().unwrap().last_used_at_ms, 9500);

        // Failures that are not stale (disabled, unsupported, …) leave the
        // lease and the answer alone — the refs are still current.
        let cell = RefCell::new(Some(lease("G.3.6.2")));
        let mut disabled = json!({ "ok": false, "url": "https://x.test/", "error": "disabled", "detail": "…" });
        note_action(&cell, "G.3.6.2", &mut disabled, 9600);
        assert_eq!(cell.borrow().as_ref().unwrap().stale_reason_for("G.3.6.2"), None);
        assert!(disabled.get("generationStillValid").is_none());
    }

    #[test]
    fn the_flag_only_treats_exact_zero_as_off() {
        assert!(env_enables(None));
        assert!(env_enables(Some("1")));
        assert!(env_enables(Some("")));
        assert!(!env_enables(Some("0")));
    }
}
