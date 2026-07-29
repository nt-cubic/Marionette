//! Lightweight agent environment preflight (Codeg-aligned checks, no binary_cache yet).
//!
//! Used by agent menu status so users get readable Fail/Warn before session/new.

use crate::agent_registry::{self, DistributionKind};
use crate::process_util;
use serde::Serialize;
use std::process::Command;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum CheckStatus {
    Pass,
    Fail,
    Warn,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckItem {
    pub check_id: String,
    pub label: String,
    pub status: CheckStatus,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreflightResult {
    pub agent_id: String,
    pub agent_name: String,
    pub passed: bool,
    pub checks: Vec<CheckItem>,
}

/// Run preflight for one agent id (PATH + runtime tools + deps from registry).
///
/// `command_override`: for custom agents whose CLI name is not in the pin table.
pub fn run_preflight(agent_id: &str) -> PreflightResult {
    run_preflight_with_cmd(agent_id, None)
}

pub fn run_preflight_with_cmd(agent_id: &str, command_override: Option<&str>) -> PreflightResult {
    let meta = agent_registry::harness_meta(agent_id);
    let cmd = command_override.unwrap_or(meta.cmd);
    let label = if meta.id == "unknown" {
        agent_id
    } else {
        meta.label
    };
    let mut checks = Vec::new();

    // 1) Primary command on PATH
    checks.push(command_on_path(
        "agent_cmd",
        cmd,
        &format!("{label} ACP command"),
    ));

    // 2) Required sibling CLIs
    for cmd in meta.requires_commands {
        checks.push(command_on_path(
            &format!("dep_{cmd}"),
            cmd,
            &format!("Required CLI `{cmd}`"),
        ));
    }

    // 3) Runtime by distribution
    match meta.distribution {
        DistributionKind::Npx => {
            checks.extend(check_node_npm(meta.node_required));
        }
        DistributionKind::Uvx => {
            checks.extend(check_uv(meta.uv_required));
            if command_missing("hermes") && command_missing("uvx") && command_missing("uv") {
                // already covered by individual checks; add summary warn if uv missing
            }
            if let Some(py) = meta.python_pin {
                checks.push(CheckItem {
                    check_id: "python_pin".into(),
                    label: "Python pin".into(),
                    status: CheckStatus::Pass,
                    message: format!(
                        "uvx should use --python {py} (Hermes requires <3.14)"
                    ),
                });
            }
        }
        DistributionKind::Binary | DistributionKind::Manual => {
            // binary_cache / vendor installer later — only PATH matters for now
        }
    }

    // Cursor: never suggest installing a global `agent` that fights Grok.
    if meta.id == "cursor" {
        checks.push(CheckItem {
            check_id: "cursor_no_agent_collision".into(),
            label: "Cursor identity".into(),
            status: CheckStatus::Pass,
            message: "Uses `cursor-agent` only — do not install a global `agent` binary (collides with Grok)".into(),
        });
    }

    let passed = checks
        .iter()
        .all(|c| !matches!(c.status, CheckStatus::Fail));

    PreflightResult {
        agent_id: agent_id.to_string(),
        agent_name: label.to_string(),
        passed,
        checks,
    }
}

fn command_missing(cmd: &str) -> bool {
    matches!(
        process_util::resolve_command_display_path(cmd),
        Ok(None) | Err(_)
    )
}

fn command_on_path(check_id: &str, cmd: &str, label: &str) -> CheckItem {
    match process_util::resolve_command_display_path(cmd) {
        Ok(Some(path)) => CheckItem {
            check_id: check_id.into(),
            label: label.into(),
            status: CheckStatus::Pass,
            message: format!("`{cmd}` → {path}"),
        },
        Ok(None) => CheckItem {
            check_id: check_id.into(),
            label: label.into(),
            status: CheckStatus::Fail,
            message: format!("`{cmd}` not found on PATH"),
        },
        Err(e) => CheckItem {
            check_id: check_id.into(),
            label: label.into(),
            status: CheckStatus::Fail,
            message: format!("lookup `{cmd}` failed: {e}"),
        },
    }
}

fn check_node_npm(node_required: Option<&str>) -> Vec<CheckItem> {
    let mut out = Vec::new();
    let node_ver = run_version("node", &["--version"]);
    match &node_ver {
        Some(v) => {
            out.push(CheckItem {
                check_id: "node_available".into(),
                label: "Node.js".into(),
                status: CheckStatus::Pass,
                message: format!("Node.js {v}"),
            });
            if let Some(req) = node_required {
                out.push(version_at_least_check(
                    "node_version",
                    "Node version",
                    v.trim_start_matches('v'),
                    req,
                ));
            }
        }
        None => out.push(CheckItem {
            check_id: "node_available".into(),
            label: "Node.js".into(),
            status: CheckStatus::Fail,
            message: "Node.js not found (needed for npm/npx agents)".into(),
        }),
    }

    match run_version("npm", &["--version"]) {
        Some(v) => out.push(CheckItem {
            check_id: "npm_available".into(),
            label: "npm".into(),
            status: CheckStatus::Pass,
            message: format!("npm {v}"),
        }),
        None => out.push(CheckItem {
            check_id: "npm_available".into(),
            label: "npm".into(),
            status: CheckStatus::Fail,
            message: "npm not found".into(),
        }),
    }

    out
}

fn check_uv(uv_required: Option<&str>) -> Vec<CheckItem> {
    let mut out = Vec::new();
    // `uv` or `uvx` on PATH is enough to install/run Hermes.
    let uv_ver = run_version("uv", &["--version"]).or_else(|| {
        run_version("uvx", &["--version"]).map(|s| {
            // uvx --version may say "uvx 0.x" — keep raw
            s
        })
    });
    match uv_ver {
        Some(v) => {
            out.push(CheckItem {
                check_id: "uv_available".into(),
                label: "uv / uvx".into(),
                status: CheckStatus::Pass,
                message: v,
            });
            if let Some(req) = uv_required {
                // Best-effort parse first semver-like token
                if let Some(sem) = first_semver_token(
                    &out
                        .last()
                        .map(|c| c.message.clone())
                        .unwrap_or_default(),
                ) {
                    out.push(version_at_least_check(
                        "uv_version",
                        "uv version",
                        &sem,
                        req,
                    ));
                }
            }
        }
        None => out.push(CheckItem {
            check_id: "uv_available".into(),
            label: "uv / uvx".into(),
            status: CheckStatus::Fail,
            message: "uv/uvx not found — install https://docs.astral.sh/uv/ to run Hermes via uvx"
                .into(),
        }),
    }
    out
}

fn run_version(cmd: &str, args: &[&str]) -> Option<String> {
    let path = process_util::resolve_command_display_path(cmd)
        .ok()
        .flatten()?;
    let output = Command::new(&path).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let err = String::from_utf8_lossy(&output.stderr);
    let line = if !text.trim().is_empty() {
        text
    } else {
        err
    };
    Some(line.lines().next().unwrap_or("").trim().to_string())
}

fn version_at_least_check(
    check_id: &str,
    label: &str,
    actual: &str,
    required: &str,
) -> CheckItem {
    match (parse_semver(actual), parse_semver(required)) {
        (Some(a), Some(r)) if a >= r => CheckItem {
            check_id: check_id.into(),
            label: label.into(),
            status: CheckStatus::Pass,
            message: format!("{actual} ≥ {required}"),
        },
        (Some(_), Some(_)) => CheckItem {
            check_id: check_id.into(),
            label: label.into(),
            status: CheckStatus::Fail,
            message: format!("{actual} < required {required}"),
        },
        _ => CheckItem {
            check_id: check_id.into(),
            label: label.into(),
            status: CheckStatus::Warn,
            message: format!("could not compare {actual} vs {required}"),
        },
    }
}

fn parse_semver(s: &str) -> Option<(u64, u64, u64)> {
    let cleaned = s.trim().trim_start_matches('v');
    let mut parts = cleaned.split(|c: char| !c.is_ascii_digit() && c != '.');
    let core = parts.next().unwrap_or("");
    let mut nums = core.split('.');
    let major = nums.next()?.parse().ok()?;
    let minor = nums.next().unwrap_or("0").parse().unwrap_or(0);
    let patch = nums.next().unwrap_or("0").parse().unwrap_or(0);
    Some((major, minor, patch))
}

fn first_semver_token(s: &str) -> Option<String> {
    for token in s.split_whitespace() {
        let t = token.trim_start_matches('v');
        if parse_semver(t).is_some() {
            return Some(t.to_string());
        }
        // "uv 0.6.1" style — skip word, next may be version
    }
    // try any substring matching N.N
    for word in s.split(|c: char| c.is_whitespace() || c == ',') {
        let t = word.trim_start_matches('v');
        if parse_semver(t).is_some() {
            return Some(t.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_compare() {
        assert!(parse_semver("22.19.0").unwrap() >= parse_semver("22.0.0").unwrap());
        assert!(parse_semver("v20.0.0").unwrap() < parse_semver("22.0.0").unwrap());
    }

    #[test]
    fn preflight_unknown_still_returns() {
        let r = run_preflight("no-such-agent-xyz");
        assert!(!r.checks.is_empty());
    }
}
