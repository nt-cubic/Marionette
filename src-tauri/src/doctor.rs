//! Headless health check: `Marionette.exe --doctor [--json]`.
//!
//! Deliberately runs **before** the single-instance lock, the WebView2 gate and
//! `app_paths::ensure_global_dir_writable()`: those three are themselves things
//! the doctor must be able to report (a locked instance, a missing runtime, a
//! Low-integrity image that cannot write `~/.marionette`). It never starts an
//! engine, never spawns an agent and never spends a token.
//!
//! The report is written to `~/.marionette/logs/doctor-report.txt` (and `.json`
//! with `--json`) because the release binary is a `windows`-subsystem exe: when
//! `tools\doctor.bat` runs it, stdout is not attached to the console, so the
//! batch file prints the file instead.
//!
//! Exit code: 0 all good, 1 warnings, 2 failures.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum Level {
    Ok,
    Warn,
    Fail,
}

impl Level {
    fn tag(self) -> &'static str {
        match self {
            Level::Ok => "OK  ",
            Level::Warn => "WARN",
            Level::Fail => "FAIL",
        }
    }
}

#[derive(Clone, Serialize)]
struct Finding {
    area: &'static str,
    level: Level,
    title: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    lines: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fix: Option<String>,
}

impl Finding {
    fn new(area: &'static str, level: Level, title: impl Into<String>) -> Self {
        Self { area, level, title: title.into(), lines: Vec::new(), fix: None }
    }
    fn line(mut self, line: impl Into<String>) -> Self {
        self.lines.push(line.into());
        self
    }
    fn fix(mut self, fix: impl Into<String>) -> Self {
        self.fix = Some(fix.into());
        self
    }
}

/// Markers that mean "something was written but the other side did not take
/// it" -- the class of failure this doctor exists for.
const DEGRADED_MARKERS: &[&str] = &[
    "injection skipped",
    "bridge skipped",
    "route skipped",
    "model not found",
    "model apply failed",
    "pre-prompt config sync failed",
    "pre-parse failed",
    "falling back to native",
    "providers/set failed",
];

pub fn run_cli() -> i32 {
    let args: Vec<String> = std::env::args().collect();
    let json = args.iter().any(|arg| arg == "--json");
    let fix = args.iter().any(|arg| arg == "--fix");
    let mut prefix = Vec::new();
    if fix {
        prefix = apply_fixes();
    }
    let (findings, exit) = collect();
    let report = {
        let mut all = prefix;
        all.extend(findings);
        all
    };
    let home = crate::app_paths::global_dir().ok();
    let text = render(&report);

    if let Some(home) = home.as_deref() {
        let dir = home.join("logs");
        let _ = std::fs::create_dir_all(&dir);
        let wrote = std::fs::write(dir.join("doctor-report.txt"), &text).is_ok();
        if json {
            if let Ok(body) = serde_json::to_string_pretty(&report) {
                let _ = std::fs::write(dir.join("doctor-report.json"), body);
            }
        }
        // Unwritable data dir (the Low-integrity trap) must still leave a
        // report somewhere the batch file can print.
        if !wrote {
            let _ = std::fs::write(std::env::temp_dir().join("marionette-doctor.txt"), &text);
        }
    }
    print!("{text}");
    let _ = std::io::Write::flush(&mut std::io::stdout());
    exit
}

fn render(findings: &[Finding]) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "Marionette doctor  v{}", env!("CARGO_PKG_VERSION"));
    let _ = writeln!(out, "exe  : {}", std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default());
    let _ = writeln!(out, "home : {}", crate::app_paths::global_dir().map(|p| p.display().to_string()).unwrap_or_else(|e| e));
    let _ = writeln!(out, "{}", "-".repeat(72));
    for f in findings {
        let _ = writeln!(out, "[{}] {:<10} {}", f.level.tag(), f.area, f.title);
        for line in &f.lines {
            let _ = writeln!(out, "       {line}");
        }
        if let Some(fix) = &f.fix {
            let _ = writeln!(out, "       -> {fix}");
        }
    }
    let _ = writeln!(out, "{}", "-".repeat(72));
    let ok = findings.iter().filter(|f| f.level == Level::Ok).count();
    let warn = findings.iter().filter(|f| f.level == Level::Warn).count();
    let fail = findings.iter().filter(|f| f.level == Level::Fail).count();
    let _ = writeln!(out, "{ok} ok / {warn} warn / {fail} fail");
    if let Ok(home) = crate::app_paths::global_dir() {
        let _ = writeln!(out, "report: {}", home.join("logs").join("doctor-report.txt").display());
    }
    out
}

fn collect() -> (Vec<Finding>, i32) {
    let mut findings = Vec::new();
    let home = crate::app_paths::global_dir().ok();

    findings.push(check_data_dir());
    let (engine_live, engine_finding) = check_engine();
    findings.push(engine_finding);
    let (catalog_ids, catalog_finding) = check_catalog(engine_live.is_some());
    findings.push(catalog_finding);
    findings.push(check_providers(home.as_deref()));
    findings.push(check_bindings(home.as_deref(), &catalog_ids));
    findings.push(check_session_prefs(home.as_deref(), &catalog_ids));
    findings.push(check_codex(engine_live.map(|(port, _)| port)));
    findings.push(check_logs(home.as_deref()));

    let exit = if findings.iter().any(|f| f.level == Level::Fail) {
        2
    } else if findings.iter().any(|f| f.level == Level::Warn) {
        1
    } else {
        0
    };
    (findings, exit)
}

fn check_data_dir() -> Finding {
    match crate::app_paths::ensure_global_dir_writable() {
        Ok(()) => Finding::new("data-dir", Level::Ok, "writable"),
        Err(error) => Finding::new("data-dir", Level::Fail, "NOT writable")
            .line(short(&error, 300))
            .fix(
                "Sandbox/integrity: run  icacls <app folder> /setintegritylevel (OI)(CI)Medium  \
                 then reopen. Without this the exe exits silently.",
            ),
    }
}

/// Live engine probe from *another* process: `serve.json` first, then the
/// historical default ports. Returns (port, models).
fn probe_engine() -> Option<(u16, usize)> {
    let token = crate::engine::gateway_token();
    let mut candidates: Vec<u16> = Vec::new();
    if let Ok(dir) = crate::engine::engine_dir() {
        if let Ok(text) = std::fs::read_to_string(dir.join("serve.json")) {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
                if let Some(port) = value.get("port").and_then(|v| v.as_u64()) {
                    candidates.push(port as u16);
                }
            }
        }
    }
    for port in [3425u16, 3426, 3427, 3428, 3429] {
        if !candidates.contains(&port) {
            candidates.push(port);
        }
    }
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_millis(800))
        .build();
    for port in candidates {
        let url = format!("http://127.0.0.1:{port}/v1/models");
        let Ok(resp) = agent
            .get(&url)
            .set("Authorization", &format!("Bearer {token}"))
            .call()
        else {
            continue;
        };
        let count = resp
            .into_json::<serde_json::Value>()
            .ok()
            .and_then(|v| v.get("data").and_then(|d| d.as_array()).map(|a| a.len()))
            .unwrap_or(0);
        if count > 0 {
            return Some((port, count));
        }
    }
    None
}

fn check_engine() -> (Option<(u16, usize)>, Finding) {
    match probe_engine() {
        Some((port, models)) => (
            Some((port, models)),
            Finding::new(
                "engine",
                Level::Ok,
                format!("serving on 127.0.0.1:{port} ({models} models)"),
            ),
        ),
        None => (
            None,
            Finding::new("engine", Level::Warn, "not running (not answered on 127.0.0.1:3425-3429)")
                .line("Bridges that are injected at spawn time (opencode/pi/dsh/zcode, codex alias) are only written while the engine is up.")
                .fix("Open Marionette once (it starts lazily), or send one message in any session."),
        ),
    }
}

fn check_catalog(engine_running: bool) -> (Vec<String>, Finding) {
    let (rows, offline) = crate::engine::catalog_direct_or_cached_entries();
    let ids: Vec<String> = rows.iter().map(|row| row.id.clone()).collect();
    if ids.is_empty() {
        return (
            ids,
            Finding::new("catalog", Level::Fail, "no models")
                .line("The engine lists no models, so no shared model can be picked anywhere.")
                .fix("Open model settings once and press refresh; check the engine log if sources are red."),
        );
    }
    let source = if offline || !engine_running { "offline cache" } else { "live" };
    (
        ids,
        Finding::new("catalog", Level::Ok, format!("{} models ({source})", rows.len())),
    )
}

fn check_providers(home: Option<&Path>) -> Finding {
    let Some(path) = home.map(|h| h.join("engine").join("data").join("providers.json")) else {
        return Finding::new("providers", Level::Warn, "no data dir");
    };
    if !path.exists() {
        return Finding::new("providers", Level::Warn, "providers.json missing")
            .fix("Open model settings once; the engine registers sources on first use.");
    }
    let parsed = std::fs::read_to_string(&path)
        .map_err(|e| e.to_string())
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).map_err(|e| e.to_string()));
    match parsed {
        Ok(value) => {
            let list = value.get("providers").and_then(|v| v.as_array()).cloned().unwrap_or_default();
            if list.is_empty() {
                Finding::new("providers", Level::Warn, "no registered sources")
                    .fix("Add an API key or sign in to a subscription in model settings.")
            } else {
                let names: Vec<String> = list
                    .iter()
                    .filter_map(|p| p.get("id").and_then(|v| v.as_str()).map(str::to_string))
                    .take(8)
                    .collect();
                Finding::new("providers", Level::Ok, format!("{} source(s)", list.len()))
                    .line(names.join(", "))
            }
        }
        Err(error) => Finding::new("providers", Level::Fail, "providers.json is not valid JSON")
            .line(short(&error, 200))
            .fix("Move the file aside; the engine rebuilds it on the next start."),
    }
}

fn check_bindings(home: Option<&Path>, catalog: &[String]) -> Finding {
    let Some(path) = home.map(|h| h.join("bridge-bindings.json")) else {
        return Finding::new("bindings", Level::Warn, "no data dir");
    };
    if !path.exists() {
        return Finding::new("bindings", Level::Ok, "no session bindings");
    }
    let Ok(file) = crate::model_bridge::load_bindings_at(&path) else {
        return Finding::new("bindings", Level::Fail, "bridge-bindings.json is corrupt")
            .fix("Delete it; sessions fall back to their own model.");
    };
    let mut dead: Vec<(String, String)> = file
        .bindings
        .iter()
        .filter(|(_, sel)| !catalog.iter().any(|id| id == &sel.model_id))
        .map(|(sid, sel)| (sid.clone(), sel.model_id.clone()))
        .collect();
    dead.sort();
    if dead.is_empty() {
        Finding::new("bindings", Level::Ok, format!("{} binding(s), all resolvable", file.bindings.len()))
    } else {
        let mut f = Finding::new(
            "bindings",
            Level::Warn,
            format!("{} of {} binding(s) point at a model the engine no longer lists", dead.len(), file.bindings.len()),
        );
        for (sid, model) in dead.iter().take(5) {
            f = f.line(format!("{sid} -> {model}"));
        }
        f.fix("Pick another shared model in those sessions (or clear the binding). A stale one silently falls back to the agent default.")
    }
}

fn check_session_prefs(home: Option<&Path>, catalog: &[String]) -> Finding {
    let Some(home) = home else {
        return Finding::new("session-prefs", Level::Warn, "no data dir");
    };
    let mut stale: Vec<(String, String, String)> = Vec::new();
    let mut total = 0usize;

    let mut files: Vec<PathBuf> = Vec::new();
    files.push(home.join("chats").join("sessions").join("index.json"));
    if let Ok(text) = std::fs::read_to_string(home.join("projects.json")) {
        if let Ok(list) = serde_json::from_str::<Vec<serde_json::Value>>(&text) {
            for project in list {
                if let Some(root) = project.get("rootPath").and_then(|v| v.as_str()) {
                    files.push(crate::app_paths::project_dir(Path::new(root)).join("sessions").join("index.json"));
                }
            }
        }
    }

    for file in files {
        let Ok(text) = std::fs::read_to_string(&file) else { continue };
        let Ok(list) = serde_json::from_str::<Vec<serde_json::Value>>(&text) else { continue };
        for session in list {
            let pref = session.get("preferredModel").and_then(|v| v.as_str()).unwrap_or("").trim();
            if pref.is_empty() {
                continue;
            }
            total += 1;
            let Some(inner) = bridge_wire_inner(pref) else { continue };
            if !catalog.iter().any(|id| id == inner) {
                stale.push((
                    session.get("id").and_then(|v| v.as_str()).unwrap_or("?").to_string(),
                    session.get("agentId").and_then(|v| v.as_str()).unwrap_or("?").to_string(),
                    pref.to_string(),
                ));
            }
        }
    }

    if stale.is_empty() {
        Finding::new("session-prefs", Level::Ok, format!("{total} saved model choice(s), all resolvable"))
    } else {
        let mut f = Finding::new(
            "session-prefs",
            Level::Warn,
            format!("{} saved choice(s) name a shared model the engine no longer lists", stale.len()),
        );
        for (sid, agent, pref) in stale.iter().take(5) {
            f = f.line(format!("{sid} [{agent}] -> {pref}"));
        }
        f.fix("Those sessions will open on the agent default. Pick a model again in them.")
    }
}

fn check_codex(engine_base: Option<u16>) -> Finding {
    let Some(home) = std::env::var_os("USERPROFILE").map(PathBuf::from) else {
        return Finding::new("codex", Level::Ok, "skipped");
    };
    let config = home.join(".codex").join("config.toml");
    if !config.exists() {
        return Finding::new("codex", Level::Ok, "not installed (no ~/.codex/config.toml)");
    }
    let text = std::fs::read_to_string(&config).unwrap_or_default();
    let points_at_engine = text.contains("127.0.0.1") || text.contains("localhost");
    match (engine_base, points_at_engine) {
        (Some(port), true) => {
            let current = text.contains(&format!(":{port}")) || text.contains(&format!(":{port}/"));
            if current {
                Finding::new("codex", Level::Ok, "config points at the engine")
            } else {
                Finding::new("codex", Level::Warn, "config points at a local port the engine is not on")
                    .line(format!("engine is on {port}"))
                    .fix("Open a Codex session once; the next spawn rewrites providers/set.")
            }
        }
        (Some(_), false) => Finding::new("codex", Level::Warn, "engine is running but ~/.codex/config.toml is not bridged")
            .fix("Open a Codex session with a shared model bound; if it still uses the subscription, that is the burn case."),
        (None, true) => Finding::new("codex", Level::Ok, "bridged, engine asleep (starts on demand)")
            .line("the alias is rewritten at spawn"),
        (None, false) => Finding::new("codex", Level::Ok, "native only (no engine bridge)"),
    }
}

fn check_logs(home: Option<&Path>) -> Finding {
    let Some(home) = home else {
        return Finding::new("logs", Level::Warn, "no data dir");
    };
    let dir = home.join("logs");
    let mut hits: Vec<(u128, String)> = Vec::new();
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let window = Duration::from_secs(24 * 3600).as_millis();

    for name in ["dev.log", "dev.log.1"] {
        let Ok(text) = std::fs::read_to_string(dir.join(name)) else { continue };
        for line in text.lines() {
            if !(line.contains("[warn]") || line.contains("[error]")) || line.contains("session/update") {
                continue;
            }
            if !DEGRADED_MARKERS.iter().any(|m| line.contains(m)) {
                continue;
            }
            let ts = line
                .strip_prefix('[')
                .and_then(|rest| rest.split(']').next())
                .and_then(|num| num.parse::<u128>().ok())
                .unwrap_or(0);
            if now.saturating_sub(ts) > window {
                continue;
            }
            hits.push((ts, line.to_string()));
        }
        if !hits.is_empty() {
            break;
        }
    }

    if hits.is_empty() {
        return Finding::new("logs", Level::Ok, "no degraded wiring lines in the last 24h");
    }
    hits.sort_by_key(|(ts, _)| *ts);
    let mut f = Finding::new("logs", Level::Warn, format!("{} degraded wiring line(s) in the last 24h", hits.len()));
    for (_, line) in hits.iter().rev().take(3) {
        f = f.line(short(line, 180));
    }
    f.fix("These are the 'config written but not accepted' markers. Check them before trusting a model chip.")
}

/// Strips the native wire prefix: marionette/ , marionette:: , marionette backslash.
/// `--doctor --fix`: repairs the two data-class findings (dead bindings, saved
/// wire ids the engine no longer lists). Refuses while the app is running: the
/// live process holds both files in memory and would rewrite them on top.
fn apply_fixes() -> Vec<Finding> {
    let Some(home) = crate::app_paths::global_dir().ok() else {
        return vec![Finding::new("fix", Level::Fail, "no data dir")];
    };
    if probe_engine().is_some() {
        return vec![Finding::new("fix", Level::Fail, "close Marionette first")
            .line("The running app keeps sessions and bindings in memory; it would overwrite these repairs.")];
    }
    let (rows, _) = crate::engine::catalog_direct_or_cached_entries();
    let catalog: Vec<String> = rows.into_iter().map(|row| row.id).collect();
    if catalog.is_empty() {
        return vec![Finding::new("fix", Level::Fail, "no catalog to compare against")
            .line("Run the app once so the engine writes a catalog, then retry.")];
    }
    let mut out = fix_bindings(&home, &catalog);
    out.extend(fix_session_prefs(&home, &catalog));
    if out.is_empty() {
        out.push(Finding::new("fix", Level::Ok, "nothing to repair"));
    }
    out
}

fn fix_bindings(home: &Path, catalog: &[String]) -> Vec<Finding> {
    let path = home.join("bridge-bindings.json");
    if !path.exists() {
        return Vec::new();
    }
    let Ok(mut file) = crate::model_bridge::load_bindings_at(&path) else {
        return vec![Finding::new("fix", Level::Fail, "bridge-bindings.json unreadable")
            .fix("Delete it; sessions fall back to their own model.")];
    };
    let before = file.bindings.len();
    let dead: Vec<String> = file
        .bindings
        .iter()
        .filter(|(_, sel)| !catalog.iter().any(|id| id == &sel.model_id))
        .map(|(sid, _)| sid.clone())
        .collect();
    if dead.is_empty() {
        return Vec::new();
    }
    let _ = std::fs::copy(&path, home.join("bridge-bindings.json.bak"));
    for sid in &dead {
        file.bindings.remove(sid);
    }
    let Ok(body) = serde_json::to_string_pretty(&file) else {
        return vec![Finding::new("fix", Level::Fail, "could not serialise bindings")];
    };
    if std::fs::write(&path, format!("{body}\n")).is_err() {
        return vec![Finding::new("fix", Level::Fail, "could not write bridge-bindings.json")];
    }
    vec![Finding::new(
        "fix",
        Level::Ok,
        format!("removed {} dead binding(s) of {}", dead.len(), before),
    )
    .line(dead.iter().take(5).cloned().collect::<Vec<_>>().join(", "))]
}

fn fix_session_prefs(home: &Path, catalog: &[String]) -> Vec<Finding> {
    let mut cleared = 0usize;
    let mut touched: Vec<String> = Vec::new();
    for file in session_index_files(home) {
        let Ok(text) = std::fs::read_to_string(&file) else { continue };
        let Ok(mut list) = serde_json::from_str::<Vec<serde_json::Value>>(&text) else { continue };
        let mut changed = false;
        for session in list.iter_mut() {
            let pref = session
                .get("preferredModel")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let Some(pref) = pref else { continue };
            let Some(inner) = bridge_wire_inner(&pref) else { continue };
            if catalog.iter().any(|id| id == inner) {
                continue;
            }
            session["preferredModel"] = serde_json::Value::Null;
            cleared += 1;
            changed = true;
        }
        if !changed {
            continue;
        }
        let _ = std::fs::copy(&file, file.with_extension("json.bak"));
        if let Ok(body) = serde_json::to_string_pretty(&list) {
            if std::fs::write(&file, format!("{body}\n")).is_ok() {
                touched.push(
                    file.file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("index.json")
                        .to_string(),
                );
            }
        }
    }
    if cleared == 0 {
        return Vec::new();
    }
    vec![Finding::new(
        "fix",
        Level::Ok,
        format!("cleared {cleared} stale saved model choice(s)"),
    )
    .line(touched.join(", "))]
}

fn session_index_files(home: &Path) -> Vec<PathBuf> {
    let mut files = vec![home.join("chats").join("sessions").join("index.json")];
    if let Ok(text) = std::fs::read_to_string(home.join("projects.json")) {
        if let Ok(list) = serde_json::from_str::<Vec<serde_json::Value>>(&text) {
            for project in list {
                if let Some(root) = project.get("rootPath").and_then(|v| v.as_str()) {
                    files.push(
                        crate::app_paths::project_dir(Path::new(root))
                            .join("sessions")
                            .join("index.json"),
                    );
                }
            }
        }
    }
    files
}

fn bridge_wire_inner(pref: &str) -> Option<&str> {
    for prefix in ["marionette/", "marionette::", "marionette\\"] {
        if let Some(rest) = pref.strip_prefix(prefix) {
            return Some(rest);
        }
    }
    None
}

fn short(text: &str, max: usize) -> String {
    let flat = text.replace(['\r', '\n'], " ");
    if flat.chars().count() <= max {
        return flat;
    }
    let mut out: String = flat.chars().take(max).collect();
    out.push_str("...");
    out
}

