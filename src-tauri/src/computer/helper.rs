//! The Computer Use helper: `marionette.exe --marionette-computer-helper`.
//!
//! It is the only thing that talks to cua-driver, and it owns the driver
//! process: hardened environment (fixed PATH, fresh run dir as HOME, telemetry
//! off, embedded mode), executable hash re-verified before EVERY start, one
//! request in flight (the driver is serial), kill-on-drop. Marionette dying
//! closes the helper's stdin — the helper exits and takes the driver with it.
//!
//! Everything the main process can ask for is a closed op set (protocol.rs);
//! the helper re-assembles each into a fixed driver call. Key chords pass the
//! whitelist (keys.rs) first; paste additionally requires the clipboard to
//! still be exactly what the agent wrote (sequence number).

use std::io::{BufRead, Write};
use std::path::PathBuf;

use serde_json::{json, Value};
use sha2::Digest;

use super::protocol::{
    self, Frame, WindowAction, OP_ACT, OP_CAPTURE, OP_CLIPBOARD_READ, OP_CLIPBOARD_WRITE,
    OP_CONFIGURE, OP_HALT, OP_LIST_WINDOWS, OP_SNAPSHOT, PROTOCOL_VERSION,
};
use super::keys::{self, Scope, Verdict};
use super::win;

/// Helper data dir: the driver's per-launch homes live here too, one run
/// directory each (`~/.marionette/computer-helper/run-<n>/`).
fn data_dir() -> PathBuf {
    crate::app_paths::global_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
        .join("computer-helper")
}

const ACT_TIMEOUT_SECS: u64 = 120;
const WINDOW_STATE_TIMEOUT_SECS: u64 = 60;
const LIST_TIMEOUT_SECS: u64 = 20;

// ---------------------------------------------------------------------------
// Driver client (MCP over stdio, newline-delimited JSON-RPC)
// ---------------------------------------------------------------------------

struct DriverProc {
    child: std::process::Child,
    next_id: u64,
}

impl DriverProc {
    fn spawn(driver_path: &str, version: &str) -> Result<DriverProc, String> {
        // Executable pin, before every start: an install-time check says
        // nothing about a file replaced since.
        let exe = std::path::Path::new(driver_path);
        let mut hasher = sha2::Sha256::new();
        let mut file = std::fs::File::open(exe).map_err(|e| format!("driver open: {e}"))?;
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let read = std::io::Read::read(&mut file, &mut buffer).map_err(|e| format!("driver read: {e}"))?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        let digest = format!("{:x}", hasher.finalize());
        if digest != super::driver::WIN_X64_EXECUTABLE_SHA256 {
            return Err(format!("driver executable sha256 mismatch: {digest}"));
        }

        // Fresh run dir: the driver's HOME for this launch (nothing it writes
        // outlives the launch we can name).
        let base = data_dir();
        let run_dir = base.join(format!("run-{}", std::process::id()));
        std::fs::create_dir_all(run_dir.join("tmp"))
            .map_err(|e| format!("run dir: {e}"))?;

        let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
        let run = run_dir.to_string_lossy().to_string();
        let tmp = run_dir.join("tmp").to_string_lossy().to_string();
        let mut command = std::process::Command::new(driver_path);
        command
            // cua-driver speaks MCP-on-stdio only in this mode (codeg
            // driver_proc.rs pins the same three args; without them the
            // process runs some other surface and never answers initialize).
            .args(["mcp", "--direct", "--no-overlay"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .env_clear()
            .env("CUA_DRIVER_RS_TELEMETRY_ENABLED", "false")
            .env("CUA_TELEMETRY_ENABLED", "false")
            .env("CUA_DRIVER_RS_UPDATE_CHECK", "false")
            .env("CUA_DRIVER_EMBEDDED", "1")
            .env("CUA_DRIVER_DISABLE_RUN_SCRIPT", "1")
            // Ten years: the driver's default idle kill (5 min) would reap a
            // session mid-conversation.
            .env("CUA_DRIVER_RS_SESSION_IDLE_TTL_SECS", "315360000")
            .env("PATH", format!(r"{system_root}\System32;{system_root};{system_root}\System32\Wbem"))
            .env("SystemRoot", &system_root)
            .env("windir", &system_root)
            .env("TEMP", &tmp)
            .env("TMP", &tmp)
            .env("LOCALAPPDATA", &run)
            .env("APPDATA", &run)
            .env("USERPROFILE", &run)
            .current_dir(&run_dir);
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let _ = version;
        let child = command.spawn().map_err(|e| format!("driver spawn: {e}"))?;
        let mut driver = DriverProc { child, next_id: 1 };
        driver.initialize()?;
        Ok(driver)
    }

    fn call_raw(&mut self, method: &str, params: &Value, timeout_secs: u64) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let message = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let stdin = self
            .child
            .stdin
            .as_mut()
            .ok_or("driver stdin is gone")?;
        let line = format!("{message}\n");
        stdin
            .write_all(line.as_bytes())
            .and_then(|_| stdin.flush())
            .map_err(|e| format!("driver write: {e}"))?;
        // Single in flight: read lines until our id answers (skipping any
        // server notifications).
        let stdout = self
            .child
            .stdout
            .as_mut()
            .ok_or("driver stdout is gone")?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
        let mut reader = std::io::BufReader::new(stdout);
        loop {
            if std::time::Instant::now() > deadline {
                return Err(format!("driver timeout: {method}"));
            }
            let mut line = String::new();
            // BufReader::read_line blocks without a deadline of its own; the
            // driver answers every request, so the loop bound above is the
            // practical guard (a hung driver takes the whole helper down at
            // the process level, same as codeg's WRITE_TIMEOUT posture).
            let bytes = reader
                .read_line(&mut line)
                .map_err(|e| format!("driver read: {e}"))?;
            if bytes == 0 {
                return Err("driver closed its stdout".to_string());
            }
            let Ok(answer) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if answer.get("id").and_then(Value::as_u64) == Some(id) {
                return Ok(answer);
            }
        }
    }

    fn initialize(&mut self) -> Result<(), String> {
        let init = self.call_raw(
            "initialize",
            &json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "marionette", "version": env!("CARGO_PKG_VERSION")},
            }),
            20,
        )?;
        if init.get("error").is_some() {
            return Err(format!("driver initialize: {}", init["error"]));
        }
        // notifications/initialized (no id, no answer expected).
        if let Some(stdin) = self.child.stdin.as_mut() {
            let _ = stdin.write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n");
            let _ = stdin.flush();
        }
        Ok(())
    }

    fn tool(&mut self, name: &str, args: &Value, timeout_secs: u64) -> Result<ToolResult, String> {
        let answer = self.call_raw(
            "tools/call",
            &json!({"name": name, "arguments": args}),
            timeout_secs,
        )?;
        if let Some(error) = answer.get("error") {
            return Ok(ToolResult { is_error: true, structured: Some(error.clone()), image: None });
        }
        let result = answer
            .get("result")
            .cloned()
            .ok_or_else(|| "driver answer had no result".to_string())?;
        let is_error = result.get("isError").and_then(Value::as_bool).unwrap_or(false);
        // MCP 2025-06-18 puts structured output in `structuredContent`; the
        // driver's refusals/errors ride inside it at /refusal/code or
        // /error/code (codeg ToolResult).
        let structured = result
            .get("structuredContent")
            .filter(|v| !v.is_null())
            .or_else(|| result.get("structured"))
            .cloned();
        let image = result
            .get("content")
            .and_then(Value::as_array)
            .and_then(|items| {
                items.iter().find_map(|item| {
                    let data = item.get("data").and_then(Value::as_str)?;
                    let mime = item.get("mimeType").and_then(Value::as_str).unwrap_or("image/png");
                    Some((data.to_string(), mime.to_string()))
                })
            });
        Ok(ToolResult { is_error, structured, image })
    }

    fn shutdown(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for DriverProc {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct ToolResult {
    is_error: bool,
    structured: Option<Value>,
    image: Option<(String, String)>,
}

// ---------------------------------------------------------------------------
// Helper state and dispatch
// ---------------------------------------------------------------------------

struct Helper {
    driver: Option<DriverProc>,
    driver_path: Option<String>,
    /// Clipboard sequence number right after the agent's own write; an
    /// unchanged number at paste time is the only paste that goes through.
    agent_clipboard_seq: Option<u32>,
}

fn err(code: &str, message: impl std::fmt::Display) -> Value {
    json!({"code": code, "message": message.to_string()})
}

impl Helper {
    fn new() -> Helper {
        Helper { driver: None, driver_path: None, agent_clipboard_seq: None }
    }

    fn ensure_driver(&mut self) -> Result<&mut DriverProc, Value> {
        if self.driver.is_none() {
            let path = self
                .driver_path
                .clone()
                .ok_or_else(|| err("driver-unconfigured", "configure came first or not at all"))?;
            let driver = DriverProc::spawn(&path, super::driver::DRIVER_VERSION)
                .map_err(|e| err("driver-unavailable", &e))?;
            self.driver = Some(driver);
        }
        Ok(self.driver.as_mut().unwrap())
    }

    fn op_configure(&mut self, params: &Value) -> Result<Value, Value> {
        let path = params
            .get("driverPath")
            .and_then(Value::as_str)
            .ok_or_else(|| err("bad-request", "configure needs driverPath"))?
            .to_string();
        if !std::path::Path::new(&path).is_file() {
            return Err(err("driver-missing", "driver executable is not on disk"));
        }
        // A re-configure with a different path restarts the driver.
        if self.driver_path.as_deref() != Some(path.as_str()) {
            if let Some(driver) = self.driver.as_mut() {
                driver.shutdown();
            }
            self.driver = None;
        }
        self.driver_path = Some(path);
        Ok(json!({"ok": true}))
    }

    /// Windows the driver knows, enriched with process facts and grantability
    /// (own app + blocklist filtered) — the picker list and the tool's
    /// list_windows share this answer.
    fn op_list_windows(&mut self, params: &Value) -> Result<Value, Value> {
        let own_exe = std::env::var("MARIONETTE_OWN_EXE").unwrap_or_default();
        let by_pid = params.get("pid").and_then(Value::as_u64).map(|p| p as u32);
        let driver = self.ensure_driver()?;
        let mut args = json!({});
        if let Some(pid) = by_pid {
            args["pid"] = json!(pid);
        }
        let result = driver
            .tool("list_windows", &args, LIST_TIMEOUT_SECS)
            .map_err(|e| err("driver-unavailable", &e))?;
        if result.is_error {
            return Err(err("driver-failed", &tool_error_text("list_windows", &result)));
        }
        let structured = result.structured.clone();
        let windows = structured
            .as_ref()
            .and_then(|s| s.get("windows").cloned())
            .or(structured)
            .and_then(|w| w.as_array().cloned())
            .unwrap_or_default();
        let mut out = Vec::new();
        for window in windows {
            let Some(pid) = window.get("pid").and_then(Value::as_u64) else { continue };
            let facts = win::process_facts(pid as u32);
            let grantable = match &facts {
                Some(facts) => {
                    !win::is_own_image(&facts.image_path, &own_exe) && !win::is_blocklisted(&facts.image_path)
                }
                None => false,
            };
            out.push(json!({
                "windowId": window.get("window_id").cloned().unwrap_or(window.get("windowId").cloned().unwrap_or(Value::Null)),
                "pid": pid,
                "title": window.get("title").cloned().unwrap_or(Value::Null),
                "bounds": window.get("bounds").cloned().unwrap_or(Value::Null),
                "minimized": window.get("minimized").and_then(Value::as_bool).unwrap_or(false),
                "processImage": facts.as_ref().map(|f| f.image_path.clone()),
                "processStartedAtMs": facts.as_ref().map(|f| f.started_at_ms),
                "grantable": grantable,
            }));
        }
        Ok(json!({ "windows": out }))
    }

    /// Capture a window as a JPEG data URL (native size from the driver,
    /// shrunk helper-side like codeg: the driver never scales, coordinates
    /// stay 1:1).
    fn op_capture(&mut self, params: &Value) -> Result<Value, Value> {
        let (pid, window_id) = target_of(params)?;
        let driver = self.ensure_driver()?;
        let result = driver
            .tool(
                "verify_state",
                &json!({
                    "pid": pid,
                    "window_id": window_id,
                    "expect": [{ "window": { "exists": true } }],
                    "timeout_ms": 0,
                    "stable_samples": 1,
                    "include_screenshot": true,
                }),
                WINDOW_STATE_TIMEOUT_SECS,
            )
            .map_err(|e| err("driver-unavailable", &e))?;
        if result.is_error {
            return Err(err("occluded", "the window could not be captured"));
        }
        let Some((png_base64, mime)) = result.image else {
            return Err(err("occluded", "the window returned no picture"));
        };
        if mime != "image/png" {
            return Err(err("capture-failed", format!("capture came back as {mime}")));
        }
        let max_width = params.get("maxWidth").and_then(Value::as_u64).unwrap_or(1568) as u32;
        let shrunk = shrink_png_to_jpeg(&png_base64, max_width).map_err(|e| err("capture-failed", &e))?;
        Ok(json!({
            "dataUrl": shrunk.0,
            "width": shrunk.1,
            "height": shrunk.2,
        }))
    }

    /// Accessibility tree read (no picture): elements carry the driver's
    /// `element_token`s, which act() accepts.
    fn op_snapshot(&mut self, params: &Value) -> Result<Value, Value> {
        let (pid, window_id) = target_of(params)?;
        let driver = self.ensure_driver()?;
        let result = driver
            .tool(
                "get_window_state",
                &json!({
                    "pid": pid,
                    "window_id": window_id,
                    "include_screenshot": false,
                    "include_accessibility_tree": true,
                    "max_image_dimension": 0,
                }),
                WINDOW_STATE_TIMEOUT_SECS,
            )
            .map_err(|e| err("driver-unavailable", &e))?;
        if result.is_error {
            return Err(err("read-failed", &tool_error_text("get_window_state", &result)));
        }
        let mut structured = result
            .structured
            .clone()
            .unwrap_or_else(|| json!({}));
        // Direct the agent at the tokens the driver mints.
        if let Some(object) = structured.as_object_mut() {
            object.insert("windowId".to_string(), json!(window_id));
            object.insert("pid".to_string(), json!(pid));
        }
        Ok(structured)
    }

    fn op_act(&mut self, params: &Value) -> Result<Value, Value> {
        let (pid, window_id) = target_of(params)?;
        let action: WindowAction =
            serde_json::from_value(params.get("action").cloned().unwrap_or(Value::Null))
                .map_err(|e| err("bad-request", &format!("action: {e}")))?;
        let scope = Scope::Window;
        if let WindowAction::Press { key, .. } = &action {
            match keys::classify(scope, key) {
                Verdict::Allowed => {}
                Verdict::Paste => {
                    // The one paste that goes: the clipboard still holds
                    // exactly what the agent wrote (nothing bumped it since).
                    let unchanged = self
                        .agent_clipboard_seq
                        .map(|seq| win::clipboard_sequence() == seq)
                        .unwrap_or(false);
                    if !unchanged {
                        return Err(err(
                            "paste-refused",
                            "the clipboard is the user's; paste is refused unless it still holds exactly what you wrote with computer_clipboard_write",
                        ));
                    }
                }
                Verdict::Denied => {
                    return Err(err(
                        "key-refused",
                        &format!("the key chord \"{key}\" is outside what a shared window may be asked to press"),
                    ));
                }
            }
        }
        let driver = self.ensure_driver()?;
        let (tool, args) = driver_call(pid, window_id, &action);
        let result = driver
            .tool(&tool, &args, ACT_TIMEOUT_SECS)
            .map_err(|e| err("driver-unavailable", &e))?;
        if result.is_error {
            return Err(err("action-failed", &tool_error_text(&tool, &result)));
        }
        // submit typing ends with Enter.
        if let WindowAction::Type { submit: true, .. } = &action {
            let enter = driver_call(
                pid,
                window_id,
                &WindowAction::Press { key: "Enter".to_string(), token: None },
            );
            let _ = driver.tool(&enter.0, &enter.1, ACT_TIMEOUT_SECS);
        }
        Ok(json!({"done": true, "tool": tool}))
    }

    fn op_clipboard_read(&mut self, _params: &Value) -> Result<Value, Value> {
        // Reading the user's clipboard is only offered while an agent holds
        // an active grant — the main process checks that; the helper just
        // reads (never syncs, never forwards on its own).
        let text = win::clipboard_read_text().unwrap_or_default();
        Ok(json!({"text": text}))
    }

    fn op_clipboard_write(&mut self, params: &Value) -> Result<Value, Value> {
        let text = params
            .get("text")
            .and_then(Value::as_str)
            .ok_or_else(|| err("bad-request", "clipboard-write needs text"))?
            .to_string();
        match win::clipboard_write_text(&text) {
            Some(seq) => {
                self.agent_clipboard_seq = Some(seq);
                Ok(json!({"ok": true}))
            }
            None => Err(err("clipboard-failed", "the clipboard refused the write")),
        }
    }
}

fn target_of(params: &Value) -> Result<(u64, u64), Value> {
    let pid = params
        .get("pid")
        .and_then(Value::as_u64)
        .ok_or_else(|| err("bad-request", "missing pid"))?;
    let window_id = params
        .get("windowId")
        .and_then(Value::as_u64)
        .or_else(|| params.get("window_id").and_then(Value::as_u64))
        .ok_or_else(|| err("bad-request", "missing windowId"))?;
    Ok((pid, window_id))
}

fn tool_error_text(tool: &str, result: &ToolResult) -> String {
    result
        .structured
        .as_ref()
        .and_then(|s| {
            s.pointer("/refusal/code")
                .or_else(|| s.pointer("/error/code"))
                .or_else(|| s.get("message"))
                .or_else(|| s.get("error"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| format!("{tool} failed"))
}

/// The fixed translation of every action into one driver call — the closed
/// set. Coordinates and tokens only; no driver tool name ever crosses.
fn driver_call(pid: u64, window_id: u64, action: &WindowAction) -> (String, Value) {
    let mut args = json!({
        "pid": pid,
        "window_id": window_id,
        "delivery_mode": "background",
    });
    match action {
        WindowAction::Click { x, y, token, button, count, modifiers } => {
            let tool = match (button.as_deref().unwrap_or("left"), count.unwrap_or(1)) {
                ("left", 1) | ("middle", 1) => "click",
                ("left", 2) => "double_click",
                ("right", 1) => "right_click",
                _ => "click",
            };
            if button.as_deref() == Some("middle") {
                args["button"] = json!("middle");
            }
            if !modifiers.is_empty() {
                args["modifier"] = json!(modifiers);
            }
            if let Some(token) = token {
                args["element_token"] = json!(token);
            } else {
                args["x"] = json!(x.unwrap_or(0.0));
                args["y"] = json!(y.unwrap_or(0.0));
            }
            (tool.to_string(), args)
        }
        WindowAction::Drag { from_x, from_y, to_x, to_y, duration_ms } => {
            let duration = duration_ms.unwrap_or(300);
            args["from_x"] = json!(from_x.floor());
            args["from_y"] = json!(from_y.floor());
            args["to_x"] = json!(to_x.floor());
            args["to_y"] = json!(to_y.floor());
            args["button"] = json!("left");
            args["duration_ms"] = json!(duration);
            args["steps"] = json!((duration / 50).clamp(1, 40));
            ("drag".to_string(), args)
        }
        WindowAction::Scroll { direction, amount, unit, x, y, token } => {
            args["direction"] = json!(direction);
            args["amount"] = json!((*amount).clamp(1, 100));
            args["by"] = json!(if unit == "page" { "page" } else { "line" });
            if let Some(token) = token {
                args["element_token"] = json!(token);
            } else if x.is_some() && y.is_some() {
                args["x"] = json!(x);
                args["y"] = json!(y);
            }
            ("scroll".to_string(), args)
        }
        WindowAction::Type { token, text, .. } => {
            if let Some(token) = token {
                args["element_token"] = json!(token);
            }
            args["text"] = json!(text);
            ("type_text".to_string(), args)
        }
        WindowAction::Press { key, token } => {
            if let Some(token) = token {
                args["element_token"] = json!(token);
            }
            args["key"] = json!(key);
            ("press_key".to_string(), args)
        }
        WindowAction::SetValue { token, value } => {
            args["element_token"] = json!(token);
            args["value"] = json!(value);
            ("set_value".to_string(), args)
        }
        WindowAction::Restore => ("bring_to_front".to_string(), args),
    }
}

/// PNG base64 → narrowed JPEG (data URL, width, height); the budget loop from
/// the browser screenshot, same numbers.
fn shrink_png_to_jpeg(png_base64: &str, max_width: u32) -> Result<(String, u32, u32), String> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(png_base64)
        .map_err(|e| format!("decode: {e}"))?;
    let mut image = image::load_from_memory(&bytes).map_err(|e| format!("decode png: {e}"))?;
    if image.width() > max_width {
        let height =
            ((f64::from(image.height()) / f64::from(image.width())) * f64::from(max_width)).round().max(1.0) as u32;
        image = image.resize_exact(max_width, height, image::imageops::FilterType::Lanczos3);
    }
    let mut jpeg = Vec::new();
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 85);
    image.write_with_encoder(encoder).map_err(|e| format!("encode: {e}"))?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(&jpeg);
    Ok((format!("data:image/jpeg;base64,{encoded}"), image.width(), image.height()))
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Run the helper until stdin closes. Returns the process exit code.
pub fn run_helper() -> i32 {
    let dir = data_dir();
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::env::set_current_dir(&dir);
    let mut stdout = std::io::stdout();
    let mut stdin = std::io::stdin();
    if protocol::write_frame(&mut stdout, &Frame::Ready { protocol: PROTOCOL_VERSION }).is_err() {
        return 2;
    }
    let mut helper = Helper::new();
    loop {
        let frame = match protocol::read_frame(&mut stdin) {
            Ok(frame) => frame,
            Err(_) => break, // stdin closed: Marionette is gone
        };
        let Frame::Request { id, op, params } = frame else {
            continue;
        };
        let reply = if !protocol::OPS.contains(&op.as_str()) {
            Err(err("bad-request", &format!("unknown op: {op}")))
        } else {
            match op.as_str() {
                OP_CONFIGURE => helper.op_configure(&params),
                OP_LIST_WINDOWS => helper.op_list_windows(&params),
                OP_CAPTURE => helper.op_capture(&params),
                OP_SNAPSHOT => helper.op_snapshot(&params),
                OP_ACT => helper.op_act(&params),
                OP_CLIPBOARD_READ => helper.op_clipboard_read(&params),
                OP_CLIPBOARD_WRITE => helper.op_clipboard_write(&params),
                OP_HALT => {
                    if let Some(driver) = helper.driver.as_mut() {
                        driver.shutdown();
                    }
                    helper.driver = None;
                    Ok(json!({"halted": true}))
                }
                _ => Err(err("bad-request", "unreachable")),
            }
        };
        let frame = match reply {
            Ok(result) => Frame::Reply { id, ok: true, result: Some(result), error: None },
            Err(error) => Frame::Reply { id, ok: false, result: None, error: Some(error) },
        };
        if protocol::write_frame(&mut stdout, &frame).is_err() {
            break;
        }
    }
    if let Some(driver) = helper.driver.as_mut() {
        driver.shutdown();
    }
    0
}
