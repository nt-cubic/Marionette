//! Computer Use (batch 3): the main-process half.
//!
//! The helper (`marionette.exe --marionette-computer-helper`, helper.rs) owns
//! cua-driver; this module owns everything around it — helper process
//! lifecycle, the grant table (Window/App/Screen, TTL from last use), the
//! permission-card flow agent-initiated actions go through, the global Stop
//! shortcut, and the commands the share dialog drives. The tool surface lives
//! in tools_mcp and calls the SAME grant checks — never a second set.

pub mod driver;
pub mod keys;
pub mod protocol;
#[cfg(target_os = "windows")]
pub mod helper;
#[cfg(target_os = "windows")]
pub mod win;

use std::collections::HashMap;
use std::sync::mpsc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

/// Grants lapse after this much DISUSE (checked at use, from the last read —
/// wall clock, so sleep counts too). Configurable later; the number first.
const GRANT_TTL: Duration = Duration::from_secs(30 * 60);

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize)]
#[allow(dead_code)] // App scope is the protocol's full shape; sharing UI grows into it.
pub enum TargetKey {
    Window { pid: u32, started_at_ms: u64, window_id: u64 },
    App { pid: u32, started_at_ms: u64 },
    Screen,
}

impl TargetKey {
    fn from_params(params: &Value) -> Option<TargetKey> {
        match params.get("kind").and_then(Value::as_str)? {
            "screen" => Some(TargetKey::Screen),
            "window" => Some(TargetKey::Window {
                pid: params.get("pid").and_then(Value::as_u64)? as u32,
                started_at_ms: params.get("startedAtMs").and_then(Value::as_u64)?,
                window_id: params.get("windowId").and_then(Value::as_u64)?,
            }),
            "app" => Some(TargetKey::App {
                pid: params.get("pid").and_then(Value::as_u64)? as u32,
                started_at_ms: params.get("startedAtMs").and_then(Value::as_u64)?,
            }),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    None,
    Read,
    Control,
}

impl Level {
    pub fn allows(self, required: Level) -> bool {
        (self as u8) >= (required as u8)
    }
    pub fn parse(value: &str) -> Level {
        match value {
            "read" => Level::Read,
            "control" => Level::Control,
            _ => Level::None,
        }
    }
}

struct Grant {
    level: Level,
    last_used: Instant,
    /// Human label for the share list / permission card.
    label: String,
}

struct State {
    grants: HashMap<TargetKey, Grant>,
    helper: Option<HelperProc>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> R {
    let mut guard = STATE.lock().expect("computer state poisoned");
    let state = guard.get_or_insert_with(|| State { grants: HashMap::new(), helper: None });
    f(state)
}

// ---------------------------------------------------------------------------
// Helper process
// ---------------------------------------------------------------------------

/// The helper's IO plumbing, shared between requesters and the reader
/// thread. Deliberately NOT behind the STATE lock: a request waits on its
/// reply channel for seconds — holding STATE meanwhile would deadlock the
/// reader's routing (observed as "helper did not answer configure in time").
struct HelperIo {
    writer: std::process::ChildStdin,
    pending: HashMap<u64, mpsc::Sender<Result<Value, String>>>,
    next_id: u64,
}

struct HelperProc {
    child: std::process::Child,
    io: std::sync::Arc<Mutex<HelperIo>>,
}

impl HelperProc {
    fn spawn(app: &AppHandle) -> Result<HelperProc, String> {
        let exe = std::env::current_exe().map_err(|e| format!("current exe: {e}"))?;
        let mut command = std::process::Command::new(&exe);
        command
            .arg("--marionette-computer-helper")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .env("MARIONETTE_OWN_EXE", exe.to_string_lossy().to_string());
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = command.spawn().map_err(|e| format!("helper spawn: {e}"))?;
        let writer = child.stdin.take().ok_or("helper stdin")?;
        let reader = child.stdout.take().ok_or("helper stdout")?;
        let io = std::sync::Arc::new(Mutex::new(HelperIo {
            writer,
            pending: HashMap::new(),
            next_id: 1,
        }));
        // Reader thread: routes each reply through the IO lock only — never
        // STATE — so routing works even while a requester blocks on its
        // channel inside a with_state closure (the self-deadlock this whole
        // design exists to prevent).
        let io_for_reader = io.clone();
        let app_for_reader = app.clone();
        std::thread::Builder::new()
            .name("computer-helper-reader".to_string())
            .spawn(move || {
                let mut reader = std::io::BufReader::new(reader);
                loop {
                    match protocol::read_frame(&mut reader) {
                        Ok(protocol::Frame::Ready { protocol: version }) => {
                            if version != protocol::PROTOCOL_VERSION {
                                break;
                            }
                        }
                        Ok(protocol::Frame::Reply { id, ok, result, error }) => {
                            if let Ok(mut io) = io_for_reader.lock() {
                                if let Some(sender) = io.pending.remove(&id) {
                                    let answer = if ok {
                                        Ok(result.unwrap_or(Value::Null))
                                    } else {
                                        Err(error
                                            .map(|e| e.to_string())
                                            .unwrap_or_else(|| "helper error".to_string()))
                                    };
                                    let _ = sender.send(answer);
                                }
                            }
                        }
                        _ => break,
                    }
                }
                // Helper gone: fail every pending caller and clear the proc.
                if let Ok(mut io) = io_for_reader.lock() {
                    for (_, sender) in io.pending.drain() {
                        let _ = sender.send(Err("helper exited".to_string()));
                    }
                }
                let _ = app_for_reader.emit("marionette-computer-state", json!({"helper": "gone"}));
                with_state(|state| {
                    if let Some(helper) = state.helper.as_mut() {
                        let _ = helper.child.kill();
                    }
                    state.helper = None;
                });
            })
            .map_err(|e| format!("reader thread: {e}"))?;
        Ok(HelperProc { child, io })
    }

    /// One round trip. The IO lock is held only to register the pending
    /// channel and write the frame; the wait happens lock-free.
    fn request(&self, op: &str, params: Value, timeout: Duration) -> Result<Value, String> {
        let (tx, rx) = mpsc::channel();
        let frame = {
            let mut io = self
                .io
                .lock()
                .map_err(|_| "helper io lock poisoned".to_string())?;
            let id = io.next_id;
            io.next_id += 1;
            io.pending.insert(id, tx);
            protocol::Frame::Request { id, op: op.to_string(), params }
        };
        {
            let mut io = self
                .io
                .lock()
                .map_err(|_| "helper io lock poisoned".to_string())?;
            if protocol::write_frame(&mut io.writer, &frame).is_err() {
                // Drop the registration on the floor; the channel closes and
                // the recv below fails immediately.
            }
        }
        rx.recv_timeout(timeout)
            .map_err(|_| format!("helper did not answer {op} in time"))?
    }
}

/// One helper round trip, spawning/restarting the helper when needed.
fn helper_request(app: &AppHandle, op: &str, params: Value, timeout: Duration) -> Result<Value, String> {
    if !driver::is_installed() {
        return Err("driver-not-installed".to_string());
    }
    let helper_was_up = with_state(|state| state.helper.is_some());
    if !helper_was_up {
        let proc = HelperProc::spawn(app)?;
        // Register BEFORE configure: the reader thread routes replies through
        // STATE.helper — a proc that waits for configure before entering the
        // table can never receive its own configure answer (the
        // "helper did not answer configure in time" self-deadlock, 2026-11-01).
        with_state(|state| state.helper = Some(proc));
        let exe = driver::installed_exe_path()?;
        let configured = with_state(|state| match state.helper.as_mut() {
            Some(helper) => helper.request(
                protocol::OP_CONFIGURE,
                json!({"driverPath": exe.to_string_lossy()}),
                Duration::from_secs(15),
            ),
            None => Err("helper vanished during configure".to_string()),
        });
        if configured.is_err() {
            // Drop the half-born helper entirely; the next call retries.
            with_state(|state| {
                if let Some(helper) = state.helper.as_mut() {
                    let _ = helper.child.kill();
                }
                state.helper = None;
            });
            return Err("helper did not come up (configure failed)".to_string());
        }
    }
    let result = with_state(|state| match state.helper.as_mut() {
        Some(helper) => helper.request(op, params, timeout),
        None => Err("helper is gone".to_string()),
    });
    if result.is_err() {
        // A dead helper is not kept around to poison the next call.
        with_state(|state| {
            if let Some(helper) = state.helper.as_mut() {
                let _ = helper.child.kill();
            }
            state.helper = None;
        });
    }
    result
}

// ---------------------------------------------------------------------------
// Grants
// ---------------------------------------------------------------------------

fn grant_for(key: &TargetKey) -> Option<Level> {
    with_state(|state| match state.grants.get_mut(key) {
        Some(grant) => {
            if grant.last_used.elapsed() > GRANT_TTL {
                state.grants.remove(key);
                return None;
            }
            grant.last_used = Instant::now();
            Some(grant.level)
        }
        None => None,
    })
}

fn set_grant(key: TargetKey, level: Level, label: String) {
    with_state(|state| match level {
        Level::None => {
            state.grants.remove(&key);
        }
        _ => {
            state.grants.insert(key, Grant { level, label, last_used: Instant::now() });
        }
    });
}

pub fn revoke_all() {
    // Halt the driver too: an in-flight capture dies with it.
    let _ = helper_request_global(protocol::OP_HALT, json!({}));
    with_state(|state| state.grants.clear());
}

/// Halt path (Stop shortcut / revoke-all): fire a halt frame and kill the
/// helper — the reader thread drops with it and takes the driver down.
fn helper_request_global(op: &str, params: Value) -> Result<Value, String> {
    with_state(|state| {
        let Some(helper) = state.helper.as_mut() else {
            return Ok(json!({"halted": true, "helper": false}));
        };
        let frame = protocol::Frame::Request {
            id: {
                let mut io = helper.io.lock().map_err(|_| "io poisoned".to_string())?;
                let id = io.next_id;
                io.next_id += 1;
                id
            },
            op: op.to_string(),
            params,
        };
        if let Ok(mut io) = helper.io.lock() {
            let _ = protocol::write_frame(&mut io.writer, &frame);
        }
        let _ = helper.child.kill();
        state.helper = None;
        Ok(json!({"halted": true}))
    })
}

/// The refusal note the agent reads when a target is not shared (refusal is
/// a value; the turn survives).
fn grant_required_note() -> &'static str {
    "This window is not shared. Ask the user to share it from Marionette's \
     Computer Use panel (Ctrl K → 共享窗口给 agent), then retry."
}

fn read_refusal(target: Value) -> Value {
    json!({"target": target, "error": "computer_grant_required", "note": grant_required_note()})
}

/// Read-level gate with the agent-initiated card: unshared target → the user
/// answers (allow once / share it read / deny); denial returns the refusal.
fn ensure_read(app: &AppHandle, session_id: &str, key: &TargetKey, label: &str, target: Value) -> Result<Value, String> {
    if grant_for(key).unwrap_or(Level::None).allows(Level::Read) {
        return Ok(json!({"ok": true}));
    }
    if crate::browser::session_auto_approves(session_id) {
        return Ok(json!({"ok": true, "once": true}));
    }
    let allowed = crate::browser::request_permission(
        app,
        session_id,
        "computer",
        "",
        label,
        &format!("read this window ({label})"),
    );
    if allowed {
        return Ok(json!({"ok": true, "once": true}));
    }
    Ok(read_refusal(target))
}

/// Control-level gate: same card, sharing at Control when remembered.
#[allow(clippy::too_many_arguments)]
fn ensure_control(
    app: &AppHandle,
    session_id: &str,
    key: &TargetKey,
    label: &str,
    target: Value,
    action_summary: &str,
) -> Result<Value, String> {
    if grant_for(key).unwrap_or(Level::None).allows(Level::Control) {
        return Ok(json!({"ok": true}));
    }
    if crate::browser::session_auto_approves(session_id) {
        return Ok(json!({"ok": true, "once": true}));
    }
    let allowed = crate::browser::request_permission(
        app,
        session_id,
        "computer",
        "",
        label,
        action_summary,
    );
    if allowed {
        return Ok(json!({"ok": true, "once": true}));
    }
    Ok(json!({
        "target": target,
        "error": "computer_control_required",
        "note": grant_required_note(),
    }))
}

// ---------------------------------------------------------------------------
// Tool entry points (tools_mcp calls these — one gate per capability)
// ---------------------------------------------------------------------------

pub fn tool_list_windows(app: &AppHandle) -> Result<Value, String> {
    helper_request(app, protocol::OP_LIST_WINDOWS, json!({}), Duration::from_secs(30))
}

/// Window identity: (pid, started_at, driver window id). `started_at` comes
/// from the listing row (the helper attached the process facts).
fn window_key_of(window: &Value) -> Option<TargetKey> {
    let pid = window.get("pid").and_then(Value::as_u64)? as u32;
    let window_id = window
        .get("windowId")
        .and_then(Value::as_u64)
        .or_else(|| window.get("window_id").and_then(Value::as_u64))?;
    let started_at_ms = window.get("processStartedAtMs").and_then(Value::as_u64).unwrap_or(0);
    Some(TargetKey::Window { pid, started_at_ms, window_id })
}

pub fn tool_capture(
    app: &AppHandle,
    session_id: &str,
    params: &Value,
) -> Result<Value, String> {
    let listing = helper_request(app, protocol::OP_LIST_WINDOWS, json!({}), Duration::from_secs(30))?;
    let Some(row) = find_window_row(&listing, params) else {
        return Ok(json!({"error": "computer_no_such_target"}));
    };
    let Some(key) = window_key_of(&row) else {
        return Ok(json!({"error": "computer_no_such_target"}));
    };
    let label = row.get("title").and_then(Value::as_str).unwrap_or("window").to_string();
    let target = json!({"pid": params.get("pid"), "windowId": params.get("windowId")});
    let gate = ensure_read(app, session_id, &key, &label, target.clone())?;
    if gate.get("ok").is_none() {
        return Ok(gate);
    }
    let mut call_params = json!({
        "pid": params.get("pid"),
        "windowId": params.get("windowId"),
    });
    if let Some(max_width) = params.get("maxWidth") {
        call_params["maxWidth"] = max_width.clone();
    }
    helper_request(app, protocol::OP_CAPTURE, call_params, Duration::from_secs(90))
}

pub fn tool_snapshot(
    app: &AppHandle,
    session_id: &str,
    params: &Value,
) -> Result<Value, String> {
    let listing = helper_request(app, protocol::OP_LIST_WINDOWS, json!({}), Duration::from_secs(30))?;
    let Some(row) = find_window_row(&listing, params) else {
        return Ok(json!({"error": "computer_no_such_target"}));
    };
    let Some(key) = window_key_of(&row) else {
        return Ok(json!({"error": "computer_no_such_target"}));
    };
    let label = row.get("title").and_then(Value::as_str).unwrap_or("window").to_string();
    let target = json!({"pid": params.get("pid"), "windowId": params.get("windowId")});
    let gate = ensure_read(app, session_id, &key, &label, target.clone())?;
    if gate.get("ok").is_none() {
        return Ok(gate);
    }
    let mut tree = helper_request(
        app,
        protocol::OP_SNAPSHOT,
        json!({"pid": params.get("pid"), "windowId": params.get("windowId")}),
        Duration::from_secs(90),
    )?;
    // Trim the serialized tree to the tool's default budget.
    if let Some(object) = tree.as_object_mut() {
        object.insert("title".to_string(), json!(label));
    }
    Ok(tree)
}

pub fn tool_act(app: &AppHandle, session_id: &str, params: &Value) -> Result<Value, String> {
    let listing = helper_request(app, protocol::OP_LIST_WINDOWS, json!({}), Duration::from_secs(30))?;
    let Some(row) = find_window_row(&listing, params) else {
        return Ok(json!({"error": "computer_no_such_target"}));
    };
    let Some(key) = window_key_of(&row) else {
        return Ok(json!({"error": "computer_no_such_target"}));
    };
    let label = row.get("title").and_then(Value::as_str).unwrap_or("window").to_string();
    let action: protocol::WindowAction =
        serde_json::from_value(params.get("action").cloned().unwrap_or(Value::Null))
            .map_err(|e| format!("bad action: {e}"))?;
    let target = json!({"pid": params.get("pid"), "windowId": params.get("windowId")});
    let summary = format!("{} on window \"{}\"", action.summary(), label);
    let gate = ensure_control(app, session_id, &key, &label, target.clone(), &summary)?;
    if gate.get("ok").is_none() {
        return Ok(gate);
    }
    let acted = helper_request(
        app,
        protocol::OP_ACT,
        json!({"pid": params.get("pid"), "windowId": params.get("windowId"), "action": params.get("action")}),
        Duration::from_secs(180),
    )?;
    Ok(acted)
}

pub fn tool_clipboard(app: &AppHandle, session_id: &str, write: bool, params: &Value) -> Result<Value, String> {
    // The clipboard is the user's: both directions need an active grant
    // somewhere (a shared session in flight), not just any session.
    let active = with_state(|state| {
        state
            .grants
            .values()
            .any(|grant| grant.last_used.elapsed() <= GRANT_TTL && grant.level.allows(Level::Read))
    });
    if !active && !crate::browser::session_auto_approves(session_id) {
        return Ok(json!({
            "error": "computer_grant_required",
            "note": "The clipboard needs an active shared window. Ask the user to share one first.",
        }));
    }
    let (op, call) = if write {
        (protocol::OP_CLIPBOARD_WRITE, json!({"text": params.get("text")}))
    } else {
        (protocol::OP_CLIPBOARD_READ, json!({}))
    };
    helper_request(app, op, call, Duration::from_secs(30))
}

fn find_window_row(listing: &Value, params: &Value) -> Option<Value> {
    let pid = params.get("pid").and_then(Value::as_u64)?;
    let window_id = params
        .get("windowId")
        .and_then(Value::as_u64)
        .or_else(|| params.get("window_id").and_then(Value::as_u64))?;
    listing
        .get("windows")
        .and_then(Value::as_array)?
        .iter()
        .find(|row| {
            row.get("pid").and_then(Value::as_u64) == Some(pid)
                && (row.get("windowId").and_then(Value::as_u64) == Some(window_id)
                    || row.get("window_id").and_then(Value::as_u64) == Some(window_id))
        })
        .cloned()
}

// ---------------------------------------------------------------------------
// Frontend commands
// ---------------------------------------------------------------------------

#[tauri::command(async)]
pub fn computer_driver_status() -> Result<Value, String> {
    Ok(driver::driver_status())
}

#[tauri::command(async)]
pub fn computer_install_driver(app: AppHandle) -> Result<Value, String> {
    driver::install(&app)
}

#[tauri::command(async)]
pub fn computer_uninstall_driver(app: AppHandle) -> Result<Value, String> {
    revoke_all();
    let _ = app.emit("marionette-computer-state", json!({"shares": []}));
    driver::uninstall()?;
    Ok(json!({"ok": true}))
}

#[tauri::command(async)]
pub fn computer_list_shareable(app: AppHandle) -> Result<Value, String> {
    // A helper/driver hiccup (cold start, slow handshake) must NOT read as
    // "driver not installed" — that flipped the dialog back to the install
    // button right after a successful install (2026-11-01). Degrade to an
    // empty list + the reason.
    let (listing, helper_error) = if driver::is_installed() {
        match helper_request(&app, protocol::OP_LIST_WINDOWS, json!({}), Duration::from_secs(30)) {
            Ok(listing) => (listing, None),
            Err(error) => (json!({"windows": []}), Some(error)),
        }
    } else {
        (json!({"windows": []}), None)
    };
    let mut windows = listing
        .get("windows")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for window in windows.iter_mut() {
        if let Some(key) = window_key_of(window) {
            let level = grant_for(&key).unwrap_or(Level::None);
            window["shared"] = json!(match level {
                Level::None => "none",
                Level::Read => "read",
                Level::Control => "control",
            });
        } else {
            window["shared"] = json!("none");
        }
    }
    let shares: Vec<Value> = with_state(|state| {
        state
            .grants
            .iter()
            .map(|(key, grant)| {
                json!({
                    "key": serde_json::to_value(key).unwrap_or(Value::Null),
                    "level": match grant.level {
                        Level::None => "none",
                        Level::Read => "read",
                        Level::Control => "control",
                    },
                    "label": grant.label,
                })
            })
            .collect()
    });
    Ok(json!({
        "windows": windows,
        "shares": shares,
        "driver": driver::driver_status(),
        "helperError": helper_error,
    }))
}

#[tauri::command(async)]
pub fn computer_share(app: AppHandle, target: Value, level: String) -> Result<Value, String> {
    // Sharing requires the driver (windows are addressed by the driver's ids).
    if !driver::is_installed() {
        return Err("driver-not-installed".to_string());
    }
    let Some(key) = TargetKey::from_params(&target) else {
        return Err("bad target".to_string());
    };
    // Own app and blocklisted processes never become shareable, by hand or
    // otherwise — the listing marks them; the share itself re-checks.
    if let TargetKey::Window { pid, window_id, .. } = &key {
        let listing = helper_request(&app, protocol::OP_LIST_WINDOWS, json!({}), Duration::from_secs(30))?;
        let row = listing
            .get("windows")
            .and_then(Value::as_array)
            .and_then(|rows| {
                rows.iter().find(|row| {
                    row.get("pid").and_then(Value::as_u64) == Some(*pid as u64)
                        && (row.get("windowId").and_then(Value::as_u64) == Some(*window_id)
                            || row.get("window_id").and_then(Value::as_u64) == Some(*window_id))
                })
            })
            .cloned();
        match row {
            Some(row) if row.get("grantable").and_then(Value::as_bool) == Some(true) => {
                let label = row
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or("window")
                    .to_string();
                set_grant(key.clone(), Level::parse(&level), label);
            }
            _ => {
                return Err("this window cannot be shared".to_string());
            }
        }
    } else {
        let label = match &key {
            TargetKey::Screen => crate::ts!("整个屏幕", "Entire screen").to_string(),
            _ => "app".to_string(),
        };
        set_grant(key.clone(), Level::parse(&level), label);
    }
    let _ = app.emit("marionette-computer-state", json!({"updated": true}));
    Ok(json!({"ok": true}))
}

#[tauri::command(async)]
pub fn computer_revoke_all(app: AppHandle) -> Result<Value, String> {
    revoke_all();
    let _ = app.emit("marionette-computer-state", json!({"updated": true}));
    Ok(json!({"ok": true}))
}

/// Global Stop shortcut handler (Ctrl+Alt+Shift+S default, registered in
/// main.rs): revoke everything, halt the driver, note the stop.
pub fn stop_shortcut_pressed(app: &AppHandle) {
    revoke_all();
    let _ = app.emit("marionette-computer-state", json!({"stopped": true}));
    crate::debug_log::append("computer", "info", "", "stop shortcut: all shares revoked", None);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_rank() {
        assert!(Level::Control.allows(Level::Read));
        assert!(!Level::Read.allows(Level::Control));
    }

    #[test]
    fn target_keys_parse_the_frontend_shape() {
        let key = TargetKey::from_params(&json!({
            "kind": "window", "pid": 8, "startedAtMs": 99, "windowId": 4242
        }));
        assert_eq!(
            key,
            Some(TargetKey::Window { pid: 8, started_at_ms: 99, window_id: 4242 })
        );
        assert_eq!(TargetKey::from_params(&json!({"kind": "screen"})), Some(TargetKey::Screen));
        assert_eq!(TargetKey::from_params(&json!({"kind": "nope"})), None);
    }
}
