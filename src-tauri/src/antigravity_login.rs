//! One-shot Google login driver for the Antigravity ACP server (issue #14).
//!
//! Antigravity has no `login` CLI subcommand — its sign-in is the ACP
//! top-level `authenticate` request: the server prints a consent URL and
//! opens the default browser itself (verified in M0,
//! docs/research/research-antigravity.md §三/§M0). This module drives a
//! throwaway server process through `initialize` → `authenticate`, lets the
//! user complete the browser flow, and exits; the credential lands exactly
//! where real sessions look for it (`<GEMINI_HOME>/antigravity-acp/
//! acp_token.json`), so the UI's auth poll picks the change up by probing
//! the file — no extra events needed.
//!
//! The child is a PyInstaller onefile like every spawn of this server: it
//! gets the per-launch TMP/TEMP scratch redirect and a kill-on-close Job
//! Object, same red lines as `acp.rs`'s session launches.

use std::io::{BufRead, BufReader, Write};
use std::process::{ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// `initialize` must answer within this (cold unpack of the onefile archive).
const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(60);
/// `authenticate` resolves only after the user finishes the browser consent
/// (the server itself waits up to 300s — stay just under).
const AUTHENTICATE_TIMEOUT: Duration = Duration::from_secs(240);

/// Kick off the login flow on a background thread; returns immediately so
/// the button handler can show its hint and start polling the probe.
pub fn start_login() -> Result<serde_json::Value, String> {
    let exe = resolve_server_exe()?;
    // settings.json auth.type must exist or the server refuses to auth at
    // all (-32000) — same sync the session spawn path performs.
    if let crate::antigravity_support::SettingsSync::WriteFailed(ref error) =
        crate::antigravity_support::ensure_settings_auth_type()
    {
        return Err(format!("settings sync failed: {error}"));
    }
    let scratch = crate::antigravity_support::create_scratch(&format!(
        "login-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0)
    ));
    std::thread::Builder::new()
        .name("agy-login".into())
        .spawn(move || {
            let result = drive(&exe, scratch.as_deref());
            match &result {
                Ok(()) => crate::debug_log::append(
                    "auth",
                    "info",
                    "",
                    "antigravity login completed (token written)",
                    None,
                ),
                Err(error) => crate::debug_log::append(
                    "auth",
                    "warn",
                    "",
                    "antigravity login driver failed",
                    Some(error),
                ),
            }
        })
        .map_err(|e| format!("spawn login driver failed: {e}"))?;
    Ok(serde_json::json!({
        "started": true,
        "message": crate::ts!(
            "已发起 Google 登录——在浏览器里完成授权后回到这里，状态会自动刷新。",
            "Google login started — complete the authorization in your browser, then return here; the status refreshes on its own."
        ),
    }))
}

/// Installed tree first, PATH second — but never a download (a settings-panel
/// click must not silently pull ~105 MB).
fn resolve_server_exe() -> Result<std::path::PathBuf, String> {
    if let Some(entry) = crate::antigravity_support::installed_entry() {
        return Ok(entry);
    }
    let resolved = crate::process_util::resolve_spawn_command("agy_acp_server").map_err(
        |error| {
            format!(
                "{} ({error})",
                crate::ts!(
                    "未找到 agy_acp_server——先在 Agent 菜单里安装 Antigravity，或把解压目录加入 PATH",
                    "agy_acp_server not found — install Antigravity from the agent menu first, or put its folder on PATH"
                )
            )
        },
    )?;
    Ok(std::path::PathBuf::from(&resolved.program))
}

fn drive(exe: &std::path::Path, scratch: Option<&std::path::Path>) -> Result<(), String> {
    let mut command = Command::new(exe);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("PYTHONUNBUFFERED", "1")
        .env("AGY_ACP_FORCE_FILE_STORAGE", "1");
    if let Some(scratch) = scratch {
        let value = scratch.to_string_lossy().to_string();
        command
            .env("TMP", &value)
            .env("TEMP", &value)
            .env("TMPDIR", &value);
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command.spawn().map_err(|e| format!("spawn server: {e}"))?;
    #[cfg(target_os = "windows")]
    let _job = crate::process_util::KillJobGuard::create(&child, "agy-login");
    let mut stdin = child.stdin.take().ok_or("stdin unavailable")?;
    let stdout = child.stdout.take().ok_or("stdout unavailable")?;
    let stderr = child.stderr.take().ok_or("stderr unavailable")?;
    // The consent URL moved between stdout/stderr across server versions, so
    // both pipes feed one line stream (M0 §M0: 1.5.0 prints it on stderr).
    let (lines_tx, lines_rx) = mpsc::channel::<String>();
    let stdout_tx = lines_tx.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().flatten() {
            if stdout_tx.send(line).is_err() {
                return;
            }
        }
    });
    let stderr_tx = lines_tx;
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().flatten() {
            if stderr_tx.send(line).is_err() {
                return;
            }
        }
    });

    let result = run_handshake(&mut stdin, lines_rx);
    let _ = child.kill();
    let _ = child.wait();
    if let Some(scratch) = scratch {
        crate::antigravity_support::cleanup_scratch(scratch);
    }
    result
}

fn send_jsonrpc(
    stdin: &mut ChildStdin,
    id: u64,
    method: &str,
    params: serde_json::Value,
) -> Result<(), String> {
    let payload = serde_json::json!({
        "jsonrpc": "2.0", "id": id, "method": method, "params": params
    });
    let mut line =
        serde_json::to_string(&payload).map_err(|e| format!("serialize {method}: {e}"))?;
    line.push('\n');
    stdin
        .write_all(line.as_bytes())
        .map_err(|e| format!("write {method}: {e}"))?;
    stdin.flush().map_err(|e| format!("flush {method}: {e}"))
}

/// initialize → authenticate; scrapes the consent URL along the way (the
/// server opens the browser itself — the captured URL is only the support
/// trail for "browser did not open" cases).
fn run_handshake(stdin: &mut ChildStdin, lines: mpsc::Receiver<String>) -> Result<(), String> {
    send_jsonrpc(
        stdin,
        1,
        "initialize",
        serde_json::json!({
            "protocolVersion": 1,
            "clientCapabilities": { "fs": { "readTextFile": false, "writeTextFile": false } },
            "clientInfo": { "name": "marionette-login", "version": env!("CARGO_PKG_VERSION") },
        }),
    )?;
    wait_response(&lines, 1, Instant::now() + INITIALIZE_TIMEOUT, None)?;

    // The free-tier method is the first advertised (M0: oauth-personal).
    send_jsonrpc(
        stdin,
        2,
        "authenticate",
        serde_json::json!({ "methodId": "oauth-personal" }),
    )?;
    let mut auth_url: Option<String> = None;
    let mut scrape = |line: &str| {
        if auth_url.is_none() {
            if let Some(url) = crate::antigravity_support::extract_auth_url(line) {
                auth_url = Some(url.to_string());
            }
        }
    };
    wait_response(
        &lines,
        2,
        Instant::now() + AUTHENTICATE_TIMEOUT,
        Some(&mut scrape),
    )?;
    if let Some(url) = auth_url {
        crate::debug_log::append("auth", "info", "", "antigravity consent url", Some(&url));
    }
    Ok(())
}

/// Consume lines until the JSON-RPC response for `id` arrives (its `result`
/// or `error` both count — an error carries the server's own reason). EOF
/// before any response = the process died.
fn wait_response(
    lines: &mpsc::Receiver<String>,
    id: u64,
    deadline: Instant,
    mut on_line: Option<&mut dyn FnMut(&str)>,
) -> Result<(), String> {
    loop {
        let timeout = deadline
            .checked_duration_since(Instant::now())
            .unwrap_or(Duration::ZERO);
        if timeout.is_zero() {
            return Err(format!("timed out waiting for response id={id}"));
        }
        let line = lines
            .recv_timeout(timeout)
            .map_err(|_| format!("timed out waiting for response id={id}"))?;
        if let Some(hook) = on_line.as_mut() {
            hook(&line);
        }
        let trimmed = line.trim();
        if !trimmed.starts_with('{') {
            continue;
        }
        let Ok(message) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue;
        };
        if message.get("id").and_then(serde_json::Value::as_u64) != Some(id) {
            continue;
        }
        if let Some(error) = message.get("error") {
            let reason = error
                .pointer("/message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown error");
            return Err(format!("server rejected the login: {reason}"));
        }
        return Ok(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wait_response_times_out_and_parses() {
        let (tx, rx) = mpsc::channel::<String>();
        // Non-JSON noise first (server log lines), then the response.
        tx.send("I1119 main.py:124] noise".to_string()).unwrap();
        tx.send(r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1}}"#.to_string())
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        wait_response(&rx, 1, deadline, None).expect("id=1 response");

        // Wrong id must be skipped; error shape must surface the reason.
        tx.send(r#"{"jsonrpc":"2.0","id":9,"result":{}}"#.to_string())
            .unwrap();
        tx.send(r#"{"jsonrpc":"2.0","id":2,"error":{"message":"auth failed"}}"#.to_string())
            .unwrap();
        let err = wait_response(&rx, 2, Instant::now() + Duration::from_secs(2), None).unwrap_err();
        assert!(err.contains("auth failed"), "{err}");

        // Deadline already passed → immediate timeout, no panic.
        let err = wait_response(&rx, 3, Instant::now() - Duration::from_secs(1), None).unwrap_err();
        assert!(err.contains("timed out"), "{err}");
    }

    /// Opt-in real-machine oracle: drives the actual server through
    /// initialize → authenticate. With a cached token the authenticate call
    /// resolves quickly (already signed in); without one it waits for the
    /// browser flow — only run this interactively.
    /// `cargo test --bin marionette live_login_driver -- --ignored --nocapture`
    #[test]
    #[ignore = "spawns the real agy_acp_server; needs a cached token to finish fast"]
    fn live_login_driver_completes() {
        let exe = resolve_server_exe().expect("installed agy server");
        println!("server: {}", exe.display());
        let scratch = crate::antigravity_support::create_scratch("login-oracle");
        let outcome = drive(&exe, scratch.as_deref());
        println!("driver outcome: {outcome:?}");
        assert!(outcome.is_ok(), "{outcome:?}");
    }

    #[test]
    fn line_hook_sees_noise_before_response() {
        let (tx, rx) = mpsc::channel::<String>();
        tx.send(format!(
            "I1119 x] {}https://accounts.google.com/o/oauth2/v2/auth?x=1",
            crate::antigravity_support::AUTH_URL_MARKER
        ))
        .unwrap();
        tx.send(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#.to_string())
            .unwrap();
        let seen = std::cell::RefCell::new(None::<String>);
        let mut hook = |line: &str| {
            if let Some(url) = crate::antigravity_support::extract_auth_url(line) {
                *seen.borrow_mut() = Some(url.to_string());
            }
        };
        wait_response(
            &rx,
            1,
            Instant::now() + Duration::from_secs(2),
            Some(&mut hook),
        )
        .unwrap();
        assert_eq!(
            seen.into_inner().as_deref(),
            Some("https://accounts.google.com/o/oauth2/v2/auth?x=1")
        );
    }
}
