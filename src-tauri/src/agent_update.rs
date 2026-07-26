//! Installed vs published version for the agent CLIs.
//!
//! AgentShell launches whatever `opencode` / `codex-acp` / … happens to be on
//! PATH, so without this the user has no way to tell which build they are
//! talking to, and an install from three weeks ago looks identical to a fresh
//! one. Everything here runs off the main thread: `--version` shells out and
//! the registry lookup is network I/O.

use crate::models::AgentConfig;
use serde::Serialize;
use serde_json::Value;
use std::process::Command;
use std::time::Duration;

/// Registry lookups are a nicety — never let one stall a launch.
const REGISTRY_TIMEOUT: Duration = Duration::from_secs(8);
/// A CLI that will answer `--version` answers fast.
const VERSION_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentVersionInfo {
    pub id: String,
    /// npm package name, when AgentShell knows how to install this agent.
    pub package: Option<String>,
    pub installed: Option<String>,
    pub latest: Option<String>,
    pub update_available: bool,
    /// Why a field is empty (offline, no `--version`, manual install, …).
    pub note: Option<String>,
}

/// Ask the CLI what it is. Best-effort: agents disagree about `--version`, and
/// a missing answer is normal rather than an error worth surfacing loudly.
pub fn installed_version(command: &str) -> Option<String> {
    let resolved = crate::process_util::resolve_spawn_command(command).ok()?;
    let mut cmd = Command::new(&resolved.program);
    resolved.apply_to(&mut cmd);
    cmd.arg("--version");
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    parse_version(&output_with_timeout(cmd, VERSION_TIMEOUT)?)
}

/// `Command::output()` waits forever, and `--version` is being handed to a
/// third-party CLI we do not control. One that never answers would otherwise
/// stall every remaining agent behind it on this thread — so bound the wait and
/// treat silence as "version unknown".
fn output_with_timeout(mut cmd: Command, wait: Duration) -> Option<String> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let text = cmd.output().ok().map(|out| {
            format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            )
        });
        let _ = tx.send(text);
    });
    rx.recv_timeout(wait).ok().flatten()
}

/// First semver-looking token in `--version` output.
///
/// CLIs pad this with names, banners and ANSI colour, so anchoring on the shape
/// of the number is far more durable than trying to match each agent's format.
pub fn parse_version(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if !bytes[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        // A preceding digit/dot means we are mid-token — skip to its end.
        if i > 0 && (bytes[i - 1].is_ascii_digit() || bytes[i - 1] == b'.') {
            i += 1;
            continue;
        }
        let start = i;
        let mut dots = 0;
        while i < bytes.len() {
            let c = bytes[i];
            if c.is_ascii_digit() {
                i += 1;
            } else if c == b'.' && i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit() {
                dots += 1;
                i += 1;
            } else {
                break;
            }
        }
        if dots >= 2 {
            let mut end = i;
            // Keep a prerelease/build suffix: 1.2.3-beta.1
            if end < bytes.len() && (bytes[end] == b'-' || bytes[end] == b'+') {
                end += 1;
                while end < bytes.len()
                    && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'.' || bytes[end] == b'-')
                {
                    end += 1;
                }
            }
            return Some(text[start..end].to_string());
        }
    }
    None
}

fn latest_version(package: &str) -> Option<String> {
    let url = format!("https://registry.npmjs.org/{package}/latest");
    let resp = ureq::get(&url)
        .set("Accept", "application/json")
        .set("User-Agent", "AgentShell/0.1 (agent-update)")
        .timeout(REGISTRY_TIMEOUT)
        .call()
        .ok()?;
    let value: Value = resp.into_json().ok()?;
    value
        .get("version")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Compare two dotted versions numerically; non-numeric tails lose to numbers.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    let core = |v: &str| -> Vec<u64> {
        v.split(['-', '+'])
            .next()
            .unwrap_or(v)
            .split('.')
            .map(|part| part.parse::<u64>().unwrap_or(0))
            .collect()
    };
    let (a, b) = (core(candidate), core(current));
    for i in 0..a.len().max(b.len()) {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        if x != y {
            return x > y;
        }
    }
    // Equal cores: a release outranks the prerelease of the same number.
    !candidate.contains('-') && current.contains('-')
}

pub fn version_info(agent: &AgentConfig, check_registry: bool) -> AgentVersionInfo {
    let installed = installed_version(&agent.command);
    let package = agent.install.package.clone();

    let latest = match (&package, check_registry) {
        (Some(pkg), true) => latest_version(pkg),
        _ => None,
    };

    let update_available = match (&installed, &latest) {
        (Some(installed), Some(latest)) => is_newer(latest, installed),
        _ => false,
    };

    let note = if installed.is_none() {
        Some("This CLI does not report `--version`".to_string())
    } else if package.is_none() {
        Some("Installed outside npm — update with its own installer".to_string())
    } else if check_registry && latest.is_none() {
        Some("Could not reach the npm registry".to_string())
    } else {
        None
    };

    AgentVersionInfo {
        id: agent.id.clone(),
        package,
        installed,
        latest,
        update_available,
        note,
    }
}

pub fn all_version_info(check_registry: bool) -> Vec<AgentVersionInfo> {
    AgentConfig::defaults()
        .iter()
        .map(|agent| version_info(agent, check_registry))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_version_out_of_noisy_cli_output() {
        assert_eq!(parse_version("1.2.3").as_deref(), Some("1.2.3"));
        assert_eq!(parse_version("opencode 0.14.7\n").as_deref(), Some("0.14.7"));
        assert_eq!(
            parse_version("codex-acp v2.10.0-beta.3 (build 99)").as_deref(),
            Some("2.10.0-beta.3")
        );
        // A two-part number is not a version — keep looking.
        assert_eq!(parse_version("node 22 / tool 3.4.5").as_deref(), Some("3.4.5"));
        assert_eq!(parse_version("no numbers here"), None);
        assert_eq!(parse_version("only 12 and 3.4"), None);
    }

    #[test]
    fn a_cli_that_never_answers_version_does_not_hang_the_check() {
        // `timeout /t` on Windows, `sleep` elsewhere — either way it outlives
        // the bound, so the call must give up rather than block.
        let mut cmd = if cfg!(windows) {
            let mut c = Command::new("cmd");
            c.args(["/C", "ping -n 30 127.0.0.1 > NUL"]);
            c
        } else {
            let mut c = Command::new("sleep");
            c.arg("30");
            c
        };
        cmd.stdin(std::process::Stdio::null());

        let started = std::time::Instant::now();
        let got = output_with_timeout(cmd, Duration::from_millis(300));

        assert!(got.is_none(), "a silent CLI must report no version");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "must give up at the bound, waited {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn compares_versions_numerically_not_lexically() {
        assert!(is_newer("0.10.0", "0.9.9"), "10 > 9 even though '1' < '9'");
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(!is_newer("1.2.3", "1.2.3"));
        assert!(!is_newer("1.2.2", "1.2.3"));
        assert!(is_newer("1.2", "1.1.9"), "short versions pad with zeros");
        assert!(is_newer("1.2.3", "1.2.3-beta.1"), "release beats its prerelease");
        assert!(!is_newer("1.2.3-beta.1", "1.2.3"));
    }
}
