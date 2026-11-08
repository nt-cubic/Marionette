//! Child-webview browser surfaces (Windows).
//!
//! A browser tab is a raw wry child webview of the main window — tauri knows
//! nothing about it, which is exactly what keeps the app's own webview alone
//! (codeg `surface_child.rs` shape). Everything agent-facing on top of the
//! page — the aria engine, the send primitive — lives in a CDP **isolated
//! world** installed here: an unshared page never gets read code injected
//! into it (`agent.bundle.js` is evaluated only after a read is granted).
//!
//! wry's `WebView` is not `Send`: every surface lives on the main thread in a
//! thread-local registry, and the public entry points hop there through
//! `run_on_main_thread` + a oneshot. The one rule from codeg that travels
//! with this design: **never call a surface handle while holding the registry
//! borrow** — the CDP waits below pump the message loop, and whatever that
//! pump dispatches (a tab closing, another opening) must find the registry
//! free. Every entry point below clones its `Rc<Surface>` out of the borrow
//! before doing anything slow.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::sync::mpsc;

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, WebviewWindow};
use wry::{NewWindowResponse, Rect, WebView, WebViewBuilder, WebContext, WebViewExtWindows};
use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2;
use webview2_com::{
    CallDevToolsProtocolMethodCompletedHandler, DevToolsProtocolEventReceivedEventHandler,
};
use windows::core::{HSTRING, Interface, PWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::UI::Shell::{DefSubclassProc, GetWindowSubclass, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClientRect, SetWindowPos, SIZE_MINIMIZED, SWP_ASYNCWINDOWPOS,
    SWP_NOACTIVATE, SWP_NOZORDER, WM_ENTERSIZEMOVE, WM_MOVE, WM_MOVING, WM_NCDESTROY,
    WM_SETFOCUS, WM_SIZE,
};

use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2Controller, COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC,
};

use super::agent::{self, SnapshotRequest};

/// Isolated world name + binding — must match the vendored bundle (js/VENDOR.md).
pub const WORLD_NAME: &str = "codeg";
pub const BINDING_NAME: &str = "__codegSend";

/// The helper with the engine-console flag prepended (WebView2 reports the
/// page console itself over CDP; without the flag the helper would relay
/// every line a second time). Mirrors codeg `channel.rs::HELPER_JS_ENGINE_CONSOLE`.
const HELPER_JS: &str = concat!(
    "globalThis.__codegEngineConsole = true;\n",
    include_str!("js/helper.js")
);

/// Console ring: lines per tab, per document (cleared on navigation).
const CONSOLE_RING_CAPACITY: usize = 200;
const CONSOLE_MAX_TEXT_CHARS: usize = 4096;
/// Screenshot budget: base64 bytes (codeg `capture.rs`), JPEG quality, widths.
pub const CAPTURE_MAX_ENCODED_BYTES: usize = 3_500_000;
const CAPTURE_JPEG_QUALITY: u8 = 85;
const CAPTURE_DEFAULT_MAX_WIDTH: u32 = 1568;
const CAPTURE_MIN_WIDTH: u32 = 64;

/// Agent access level on one tab. None < Read < Control, hand-ranked so a new
/// level forces a look at every comparison (codeg `agent.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrantLevel {
    None,
    Read,
    Control,
}

impl GrantLevel {
    fn rank(self) -> u8 {
        match self {
            GrantLevel::None => 0,
            GrantLevel::Read => 1,
            GrantLevel::Control => 2,
        }
    }
    pub fn allows(self, required: GrantLevel) -> bool {
        self.rank() >= required.rank()
    }
    pub fn as_str(self) -> &'static str {
        match self {
            GrantLevel::None => "none",
            GrantLevel::Read => "read",
            GrantLevel::Control => "control",
        }
    }
    pub fn parse(value: &str) -> GrantLevel {
        match value {
            "read" => GrantLevel::Read,
            "control" => GrantLevel::Control,
            _ => GrantLevel::None,
        }
    }
}

/// A grant lives on the tab, bound to one origin: any navigation that leaves
/// the origin drops it (codeg `agent.rs` — no second map to keep in sync).
/// `origin`/`granted_at_ms` are audit bookkeeping for the activity view the
/// frontend will grow — kept so grants round-trip whole.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct AgentGrant {
    pub level: GrantLevel,
    pub origin: String,
    pub granted_at_ms: u64,
}

/// One console line, page-controlled text capped at the boundary.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsoleLine {
    pub level: String,
    pub text: String,
    pub at_ms: u64,
}

/// Frontend-facing tab summary (event `marionette-browser-tabs`).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TabInfo {
    pub id: String,
    pub url: String,
    pub title: String,
    pub origin: String,
    pub grant: String,
    pub opened_by_agent: bool,
}

pub struct Surface {
    pub id: String,
    pub opened_by_agent: bool,
    pub webview: WebView,
    /// World execution context id (CDP), when the world exists for the
    /// current document. Cleared when that context is destroyed.
    world_ctx: Cell<Option<i64>>,
    /// The CDP world machinery has been installed for this surface (lazy,
    /// first read — see finish_surface).
    world_installed: Cell<bool>,
    /// Bumped on every main-frame navigation; folded into the snapshot epoch
    /// so a ref from before a navigation is refused after it.
    pub nav_epoch: Cell<u64>,
    /// Bumped on every snapshot taken; the generation half of the epoch.
    pub generation: Cell<u64>,
    console: RefCell<VecDeque<ConsoleLine>>,
    pub(crate) url: RefCell<String>,
    title: RefCell<String>,
    pub(crate) origin: RefCell<String>,
    pub grant: RefCell<Option<AgentGrant>>,
}

impl Surface {
    fn info(&self) -> TabInfo {
        TabInfo {
            id: self.id.clone(),
            url: self.url.borrow().clone(),
            title: self.title.borrow().clone(),
            origin: self.origin.borrow().clone(),
            grant: self
                .grant
                .borrow()
                .as_ref()
                .map(|g| g.level.as_str().to_string())
                .unwrap_or_else(|| "none".to_string()),
            opened_by_agent: self.opened_by_agent,
        }
    }
}

thread_local! {
    static SURFACES: RefCell<HashMap<String, Rc<Surface>>> = RefCell::new(HashMap::new());
    static PTR_TO_ID: RefCell<HashMap<usize, String>> = RefCell::new(HashMap::new());
    static NEXT_ID: Cell<u64> = Cell::new(1);
    /// One WebContext per profile, shared by every tab of that profile — this
    /// is what keeps cookies/storage in one place and popups from being
    /// refused across environments (codeg WEB_CONTEXTS shape: contexts stored
    /// by value, borrowed for the build, never dropped while tabs live).
    static WEB_CONTEXTS: RefCell<HashMap<String, WebContext>> = RefCell::new(HashMap::new());
    static RESIZE_HOOK: Cell<bool> = Cell::new(false);
}

/// `PWSTR` out-param helper (codeg shim shape).
fn pwstr_out(read: impl FnOnce(*mut PWSTR) -> windows::core::Result<()>) -> Option<String> {
    let mut value = PWSTR::null();
    read(&mut value).ok()?;
    Some(webview2_com::take_pwstr(value))
}

/// All tabs as the frontend sees them. Main thread.
pub fn tabs_snapshot() -> Vec<TabInfo> {
    SURFACES.with(|surfaces| surfaces.borrow().values().map(|s| s.info()).collect())
}

/// Push the tab list to the frontend. Call on the main thread after any
/// mutation; safe to call from anywhere via `emit_tabs_via`.
pub fn emit_tabs(app: &AppHandle) {
    let _ = app.emit("marionette-browser-tabs", tabs_snapshot());
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Wall clock for grant bookkeeping (mod.rs).
pub fn now_ms_pub() -> u64 {
    now_ms()
}

// ---------------------------------------------------------------------------
// CDP plumbing (codeg shim/windows.rs shape, reduced to what we use)
// ---------------------------------------------------------------------------

/// Call a CDP method and pump the message loop until its completion arrives.
/// The caller must NOT hold the surfaces borrow: the pump can dispatch
/// surface lifecycle (that rule is why every public fn clones its Rc first).
fn call_and_wait(webview: &ICoreWebView2, method: &str, params: &str) -> Result<String, String> {
    let answer = Rc::new(RefCell::new(String::new()));
    let received = answer.clone();
    let webview = webview.clone();
    let name = HSTRING::from(method);
    let params = HSTRING::from(params);
    CallDevToolsProtocolMethodCompletedHandler::wait_for_async_operation(
        Box::new(move |handler| {
            // SAFETY: main thread, live webview; the handler outlives the call.
            unsafe { webview.CallDevToolsProtocolMethod(&name, &params, &handler) }
                .map_err(Into::into)
        }),
        Box::new(move |result, json| {
            result?;
            *received.borrow_mut() = json;
            Ok(())
        }),
    )
    .map_err(|err| format!("{method} failed: {err:?}"))?;
    let out = answer.borrow().clone();
    Ok(out)
}

/// Route one CDP event into `on_event`. The receiver stays alive as long as
/// the surface's webview does (WebView2 owns the handler).
fn subscribe(webview: &ICoreWebView2, key: usize, event: &str) -> Result<(), String> {
    let name = HSTRING::from(event);
    // SAFETY: main thread, live webview.
    let receiver = unsafe { webview.GetDevToolsProtocolEventReceiver(&name) }
        .map_err(|e| format!("cannot subscribe to {event}: {e}"))?;
    let received = event.to_string();
    // SAFETY: main thread; WebView2 owns the handler.
    let mut token = 0i64;
    unsafe {
        receiver.add_DevToolsProtocolEventReceived(
            &DevToolsProtocolEventReceivedEventHandler::create(Box::new(move |_, args| {
                if let Some(args) = args {
                    let json = pwstr_out(|out| args.ParameterObjectAsJson(out));
                    if let Some(json) = json {
                        on_cdp_event(key, &received, &json);
                    }
                }
                Ok(())
            })),
            &mut token,
        )
        .map_err(|e| format!("subscribe {event} failed: {e}"))?;
    }
    Ok(())
}

/// Central CDP event fan-out: contexts, console, page navigation, and the
/// helper's binding messages. `key` is the `ICoreWebView2` pointer — the one
/// identity a binding message reports.
fn on_cdp_event(key: usize, event: &str, parameter_json: &str) {
    let value: Value = match serde_json::from_str(parameter_json) {
        Ok(value) => value,
        Err(_) => return,
    };
    let tab_id = match PTR_TO_ID.with(|map| map.borrow().get(&key).cloned()) {
        Some(id) => id,
        None => return,
    };
    let surface = match SURFACES.with(|surfaces| surfaces.borrow().get(&tab_id).cloned()) {
        Some(surface) => surface,
        None => return,
    };
    match event {
        "Runtime.executionContextCreated" => {
            let name = value["context"]["name"].as_str().unwrap_or("");
            let id = value["context"]["id"].as_i64();
            if name == WORLD_NAME {
                if let Some(id) = id {
                    surface.world_ctx.set(Some(id));
                }
            }
        }
        "Runtime.executionContextDestroyed" | "Runtime.executionContextsCleared" => {
            let gone = value["executionContextId"].as_i64();
            let current = surface.world_ctx.get();
            match gone {
                Some(id) if Some(id) == current => surface.world_ctx.set(None),
                None => surface.world_ctx.set(None), // contextsCleared
                _ => {}
            }
        }
        "Page.frameNavigated" => {
            let frame = &value["frame"];
            let is_main = frame["parentId"].is_null();
            if !is_main {
                return;
            }
            let url = frame["url"].as_str().unwrap_or("").to_string();
            apply_main_frame_navigation(&surface, url);
        }
        "Runtime.consoleAPICalled" => {
            let level = value["type"].as_str().unwrap_or("log").to_string();
            let mut parts: Vec<String> = Vec::new();
            if let Some(args) = value["args"].as_array() {
                for arg in args.iter().take(32) {
                    let text = arg["value"]
                        .as_str()
                        .map(str::to_string)
                        .or_else(|| arg["description"].as_str().map(str::to_string))
                        .unwrap_or_default();
                    if !text.is_empty() {
                        parts.push(text);
                    }
                }
            }
            let text: String = parts.join(" ").chars().take(CONSOLE_MAX_TEXT_CHARS).collect();
            if !text.is_empty() {
                push_console(&surface, level, text);
            }
        }
        "Runtime.exceptionThrown" => {
            let details = &value["exceptionDetails"];
            let mut text = details["text"].as_str().unwrap_or("exception").to_string();
            if let Some(exc) = details["exception"]["description"].as_str() {
                text = exc.to_string();
            }
            let text: String = text.chars().take(CONSOLE_MAX_TEXT_CHARS).collect();
            push_console(&surface, "error".to_string(), text);
        }
        "Runtime.bindingCalled" => {
            if value["name"].as_str() != Some(BINDING_NAME) {
                return;
            }
            // Helper envelopes: {kind, payload} — page-controlled input, sizes
            // already capped by the helper itself. We consume nav-state only.
            let payload = value["payload"].as_str().unwrap_or("");
            if let Ok(message) = serde_json::from_str::<Value>(payload) {
                if message["kind"].as_str() == Some("nav-state") {
                    let url = message["payload"]["url"].as_str().unwrap_or("").to_string();
                    if !url.is_empty() {
                        apply_main_frame_navigation(&surface, url);
                    }
                }
            }
        }
        _ => {}
    }
}

fn push_console(surface: &Surface, level: String, text: String) {
    let mut ring = surface.console.borrow_mut();
    ring.push_back(ConsoleLine {
        level,
        text,
        at_ms: now_ms(),
    });
    while ring.len() > CONSOLE_RING_CAPACITY {
        ring.pop_front();
    }
}

/// A main-frame navigation (or SPA route change reported by the helper):
/// update url/origin/title, reset the console ring, re-evaluate the grant.
pub fn apply_main_frame_navigation(surface: &Surface, url: String) {
    if url.is_empty() || url.starts_with("data:") {
        return;
    }
    let origin = url_origin(&url);
    let origin_changed = *surface.origin.borrow() != origin;
    *surface.url.borrow_mut() = url;
    *surface.origin.borrow_mut() = origin.clone();
    surface.console.borrow_mut().clear();
    surface.nav_epoch.set(surface.nav_epoch.get() + 1);
    surface.world_ctx.set(None); // new document, new contexts
    if origin_changed {
        // The grant is bound to the origin it was given on. Agent-opened tabs
        // re-take Read on the new origin: the agent chose to follow the link
        // (research flow would die otherwise). User-opened tabs drop to None.
        let mut grant = surface.grant.borrow_mut();
        match (&*grant, surface.opened_by_agent) {
            (Some(_), true) => {
                grant.replace(AgentGrant {
                    level: GrantLevel::Read,
                    origin: origin.clone(),
                    granted_at_ms: now_ms(),
                });
            }
            _ => {
                grant.take();
            }
        }
    }
    // Title follows the new document (best effort, fire and forget).
    let webview2 = surface.webview.webview();
    if let Ok(answer) = call_and_wait(
        &webview2,
        "Runtime.evaluate",
        &json!({"expression": "document.title || \"\"", "returnByValue": true}).to_string(),
    ) {
        if let Ok(parsed) = serde_json::from_str::<Value>(&answer) {
            if let Some(title) = parsed["result"]["value"].as_str() {
                *surface.title.borrow_mut() = title.to_string();
            }
        }
    }
}

fn url_origin(url: &str) -> String {
    // Good enough for http(s) origins; anything odd stays as-is and simply
    // never matches a grant.
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    let scheme_len = url.len() - rest.len();
    let end = rest.find('/').unwrap_or(rest.len());
    url[..scheme_len + end].to_string()
}

// ---------------------------------------------------------------------------
// World install + evaluate (codeg agent.rs / shim install_world shape)
// ---------------------------------------------------------------------------

/// Install the helper world on a fresh webview: events, binding, the
/// document-start script, and a world for a document that is already here.
/// Must run before the first navigation.
fn install_world(webview2: &ICoreWebView2, key: usize) -> Result<(), String> {
    for event in [
        "Runtime.executionContextCreated",
        "Runtime.executionContextDestroyed",
        "Runtime.executionContextsCleared",
        "Runtime.bindingCalled",
        "Page.frameNavigated",
        "Runtime.consoleAPICalled",
        "Runtime.exceptionThrown",
    ] {
        subscribe(webview2, key, event)?;
    }
    call_and_wait(webview2, "Page.enable", "{}")?;
    call_and_wait(webview2, "Runtime.enable", "{}")?;
    call_and_wait(
        webview2,
        "Runtime.addBinding",
        &json!({ "name": BINDING_NAME, "executionContextName": WORLD_NAME }).to_string(),
    )?;
    call_and_wait(
        webview2,
        "Page.addScriptToEvaluateOnNewDocument",
        &json!({ "source": HELPER_JS, "worldName": WORLD_NAME }).to_string(),
    )?;
    // A document already here (about:blank at creation) gets its world now.
    let world = call_and_wait(
        webview2,
        "Page.createIsolatedWorld",
        &json!({
            "frameId": main_frame_id(webview2)?,
            "worldName": WORLD_NAME,
            "grantUniverlalAccess": false
        })
        .to_string(),
    )?;
    if let Ok(parsed) = serde_json::from_str::<Value>(&world) {
        if let Some(id) = parsed["executionContextId"].as_i64() {
            // Route the context id to the surface that owns this webview.
            if let Some(tab_id) = PTR_TO_ID.with(|map| map.borrow().get(&key).cloned()) {
                if let Some(surface) = SURFACES.with(|surfaces| surfaces.borrow().get(&tab_id).cloned()) {
                    surface.world_ctx.set(Some(id));
                }
            }
        }
    }
    Ok(())
}

fn main_frame_id(webview2: &ICoreWebView2) -> Result<String, String> {
    let tree = call_and_wait(webview2, "Page.getFrameTree", "{}")?;
    serde_json::from_str::<Value>(&tree)
        .ok()
        .and_then(|v| v["frameTree"]["frame"]["id"].as_str().map(str::to_string))
        .ok_or_else(|| "no main frame".to_string())
}

/// Evaluate an expression in the agent world, returning its JSON string
/// value. Callers must not hold the surfaces borrow (the wait pumps).
fn evaluate_in_world(surface: &Surface, expression: &str) -> Result<Value, String> {
    let webview2 = surface.webview.webview();
    let context = surface
        .world_ctx
        .get()
        .ok_or_else(|| "agent world is not in this document".to_string())?;
    let answer = call_and_wait(
        &webview2,
        "Runtime.evaluate",
        &json!({
            "expression": expression,
            "contextId": context,
            "returnByValue": true,
            "awaitPromise": false
        })
        .to_string(),
    )?;
    let parsed: Value = serde_json::from_str(&answer).map_err(|e| format!("cdp answer: {e}"))?;
    if let Some(err) = parsed["exceptionDetails"].as_object() {
        return Err(format!(
            "page-side error: {}",
            err.get("text").and_then(Value::as_str).unwrap_or("unknown")
        ));
    }
    let raw = parsed["result"]["value"]
        .as_str()
        .ok_or_else(|| "engine did not answer".to_string())?;
    serde_json::from_str(raw).map_err(|e| format!("engine answer is not json: {e}"))
}

// ---------------------------------------------------------------------------
// Agent operations (probe → install → call, codeg agent.rs)
// ---------------------------------------------------------------------------

pub const ENGINE_ABSENT: &str = "absent";

fn epoch_of(surface: &Surface) -> String {
    agent::epoch(surface.generation.get(), surface.nav_epoch.get())
}

/// Install the world on first use (snapshot/act). Idempotent per surface.
fn ensure_world(surface: &Surface) -> Result<(), String> {
    if surface.world_installed.get() {
        return Ok(());
    }
    let webview2 = surface.webview.webview();
    install_world(&webview2, webview2.as_raw() as usize)?;
    surface.world_installed.set(true);
    Ok(())
}

/// Take an aria snapshot; installs the engine into the document on first use.
pub fn snapshot(surface: &Surface, request: &SnapshotRequest) -> Result<Value, String> {
    ensure_world(surface)?;
    let epoch = epoch_of(surface);
    let probe = agent::probe_and_snapshot(request, &epoch);
    let first = evaluate_in_world(surface, &probe)?;
    if first.as_str() == Some(ENGINE_ABSENT) {
        let install = agent::install_and_snapshot(request, &epoch);
        return evaluate_in_world(surface, &install);
    }
    surface.generation.set(surface.generation.get() + 1);
    Ok(first)
}

/// Perform one action (click/hover/type/press/select) by element ref.
pub fn act(surface: &Surface, request: &agent::ActionRequest) -> Result<Value, String> {
    ensure_world(surface)?;
    let expression = agent::act_call(request);
    let answer = evaluate_in_world(surface, &expression)?;
    surface.generation.set(surface.generation.get() + 1);
    Ok(answer)
}

/// Console tail, newest last. A `missing` count tells the caller the ring
/// dropped lines — "nothing more printed" and "more existed" differ.
pub fn console_lines(surface: &Surface, limit: usize) -> Value {
    let _ = ensure_world(surface);
    let ring = surface.console.borrow();
    let total = ring.len();
    let skip = total.saturating_sub(limit);
    let lines: Vec<&ConsoleLine> = ring.iter().skip(skip).collect();
    json!({
        "lines": lines,
        "missing": skip,
    })
}

/// Screenshot: CDP JPEG capture, narrowed until it fits the byte budget
/// (codeg capture.rs loop).
pub fn screenshot(surface: &Surface, max_width: Option<u32>) -> Result<Value, String> {
    ensure_world(surface)?;
    let webview2 = surface.webview.webview();
    let answer = call_and_wait(
        &webview2,
        "Page.captureScreenshot",
        &json!({ "format": "jpeg", "quality": CAPTURE_JPEG_QUALITY }).to_string(),
    )?;
    let parsed: Value = serde_json::from_str(&answer).map_err(|e| format!("capture: {e}"))?;
    let data = parsed["data"].as_str().ok_or("capture returned no image")?;
    let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, data)
        .map_err(|e| format!("capture decode: {e}"))?;
    let max_width = max_width.unwrap_or(CAPTURE_DEFAULT_MAX_WIDTH).max(1);
    let mut image = image::load_from_memory(&bytes).map_err(|e| format!("capture decode: {e}"))?;
    image = narrow_to(image, max_width);
    loop {
        let mut jpeg = Vec::new();
        let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, CAPTURE_JPEG_QUALITY);
        image
            .write_with_encoder(encoder)
            .map_err(|e| format!("capture encode: {e}"))?;
        if jpeg.len() <= CAPTURE_MAX_ENCODED_BYTES || image.width() <= CAPTURE_MIN_WIDTH {
            let encoded = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &jpeg);
            return Ok(json!({
                "dataUrl": format!("data:image/jpeg;base64,{encoded}"),
                "width": image.width(),
                "height": image.height(),
            }));
        }
        let ratio = (CAPTURE_MAX_ENCODED_BYTES as f64 / jpeg.len() as f64).sqrt() * 0.9;
        let narrower = ((f64::from(image.width()) * ratio).floor() as u32)
            .clamp(CAPTURE_MIN_WIDTH, image.width().saturating_sub(1).max(CAPTURE_MIN_WIDTH));
        image = narrow_to(image, narrower);
    }
}

fn narrow_to(image: image::DynamicImage, width: u32) -> image::DynamicImage {
    if image.width() <= width {
        image
    } else {
        let height = ((f64::from(image.height()) / f64::from(image.width())) * f64::from(width))
            .round()
            .max(1.0) as u32;
        image.resize_exact(width, height, image::imageops::FilterType::Lanczos3)
    }
}

// ---------------------------------------------------------------------------
// Resize hook (codeg hold_resize_hook — see the long comment there; our wry
// 0.55.1 has exactly the unconditional detach on child drop)
// ---------------------------------------------------------------------------

const RESIZE_HOOK_ID: usize = 0xC0DE;

/// Give the main window a resize hook wry cannot take away: wry detaches its
/// parent subclass whenever ANY child webview of the window is dropped, and a
/// browser tab is one. Idempotent; installs before the first tab is built.
pub fn ensure_resize_hook(window: &WebviewWindow) -> Result<(), String> {
    if RESIZE_HOOK.with(|installed| installed.get()) {
        return Ok(());
    }
    let hwnd = window.hwnd().map_err(|e| e.to_string())?.0 as isize;
    let label = window.label().to_string();
    window
        .with_webview(move |platform| {
            let hwnd = HWND(hwnd as _);
            // SAFETY: main thread; hwnd is this window's.
            if unsafe { GetWindowSubclass(hwnd, Some(resize_hook_proc), RESIZE_HOOK_ID, None) }
                .as_bool()
            {
                return;
            }
            let held = Box::into_raw(Box::new(platform.controller()));
            // SAFETY: as above; the subclass carries the Box for the window's life.
            let installed =
                unsafe { SetWindowSubclass(hwnd, Some(resize_hook_proc), RESIZE_HOOK_ID, held as usize) };
            if !installed.as_bool() {
                drop(unsafe { Box::from_raw(held) });
                crate::debug_log::append(
                    "browser",
                    "warn",
                    "",
                    "resize hook install failed; closing a tab will freeze the app webview size",
                    Some(&label),
                );
            } else {
                RESIZE_HOOK.with(|flag| flag.set(true));
            }
        })
        .map_err(|e| e.to_string())
}

unsafe extern "system" fn resize_hook_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    held: usize,
) -> LRESULT {
    if held == 0 {
        return DefSubclassProc(hwnd, msg, wparam, lparam);
    }
    let controller = held as *const ICoreWebView2Controller;
    match msg {
        WM_SIZE if wparam.0 != SIZE_MINIMIZED as usize => {
            let mut client = RECT::default();
            if unsafe { GetClientRect(hwnd, &mut client) }.is_ok() {
                let width = client.right - client.left;
                let height = client.bottom - client.top;
                let _ = unsafe {
                    (*controller).SetBounds(RECT { left: 0, top: 0, right: width, bottom: height })
                };
                let mut host = HWND::default();
                if unsafe { (*controller).ParentWindow(&mut host) }.is_ok() {
                    let _ = unsafe {
                        SetWindowPos(
                            host,
                            None,
                            0,
                            0,
                            width,
                            height,
                            SWP_ASYNCWINDOWPOS | SWP_NOACTIVATE | SWP_NOZORDER,
                        )
                    };
                }
            }
        }
        WM_SETFOCUS | WM_ENTERSIZEMOVE => {
            let _ = unsafe { (*controller).MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC) };
        }
        WM_MOVE | WM_MOVING => {
            let _ = unsafe { (*controller).NotifyParentWindowPositionChanged() };
        }
        WM_NCDESTROY => {
            let _ = unsafe { RemoveWindowSubclass(hwnd, Some(resize_hook_proc), RESIZE_HOOK_ID) };
            drop(unsafe { Box::from_raw(held as *mut ICoreWebView2Controller) });
        }
        _ => {}
    }
    DefSubclassProc(hwnd, msg, wparam, lparam)
}

// ---------------------------------------------------------------------------
// Surface lifecycle (main thread)
// ---------------------------------------------------------------------------

/// A Chrome-shaped UA: many sites serve WebView2's default UA a degraded or
/// blocked experience (codeg `profile::default_user_agent` rationale).
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

fn profile_dir() -> Result<std::path::PathBuf, String> {
    crate::app_paths::global_dir()
        .map(|dir| dir.join("browser-profiles").join("default"))
        .map_err(|e| format!("global dir: {e}"))
}

/// Create a tab webview as a child of the main window. Main thread only.
/// Lazy since 2026-11-06: no startup prewarm anymore (it kept a whole second
/// WebView2 process group resident for the process lifetime even when no tab
/// was ever opened); every tab — including the first — builds here through
/// `build_surface`, the path 2nd+ tabs always took.
pub fn create_surface(app: &AppHandle, url: &str, opened_by_agent: bool) -> Result<String, String> {
    let surface = build_surface(app, url, opened_by_agent)?;
    let id = surface.id.clone();
    register_surface(app, &surface);
    Ok(id)
}

/// Build the webview and its Surface WITHOUT touching the registry;
/// `create_surface` registers immediately after.
fn build_surface(app: &AppHandle, url: &str, opened_by_agent: bool) -> Result<Rc<Surface>, String> {
    let window = app
        .get_webview_window("main")
        .ok_or("main window is gone")?;
    ensure_resize_hook(&window)?;

    let id = format!("b{}", NEXT_ID.with(|n| {
        let value = n.get();
        n.set(value + 1);
        value
    }));
    // `about:blank` is the prewarm target (and nothing else): no network at
    // startup, and the first real tab simply navigates the same surface.
    let url = if url == "about:blank" {
        url.to_string()
    } else {
        normalize_url(url)?
    };
    let origin = url_origin(&url);

    // Popups become tabs of their source's kind. The engine's own popup
    // window is refused; the tab is opened on the next turn of the message
    // loop (creating a webview inside the new-window callback would re-enter
    // the WebView2 dispatch that is calling us).
    let popup_app = app.clone();
    let popup_by_agent = opened_by_agent;

    let profile = profile_dir()?;
    let webview = WEB_CONTEXTS.with(|contexts| {
        let mut contexts = contexts.borrow_mut();
        let context = contexts
            .entry("default".to_string())
            .or_insert_with(|| WebContext::new(Some(profile)));
        WebViewBuilder::new_with_web_context(context)
            .with_id(&id)
            // 1×1 hidden until the frontend measures the host: if the bounds
            // sync ever fails, the failure mode is an invisible dot — not a
            // slab over the titlebar eating every click.
            .with_bounds(Rect {
                position: wry::dpi::LogicalPosition::new(0.0, 0.0).into(),
                size: wry::dpi::LogicalSize::new(1.0, 1.0).into(),
            })
            .with_visible(false)
            .with_focused(false)
            .with_user_agent(USER_AGENT)
            .with_new_window_req_handler(move |url, _features| {
                let app_for_job = popup_app.clone();
                let _ = popup_app.run_on_main_thread(move || {
                    if let Err(error) = create_surface(&app_for_job, &url, popup_by_agent) {
                        crate::debug_log::append("browser", "warn", "", "popup open failed", Some(&error));
                    }
                });
                NewWindowResponse::Deny
            })
            .build_as_child(&window)
            .map_err(|e| format!("webview build failed: {e}"))
    })?;

    let surface = Rc::new(Surface {
        id,
        opened_by_agent,
        webview,
        world_ctx: Cell::new(None),
        world_installed: Cell::new(false),
        nav_epoch: Cell::new(0),
        generation: Cell::new(0),
        console: RefCell::new(VecDeque::new()),
        url: RefCell::new(url.clone()),
        title: RefCell::new(String::new()),
        origin: RefCell::new(origin.clone()),
        grant: RefCell::new(if opened_by_agent {
            // The agent chose to open this page: reading it is the point.
            // Control still asks (permission card / manual share).
            Some(AgentGrant { level: GrantLevel::Read, origin, granted_at_ms: now_ms() })
        } else {
            None
        }),
    });

    // Plain Navigate: synchronous fire, no nested pump. (The world — CDP
    // subscriptions + document-start script — installs LAZILY at the first
    // read; installing here pumps inside the dispatch and wedges the app
    // webview's renderer on our tao build, 2026-11-01 bisect.)
    surface.webview.load_url(&url).map_err(|e| format!("navigate failed: {e}"))?;
    Ok(surface)
}

/// Enter a surface into the registry and tell the frontend.
fn register_surface(app: &AppHandle, surface: &Rc<Surface>) {
    let webview2 = surface.webview.webview();
    let key = webview2.as_raw() as usize;
    SURFACES.with(|surfaces| surfaces.borrow_mut().insert(surface.id.clone(), surface.clone()));
    PTR_TO_ID.with(|map| map.borrow_mut().insert(key, surface.id.clone()));
    emit_tabs(app);
}


/// The ICoreWebView2 of a surface (mod.rs diagnostics).
pub fn webview2_of(surface: &Surface) -> ICoreWebView2 {
    surface.webview.webview()
}

/// Diagnostic raw CDP call returning the answer JSON as a String (E2E only).
pub fn debug_cdp(webview2: &ICoreWebView2, method: &str, params: &str) -> Option<String> {
    call_and_wait(webview2, method, params).ok()
}

pub fn normalize_url(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("empty url".to_string());
    }
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        Ok(trimmed.to_string())
    } else if trimmed.contains(' ') || !trimmed.contains('.') {
        Err("only http(s) urls can be opened".to_string())
    } else {
        Ok(format!("https://{trimmed}"))
    }
}

// (No prewarm machinery since 2026-11-06: startup prewarm kept a second
// WebView2 process group resident for the whole process lifetime even when no
// browser tab was ever opened. Tabs are built lazily in `create_surface`; the
// documented fallback if first-tab creation ever freezes on our tao build is
// restoring a startup prewarm pool — see plan-browser-computer-use.md §9.)

/// Drop a tab. Main thread. The wry Drop detaches the parent subclass —
/// exactly the case `ensure_resize_hook` exists for.
pub fn close_surface(app: &AppHandle, tab_id: &str) -> Result<(), String> {
    let surface = SURFACES.with(|surfaces| surfaces.borrow().get(tab_id).cloned());
    let Some(surface) = surface else {
        return Ok(());
    };
    let webview2 = surface.webview.webview();
    let key = webview2.as_raw() as usize;
    SURFACES.with(|surfaces| surfaces.borrow_mut().remove(tab_id));
    PTR_TO_ID.with(|map| map.borrow_mut().remove(&key));
    drop(surface); // the wry WebView goes here
    emit_tabs(app);
    Ok(())
}

/// Look a surface up and run a closure on the main thread. This is the ONLY
/// shape non-main-thread code may use to touch surfaces.
pub fn with_surface<R: Send + 'static>(
    app: &AppHandle,
    tab_id: &str,
    f: impl FnOnce(&Surface) -> R + Send + 'static,
) -> Result<R, String> {
    let (tx, rx) = mpsc::channel();
    let tab_id = tab_id.to_string();
    let missing = tab_id.clone();
    app.run_on_main_thread(move || {
        let result = SURFACES.with(|surfaces| surfaces.borrow().get(&tab_id).cloned()).map(|s| f(&s));
        let _ = tx.send(result);
    })
    .map_err(|e| format!("main thread hop: {e}"))?;
    match rx.recv() {
        Ok(Some(value)) => Ok(value),
        Ok(None) => Err(format!("no such browser tab: {missing}")),
        Err(_) => Err("main thread did not answer".to_string()),
    }
}

/// Direct variant for code already ON the main thread (e.g. inside
/// `with_surface` callbacks that need a second surface).
pub fn find_surface(tab_id: &str) -> Option<Rc<Surface>> {
    SURFACES.with(|surfaces| surfaces.borrow().get(tab_id).cloned())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::{normalize_url, url_origin, GrantLevel};

    #[test]
    fn grant_levels_rank() {
        assert!(GrantLevel::Control.allows(GrantLevel::Read));
        assert!(GrantLevel::Read.allows(GrantLevel::Read));
        assert!(!GrantLevel::Read.allows(GrantLevel::Control));
        assert!(!GrantLevel::None.allows(GrantLevel::Read));
    }

    #[test]
    fn origins_strip_paths_and_queries() {
        assert_eq!(url_origin("https://example.com/a/b?x=1"), "https://example.com");
        assert_eq!(url_origin("http://localhost:5173/"), "http://localhost:5173");
        assert_eq!(url_origin("about:blank"), "about:blank");
    }

    #[test]
    fn urls_normalized_or_refused() {
        assert_eq!(
            normalize_url("example.com/docs").unwrap(),
            "https://example.com/docs"
        );
        assert!(normalize_url("how to fly").is_err());
        assert!(normalize_url("ftp://x").is_err());
        assert!(normalize_url("").is_err());
    }
}
