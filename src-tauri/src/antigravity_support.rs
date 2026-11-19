//! Google Antigravity (`agy_acp_server`) host-side support.
//!
//! Three responsibilities, all documented in
//! `docs/research/research-antigravity.md` (M0 verified 2026-11-19 against
//! agy-acp-server 1.5.0):
//!
//! 1. **settings.json auth sync** — the server refuses `session/new` with
//!    `-32000 Authentication required` unless `<GEMINI_HOME>/antigravity-acp/
//!    settings.json` carries an `auth.type`. Marionette writes the free-tier
//!    default (`oauth-personal`) before spawn. The write is read-modify-write
//!    on that single key and **fail-closed**: the server parses Hjson while we
//!    only write strict JSON, so a file we cannot parse is left untouched
//!    (it may carry hand-written comments or gcp keys that belong to the user).
//!
//! 2. **Per-launch scratch directories** — the Windows build is a PyInstaller
//!    *onefile* that unpacks ~150 MB (1.5.0; earlier builds 1.17 GB) into the
//!    temp dir on **every launch** and only cleans up on a graceful exit.
//!    Marionette ends agents with a kill-tree, so that cleanup never runs.
//!    The bootloader resolves its extraction root via `GetTempPathW`
//!    (`TMP`→`TEMP`→`USERPROFILE`), so pointing `TMP`/`TEMP`/`TMPDIR` at a
//!    directory we own turns the leak into a directory we delete. Applied to
//!    **this agent only**: lending `TMP` to agents that run MSYS2 shells
//!    hijacks the user's Git Bash `/tmp` (codeg #1025, see research §四).
//!
//! 3. **Auth URL capture** — with `auth.type` set, the server triggers the
//!    Google OAuth loopback flow by itself (it prints the consent URL and
//!    opens the default browser). We watch stderr for the marker line and
//!    surface it as a notice card so sign-in is still possible when the
//!    browser did not open (remote desktop, no default browser).

use std::fs;
use std::path::{Path, PathBuf};

pub const AGENT_ID: &str = "antigravity";

/// Marker the server prints next to the consent URL (stderr on 1.5.0; the
/// pipe moved between stdout/stderr across versions, so both are scanned).
pub const AUTH_URL_MARKER: &str = "Open the following link to authenticate the ACP server: ";

/// Subdirectory of the Gemini home the ACP server owns.
const ACP_SUBDIR: &str = "antigravity-acp";

/// Free-tier default; the first of the four advertised authMethods.
const DEFAULT_AUTH_TYPE: &str = "oauth-personal";

/// Scratch dirs older than this at startup are leftovers from a crashed run.
const SCRATCH_MAX_AGE_SECS: u64 = 24 * 60 * 60;

/// `$GEMINI_HOME` (pointing at the `.gemini` directory **itself** — unlike
/// Gemini CLI's `GEMINI_CLI_HOME`, which names the parent), else `~/.gemini`.
/// Empty counts as unset; a leading `~` is expanded like upstream's
/// `os.path.expanduser` (a literal `~` directory was a real bug in codeg).
pub fn gemini_home() -> PathBuf {
    let home = std::env::var_os("GEMINI_HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .or_else(home_dir)
        .unwrap_or_else(|| PathBuf::from("."));
    expand_tilde(home)
}

/// `<GEMINI_HOME>/antigravity-acp` — settings, tokens, conversations.
pub fn antigravity_acp_dir() -> PathBuf {
    gemini_home().join(ACP_SUBDIR)
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

fn expand_tilde(path: PathBuf) -> PathBuf {
    let Some(raw) = path.to_str() else { return path };
    if raw == "~" {
        return home_dir().unwrap_or(path);
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        if let Some(home) = home_dir() {
            return home.join(rest);
        }
    }
    path
}

/// Outcome of [`ensure_settings_auth_type`] — logged, never fatal to launch.
#[derive(Debug, PartialEq, Eq)]
pub enum SettingsSync {
    /// `auth.type` written (file or key was missing).
    Written,
    /// File already carried the expected `auth.type`.
    AlreadyCurrent,
    /// File exists but is not strict JSON — left untouched (fail-closed: the
    /// server may still be able to read it as Hjson).
    UnreadableLeftAlone,
    /// IO error while writing — the server will surface auth failure itself.
    WriteFailed(String),
}

/// Make sure `<GEMINI_HOME>/antigravity-acp/settings.json` has an `auth.type`
/// (default `oauth-personal`). Read-modify-write on the single key; never
/// rewrites a file whose existing content cannot be parsed as strict JSON.
pub fn ensure_settings_auth_type() -> SettingsSync {
    ensure_settings_auth_type_in(&antigravity_acp_dir())
}

fn ensure_settings_auth_type_in(acp_dir: &Path) -> SettingsSync {
    let path = acp_dir.join("settings.json");
    let mut root: serde_json::Value = match fs::read_to_string(&path) {
        Ok(text) => match serde_json::from_str(&text) {
            Ok(value) => value,
            // The server parses Hjson; a file we cannot parse as strict JSON
            // may still be perfectly valid for it — hands off.
            Err(_) => return SettingsSync::UnreadableLeftAlone,
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => serde_json::json!({}),
        Err(err) => return SettingsSync::WriteFailed(format!("read settings.json: {err}")),
    };
    if root
        .get("auth")
        .and_then(|a| a.get("type"))
        .and_then(|t| t.as_str())
        == Some(DEFAULT_AUTH_TYPE)
    {
        return SettingsSync::AlreadyCurrent;
    }
    // Only touch `auth.type`; anything structurally unexpected is not ours
    // to reshape.
    let Some(obj) = root.as_object_mut() else {
        return SettingsSync::UnreadableLeftAlone;
    };
    let auth = obj
        .entry("auth")
        .or_insert_with(|| serde_json::json!({}));
    let Some(auth_obj) = auth.as_object_mut() else {
        return SettingsSync::UnreadableLeftAlone;
    };
    auth_obj.insert(
        "type".to_string(),
        serde_json::Value::String(DEFAULT_AUTH_TYPE.to_string()),
    );
    if let Err(err) = fs::create_dir_all(acp_dir) {
        return SettingsSync::WriteFailed(format!("create antigravity-acp dir: {err}"));
    }
    let text = match serde_json::to_string_pretty(&root) {
        Ok(text) => text,
        Err(err) => return SettingsSync::WriteFailed(format!("serialize settings: {err}")),
    };
    // Same-dir temp file + rename keeps the write atomic for the server's
    // own settings reader (which mirrors this shape upstream).
    let tmp = path.with_extension("json.marionette-tmp");
    if let Err(err) = fs::write(&tmp, text.as_bytes()) {
        return SettingsSync::WriteFailed(format!("write temp settings: {err}"));
    }
    match fs::rename(&tmp, &path) {
        Ok(()) => SettingsSync::Written,
        Err(err) => SettingsSync::WriteFailed(format!("rename settings: {err}")),
    }
}

/// Local login probe for `probe_agent_auth` (`commands.rs`).
/// Statuses match the `auth_result` contract there: `logged_in` / `logged_out`.
pub fn auth_probe() -> (&'static str, String) {
    if antigravity_acp_dir().join("acp_token.json").is_file() {
        return ("logged_in", "Google account token present".to_string());
    }
    (
        "logged_out",
        "No Google account token — starting a session opens the browser sign-in".to_string(),
    )
}

/// Extract the consent URL from a printed marker line, if this is one.
/// The URL runs to the first whitespace (nothing in a query string can carry
/// an unescaped space, and a JSON frame spliced onto the same line would
/// start after one).
pub fn extract_auth_url(line: &str) -> Option<&str> {
    let rest = &line[line.find(AUTH_URL_MARKER)? + AUTH_URL_MARKER.len()..];
    let url = rest.split_whitespace().next()?;
    url.starts_with("https://").then_some(url)
}

/// Whether `command` names the Antigravity ACP server binary. Probe paths
/// (`--version` checks) must **never execute** it: the Windows build is a
/// PyInstaller onefile, so even a `--version` that exits in milliseconds
/// unpacks ~100 MB into %TEMP% first and leaks it when the probe's timeout
/// kill lands (measured 2026-11-19: every agent-menu open cost 2–3 dirs).
pub fn is_server_command(command: &str) -> bool {
    let trimmed = command
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(command)
        .trim()
        .to_ascii_lowercase();
    let stem = trimmed.strip_suffix(".exe").unwrap_or(&trimmed);
    stem == "agy_acp_server"
}

// ── Per-launch scratch directories ──────────────────────────────────────────

/// Create the per-launch scratch dir and hand it back for env wiring.
/// `None` when the global dir is unavailable — launch proceeds without the
/// redirect (the leak returns, but a broken launch is worse).
pub fn create_scratch(session_id: &str) -> Option<PathBuf> {
    let root = crate::app_paths::global_dir().ok()?.join("scratch").join("antigravity");
    create_scratch_in(&root, session_id)
}

fn create_scratch_in(root: &Path, session_id: &str) -> Option<PathBuf> {
    let dir = root.join(session_id);
    fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// Remove a scratch dir once its process tree is gone. Best-effort: a failure
/// is left for the startup sweep to retry.
pub fn cleanup_scratch(dir: &Path) {
    let _ = fs::remove_dir_all(dir);
    // Self-tidy the now-empty chain (scratch/antigravity/<session>) but never
    // walk above the scratch parent — other agents' scratch dirs may live
    // there later. The check-then-remove race with a concurrent session is
    // harmless: its dir is never empty at that point.
    if let Some(stop) = dir.parent().and_then(|p| p.parent()) {
        remove_empty_up_to(dir, stop);
    }
}

fn remove_empty_up_to(dir: &Path, stop: &Path) {
    let mut cursor = Some(dir.to_path_buf());
    while let Some(current) = cursor {
        if current.as_path() == stop {
            break;
        }
        // A read failure here usually means the dir is already gone — keep
        // walking up so the parents still get tidied.
        let empty = fs::read_dir(&current)
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(true);
        if !empty {
            break;
        }
        let parent = current.parent().map(|p| p.to_path_buf());
        let _ = fs::remove_dir(&current);
        cursor = parent;
    }
}

/// Delete scratch dirs older than `max_age` under the real root. Called once
/// at startup; fresh dirs are never touched (a session handed over from a
/// previous instance could still own one).
pub fn sweep_stale_scratch() {
    let Ok(global) = crate::app_paths::global_dir() else { return };
    sweep_stale_in(
        &global.join("scratch").join("antigravity"),
        std::time::Duration::from_secs(SCRATCH_MAX_AGE_SECS),
    );
}

fn sweep_stale_in(root: &Path, max_age: std::time::Duration) {
    let Ok(entries) = fs::read_dir(root) else { return };
    let cutoff = std::time::SystemTime::now()
        .checked_sub(max_age)
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
    for entry in entries.flatten() {
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .map(|mtime| mtime < cutoff)
            .unwrap_or(false);
        if stale {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "marionette-antigravity-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn auth_url_extracts_to_first_whitespace() {
        let url = "https://accounts.google.com/o/oauth2/v2/auth?state=x&code_challenge=y";
        // absl prefixes its log header on stderr — the marker is matched as a
        // substring, not a prefix.
        assert_eq!(
            extract_auth_url(&format!("I1119 main.py:124] {AUTH_URL_MARKER}{url} trailing")),
            Some(url)
        );
        assert_eq!(extract_auth_url("unrelated stderr"), None);
        assert_eq!(
            extract_auth_url(&format!("{AUTH_URL_MARKER}http://not-https/")),
            None
        );
    }

    #[test]
    fn settings_sync_creates_rewrites_and_leaves_unreadable_alone() {
        let acp_dir = tempdir("settings");
        let settings = acp_dir.join("settings.json");

        assert_eq!(
            ensure_settings_auth_type_in(&acp_dir),
            SettingsSync::Written
        );
        let v: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&settings).unwrap()).unwrap();
        assert_eq!(v["auth"]["type"], DEFAULT_AUTH_TYPE);

        // Existing keys survive; only auth.type is touched.
        fs::write(
            &settings,
            r#"{"auth":{"type":"gemini-api-key"},"gcp":{"project":"p"}}"#,
        )
        .unwrap();
        assert_eq!(
            ensure_settings_auth_type_in(&acp_dir),
            SettingsSync::Written
        );
        let v: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&settings).unwrap()).unwrap();
        assert_eq!(v["auth"]["type"], DEFAULT_AUTH_TYPE);
        assert_eq!(v["gcp"]["project"], "p");

        assert_eq!(
            ensure_settings_auth_type_in(&acp_dir),
            SettingsSync::AlreadyCurrent
        );

        // Hjson (hand-written comments) → fail-closed, file untouched.
        fs::write(&settings, "// hand-written\n{ auth.type = \"gateway\" }").unwrap();
        assert_eq!(
            ensure_settings_auth_type_in(&acp_dir),
            SettingsSync::UnreadableLeftAlone
        );
        assert!(fs::read_to_string(&settings).unwrap().starts_with("//"));
        let _ = fs::remove_dir_all(&acp_dir);
    }

    #[test]
    fn server_command_match_is_stem_exact() {
        // Matches (path-stripped, case-insensitive, optional .exe).
        assert!(is_server_command("agy_acp_server"));
        assert!(is_server_command("agy_acp_server.exe"));
        assert!(is_server_command(r"D:\tools\agy\AGY_ACP_SERVER.EXE"));
        assert!(is_server_command("dist/agy_acp_server"));
        // Near-miss names must not match (measured-leak guard stays tight).
        assert!(!is_server_command("agy_acp_server_real.exe"));
        assert!(!is_server_command("agy_acp_server.exe.bak"));
        assert!(!is_server_command("goose.exe"));
        assert!(!is_server_command(""));
    }

    #[test]
    fn gemini_home_tilde_passthrough_shapes() {
        assert_eq!(
            expand_tilde(PathBuf::from("rel/path")),
            PathBuf::from("rel/path")
        );
        assert!(expand_tilde(PathBuf::from("C:/plain/dir")).is_absolute());
    }

    #[test]
    fn scratch_sweep_deletes_only_stale_and_cleanup_self_tidies() {
        let root = tempdir("scratch");
        let stale = create_scratch_in(&root, "stale-session").unwrap();
        let fresh = create_scratch_in(&root, "fresh-session").unwrap();
        fs::write(stale.join("marker"), b"x").unwrap();

        // A max_age so large the cutoff underflows to the epoch: nothing is
        // stale, both dirs must survive.
        sweep_stale_in(&root, std::time::Duration::from_secs(u64::MAX / 2));
        assert!(stale.exists());
        assert!(fresh.exists());

        // max_age=0 → cutoff is "now": everything created before this moment
        // counts as stale and goes.
        sweep_stale_in(&root, std::time::Duration::ZERO);
        assert!(!stale.exists());
        assert!(!fresh.exists());

        // cleanup_scratch removes the dir and self-tidies empty parents.
        let third = create_scratch_in(&root, "third-session").unwrap();
        cleanup_scratch(&third);
        assert!(!third.exists());
        assert!(!root.exists(), "empty parents self-tidy");
    }
}
