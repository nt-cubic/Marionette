//! Recent-project discovery (T3 P1-6): read-only scan of the local agent
//! session stores for candidate project roots.
//!
//! Contract:
//! - Never writes anything. No project, no session, no preference, no
//!   `.marionette` directory: discovery only reads.
//! - Never talks to Git. No URL cloning, no remotes, no network.
//! - Per-source failures stay isolated: one unreadable store never hides the
//!   others, and the whole call is safe to ignore (the normal Add Project path
//!   does not depend on it).
//! - Results are bounded, so a huge store cannot stall the dialog.
//!
//! Session counts are per store: the same conversation mirrored into two
//! agents' stores counts twice. They are an activity hint for choosing a root,
//! not a deduplicated ledger.

use super::path_norm::{canonicalize_for_match, normalize_path};
use super::util::{home_dir, recency_ms};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Hard cap of candidates handed to the UI.
const MAX_CANDIDATES: usize = 50;
/// Per-source hit cap (protects against a giant store).
pub(crate) const MAX_HITS_PER_SOURCE: usize = 4000;

/// One session seen in a store, already reduced to what discovery needs.
#[derive(Clone, Debug)]
pub struct DiscoveryHit {
    pub cwd: String,
    pub agent_id: &'static str,
    /// Number of sessions behind this row (SQL stores may aggregate).
    pub count: u32,
    pub last_active_at: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredProject {
    pub root_path: String,
    pub name: String,
    pub session_count: u32,
    pub agents: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_active_at: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryReport {
    pub projects: Vec<DiscoveredProject>,
    pub scanned_sources: Vec<String>,
    pub failed_sources: Vec<String>,
    pub truncated: bool,
}

#[derive(Default)]
struct Accum {
    display: String,
    session_count: u32,
    agents: HashSet<String>,
    last_active_at: Option<String>,
}

/// Scan every supported store once and group sessions by project root.
/// Never returns an error: missing stores simply produce no hits.
pub fn scan() -> DiscoveryReport {
    let mut hits = Vec::new();
    let mut scanned = Vec::new();
    let mut failed = Vec::new();

    collect(
        "claude",
        super::claude::discover_hits,
        &mut hits,
        &mut scanned,
        &mut failed,
    );
    collect(
        "codex",
        super::codex::discover_hits,
        &mut hits,
        &mut scanned,
        &mut failed,
    );
    collect(
        "grok",
        super::grok::discover_hits,
        &mut hits,
        &mut scanned,
        &mut failed,
    );
    collect(
        "opencode",
        super::opencode::discover_hits,
        &mut hits,
        &mut scanned,
        &mut failed,
    );
    collect(
        "pi",
        super::pi::discover_pi_hits,
        &mut hits,
        &mut scanned,
        &mut failed,
    );
    collect(
        "omp",
        super::pi::discover_omp_hits,
        &mut hits,
        &mut scanned,
        &mut failed,
    );
    collect(
        "kimi",
        super::jsonl_agents::discover_kimi_hits,
        &mut hits,
        &mut scanned,
        &mut failed,
    );
    collect(
        "codebuddy",
        super::jsonl_agents::discover_codebuddy_hits,
        &mut hits,
        &mut scanned,
        &mut failed,
    );
    collect(
        "openclaw",
        super::jsonl_agents::discover_openclaw_hits,
        &mut hits,
        &mut scanned,
        &mut failed,
    );
    collect(
        "cline",
        super::cline::discover_hits,
        &mut hits,
        &mut scanned,
        &mut failed,
    );
    collect(
        "gemini",
        super::gemini::discover_hits,
        &mut hits,
        &mut scanned,
        &mut failed,
    );
    collect(
        "cursor",
        super::cursor::discover_hits,
        &mut hits,
        &mut scanned,
        &mut failed,
    );
    collect(
        "deepseek",
        super::deepseek::discover_hits,
        &mut hits,
        &mut scanned,
        &mut failed,
    );
    collect(
        "zcode",
        super::zcode::discover_hits,
        &mut hits,
        &mut scanned,
        &mut failed,
    );
    collect(
        "antigravity",
        super::antigravity::discover_hits,
        &mut hits,
        &mut scanned,
        &mut failed,
    );

    let mut truncated = false;
    let mut by_key: HashMap<String, Accum> = HashMap::new();
    for hit in hits {
        let Some(root) = plausible_root(&hit.cwd) else {
            continue;
        };
        let key = canonicalize_for_match(&root).to_ascii_lowercase();
        if key.is_empty() {
            continue;
        }
        if !by_key.contains_key(&key) && by_key.len() >= MAX_CANDIDATES * 4 {
            truncated = true;
            continue;
        }
        let entry = by_key.entry(key).or_insert_with(|| Accum {
            display: root,
            ..Accum::default()
        });
        entry.session_count = entry.session_count.saturating_add(hit.count.max(1));
        entry.agents.insert(hit.agent_id.to_string());
        entry.last_active_at = newer(entry.last_active_at.take(), hit.last_active_at);
    }

    let mut projects: Vec<DiscoveredProject> = by_key
        .into_values()
        .map(|acc| {
            let mut agents: Vec<String> = acc.agents.into_iter().collect();
            agents.sort();
            DiscoveredProject {
                name: project_name(&acc.display),
                root_path: acc.display,
                session_count: acc.session_count,
                agents,
                last_active_at: acc.last_active_at,
            }
        })
        .collect();
    projects.sort_by(|a, b| {
        let ta = a.last_active_at.as_deref().map(recency_ms).unwrap_or(0);
        let tb = b.last_active_at.as_deref().map(recency_ms).unwrap_or(0);
        tb.cmp(&ta)
            .then_with(|| b.session_count.cmp(&a.session_count))
            .then_with(|| {
                a.root_path
                    .to_ascii_lowercase()
                    .cmp(&b.root_path.to_ascii_lowercase())
            })
    });
    if projects.len() > MAX_CANDIDATES {
        projects.truncate(MAX_CANDIDATES);
        truncated = true;
    }

    if !failed.is_empty() {
        crate::debug_log::append(
            "discovery",
            "warn",
            "",
            &format!(
                "some session stores could not be read: {}",
                failed.join(", ")
            ),
            None,
        );
    }

    DiscoveryReport {
        projects,
        scanned_sources: scanned,
        failed_sources: failed,
        truncated,
    }
}

fn collect(
    name: &'static str,
    scan: fn() -> Vec<DiscoveryHit>,
    hits: &mut Vec<DiscoveryHit>,
    scanned: &mut Vec<String>,
    failed: &mut Vec<String>,
) {
    scanned.push(name.to_string());
    match std::thread::spawn(scan).join() {
        Ok(mut rows) => {
            if rows.len() > MAX_HITS_PER_SOURCE {
                rows.truncate(MAX_HITS_PER_SOURCE);
            }
            hits.append(&mut rows);
        }
        Err(_) => failed.push(name.to_string()),
    }
}

/// A root worth offering: an existing absolute directory that is not a
/// system / home / agent-store location. Returns the normalized display path.
fn plausible_root(raw: &str) -> Option<String> {
    let path = normalize_path(raw);
    if path.is_empty() || !Path::new(&path).is_dir() {
        return None;
    }
    if !is_plausible_project_root(&path) {
        return None;
    }
    Some(path)
}

pub(crate) fn is_plausible_project_root(raw: &str) -> bool {
    let path = normalize_path(raw);
    if path.is_empty() {
        return false;
    }
    let is_absolute = (path.len() >= 2 && path.as_bytes()[1] == b':')
        || path.starts_with('/')
        || path.starts_with("\\\\");
    if !is_absolute {
        return false;
    }

    let fwd = path.replace('\\', "/");
    let lower = fwd.to_ascii_lowercase();
    let comps: Vec<&str> = lower.split('/').filter(|s| !s.is_empty()).collect();

    if let Some(first) = comps.first() {
        if first.len() == 2 && first.ends_with(':') {
            // Windows drive: `C:/` itself is not a project.
            if comps.len() < 2 {
                return false;
            }
        } else if lower.starts_with("//") {
            // UNC: `//server/share` itself is not a project.
            if comps.len() < 3 {
                return false;
            }
        } else if comps.len() < 2 {
            return false;
        }
    } else {
        return false;
    }

    if lower.contains("/.marionette") {
        return false;
    }

    const BLOCKED_PREFIXES: &[&str] = &[
        "c:/windows",
        "c:/program files",
        "c:/program files (x86)",
        "c:/programdata",
        "c:/$recycle.bin",
        "/usr",
        "/etc",
        "/var",
        "/private",
        "/system",
        "/library",
    ];
    for prefix in BLOCKED_PREFIXES {
        if lower == *prefix || lower.starts_with(&format!("{prefix}/")) {
            return false;
        }
    }

    if let Some(home) = home_dir() {
        let h = home
            .to_string_lossy()
            .replace('\\', "/")
            .trim_end_matches('/')
            .to_ascii_lowercase();
        if !h.is_empty() {
            if lower == h {
                return false;
            }
            if let Some(rel) = lower.strip_prefix(&format!("{h}/")) {
                let first = rel.split('/').next().unwrap_or("");
                const AGENT_STORE_DIRS: &[&str] = &[
                    "appdata",
                    ".marionette",
                    ".claude",
                    ".codex",
                    ".grok",
                    ".gemini",
                    ".pi",
                    ".omp",
                    ".oh-my-pi",
                    ".dsh",
                    ".zcode",
                    ".cursor",
                    ".cline",
                    ".kimi-code",
                    ".codebuddy",
                    ".openclaw",
                    ".local",
                    ".config",
                    ".cache",
                    "node_modules",
                ];
                if AGENT_STORE_DIRS.contains(&first) {
                    return false;
                }
            }
        }
    }

    // `C:/Users/<someone>` is somebody's home, never a project root.
    if comps.len() == 3
        && comps[0].len() == 2
        && comps[0].ends_with(':')
        && comps[1].eq_ignore_ascii_case("users")
    {
        return false;
    }

    true
}

fn newer(a: Option<String>, b: Option<String>) -> Option<String> {
    match (a, b) {
        (None, None) => None,
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (Some(a), Some(b)) => {
            if recency_ms(&b) > recency_ms(&a) {
                Some(b)
            } else {
                Some(a)
            }
        }
    }
}

fn project_name(path: &str) -> String {
    path.trim_end_matches(['\\', '/'])
        .rsplit(['\\', '/'])
        .find(|part| !part.is_empty())
        .unwrap_or(path)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_system_and_store_roots() {
        for bad in [
            "",
            "C:\\",
            "C:\\Windows",
            "C:\\Program Files\\Foo",
            "C:\\Users\\someone",
            "relative\\path",
        ] {
            assert!(
                !is_plausible_project_root(bad),
                "{bad} should not be a candidate"
            );
        }
        assert!(is_plausible_project_root("D:\\Projects\\Marionette"));
        assert!(is_plausible_project_root("D:\\src"));
    }

    #[test]
    fn newer_prefers_the_more_recent_stamp() {
        assert_eq!(
            newer(
                Some("2026-08-24T15:52:48Z".into()),
                Some("2026-08-24T16:10:48Z".into())
            ),
            Some("2026-08-24T16:10:48Z".into())
        );
        assert_eq!(newer(None, None), None);
    }

    #[test]
    fn project_name_takes_the_last_segment() {
        assert_eq!(project_name("D:\\Projects\\Foo\\"), "Foo");
        assert_eq!(project_name("/home/me/bar"), "bar");
    }
}
