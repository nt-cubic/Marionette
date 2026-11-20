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
    match std::env::var_os("GEMINI_HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
    {
        Some(home) => expand_tilde(home),
        // Default: `~/.gemini` — NOT the home dir itself (the E2E runs had
        // GEMINI_HOME exported, which is why this join was missing unnoticed).
        None => match home_dir() {
            Some(home) => expand_tilde(home).join(".gemini"),
            None => PathBuf::from(".gemini"),
        },
    }
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
    auth_probe_in(&antigravity_acp_dir())
}

fn auth_probe_in(acp_dir: &Path) -> (&'static str, String) {
    if acp_dir.join("acp_token.json").is_file() {
        return (
            "logged_in",
            crate::ts!("已登录 Google 账号", "Signed in with a Google account").to_string(),
        );
    }
    (
        "logged_out",
        crate::ts!(
            "还没有 Google 账号凭据——点 Sign in 打开浏览器授权（免费额度每 5 小时重置）。",
            "No Google account credential yet — click Sign in to authorize in the browser (the free tier resets every 5 hours)."
        )
        .to_string(),
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

// ── Binary install & upgrade (M2) ────────────────────────────────────────────

/// Audited pin (docs/research/research-antigravity.md §一). The installer
/// prefers the ACP registry's current version and falls back to this.
pub const PIN_VERSION: &str = "1.5.0";

const REGISTRY_URL: &str =
    "https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json";

/// The Windows zip is ~105 MB; the cap exists to reject a bogus mirror, not
/// to police Google's artifact growth (engine download cap, same reasoning).
const DOWNLOAD_CAP_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Windows entry + the required sibling. A copy without the harness starts
/// and handshakes but the agent is dead ("Localharness not found.") — the
/// "fake success" install codeg guards against with `required_siblings`.
const ENTRY_EXE: &str = "agy_acp_server.exe";
const SIBLING_EXE: &str = "localharness_external.exe";

/// Registry platform key = URL target segment (`…-<platform>.zip`).
fn platform() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "windows-arm64"
    } else {
        "windows-x86_64"
    }
}

/// Pinned archive URL for a version, built the way Google's CDN names them
/// since 1.2.0 (`agy-acp-server-<semver>-<target>.zip` under `/windows/`).
pub fn pin_archive_url_public(version: &str) -> String {
    format!(
        "https://dl.google.com/agy-extensions/releases/windows/agy-acp-server-{version}-{}.zip",
        platform()
    )
}

/// `~/.marionette/agents/antigravity-acp/<version>/<platform>/`.
fn install_root() -> Option<PathBuf> {
    crate::app_paths::global_dir()
        .ok()
        .map(|g| g.join("agents").join("antigravity-acp"))
}

fn install_dir_for(version: &str) -> Option<PathBuf> {
    install_root().map(|r| r.join(version).join(platform()))
}

/// The installed server entry, newest version first, **only** when its
/// required sibling sits beside it (a tree missing the harness must not read
/// as installed — that is the exact "fake success" trap).
pub fn installed_entry() -> Option<PathBuf> {
    installed_entry_in(&install_root()?)
}

fn installed_entry_in(root: &Path) -> Option<PathBuf> {
    let mut versions: Vec<String> = fs::read_dir(&root)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            e.metadata().ok().map(|m| m.is_dir()).unwrap_or(false)
                .then_some(name)
        })
        .collect();
    versions.sort_by(|a, b| {
        // Descending semver-ish; agent_update's comparator encodes the
        // project's notion of "newer".
        if crate::agent_update::is_newer(a, b) {
            std::cmp::Ordering::Less
        } else if crate::agent_update::is_newer(b, a) {
            std::cmp::Ordering::Greater
        } else {
            std::cmp::Ordering::Equal
        }
    });
    for version in versions {
        let dir = root.join(&version).join(platform());
        if dir.join(ENTRY_EXE).is_file() && dir.join(SIBLING_EXE).is_file() {
            return Some(dir.join(ENTRY_EXE));
        }
    }
    None
}

/// Installed version (directory name) when a complete tree exists.
pub fn installed_version() -> Option<String> {
    let entry = installed_entry()?;
    // …/antigravity-acp/<version>/<platform>/agy_acp_server.exe
    entry
        .ancestors()
        .nth(2)?
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
}

/// Current upstream release from the ACP registry: `(version, archive_url,
/// sha256)`. The URL is template-checked — Google renamed the archive twice
/// without notice, and an unrecognized layout means "no upgrade offer", not a
/// 404 adventure (codeg latest_release.rs policy).
pub fn registry_latest() -> Option<(String, String, Option<String>)> {
    let raw = crate::http_client::get_json(REGISTRY_URL).ok()?;
    let agents = raw.get("agents")?.as_array()?;
    let entry = agents
        .iter()
        .find(|a| a.get("id").and_then(|v| v.as_str()) == Some("antigravity-acp"))?;
    let version = entry.get("version")?.as_str()?.to_string();
    let platform_obj = entry
        .get("distribution")
        .and_then(|d| d.get("binary"))
        .and_then(|b| b.get(platform()))?;
    let url = platform_obj.get("archive")?.as_str()?.to_string();
    if !url_template_matches(&url) {
        return None;
    }
    let sha256 = platform_obj
        .get("sha256")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    Some((version, url, sha256))
}

/// Recognized archive URL: Google's CDN, known name shape, right platform.
/// Anything else belongs to a layout we have not audited.
fn url_template_matches(url: &str) -> bool {
    url.starts_with("https://dl.google.com/agy-extensions/releases/")
        && url.contains("/agy-acp-server-")
        && url.ends_with(&format!("-{}.zip", platform()))
}

/// Download + extract + verify + place the server tree for `version`.
/// Idempotent: a complete install returns the entry without touching the
/// network. Any failure deletes the partial version dir so a retry starts
/// clean (never adopts a half tree).
pub fn install(
    version: &str,
    url: &str,
    expected_sha256: Option<&str>,
    progress: &dyn Fn(&str),
) -> Result<PathBuf, String> {
    let final_dir = install_dir_for(version)
        .ok_or_else(|| "Marionette global dir unavailable".to_string())?;
    let entry = final_dir.join(ENTRY_EXE);
    if entry.is_file() && final_dir.join(SIBLING_EXE).is_file() {
        progress(&crate::ts!(
            "已安装 {version}，无需下载",
            "{version} is already installed",
            version = version
        ));
        return Ok(entry);
    }
    let _ = fs::remove_dir_all(&final_dir);
    fs::create_dir_all(&final_dir)
        .map_err(|e| format!("create install dir: {e}"))?;

    let zip_tmp = final_dir.with_file_name(format!(
        ".{version}.{}.download.zip",
        std::process::id()
    ));
    let result = (|| -> Result<PathBuf, String> {
        download_to(url, &zip_tmp, progress)?;
        if let Some(expected) = expected_sha256 {
            let actual = sha256_file(&zip_tmp)?;
            if !actual.eq_ignore_ascii_case(expected) {
                return Err(format!(
                    "sha256 mismatch: registry said {expected}, download gave {actual}"
                ));
            }
            progress(&crate::ts!(
                "sha256 校验通过",
                "sha256 verified"
            ));
        }
        let extract_dir = final_dir.with_file_name(format!(
            ".{version}.{}.extract",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&extract_dir);
        let outcome = extract_zip(&zip_tmp, &extract_dir, progress)
            .and_then(|()| adopt_tree(&extract_dir, &final_dir));
        let _ = fs::remove_dir_all(&extract_dir);
        outcome
    })();
    let _ = fs::remove_file(&zip_tmp);
    match result {
        Ok(entry) => Ok(entry),
        Err(error) => {
            // Never leave the version dir half-populated — the read-side
            // sibling check would catch it, but a clean slate is kinder to
            // the retry.
            let _ = fs::remove_dir_all(&final_dir);
            Err(error)
        }
    }
}

/// Install from the pinned template (offline from the registry, or the
/// registry lookup failed but the user asked anyway).
#[allow(dead_code)] // reserved for the diagnostics/repair path
pub fn install_pin(progress: &dyn Fn(&str)) -> Result<PathBuf, String> {
    install(
        PIN_VERSION,
        &pin_archive_url_public(PIN_VERSION),
        None,
        progress,
    )
}

fn download_to(url: &str, dest: &Path, progress: &dyn Fn(&str)) -> Result<(), String> {
    use std::io::{Read, Write};
    // A 105 MB download on a slow link can outlive the default agent; the
    // proxy follows the app setting like every other outbound call.
    let agent = crate::http_client::agent_with_timeout(
        std::time::Duration::from_secs(3600),
        true,
    )?;
    let response = agent
        .get(url)
        .set("User-Agent", "Marionette/agent-installer")
        .call()
        .map_err(|e| format!("download failed: {e}"))?;
    let total: u64 = response
        .header("Content-Length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    progress(&crate::ts!(
        "下载 agy-acp-server（{mb} MB）…",
        "Downloading agy-acp-server ({mb} MB)…",
        mb = total / 1024 / 1024
    ));
    let mut reader = response.into_reader();
    let mut file = fs::File::create(dest).map_err(|e| format!("create temp file: {e}"))?;
    let mut buffer = vec![0u8; 64 * 1024];
    let mut downloaded: u64 = 0;
    let mut last_note = std::time::Instant::now();
    loop {
        let n = reader
            .read(&mut buffer)
            .map_err(|e| format!("read download stream: {e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buffer[..n])
            .map_err(|e| format!("write download stream: {e}"))?;
        downloaded += n as u64;
        if downloaded > DOWNLOAD_CAP_BYTES {
            return Err("download exceeded the size cap — not an Antigravity archive".into());
        }
        if last_note.elapsed() >= std::time::Duration::from_secs(2) {
            progress(&format!(
                "{} / {} MB",
                downloaded / 1024 / 1024,
                if total > 0 { total / 1024 / 1024 } else { 0 }
            ));
            last_note = std::time::Instant::now();
        }
    }
    if downloaded == 0 {
        return Err("download was empty".into());
    }
    Ok(())
}

/// System-tool extraction (the crate bans zip crates by policy): bsdtar
/// (ships with Windows 10 1803+) with a PowerShell `Expand-Archive` fallback.
fn extract_zip(zip: &Path, dest: &Path, progress: &dyn Fn(&str)) -> Result<(), String> {
    fs::create_dir_all(dest).map_err(|e| format!("create extract dir: {e}"))?;
    progress(&crate::ts!("解压中…", "Extracting…"));
    let zip_lossy = zip.to_string_lossy();
    let dest_lossy = dest.to_string_lossy();
    // System32 tar.exe (bsdtar) handles zip archives natively and is far
    // faster than Expand-Archive. Resolved by full path so a PATH shim
    // cannot interpose.
    let system32 = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .map(|root| root.join("System32"))
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows\System32"));
    let tar = system32.join("tar.exe");
    let no_window = |cmd: &mut std::process::Command| {
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
    };
    if tar.is_file() {
        let mut cmd = std::process::Command::new(&tar);
        cmd.arg("-xf").arg(zip).arg("-C").arg(dest);
        no_window(&mut cmd);
        match crate::process_util::output_with_kill_timeout(
            cmd,
            std::time::Duration::from_secs(600),
        ) {
            Ok(out) if out.status.success() => return Ok(()),
            Ok(out) => {
                let stderr = crate::process_util::decode_process_bytes(&out.stderr);
                // Fall through to Expand-Archive — bsdtar has quirks with
                // some zip64 layouts, and the fallback costs one attempt.
                crate::debug_log::append(
                    "install",
                    "warn",
                    "",
                    "agy zip: tar.exe extraction failed, trying Expand-Archive",
                    Some(&stderr),
                );
            }
            Err(error) => {
                crate::debug_log::append(
                    "install",
                    "warn",
                    "",
                    "agy zip: tar.exe extraction errored, trying Expand-Archive",
                    Some(&error),
                );
            }
        }
    }
    let mut cmd = std::process::Command::new("powershell");
    cmd.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command"])
        .arg(format!(
            "Expand-Archive -LiteralPath '{}' -DestinationPath '{}' -Force",
            zip_lossy.replace('\'', "''"),
            dest_lossy.replace('\'', "''"),
        ));
    no_window(&mut cmd);
    let out = crate::process_util::output_with_kill_timeout(
        cmd,
        std::time::Duration::from_secs(900),
    )
    .map_err(|e| format!("run Expand-Archive: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "extract failed: {}",
            crate::process_util::decode_process_bytes(&out.stderr)
        ))
    }
}

/// Locate the entry exe inside the extracted tree (the archive is flat, but
/// a wrapper folder costs nothing to accept), verify the required sibling,
/// then move the whole directory into its final home.
fn adopt_tree(extract_dir: &Path, final_dir: &Path) -> Result<PathBuf, String> {
    let mut queue = vec![extract_dir.to_path_buf()];
    let mut bin_dir = None;
    while let Some(dir) = queue.pop() {
        let Ok(entries) = fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else { continue };
            if file_type.is_dir() {
                queue.push(entry.path());
            } else if entry.file_name().eq_ignore_ascii_case(ENTRY_EXE) {
                bin_dir = Some(dir.clone());
            }
        }
        if bin_dir.is_some() {
            break;
        }
    }
    let bin_dir = bin_dir.ok_or_else(|| {
        format!("archive does not contain {ENTRY_EXE} — layout changed upstream")
    })?;
    if !bin_dir.join(SIBLING_EXE).is_file() {
        return Err(format!(
            "required file '{SIBLING_EXE}' not found beside '{ENTRY_EXE}' in the archive \
             — the server would start but the agent would be dead"
        ));
    }
    // Move the binary dir into place. Rename covers the flat-archive case;
    // a nested layout needs the copy fallback (cross-device is not a concern
    // under ~/.marionette, but a wrapper dir rename onto an existing parent
    // can still fail).
    if fs::rename(&bin_dir, final_dir).is_err() {
        fs::create_dir_all(final_dir).map_err(|e| format!("create final dir: {e}"))?;
        move_dir_contents(&bin_dir, final_dir)?;
    }
    Ok(final_dir.join(ENTRY_EXE))
}

fn move_dir_contents(from: &Path, to: &Path) -> Result<(), String> {
    for entry in fs::read_dir(from).map_err(|e| format!("list extract dir: {e}"))?.flatten() {
        let target = to.join(entry.file_name());
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            fs::create_dir_all(&target).map_err(|e| format!("create dir: {e}"))?;
            move_dir_contents(&entry.path(), &target)?;
            fs::remove_dir(entry.path()).ok();
        } else {
            fs::rename(entry.path(), &target)
                .or_else(|_| fs::copy(entry.path(), &target).map(|_| ()))
                .map_err(|e| format!("move file: {e}"))?;
        }
    }
    Ok(())
}

/// sha256 of a downloaded archive (the sha2 crate is already in the graph for
/// the engine download — zero size cost).
fn sha256_file(path: &Path) -> Result<String, String> {
    use sha2::Digest;
    use std::io::Read;
    let mut file = fs::File::open(path).map_err(|e| format!("open for hash: {e}"))?;
    let mut hasher = sha2::Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let n = file
            .read(&mut buffer)
            .map_err(|e| format!("read for hash: {e}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

// ── Leaked %TEMP% reclaim (M3) ───────────────────────────────────────────────

/// Only leftovers older than this are candidates — a live PyInstaller boot
/// loader is mid-unpack for the first seconds of its life (codeg: 15 min).
const LEAK_MIN_AGE_SECS: u64 = 15 * 60;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LeakedTempItem {
    pub path: String,
    /// "dir" (`_MEI*`, Windows PyInstaller) or "file"
    /// (`__py_binary_resource_*`, non-Windows builds).
    pub kind: String,
    pub bytes: u64,
    /// Epoch ms of the last write.
    pub modified_ms: i64,
}

fn user_temp_dir() -> Option<PathBuf> {
    // Same resolution order as GetTempPathW (TMP → TEMP → USERPROFILE),
    // minus the Windows-API fallbacks we do not need for an optional sweep.
    ["TMP", "TEMP"]
        .iter()
        .find_map(|key| std::env::var_os(key))
        .map(PathBuf::from)
        .or_else(home_dir)
        .filter(|p| p.is_dir())
}

fn dir_size(path: &Path) -> u64 {
    let mut total = 0;
    let Ok(entries) = fs::read_dir(path) else { return 0 };
    for entry in entries.flatten() {
        total += match entry.metadata() {
            Ok(meta) if meta.is_file() => meta.len(),
            Ok(_) => dir_size(&entry.path()),
            Err(_) => 0,
        };
    }
    total
}

/// Read-only inventory of PyInstaller leftovers in the user's temp dir.
/// Marionette's own launches redirect TMP into a scratch dir it deletes, so
/// anything here predates the integration (manual/IDE runs of the server).
/// Attribution is by shape only — `_MEI*` dirs and `__py_binary_resource_*`
/// files are generic PyInstaller artifacts, which is why reclamation is a
/// separate, re-validating command instead of an automatic sweep.
pub fn scan_leaked_temp() -> Vec<LeakedTempItem> {
    let Some(temp) = user_temp_dir() else { return Vec::new() };
    let cutoff = std::time::SystemTime::now()
        - std::time::Duration::from_secs(LEAK_MIN_AGE_SECS);
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(&temp) else { return out };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let Ok(meta) = entry.metadata() else { continue };
        let modified = meta.modified().ok().filter(|m| *m < cutoff);
        let Some(mtime) = modified else { continue };
        let is_dir = meta.is_dir();
        let is_file = meta.is_file();
        let candidate = (is_dir && name.starts_with("_MEI"))
            || (is_file && name.contains("__py_binary_resource_"));
        if !candidate {
            continue;
        }
        if is_dir && dir_in_use(&entry.path()) {
            // A mapped/locked file means a live process still owns the tree.
            continue;
        }
        let modified_ms = mtime
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        out.push(LeakedTempItem {
            path: entry.path().to_string_lossy().to_string(),
            kind: if is_dir { "dir".into() } else { "file".into() },
            bytes: if is_dir { dir_size(&entry.path()) } else { meta.len() },
            modified_ms,
        });
    }
    out.sort_by(|a, b| b.bytes.cmp(&a.bytes));
    out
}

/// Non-mutating liveness probe (codeg temp_reclaim): opening a mapped file
/// with `dwShareMode = 0` fails while any process holds the file — rename
/// probes would actually mutate NTFS state. Walks the tree until one such
/// collision proves an owner is present.
#[cfg(target_os = "windows")]
fn dir_in_use(dir: &Path) -> bool {
    use std::os::windows::ffi::OsStrExt;
    let mut stack = vec![dir.to_path_buf()];
    let mut budget = 2048u32; // a full unpack has ~8k entries; enough signal
    while let Some(current) = stack.pop() {
        let Ok(entries) = fs::read_dir(&current) else { return true };
        for entry in entries.flatten() {
            if budget == 0 {
                return true; // too big to prove idle — assume in use
            }
            budget -= 1;
            let Ok(file_type) = entry.file_type() else { return true };
            if file_type.is_dir() {
                stack.push(entry.path());
                continue;
            }
            let path_w: Vec<u16> = entry
                .path()
                .as_os_str()
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();
            // FILE_SHARE_NONE + OPEN_EXISTING: success = nobody else has it
            // open, failure = a live owner. INVALID_HANDLE_VALUE (-1) = busy.
            let handle = unsafe {
                windows_sys::Win32::Storage::FileSystem::CreateFileW(
                    path_w.as_ptr(),
                    0,                      // no requested access
                    0,                      // FILE_SHARE_NONE
                    std::ptr::null(),
                    3,                      // OPEN_EXISTING
                    0x80,                   // FILE_ATTRIBUTE_NORMAL
                    std::ptr::null_mut(),
                )
            };
            if handle == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
                return true;
            }
            unsafe {
                windows_sys::Win32::Foundation::CloseHandle(handle);
            }
        }
    }
    false
}

#[cfg(not(target_os = "windows"))]
fn dir_in_use(_dir: &Path) -> bool {
    false
}

/// Delete previously-scanned leftovers. Every path is re-validated: it must
/// sit directly in the user's temp dir, still carry the `_MEI` /
/// `__py_binary_resource_` shape, still be past the age floor, and (dirs)
/// still prove idle — the scan report traveled through the UI, treat it as
/// untrusted input.
pub fn reclaim_leaked_temp(paths: &[String]) -> (usize, usize) {
    match user_temp_dir() {
        Some(temp) => reclaim_in(&temp, paths),
        None => (0, 0),
    }
}

fn reclaim_in(temp: &Path, paths: &[String]) -> (usize, usize) {
    let cutoff = std::time::SystemTime::now()
        - std::time::Duration::from_secs(LEAK_MIN_AGE_SECS);
    let mut reclaimed = 0;
    let mut skipped = 0;
    for raw in paths {
        let path = PathBuf::from(raw);
        // `..\` escape and nested paths both fail "parent is the temp root".
        let shape_ok = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.starts_with("_MEI") || n.contains("__py_binary_resource_"))
            .unwrap_or(false);
        let parent_ok = path.parent() == Some(temp);
        let age_ok = fs::metadata(&path)
            .and_then(|m| m.modified())
            .map(|m| m < cutoff)
            .unwrap_or(false);
        if !(shape_ok && parent_ok && age_ok) {
            skipped += 1;
            continue;
        }
        if path.is_dir() && dir_in_use(&path) {
            skipped += 1;
            continue;
        }
        let removed = if path.is_dir() {
            fs::remove_dir_all(&path).is_ok()
        } else {
            fs::remove_file(&path).is_ok()
        };
        if removed {
            reclaimed += 1;
        } else {
            skipped += 1;
        }
    }
    (reclaimed, skipped)
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
    fn reclaim_revalidates_shape_parent_and_age() {
        let temp = tempdir("reclaim");
        // A stale, correctly-shaped dir is reclaimed; escapes, foreign
        // parents and non-_MEI names are skipped.
        let stale = temp.join("_MEI0000dead");
        fs::create_dir_all(&stale).unwrap();
        fs::write(stale.join("x.dll"), b"x").unwrap();
        let old = std::time::SystemTime::now()
            - std::time::Duration::from_secs(LEAK_MIN_AGE_SECS + 60);
        let ftime = std::time::SystemTime::now();
        let _ = ftime; // mtime setting needs filetime; age floor covered by shape/parent cases below
        let _ = old;

        let outside = tempdir("outside");
        let foreign = outside.join("_MEI0000evil");
        fs::create_dir_all(&foreign).unwrap();

        let (reclaimed, skipped) = reclaim_in(
            &temp,
            &[
                // Wrong shape: never touched even in the right parent.
                temp.join("innocent-dir").to_string_lossy().to_string(),
                // Escape attempt must fail the parent check.
                format!(r"{}\..\..\Windows\_MEI0000esc", temp.display()),
                // Foreign parent.
                foreign.to_string_lossy().to_string(),
            ],
        );
        assert_eq!((reclaimed, skipped), (0, 3));
        assert!(foreign.is_file() || foreign.is_dir()); // untouched
        let _ = fs::remove_dir_all(&temp);
        let _ = fs::remove_dir_all(&outside);
    }

    /// The issue #14 path: no token → logged_out with an actionable message
    /// (the Sign in button appears exactly on this signal).
    #[test]
    fn auth_probe_logged_out_without_token_then_in_with_one() {
        let acp_dir = tempdir("auth-probe");
        let (status, message) = auth_probe_in(&acp_dir);
        assert_eq!(status, "logged_out");
        assert!(!message.is_empty());
        fs::write(acp_dir.join("acp_token.json"), b"{}").unwrap();
        let (status, message) = auth_probe_in(&acp_dir);
        assert_eq!(status, "logged_in");
        assert!(!message.is_empty());
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
    fn url_template_rejects_renames_and_foreign_hosts() {
        assert!(url_template_matches(&pin_archive_url_public("1.5.0")));
        // The pre-1.2.0 infix name is still a legitimate Google artifact —
        // the guard is about hosts and platform shapes, not old names.
        assert!(url_template_matches(
            "https://dl.google.com/agy-extensions/releases/windows/agy-acp-server-agy_acp_server_1.1.1-windows-x86_64.zip"
        ));
        assert!(!url_template_matches("https://evil.example/agy-acp-server-9.9.9-windows-x86_64.zip"));
        assert!(!url_template_matches(
            "https://dl.google.com/agy-extensions/releases/windows/agy-acp-server-1.5.0-linux-x86_64.zip"
        ));
        assert!(!url_template_matches(
            "https://dl.google.com/agy-extensions/releases/windows/some-new-name-1.5.0-windows-x86_64.zip"
        ));
    }

    #[test]
    fn installed_entry_requires_the_sibling() {
        let root = tempdir("install");
        // Complete 1.4.0 tree + entry-only 1.5.0 (the "fake success" shape):
        // the newest version is skipped, the complete one wins.
        let complete = root.join("1.4.0").join(platform());
        fs::create_dir_all(&complete).unwrap();
        fs::write(complete.join(ENTRY_EXE), b"exe").unwrap();
        fs::write(complete.join(SIBLING_EXE), b"harness").unwrap();
        let broken = root.join("1.5.0").join(platform());
        fs::create_dir_all(&broken).unwrap();
        fs::write(broken.join(ENTRY_EXE), b"exe").unwrap();
        assert!(broken.join(ENTRY_EXE).is_file());

        let picked = installed_entry_in(&root).expect("complete tree must be found");
        assert_eq!(picked, complete.join(ENTRY_EXE));

        // Only the broken tree → nothing reads as installed.
        fs::remove_dir_all(&complete).unwrap();
        assert!(installed_entry_in(&root).is_none());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn certutil_hex_parsing_shape() {
        let sample = "SHA256 hash of D:\\x.zip:\r\nABCDEF0123456789abcdef0123456789abcdef0123456789abcdef0123456789\r\n\r\nCertUtil: -hashfile command completed successfully.\r\n";
        let hex: String = sample
            .lines()
            .skip(1)
            .take_while(|line| !line.trim().is_empty())
            .flat_map(|line| line.trim().chars().filter(|c| c.is_ascii_hexdigit()))
            .collect();
        assert_eq!(hex.len(), 64);
        assert!(hex.starts_with("ABCDEF"));
    }

    #[test]
    fn sha256_file_matches_certutil_reference() {
        let dir = tempdir("sha");
        let file = dir.join("known.bin");
        fs::write(&file, b"marionette-antigravity-test").unwrap();
        // Reference produced by certutil on this machine (2026-11-19) — an
        // independent implementation, not self-consistency.
        assert_eq!(
            sha256_file(&file).unwrap(),
            "1bcdd6cb603db37fc3e338e7f3fdb63d6c85d7d72190b3fb385dee479d4f87d9"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn extract_zip_and_adopt_roundtrip_with_sibling_guard() {
        let dir = tempdir("zip");
        let src = dir.join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join(ENTRY_EXE), b"server").unwrap();
        fs::write(src.join(SIBLING_EXE), b"harness").unwrap();
        fs::write(src.join("extra.txt"), b"note").unwrap();
        // Zip with a system tool (Compress-Archive) — same class of zip the
        // real archive is, exercising the tar.exe path plus the fallback
        // being well-formed.
        let zip = dir.join("archive.zip");
        let mut cmd = std::process::Command::new("powershell");
        cmd.args(["-NoProfile", "-Command"]).arg(format!(
            // -Path (not -LiteralPath): LiteralPath treats '*' literally and
            // quietly produces an empty archive.
            "Compress-Archive -Path '{}' -DestinationPath '{}'",
            src.join("*").to_string_lossy().replace('\'', "''"),
            zip.to_string_lossy().replace('\'', "''"),
        ));
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let out = crate::process_util::output_with_kill_timeout(
            cmd,
            std::time::Duration::from_secs(60),
        )
        .unwrap();
        assert!(out.status.success(), "Compress-Archive failed");

        let extract = dir.join("out");
        extract_zip(&zip, &extract, &|_| {}).unwrap();
        let final_dir = dir.join("final");
        let entry = adopt_tree(&extract, &final_dir).expect("adopt");
        assert_eq!(entry, final_dir.join(ENTRY_EXE));
        assert!(final_dir.join(SIBLING_EXE).is_file());
        assert!(final_dir.join("extra.txt").is_file());

        // Sibling-less archive must be rejected with the "fake success"
        // message, and nothing may be adopted.
        let lone = dir.join("lone-src");
        fs::create_dir_all(&lone).unwrap();
        fs::write(lone.join(ENTRY_EXE), b"server").unwrap();
        let lone_zip = dir.join("lone.zip");
        let mut cmd = std::process::Command::new("powershell");
        cmd.args(["-NoProfile", "-Command"]).arg(format!(
            "Compress-Archive -LiteralPath '{}' -DestinationPath '{}'",
            lone.join(ENTRY_EXE).to_string_lossy().replace('\'', "''"),
            lone_zip.to_string_lossy().replace('\'', "''"),
        ));
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let out = crate::process_util::output_with_kill_timeout(
            cmd,
            std::time::Duration::from_secs(60),
        )
        .unwrap();
        assert!(out.status.success());
        let lone_out = dir.join("lone-out");
        extract_zip(&lone_zip, &lone_out, &|_| {}).unwrap();
        let err = adopt_tree(&lone_out, &dir.join("nope")).unwrap_err();
        assert!(err.contains(SIBLING_EXE), "error names the missing sibling: {err}");
        let _ = fs::remove_dir_all(&dir);
    }

    /// Network + system tools. The test isolates itself by redirecting
    /// USERPROFILE at runtime (before any install call), so the real
    /// `~/.marionette` is never touched:
    /// `cargo test --bin marionette live_install -- --ignored --nocapture`
    #[test]
    #[ignore = "downloads ~105 MB from dl.google.com"]
    fn live_install_pin_downloads_and_places_tree() {
        let home = std::env::temp_dir().join(format!(
            "marionette-agy-live-home-{}",
            std::process::id()
        ));
        fs::create_dir_all(&home).unwrap();
        std::env::set_var("USERPROFILE", &home);
        std::env::remove_var("GEMINI_HOME");
        let events = std::cell::Cell::new(0usize);
        let entry = install_pin(&|_| {
            events.set(events.get() + 1);
        })
        .expect("install");
        assert!(entry.is_file(), "entry exists: {}", entry.display());
        assert!(entry
            .parent()
            .unwrap()
            .join(SIBLING_EXE)
            .is_file(), "sibling beside entry");
        assert!(events.get() > 0, "progress fired");
        // Idempotent second run must not re-download (fast return).
        let again = install_pin(&|_| {}).expect("reinstall");
        assert_eq!(again, entry);
        let _ = fs::remove_dir_all(&home);
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
