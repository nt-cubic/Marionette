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
use std::path::Path;
use std::rc::Rc;
use std::sync::mpsc;

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, Url, WebviewWindow};
use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2;
use webview2_com::{
    CallDevToolsProtocolMethodCompletedHandler, DevToolsProtocolEventReceivedEventHandler,
};
use windows::core::{Interface, HSTRING, PWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WAIT_TIMEOUT, WPARAM};
use windows::Win32::UI::Shell::{
    DefSubclassProc, GetWindowSubclass, RemoveWindowSubclass, SetWindowSubclass,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageA, GetClientRect, MsgWaitForMultipleObjectsEx, PeekMessageA, SetWindowPos,
    TranslateMessage, MSG, MWMO_INPUTAVAILABLE, PM_REMOVE, QS_ALLINPUT, SIZE_MINIMIZED,
    SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE, SWP_NOZORDER, WM_ENTERSIZEMOVE, WM_MOVE, WM_MOVING,
    WM_NCDESTROY, WM_SETFOCUS, WM_SIZE,
};
use wry::{NewWindowResponse, Rect, WebContext, WebView, WebViewBuilder, WebViewExtWindows};

use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2Controller, COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC,
};

use super::agent::{self, SnapshotRequest};
use super::find;
use super::pick;
use super::ref_lease::{self, LeaseDeath, RefLease};

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

/// Read-only page questions (`state` / `ready` / `setValue`), answered without
/// walking the tree. Kept out of the vendored aria bundle on purpose — see the
/// file's own header and js/VENDOR.md.
const PROBE_JS: &str = include_str!("js/probe.js");

/// The annotate-mode element picker (`js/picker.js`): the user-side DevTools
/// overlay. Same reasoning as the probe — own script, own binding, the
/// vendored bundle stays untouched.
const PICKER_JS: &str = include_str!("js/picker.js");

/// Console ring: lines per tab, per document (cleared on navigation).
const CONSOLE_RING_CAPACITY: usize = 200;
const CONSOLE_MAX_TEXT_CHARS: usize = 4096;
/// Screenshot budget: base64 bytes (codeg `capture.rs`), JPEG quality, widths.
pub const CAPTURE_MAX_ENCODED_BYTES: usize = 3_500_000;
const CAPTURE_JPEG_QUALITY: u8 = 85;
const CAPTURE_DEFAULT_MAX_WIDTH: u32 = 1568;
const CAPTURE_MIN_WIDTH: u32 = 64;
/// Retained pick payloads (`pickId → message`) for the screenshot crop, and
/// the tree budget of the snapshot taken just to resolve refs — the tree
/// itself is discarded, but the refs it prunes are the ones validation needs.
const PICK_RETAIN_CAP: usize = 24;
const PICK_RESOLVE_MAX_CHARS: u64 = 120_000;

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
    /// `background` or `workspace`; the frontend consumes this only for fresh
    /// tabs, then the tab behaves like every other tab.
    pub display: String,
    pub owner_session_id: Option<String>,
    /// Stable agent-facing name chosen at open time (for example work-mail).
    /// Duplicate opens return the existing tab instead of creating another.
    pub chosen_id: Option<String>,
    /// User-controlled keep flag; idle cleanup never closes a pinned tab.
    pub pinned: bool,
    pub busy: bool,
    pub status: String,
    pub last_used_at_ms: u64,
}

/// A page script dialog awaiting an answer. `message` is page-controlled
/// text and stays capped — it feeds error notes and tool answers, never a
/// UI layout.
#[derive(Debug, PartialEq, Clone)]
pub struct PendingDialog {
    pub kind: String,
    pub message: String,
    pub at_ms: u64,
}

const DIALOG_MESSAGE_MAX_CHARS: usize = 200;

/// Pure form of the event parse — the CDP payload for
/// `Page.javascriptDialogOpening` is `{message, type}`.
fn dialog_from_event(value: &Value, at_ms: u64) -> Option<PendingDialog> {
    let kind = value["type"].as_str()?;
    if kind.is_empty() {
        return None;
    }
    let message: String = value["message"]
        .as_str()
        .unwrap_or("")
        .chars()
        .take(DIALOG_MESSAGE_MAX_CHARS)
        .collect();
    Some(PendingDialog {
        kind: kind.to_string(),
        message,
        at_ms,
    })
}

/// Why a renderer-bound eval must not run while a dialog blocks the page.
/// `None` = free to eval. The note tells the model both exits: the
/// `browser_dialog` tool, or the user clicking the default dialog.
fn dialog_block_note_for(dialog: Option<&PendingDialog>) -> Option<String> {
    let dialog = dialog?;
    Some(format!(
        "dialog_blocked: the page has an open {} dialog ({:?}) — an eval would hang until it is \
         answered. Resolve it with browser_dialog (accept/dismiss), or the user can answer the \
         visible dialog; if they already did, browser_dialog reports it and clears this state.",
        dialog.kind, dialog.message
    ))
}

pub struct Surface {
    pub id: String,
    pub opened_by_agent: Cell<bool>,
    pub display: String,
    pub owner_session_id: RefCell<Option<String>>,
    pub chosen_id: Option<String>,
    pub pinned: Cell<bool>,
    /// Concurrent agent calls can overlap while waiting on a lease or CDP.
    /// The public tab snapshot stays boolean, but the counter prevents one
    /// completion from claiming the tab is idle while another call is live.
    pub busy_count: Cell<u32>,
    pub status: RefCell<String>,
    pub last_used_at_ms: Cell<u64>,
    pub webview: WebView,
    /// App handle, so the CDP event fan-out can push picker reports to the
    /// frontend without a CDP call (events can arrive inside another call's
    /// pump — that path must stay pump-free). Emit only.
    pub app: AppHandle,
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
    /// The annotate-mode picker (js/picker.js) is running on this document.
    picker_active: Cell<bool>,
    /// Pick payloads (pickId → message) retained for `pick_finalize`'s
    /// screenshot crop — the rect was measured at pick time and a crop needs
    /// it after the event. Cleared on navigation: the rects describe a dead
    /// page then.
    picks: RefCell<VecDeque<(String, Value)>>,
    /// Host-side ref lease (`ref_lease.rs`): the generation the last snapshot
    /// answered with, and what killed the ones before it. The page stays the
    /// only judge of ref freshness; this only short-answers deaths the host
    /// witnessed and annotates action responses.
    ref_lease: RefCell<Option<RefLease>>,
    /// A script dialog (alert/confirm/prompt/beforeunload) the page opened
    /// and nobody has answered yet. The WebView2 default dialog is still
    /// shown to the user; this mirror exists so renderer-bound evals can
    /// fail fast instead of hanging until someone clicks (a blocked renderer
    /// hangs `Runtime.evaluate`, and the caller holds the process-wide CDP
    /// lease while it pumps). See `dialog_block_note_for`.
    dialog: RefCell<Option<PendingDialog>>,
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
            opened_by_agent: self.opened_by_agent.get(),
            display: self.display.clone(),
            owner_session_id: self.owner_session_id.borrow().clone(),
            chosen_id: self.chosen_id.clone(),
            pinned: self.pinned.get(),
            busy: self.busy_count.get() > 0,
            status: self.status.borrow().clone(),
            last_used_at_ms: self.last_used_at_ms.get(),
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
    let mut tabs: Vec<TabInfo> =
        SURFACES.with(|surfaces| surfaces.borrow().values().map(|s| s.info()).collect());
    // Registry storage is a HashMap for O(1) lookup. The frontend tab strip
    // must still be stable across status events, so expose creation order.
    tabs.sort_by_key(|tab| {
        tab.id
            .strip_prefix('b')
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(u64::MAX)
    });
    tabs
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

/// How long any single CDP round-trip may hold the main thread. Cold
/// renderers (first browser use after startup) can sit on
/// `Page.captureScreenshot` for minutes; the stock webview2-com pump below
/// waits forever, which froze the whole app twice on 2026-11-18 (MAIN THREAD
/// STALLED ~200s+, browser surface white, ACP delivery stuck). Bounded waits
/// turn that into a tool error the agent can retry.
const CDP_WAIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// Call a CDP method and pump the message loop until its completion arrives
/// **or the deadline passes**. The caller must NOT hold the surfaces borrow:
/// the pump can dispatch surface lifecycle (that rule is why every public fn
/// clones its Rc first).
///
/// This is `wait_for_async_operation` with its `GetMessageA` pump replaced by
/// a deadline-aware one — and with a completion closure that must never panic
/// on a late answer (the stock one `expect`s the channel send; after a
/// timeout the receiver is gone and the abort would kill the process).
fn call_and_wait(webview: &ICoreWebView2, method: &str, params: &str) -> Result<String, String> {
    let (tx, rx) = mpsc::channel::<String>();
    // Late completion after a timeout: the send fails and is ignored — no panic.
    let handler =
        CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |result, json| {
            result?;
            let _ = tx.send(json);
            Ok(())
        }));
    let name = HSTRING::from(method);
    let params_h = HSTRING::from(params);
    // SAFETY: main thread, live webview; WebView2 owns the handler from here.
    unsafe { webview.CallDevToolsProtocolMethod(&name, &params_h, &handler) }
        .map_err(|err| format!("{method} failed: {err:?}"))?;

    let deadline = std::time::Instant::now() + CDP_WAIT_TIMEOUT;
    let mut msg = MSG::default();
    loop {
        if let Ok(answer) = rx.try_recv() {
            return Ok(answer);
        }
        let Some(remaining) = deadline.checked_duration_since(std::time::Instant::now()) else {
            return Err(format!(
                "{method} timed out after {}s (page too cold or busy — retry)",
                CDP_WAIT_TIMEOUT.as_secs()
            ));
        };
        // Wake on queued input only; MWMO_INPUTAVAILABLE so messages that
        // arrived before the wait still count. Plain window messages must
        // keep flowing — that is what keeps the window alive while we wait.
        // SAFETY: no handles; only the message queue is waited on.
        let wake = unsafe {
            MsgWaitForMultipleObjectsEx(
                None,
                remaining.as_millis().min(u32::MAX as u128) as u32,
                QS_ALLINPUT,
                MWMO_INPUTAVAILABLE,
            )
        };
        if wake == WAIT_TIMEOUT {
            continue; // re-check the channel, then the deadline
        }
        // SAFETY: thread-local message pump, same shape as stock GetMessageA
        // loop — dispatch everything pending, checking for the answer between
        // messages so a completion raised mid-drain ends the wait immediately.
        while unsafe { PeekMessageA(&mut msg, None, 0, 0, PM_REMOVE).as_bool() } {
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageA(&msg);
            }
            if let Ok(answer) = rx.try_recv() {
                return Ok(answer);
            }
        }
    }
}

/// Issue a CDP command without pumping the message loop, then hand control
/// back through the event loop. Safe from the WebView2 dispatch that creates a
/// surface; failures are diagnostic only because cache policy must never block
/// navigation.
fn call_then(
    webview: &ICoreWebView2,
    method: &str,
    params: &str,
    next: impl FnOnce() + 'static,
) -> Result<(), String> {
    let method_for_log = method.to_string();
    let handler =
        CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |result, _json| {
            if let Err(error) = result {
                crate::debug_log::append(
                    "browser",
                    "warn",
                    "",
                    "cache policy CDP call failed",
                    Some(&format!("{method_for_log}: {error:?}")),
                );
            }
            next();
            Ok(())
        }));
    let name = HSTRING::from(method);
    let params_h = HSTRING::from(params);
    // SAFETY: main thread, live webview; WebView2 owns the handler from here.
    unsafe { webview.CallDevToolsProtocolMethod(&name, &params_h, &handler) }
        .map_err(|error| format!("{method} failed: {error:?}"))
}

/// Development servers usually send no `Cache-Control`. Chromium's heuristic
/// freshness then keeps an old module alive even after the page switches to
/// classic scripts, producing mixed-generation errors such as
/// `Unexpected token 'export'` followed by `loadAssets is not a function`.
/// Disable HTTP cache per tab before every local navigation; public sites keep
/// the ordinary cache path.
pub fn should_bypass_http_cache(url: &str) -> bool {
    let Ok(parsed) = Url::parse(url) else {
        return false;
    };
    if !matches!(parsed.scheme(), "http" | "https") {
        return false;
    }
    let Some(host) = parsed.host_str() else {
        return false;
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host.eq_ignore_ascii_case("localhost") || host.to_ascii_lowercase().ends_with(".localhost") {
        return true;
    }
    host.parse::<std::net::IpAddr>()
        .map(|ip| ip.is_loopback() || ip.is_unspecified())
        .unwrap_or(false)
}

/// Navigate through the one policy point used by both tab creation and the
/// address bar. Navigation starts only after the cache policy call completes,
/// so the first local document already skips the stale persistent cache.
pub fn navigate(surface: &Surface, url: &str) -> Result<(), String> {
    let webview2 = surface.webview.webview();
    let app = surface.app.clone();
    let tab_id = surface.id.clone();
    let target = url.to_string();
    let policy = call_then(
        &webview2,
        "Network.setCacheDisabled",
        &json!({ "cacheDisabled": should_bypass_http_cache(url) }).to_string(),
        move || {
            let tab_for_job = tab_id.clone();
            if let Err(error) = app.run_on_main_thread(move || {
                let surface =
                    SURFACES.with(|surfaces| surfaces.borrow().get(&tab_for_job).cloned());
                if let Some(surface) = surface {
                    if let Err(error) = surface.webview.load_url(&target) {
                        crate::debug_log::append(
                            "browser",
                            "warn",
                            "",
                            "navigate after cache policy failed",
                            Some(&error.to_string()),
                        );
                    }
                }
            }) {
                crate::debug_log::append(
                    "browser",
                    "warn",
                    "",
                    "cache policy continuation failed",
                    Some(&error.to_string()),
                );
            }
        },
    );
    if let Err(error) = policy {
        crate::debug_log::append(
            "browser",
            "warn",
            "",
            "cache policy CDP call failed; navigating directly",
            Some(&error),
        );
        surface
            .webview
            .load_url(url)
            .map_err(|error| format!("navigate failed: {error}"))?;
    }
    Ok(())
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
        receiver
            .add_DevToolsProtocolEventReceived(
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
            apply_document_navigation(&surface, url, None);
            emit_tabs(&surface.app);
        }
        "Page.javascriptDialogOpening" => {
            // Zero CDP: events can arrive inside another call's pump. State
            // only — the agent sees it through the fast-fail notes and
            // resolves via browser_dialog; the user sees the default dialog.
            if let Some(dialog) = dialog_from_event(&value, now_ms()) {
                *surface.dialog.borrow_mut() = Some(dialog);
            }
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
            let text: String = parts
                .join(" ")
                .chars()
                .take(CONSOLE_MAX_TEXT_CHARS)
                .collect();
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
            let name = value["name"].as_str().unwrap_or("");
            if name == BINDING_NAME {
                // Helper envelopes: {kind, payload} — page-controlled input, sizes
                // already capped by the helper itself. We consume nav-state only.
                let payload = value["payload"].as_str().unwrap_or("");
                if let Ok(message) = serde_json::from_str::<Value>(payload) {
                    if message["kind"].as_str() == Some("nav-state") {
                        let (url, title) = nav_state_fields(&message);
                        if !url.is_empty() {
                            let old_url = surface.url.borrow().clone();
                            let old_title = surface.title.borrow().clone();
                            match classify_nav_state(&old_url, &url, &old_title, &title) {
                                NavStateChange::None => {}
                                NavStateChange::Title => apply_title_change(&surface, title),
                                NavStateChange::Route => {
                                    apply_route_navigation(&surface, url, Some(title))
                                }
                            }
                            emit_tabs(&surface.app);
                        }
                    }
                }
            } else if name == pick::PICK_BINDING {
                on_pick_event(&surface, value["payload"].as_str().unwrap_or(""));
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

fn nav_state_fields(message: &Value) -> (String, String) {
    let payload = &message["payload"];
    let url = payload["url"]
        .as_str()
        .or_else(|| payload["href"].as_str())
        .unwrap_or("")
        .to_string();
    let title = payload["title"].as_str().unwrap_or("").to_string();
    (url, title)
}

/// One picker report from the page (`js/picker.js` over `__marionettePick`).
/// This runs inside the CDP event fan-out — possibly during another call's
/// pump — so it does NO CDP work: retain the payload for `pick_finalize`'s
/// crop and push the event to the frontend, which drives everything else.
fn on_pick_event(surface: &Surface, payload: &str) {
    let Ok(mut message) = serde_json::from_str::<Value>(payload) else {
        return;
    };
    match message["kind"].as_str().unwrap_or("") {
        "cancel" | "limit" => {
            surface.picker_active.set(false);
            let _ = surface.app.emit(
                "marionette-browser-pick",
                json!({
                    "tabId": surface.id,
                    "message": if message["kind"].as_str() == Some("limit") {
                        json!({ "kind": "limit", "limit": message["limit"].clone() })
                    } else {
                        json!({ "kind": "cancel" })
                    }
                }),
            );
        }
        "pick" | "region" => {
            if message.get("url").is_none() {
                message["url"] = json!(surface.url.borrow().clone());
            }
            let pick_id = message["pickId"].as_str().unwrap_or("").to_string();
            if pick_id.is_empty() || pick_id.len() > 64 {
                return;
            }
            {
                let mut picks = surface.picks.borrow_mut();
                if let Some(slot) = picks.iter_mut().find(|(id, _)| *id == pick_id) {
                    slot.1 = message.clone();
                } else {
                    picks.push_back((pick_id, message.clone()));
                    while picks.len() > PICK_RETAIN_CAP {
                        picks.pop_front();
                    }
                }
            }
            let _ = surface.app.emit(
                "marionette-browser-pick",
                json!({ "tabId": surface.id, "message": message }),
            );
        }
        _ => {}
    }
}

/// Whether a same-document `nav-state` changed only metadata or the route.
/// A real document replacement comes from `Page.frameNavigated` instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NavStateChange {
    None,
    Title,
    Route,
}

fn classify_nav_state(
    old_url: &str,
    new_url: &str,
    old_title: &str,
    new_title: &str,
) -> NavStateChange {
    if old_url != new_url {
        NavStateChange::Route
    } else if old_title != new_title {
        NavStateChange::Title
    } else {
        NavStateChange::None
    }
}

fn apply_origin_and_title(surface: &Surface, url: &str, origin: String, title: Option<String>) {
    let origin_changed = *surface.origin.borrow() != origin;
    *surface.url.borrow_mut() = url.to_string();
    *surface.origin.borrow_mut() = origin.clone();
    if let Some(title) = title {
        *surface.title.borrow_mut() = title;
    }
    if origin_changed {
        // A grant is bound to the origin it was given on. Agent-opened tabs
        // re-take Read on the new origin: the agent chose to follow the link
        // (research flow would die otherwise). User-opened tabs drop to None.
        let mut grant = surface.grant.borrow_mut();
        match (&*grant, surface.opened_by_agent.get()) {
            (Some(_), true) => {
                grant.replace(AgentGrant {
                    level: GrantLevel::Read,
                    origin,
                    granted_at_ms: now_ms(),
                });
            }
            _ => {
                grant.take();
            }
        }
    }
}

/// A real main-frame document replacement. This function is deliberately
/// CDP-free: it runs inside the event fan-out, which can already be pumping a
/// WebView2 dispatch. The helper's later `nav-state` fills in the title.
pub fn apply_document_navigation(surface: &Surface, url: String, title: Option<String>) {
    if url.is_empty() || url.starts_with("data:") {
        return;
    }
    let origin = url_origin(&url);
    surface.console.borrow_mut().clear();
    surface.nav_epoch.set(surface.nav_epoch.get() + 1);
    lease_invalidate(surface, LeaseDeath::DocumentNavigation);
    // A dialog blocks the old document's renderer; whatever happens to it,
    // the new document opens with a clean gate.
    *surface.dialog.borrow_mut() = None;
    surface.world_ctx.set(None); // new document, new contexts
                                 // The overlay and its state died with the old document (the doc-start
                                 // script re-registers passively; it does not start itself). Retained pick
                                 // rects describe that page too — they crop nothing useful now.
    surface.picker_active.set(false);
    surface.picks.borrow_mut().clear();
    // Clear the old title immediately; the helper's nav-state supplies the
    // new one without another CDP round-trip.
    *surface.title.borrow_mut() = title.unwrap_or_default();
    apply_origin_and_title(surface, &url, origin, None);
}

/// A same-document route change (`pushState`, hash, history navigation). The
/// world survives, but refs from the previous route are invalidated. Picker
/// payloads stay available so an unfinished annotation batch can still be
/// sent; the frontend stops the in-page overlay when it sees the new URL.
fn apply_route_navigation(surface: &Surface, url: String, title: Option<String>) {
    if url.is_empty() || url.starts_with("data:") {
        return;
    }
    let origin = url_origin(&url);
    surface.nav_epoch.set(surface.nav_epoch.get() + 1);
    lease_invalidate(surface, LeaseDeath::RouteNavigation);
    apply_origin_and_title(surface, &url, origin, title);
}

fn apply_title_change(surface: &Surface, title: String) {
    *surface.title.borrow_mut() = title;
}

fn url_origin(url: &str) -> String {
    if let Ok(mut parsed) = Url::parse(url) {
        if parsed.scheme() == "file" {
            // File URLs have opaque origins. Keep each file path as its own
            // grant scope so following a local link cannot inherit the
            // previous file's read permission.
            parsed.set_query(None);
            parsed.set_fragment(None);
            return parsed.to_string();
        }
    }
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
        // Script-dialog mirror for the fast-fail gate; resolution rides
        // Page.handleJavaScriptDialog inside `resolve_dialog`, never here.
        "Page.javascriptDialogOpening",
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
    // The picker reports over its own binding (js/picker.js): separate name,
    // same world — the vendored helper's `__codegSend` stays upstream's.
    call_and_wait(
        webview2,
        "Runtime.addBinding",
        &json!({ "name": pick::PICK_BINDING, "executionContextName": WORLD_NAME }).to_string(),
    )?;
    call_and_wait(
        webview2,
        "Page.addScriptToEvaluateOnNewDocument",
        &json!({ "source": HELPER_JS, "worldName": WORLD_NAME }).to_string(),
    )?;
    // The page probes ride along with the helper: same world, same document
    // start, so a tab that can be snapped can also be asked cheap questions.
    call_and_wait(
        webview2,
        "Page.addScriptToEvaluateOnNewDocument",
        &json!({ "source": PROBE_JS, "worldName": WORLD_NAME }).to_string(),
    )?;
    // The picker as well — passive until a `start` call turns it on, so an
    // install done for a snapshot gains nothing user-visible.
    call_and_wait(
        webview2,
        "Page.addScriptToEvaluateOnNewDocument",
        &json!({ "source": PICKER_JS, "worldName": WORLD_NAME }).to_string(),
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
                if let Some(surface) =
                    SURFACES.with(|surfaces| surfaces.borrow().get(&tab_id).cloned())
                {
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

/// The world's execution context for the current document, created on demand.
///
/// A navigation destroys the world; `addScriptToEvaluateOnNewDocument`
/// rebuilds it on the next document and the context event reports it. An
/// evaluation that arrives in between would find no context, so the missing
/// case re-creates the world here instead of failing — the steady state is
/// still the cached id.
fn world_context(surface: &Surface, webview2: &ICoreWebView2) -> Result<i64, String> {
    if let Some(context) = surface.world_ctx.get() {
        return Ok(context);
    }
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
    let parsed: Value = serde_json::from_str(&world).map_err(|e| format!("cdp answer: {e}"))?;
    let id = parsed["executionContextId"]
        .as_i64()
        .ok_or_else(|| "the agent world did not report a context".to_string())?;
    surface.world_ctx.set(Some(id));
    Ok(id)
}

/// One world answer, before it is read as either shape the world produces:
/// the bare-string absent sentinel, or the JSON the engine stringified.
#[derive(Debug, PartialEq)]
enum WorldAnswer {
    Absent,
    Json(Value),
}

/// The absent sentinel is a bare string, so parsing it as JSON fails at
/// column 1. Both callers must recognise it before parsing, or the install
/// branch never runs and every snapshot fails the same way.
fn parse_world_answer(raw: &str) -> Result<WorldAnswer, String> {
    if raw == agent::ENGINE_ABSENT {
        return Ok(WorldAnswer::Absent);
    }
    serde_json::from_str(raw)
        .map(WorldAnswer::Json)
        .map_err(|e| format!("engine answer is not json: {e}"))
}

/// Evaluate an expression in the agent world and return its value verbatim.
/// Callers must not hold the surfaces borrow (the wait pumps).
fn evaluate_in_world_raw(surface: &Surface, expression: &str) -> Result<String, String> {
    if let Some(note) = dialog_block_note_for(surface.dialog.borrow().as_ref()) {
        return Err(note);
    }
    let webview2 = surface.webview.webview();
    let context = world_context(surface, &webview2)?;
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
    Ok(raw.to_string())
}

// ---------------------------------------------------------------------------
// Agent operations (probe → install → call, codeg agent.rs)
// ---------------------------------------------------------------------------

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

// One CDP conversation at a time across the whole process.
//
// `call_and_wait` pumps the message loop while it waits, so a second CDP
// call arriving mid-wait re-enters the WebView2 dispatch. Measured
// 2026-11-15: two tabs called at the same instant (debug_state + snapshot)
// froze the main thread for good. It is one main thread and one dispatch,
// so the guard is process-wide rather than per tab — a per-tab flag left
// cross-tab pairs unguarded, which is exactly the pair that wedged it.
thread_local! {
    static CDP_BUSY: Cell<bool> = const { Cell::new(false) };
}

struct CdpLease;

impl CdpLease {
    fn take() -> Result<CdpLease, String> {
        CDP_BUSY.with(|busy| {
            if busy.replace(true) {
                return Err("another browser call is still running; retry in a moment".to_string());
            }
            Ok(CdpLease)
        })
    }
}

impl Drop for CdpLease {
    fn drop(&mut self) {
        CDP_BUSY.with(|busy| busy.set(false));
    }
}

/// Lease access for the diagnostic commands in `mod.rs`, which issue their own
/// CDP calls and would otherwise overlap with a snapshot.
pub fn cdp_lease() -> Result<impl Drop, String> {
    CdpLease::take()
}

/// Take an aria snapshot; installs the engine into the document on first use.
pub fn snapshot(surface: &Surface, request: &SnapshotRequest) -> Result<Value, String> {
    let _lease = CdpLease::take()?;
    let value = snapshot_inner(surface, request)?;
    surface.generation.set(surface.generation.get() + 1);
    lease_adopt(surface, &value, LeaseDeath::SupersededBySnapshot);
    Ok(value)
}

// ---------------------------------------------------------------------------
// Ref lease glue (see ref_lease.rs for the contract: the page engine is the
// only judge of freshness; the host only answers deaths it witnessed).
// ---------------------------------------------------------------------------

/// Fold a fresh snapshot answer into the lease. The answer carries the
/// world's four-segment generation (`pick::generation_from_snapshot`
/// validates the shape); an answer without one leaves the lease untouched —
/// the page stays the only judge, which is the fail-open direction.
fn lease_adopt(surface: &Surface, snapshot: &Value, replaced: LeaseDeath) {
    if !ref_lease::enabled() {
        return;
    }
    let Some(token) = pick::generation_from_snapshot(snapshot) else {
        return;
    };
    let at = now_ms();
    let mut lease = surface.ref_lease.borrow_mut();
    *lease = Some(RefLease::adopt(
        token,
        surface.nav_epoch.get(),
        at,
        replaced,
        lease.as_ref(),
    ));
}

/// Mark the lease dead for a cause the host witnessed (both navigation
/// paths call this). First cause wins; a lease already dead keeps its
/// earlier, truer reason.
fn lease_invalidate(surface: &Surface, death: LeaseDeath) {
    if !ref_lease::enabled() {
        return;
    }
    if let Some(lease) = surface.ref_lease.borrow_mut().as_mut() {
        lease.invalidate(death);
    }
}

/// Pre-lease host behavior bumped the generation half of the epoch after
/// every action. The world never read that bump as invalidation — its
/// refsToken only moves on a new snapshot, and the host never compared the
/// model's token against its counter — so with the lease on the epoch stays
/// stable and one snapshot serves any number of consecutive actions.
/// `MARIONETTE_BROWSER_REF_LEASE=0` restores the old bump exactly.
fn bump_generation_after_act() -> bool {
    !ref_lease::enabled()
}

/// Lease-free core of `snapshot` — the caller holds the lease. The returned
/// JSON carries the world's full generation string; `pick_resolve` needs it
/// verbatim for the bundle's ref validation.
fn snapshot_inner(surface: &Surface, request: &SnapshotRequest) -> Result<Value, String> {
    ensure_world(surface)?;
    let epoch = epoch_of(surface);
    match parse_world_answer(&evaluate_in_world_raw(
        surface,
        &agent::probe_and_snapshot(request, &epoch),
    )?)? {
        WorldAnswer::Json(value) => Ok(value),
        WorldAnswer::Absent => {
            let install = agent::install_and_snapshot(request, &epoch);
            match parse_world_answer(&evaluate_in_world_raw(surface, &install)?)? {
                WorldAnswer::Json(value) => Ok(value),
                // The bundle publishes itself through `globalThis` inside the
                // same evaluation that calls it, so absent-again means the
                // bundle no longer installs — say so instead of failing on
                // the next call with an unrelated parse error.
                WorldAnswer::Absent => {
                    Err("the aria engine did not install itself in this document".to_string())
                }
            }
        }
    }
}

/// Perform one action (click/hover/type/press/select) by element ref.
pub fn act(surface: &Surface, request: &agent::ActionRequest) -> Result<Value, String> {
    // A generation the host knows is dead answers here — no world install,
    // no CDP round trip, and the reason says what actually killed it.
    if let Some(answer) = ref_lease::fast_refusal(
        &surface.ref_lease,
        &surface.url.borrow(),
        &request.generation,
    ) {
        return Ok(answer);
    }
    let _lease = CdpLease::take()?;
    ensure_world(surface)?;
    let expression = agent::act_call(request);
    match parse_world_answer(&evaluate_in_world_raw(surface, &expression)?)? {
        WorldAnswer::Json(mut value) => {
            if bump_generation_after_act() {
                surface.generation.set(surface.generation.get() + 1);
            }
            ref_lease::note_action(
                &surface.ref_lease,
                &request.generation,
                &mut value,
                now_ms(),
            );
            Ok(value)
        }
        WorldAnswer::Absent => {
            Err("the aria engine is not in this document; take a snapshot first".to_string())
        }
    }
}

/// Answer one cheap question about the page (`state` / `ready`) without
/// walking the tree. One eval, a few hundred bytes back.
pub fn probe(surface: &Surface, expression: &str) -> Result<Value, String> {
    let _lease = CdpLease::take()?;
    ensure_world(surface)?;
    match parse_world_answer(&evaluate_in_world_raw(surface, expression)?)? {
        WorldAnswer::Json(value) => Ok(value),
        WorldAnswer::Absent => {
            Err("the page probe did not install itself in this document".to_string())
        }
    }
}

/// Run one in-page find action (`search` / `next` / `prev` / `clear`).
///
/// This is a UI convenience, not an agent tool: it never takes a grant, never
/// installs the aria engine and never advances the ref lease. It does use the
/// process-wide CDP lease so it cannot overlap a snapshot, and the answer is
/// the find script's own JSON (`{count,index,truncated}`).
pub fn find(
    surface: &Surface,
    action: &str,
    query: Option<&str>,
    case_sensitive: bool,
) -> Result<Value, String> {
    let expression = match action {
        "search" => find::search_call(query.unwrap_or(""), case_sensitive),
        "next" => find::step_call(false),
        "prev" => find::step_call(true),
        "clear" => find::clear_call(),
        other => return Err(format!("unknown find action: {other}")),
    };
    let _lease = CdpLease::take()?;
    let raw = evaluate_in_world_raw(surface, &expression)?;
    serde_json::from_str(&raw).map_err(|e| format!("find answer is not json: {e}"))
}

/// Emulate a viewport for the responsive preview. Honest about what it is:
/// the page is rendered at `width x height` CSS px with a fit scale, and the
/// real browsing size is untouched once `clear_viewport` runs. `scale` is the
/// preview fit factor the frontend computes from the host rectangle.
pub fn set_viewport(
    surface: &Surface,
    width: f64,
    height: f64,
    mobile: bool,
    scale: f64,
) -> Result<Value, String> {
    let _lease = CdpLease::take()?;
    let width = width.round().clamp(200.0, 4096.0) as i64;
    let height = height.round().clamp(200.0, 4096.0) as i64;
    let scale = scale.clamp(0.25, 2.0);
    let webview2 = surface.webview.webview();
    let params = json!({
        "width": width,
        "height": height,
        // 0 = keep the page's own device pixel ratio; the preview fit factor
        // below is what scales the rendered surface.
        "deviceScaleFactor": 0,
        "mobile": mobile,
        "scale": scale,
        "screenWidth": width,
        "screenHeight": height,
        "positionX": 0,
        "positionY": 0,
        "screenOrientation": {
            "type": if width >= height { "landscapePrimary" } else { "portraitPrimary" },
            "angle": if width >= height { 90 } else { 0 }
        }
    });
    call_and_wait(
        &webview2,
        "Emulation.setDeviceMetricsOverride",
        &params.to_string(),
    )?;
    let touch = if mobile {
        json!({ "enabled": true, "maxTouchPoints": 5 })
    } else {
        json!({ "enabled": false })
    };
    // Touch emulation is additive: a failure there must not undo the metrics.
    let _ = call_and_wait(&webview2, "Emulation.setTouchEmulationEnabled", &touch.to_string());
    Ok(json!({
        "emulated": true,
        "width": width,
        "height": height,
        "mobile": mobile,
        "scale": scale,
    }))
}

/// Undo `set_viewport` entirely: metrics and touch emulation.
pub fn clear_viewport(surface: &Surface) -> Result<Value, String> {
    let _lease = CdpLease::take()?;
    let webview2 = surface.webview.webview();
    call_and_wait(&webview2, "Emulation.clearDeviceMetricsOverride", "{}")?;
    let _ = call_and_wait(
        &webview2,
        "Emulation.setTouchEmulationEnabled",
        &json!({ "enabled": false }).to_string(),
    );
    Ok(json!({ "emulated": false }))
}

/// Write one control by ref, through the page probe (Control level; the
/// caller does the grant check). Goes through `probe`'s lease, so it cannot
/// overlap a snapshot. Same ref-lease treatment as `act`: known-dead
/// generations answer from the lease, successes say the generation is
/// still good.
pub fn set_control_value(
    surface: &Surface,
    generation: &str,
    target: &str,
    value: &Value,
) -> Result<Value, String> {
    if let Some(answer) =
        ref_lease::fast_refusal(&surface.ref_lease, &surface.url.borrow(), generation)
    {
        return Ok(answer);
    }
    let mut answer = probe(
        surface,
        &crate::browser::probe::set_value_call(generation, target, value),
    )?;
    ref_lease::note_action(&surface.ref_lease, generation, &mut answer, now_ms());
    Ok(answer)
}

/// Answer a page script dialog (browser_dialog tool; Control level — the
/// caller does the grant check). This is the one CDP call allowed while the
/// gate is up: `Page.handleJavaScriptDialog` is browser-process level and
/// needs no renderer cooperation.
///
/// An absent dialog answers `present:false` honestly AND clears any stale
/// mirror — that is the recovery path when the user already answered the
/// default dialog themselves. Never auto-accepts anything: `accept` is the
/// agent's explicit choice (or the user's, via the native dialog).
pub fn resolve_dialog(
    surface: &Surface,
    accept: bool,
    prompt_text: Option<&str>,
) -> Result<Value, String> {
    let pending = surface
        .dialog
        .borrow()
        .as_ref()
        .map(|d| (d.kind.clone(), d.message.clone()));
    let Some((kind, message)) = pending else {
        *surface.dialog.borrow_mut() = None;
        return Ok(json!({
            "present": false,
            "note": "no script dialog is open on this page (or the user already answered it)",
        }));
    };
    let _lease = CdpLease::take()?;
    let webview2 = surface.webview.webview();
    let mut params = json!({ "accept": accept });
    if let Some(text) = prompt_text {
        params["promptText"] = json!(text);
    }
    let answer = call_and_wait(
        &webview2,
        "Page.handleJavaScriptDialog",
        &params.to_string(),
    );
    match answer {
        Ok(raw) => {
            let parsed: Value = serde_json::from_str(&raw).unwrap_or(json!({}));
            if parsed.get("error").is_some() {
                // "No dialog is showing" and friends: the mirror lied (the
                // user clicked first). Clear it and say so.
                *surface.dialog.borrow_mut() = None;
                return Ok(json!({
                    "present": false,
                    "note": "the dialog is already gone — the user likely answered the visible dialog",
                    "detail": parsed,
                }));
            }
            *surface.dialog.borrow_mut() = None;
            Ok(json!({ "present": true, "kind": kind, "message": message, "accepted": accept }))
        }
        // Transport-level failure: leave the mirror up (the dialog may
        // genuinely still block the page); the agent can retry or the user
        // can answer the native dialog.
        Err(error) => Err(format!("dialog resolve failed: {error}")),
    }
}

/// Console tail, newest last. A `missing` count tells the caller the ring
/// dropped lines — "nothing more printed" and "more existed" differ.
pub fn console_lines(surface: &Surface, limit: usize) -> Value {
    // Installing the world is CDP work; if another call holds the lease this
    // call skips it (the ring is already buffered) and still answers.
    if let Ok(_lease) = CdpLease::take() {
        let _ = ensure_world(surface);
    }
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
    // Page.captureScreenshot needs a renderer frame, so it stalls behind an
    // open script dialog just like an eval would — and this call would hold
    // the process-wide CDP lease while it did. Same gate as the evals.
    if let Some(note) = dialog_block_note_for(surface.dialog.borrow().as_ref()) {
        return Err(note);
    }
    let _lease = CdpLease::take()?;
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
    let image = image::load_from_memory(&bytes).map_err(|e| format!("capture decode: {e}"))?;
    encode_capture(narrow_to(image, max_width))
}

/// Encode to the `{dataUrl,width,height}` shape, re-encoding narrower until
/// the bytes fit the budget. Shared by the full-page screenshot and pick crops.
fn encode_capture(mut image: image::DynamicImage) -> Result<Value, String> {
    loop {
        let mut jpeg = Vec::new();
        let encoder =
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, CAPTURE_JPEG_QUALITY);
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
        let narrower = ((f64::from(image.width()) * ratio).floor() as u32).clamp(
            CAPTURE_MIN_WIDTH,
            image.width().saturating_sub(1).max(CAPTURE_MIN_WIDTH),
        );
        image = narrow_to(image, narrower);
    }
}

/// Crop the captured frame down to a viewport rect (CSS px × dpr). `None`
/// when the rect misses the frame entirely (page scrolled between pick and
/// crop, or a zero-size rect).
fn crop_capture(
    image: image::DynamicImage,
    rect: (f64, f64, f64, f64),
    dpr: f64,
) -> Option<image::DynamicImage> {
    let (x, y, width, height) = rect;
    if width <= 0.0 || height <= 0.0 || dpr <= 0.0 {
        return None;
    }
    let left = ((x * dpr).floor() as i64).clamp(0, i64::from(image.width()));
    let top = ((y * dpr).floor() as i64).clamp(0, i64::from(image.height()));
    let right = (((x + width) * dpr).ceil() as i64).clamp(0, i64::from(image.width()));
    let bottom = (((y + height) * dpr).ceil() as i64).clamp(0, i64::from(image.height()));
    if right <= left || bottom <= top {
        return None;
    }
    Some(image.crop_imm(
        left as u32,
        top as u32,
        (right - left) as u32,
        (bottom - top) as u32,
    ))
}

// ---------------------------------------------------------------------------
// Element picker (the browser toolbar's annotate mode — UI path, no grants:
// grants gate agent tool calls, `tool_read`/`control_gate` stay the one place)
// ---------------------------------------------------------------------------

/// Start / stop / switch the mode of the element picker on this document.
/// `start` on a running picker is the mode switch (the script keeps its
/// retained picks across it).
pub fn pick_set(surface: &Surface, active: bool, mode: &str) -> Result<Value, String> {
    let _lease = CdpLease::take()?;
    ensure_world(surface)?;
    let expression = if active {
        pick::start_call(mode)
    } else {
        pick::stop_call()
    };
    match parse_world_answer(&evaluate_in_world_raw(surface, &expression)?)? {
        WorldAnswer::Json(value) => {
            if active && value["ok"].as_bool() == Some(false) {
                let error = value["error"]
                    .as_str()
                    .unwrap_or("the page picker could not start");
                return Err(error.to_string());
            }
            surface.picker_active.set(active);
            Ok(value)
        }
        WorldAnswer::Absent => {
            Err("the page picker did not install itself in this document".to_string())
        }
    }
}

/// Forget retained picks and stop the page-side picker. Local state is cleared
/// even when the document context disappeared between the click and this
/// command; the best-effort page clear is only to reset its element map.
pub fn pick_clear(surface: &Surface) -> Result<Value, String> {
    let _lease = CdpLease::take()?;
    surface.picker_active.set(false);
    surface.picks.borrow_mut().clear();
    if surface.world_ctx.get().is_some() {
        let _ = evaluate_in_world_raw(surface, &pick::clear_call());
    }
    Ok(json!({ "ok": true }))
}

/// Crop the viewport screenshot down to one pick's rectangle. The page hid
/// its highlight first — the crop shows the page, not the picker. The rect
/// comes from the retained pick payload (measured at pick time).
pub fn pick_finalize(surface: &Surface, pick_id: &str) -> Result<Value, String> {
    // The capture below is not an eval, so the eval gate does not cover it.
    if let Some(note) = dialog_block_note_for(surface.dialog.borrow().as_ref()) {
        return Err(note);
    }
    let _lease = CdpLease::take()?;
    let (rect, dpr) = {
        let picks = surface.picks.borrow();
        let entry = picks
            .iter()
            .find(|(id, _)| id == pick_id)
            .map(|(_, message)| message)
            .ok_or_else(|| format!("no pending pick {pick_id}"))?;
        let is_region = entry["kind"].as_str() == Some("region");
        let rect_node = if is_region {
            &entry["rect"]
        } else {
            &entry["info"]["rect"]
        };
        let dpr = entry["info"]["dpr"]
            .as_f64()
            .or_else(|| entry["dpr"].as_f64())
            .unwrap_or(1.0);
        (
            (
                rect_node["x"].as_f64().unwrap_or(0.0),
                rect_node["y"].as_f64().unwrap_or(0.0),
                rect_node["width"].as_f64().unwrap_or(0.0),
                rect_node["height"].as_f64().unwrap_or(0.0),
            ),
            dpr,
        )
    };
    ensure_world(surface)?;
    // Best-effort pair: a world that cannot answer the hide call has bigger
    // problems, and the restore must be attempted either way.
    let _ = evaluate_in_world_raw(surface, &pick::capture_hide_call());
    let webview2 = surface.webview.webview();
    let captured = call_and_wait(
        &webview2,
        "Page.captureScreenshot",
        &json!({ "format": "jpeg", "quality": CAPTURE_JPEG_QUALITY }).to_string(),
    );
    let _ = evaluate_in_world_raw(surface, &pick::capture_restore_call());
    let answer = captured?;
    let parsed: Value = serde_json::from_str(&answer).map_err(|e| format!("capture: {e}"))?;
    let data = parsed["data"].as_str().ok_or("capture returned no image")?;
    let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, data)
        .map_err(|e| format!("capture decode: {e}"))?;
    let image = image::load_from_memory(&bytes).map_err(|e| format!("capture decode: {e}"))?;
    let Some(cropped) = crop_capture(image, rect, dpr) else {
        return Err("the picked rectangle is outside the captured viewport".to_string());
    };
    let full = narrow_to(cropped, CAPTURE_DEFAULT_MAX_WIDTH);
    let width = full.width();
    let height = full.height();
    let mut jpeg = Vec::new();
    let encoder =
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, CAPTURE_JPEG_QUALITY);
    full.write_with_encoder(encoder)
        .map_err(|e| format!("capture encode: {e}"))?;
    let dir = crate::app_paths::global_dir()?.join("clipboard");
    std::fs::create_dir_all(&dir).map_err(|e| format!("create clipboard dir failed: {e}"))?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let hash = pick_id.bytes().fold(0u32, |acc, byte| {
        acc.wrapping_mul(31).wrapping_add(u32::from(byte))
    });
    let path = dir.join(format!("browser-pick-{stamp}-{hash:08x}.jpg"));
    std::fs::write(&path, &jpeg).map_err(|e| format!("write pick capture failed: {e}"))?;
    let thumb = full.thumbnail(160, 120);
    let mut value = encode_capture(thumb)?;
    value["pickId"] = json!(pick_id);
    value["path"] = json!(path.to_string_lossy());
    value["mimeType"] = json!("image/jpeg");
    value["width"] = json!(width);
    value["height"] = json!(height);
    Ok(value)
}

/// Resolve the retained picks to aria refs as of a fresh snapshot — one
/// snapshot per send, not per pick. The tree itself is discarded: only its
/// `generation` feeds the world's ref validation.
pub fn pick_resolve(surface: &Surface) -> Result<Value, String> {
    let _lease = CdpLease::take()?;
    let snapshot = snapshot_inner(
        surface,
        &SnapshotRequest {
            max_chars: PICK_RESOLVE_MAX_CHARS,
        },
    )?;
    surface.generation.set(surface.generation.get() + 1);
    // The hidden full snapshot above replaced the world's refs — the agent's
    // token for the previous snapshot dies of it. Adopt this snapshot's
    // token with that cause, so the next action against the old one answers
    // "user_annotation" instead of the page's misleading truncation prose.
    lease_adopt(surface, &snapshot, LeaseDeath::UserAnnotation);
    let generation = pick::generation_from_snapshot(&snapshot)
        .ok_or("the snapshot did not name a usable generation")?;
    let refs = match parse_world_answer(&evaluate_in_world_raw(
        surface,
        &pick::resolve_refs_call(&generation),
    )?)? {
        WorldAnswer::Json(value) => value,
        // The picker never ran on this document (e.g. picks from the previous
        // page being sent after a navigation): no refs is the honest answer.
        WorldAnswer::Absent => json!({}),
    };
    Ok(json!({ "generation": generation, "refs": refs }))
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
            let installed = unsafe {
                SetWindowSubclass(hwnd, Some(resize_hook_proc), RESIZE_HOOK_ID, held as usize)
            };
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
                    (*controller).SetBounds(RECT {
                        left: 0,
                        top: 0,
                        right: width,
                        bottom: height,
                    })
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
pub fn create_surface(
    app: &AppHandle,
    url: &str,
    opened_by_agent: bool,
    display: &str,
    owner_session_id: Option<&str>,
    chosen_id: Option<&str>,
) -> Result<String, String> {
    let surface = build_surface(
        app,
        url,
        opened_by_agent,
        display,
        owner_session_id,
        chosen_id,
    )?;
    let id = surface.id.clone();
    register_surface(app, &surface);
    let url = surface.url.borrow().clone();
    if let Err(error) = navigate(&surface, &url) {
        let _ = close_surface(app, &id);
        return Err(error);
    }
    Ok(id)
}

/// Build the webview and its Surface WITHOUT touching the registry;
/// `create_surface` registers immediately after.
fn build_surface(
    app: &AppHandle,
    url: &str,
    opened_by_agent: bool,
    display: &str,
    owner_session_id: Option<&str>,
    chosen_id: Option<&str>,
) -> Result<Rc<Surface>, String> {
    let window = app
        .get_webview_window("main")
        .ok_or("main window is gone")?;
    ensure_resize_hook(&window)?;

    let id = format!(
        "b{}",
        NEXT_ID.with(|n| {
            let value = n.get();
            n.set(value + 1);
            value
        })
    );
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
    let popup_display = display.to_string();
    let popup_owner_session_id = owner_session_id.map(str::to_string);
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
                let display_for_job = popup_display.clone();
                let owner_for_job = popup_owner_session_id.clone();
                let _ = popup_app.run_on_main_thread(move || {
                    if let Err(error) = create_surface(
                        &app_for_job,
                        &url,
                        popup_by_agent,
                        &display_for_job,
                        owner_for_job.as_deref(),
                        None,
                    ) {
                        crate::debug_log::append(
                            "browser",
                            "warn",
                            "",
                            "popup open failed",
                            Some(&error),
                        );
                    }
                });
                NewWindowResponse::Deny
            })
            .build_as_child(&window)
            .map_err(|e| format!("webview build failed: {e}"))
    })?;

    let surface = Rc::new(Surface {
        id,
        opened_by_agent: Cell::new(opened_by_agent),
        display: display.to_string(),
        owner_session_id: RefCell::new(owner_session_id.map(str::to_string)),
        chosen_id: chosen_id.map(str::to_string),
        pinned: Cell::new(false),
        busy_count: Cell::new(0),
        status: RefCell::new("idle".to_string()),
        last_used_at_ms: Cell::new(now_ms()),
        webview,
        app: app.clone(),
        world_ctx: Cell::new(None),
        world_installed: Cell::new(false),
        nav_epoch: Cell::new(0),
        generation: Cell::new(0),
        picker_active: Cell::new(false),
        picks: RefCell::new(VecDeque::new()),
        ref_lease: RefCell::new(None),
        dialog: RefCell::new(None),
        console: RefCell::new(VecDeque::new()),
        url: RefCell::new(url.clone()),
        title: RefCell::new(String::new()),
        origin: RefCell::new(origin.clone()),
        grant: RefCell::new(if opened_by_agent {
            // The agent chose to open this page: reading it is the point.
            // Control still asks (permission card / manual share).
            Some(AgentGrant {
                level: GrantLevel::Read,
                origin,
                granted_at_ms: now_ms(),
            })
        } else {
            None
        }),
    });

    // Navigation starts after registration so the cache-policy continuation
    // can find this surface through the registry.
    Ok(surface)
}

/// Enter a surface into the registry and tell the frontend.
fn register_surface(app: &AppHandle, surface: &Rc<Surface>) {
    let webview2 = surface.webview.webview();
    let key = webview2.as_raw() as usize;
    SURFACES.with(|surfaces| {
        surfaces
            .borrow_mut()
            .insert(surface.id.clone(), surface.clone())
    });
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
    if trimmed
        .get(..5)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("file:"))
    {
        let url = Url::parse(trimmed).map_err(|e| format!("invalid file url: {e}"))?;
        url.to_file_path()
            .map_err(|_| "file urls must point to an absolute local path".to_string())?;
        return Ok(url.to_string());
    }
    if Path::new(trimmed).is_absolute() {
        return Url::from_file_path(trimmed)
            .map(|url| url.to_string())
            .map_err(|_| "local file path is not absolute".to_string());
    }
    let with_scheme = if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_string()
    } else if looks_like_loopback_authority(trimmed) {
        format!("http://{trimmed}")
    } else if trimmed.contains(' ') || !trimmed.contains('.') {
        return Err("only http(s) urls can be opened".to_string());
    } else {
        format!("https://{trimmed}")
    };
    Ok(rewrite_http_loopback_to_ipv4(&with_scheme))
}

/// Address-bar / agent shorthand: `localhost:5173`, `127.0.0.1:3000`, `[::1]`.
fn looks_like_loopback_authority(raw: &str) -> bool {
    let authority = raw.split(['/', '?', '#']).next().unwrap_or(raw);
    let lower = authority.to_ascii_lowercase();
    lower == "localhost"
        || lower.starts_with("localhost:")
        || lower == "127.0.0.1"
        || lower.starts_with("127.0.0.1:")
        || lower == "0.0.0.0"
        || lower.starts_with("0.0.0.0:")
        || lower == "[::1]"
        || lower.starts_with("[::1]:")
        || lower == "::1"
}

/// WebView2 on Windows often resolves `localhost` to `::1` first; servers bound
/// only to IPv4 then fail. http://localhost and http://[::1] become 127.0.0.1.
/// https stays as typed so a localhost certificate still matches.
fn rewrite_http_loopback_to_ipv4(raw: &str) -> String {
    // Bracketed IPv6 literals: some URL parsers leave `http://[::1]:port` as-is.
    if let Some(rest) = raw.strip_prefix("http://[::1]") {
        return format!("http://127.0.0.1{rest}");
    }
    let Ok(mut url) = Url::parse(raw) else {
        return raw.to_string();
    };
    if url.scheme() != "http" {
        return raw.to_string();
    }
    let host = url.host_str().unwrap_or("").to_ascii_lowercase();
    if host != "localhost" && host != "::1" && host != "0.0.0.0" {
        return raw.to_string();
    }
    if url.set_host(Some("127.0.0.1")).is_err() {
        return raw.to_string();
    }
    url.to_string()
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
        let result = SURFACES
            .with(|surfaces| surfaces.borrow().get(&tab_id).cloned())
            .map(|s| f(&s));
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
    use super::{
        classify_nav_state, dialog_block_note_for, dialog_from_event, nav_state_fields,
        normalize_url, parse_world_answer, should_bypass_http_cache, url_origin, CdpLease,
        GrantLevel, NavStateChange, PendingDialog, WorldAnswer, DIALOG_MESSAGE_MAX_CHARS,
    };
    use serde_json::json;

    #[test]
    fn grant_levels_rank() {
        assert!(GrantLevel::Control.allows(GrantLevel::Read));
        assert!(GrantLevel::Read.allows(GrantLevel::Read));
        assert!(!GrantLevel::Read.allows(GrantLevel::Control));
        assert!(!GrantLevel::None.allows(GrantLevel::Read));
    }

    #[test]
    fn origins_strip_paths_and_queries() {
        assert_eq!(
            url_origin("https://example.com/a/b?x=1"),
            "https://example.com"
        );
        assert_eq!(
            url_origin("http://localhost:5173/"),
            "http://localhost:5173"
        );
        assert_eq!(url_origin("about:blank"), "about:blank");
        assert_eq!(
            url_origin("file:///D:/Projects/One/index.html?x=1#start"),
            "file:///D:/Projects/One/index.html"
        );
        assert_ne!(
            url_origin("file:///D:/Projects/One/index.html"),
            url_origin("file:///D:/Projects/Two/index.html")
        );
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

    #[test]
    fn local_file_urls_and_absolute_windows_paths_are_preserved() {
        assert_eq!(
            normalize_url("file:///D:/Projects/XXX/index.html").unwrap(),
            "file:///D:/Projects/XXX/index.html"
        );
        assert_eq!(
            normalize_url(r"D:\Projects\XXX\index.html").unwrap(),
            "file:///D:/Projects/XXX/index.html"
        );
        assert_eq!(
            normalize_url(r"C:\Projects\My Game\index.html").unwrap(),
            "file:///C:/Projects/My%20Game/index.html"
        );
        assert!(normalize_url("file:relative/index.html").is_err());
    }

    #[test]
    fn loopback_without_scheme_opens_over_http() {
        assert_eq!(
            normalize_url("localhost:5173").unwrap(),
            "http://127.0.0.1:5173/"
        );
        assert_eq!(
            normalize_url("127.0.0.1:3000").unwrap(),
            "http://127.0.0.1:3000"
        );
        assert_eq!(
            normalize_url("http://localhost:5173/app").unwrap(),
            "http://127.0.0.1:5173/app"
        );
        assert_eq!(
            normalize_url("https://localhost/secure").unwrap(),
            "https://localhost/secure"
        );
        assert_eq!(
            normalize_url("[::1]:8080").unwrap(),
            "http://127.0.0.1:8080"
        );
    }

    #[test]
    fn local_http_origins_bypass_the_stale_cache() {
        for url in [
            "http://localhost:8138/Prototypes/DialogueLab/",
            "http://127.0.0.1:8138/index.html",
            "http://127.8.9.10/dev",
            "http://[::1]:5173/",
            "https://app.localhost/",
            "http://0.0.0.0:3000/",
        ] {
            assert!(should_bypass_http_cache(url), "{url}");
        }
    }

    #[test]
    fn public_and_non_http_urls_keep_normal_cache_policy() {
        for url in [
            "https://example.com/docs",
            "http://192.168.1.20/",
            "file:///D:/Projects/XXX/index.html",
            "about:blank",
            "not a url",
        ] {
            assert!(!should_bypass_http_cache(url), "{url}");
        }
    }

    #[test]
    fn helper_nav_state_separates_title_routes_and_documents() {
        // A title mutation must not clear the world, console, or annotation
        // payloads; a pushState route keeps the document but moves the epoch.
        assert_eq!(
            classify_nav_state(
                "https://example.com/a",
                "https://example.com/a",
                "Loading",
                "Ready",
            ),
            NavStateChange::Title
        );
        assert_eq!(
            classify_nav_state(
                "https://example.com/a",
                "https://example.com/b",
                "Loading",
                "Ready",
            ),
            NavStateChange::Route
        );
        assert_eq!(
            classify_nav_state("https://example.com/a", "https://example.com/a", "A", "A"),
            NavStateChange::None
        );
    }

    #[test]
    fn helper_nav_state_accepts_the_bundles_href_field() {
        let (url, title) = nav_state_fields(&json!({
            "payload": { "href": "https://example.com/next", "title": "Next" }
        }));
        assert_eq!(url, "https://example.com/next");
        assert_eq!(title, "Next");
    }

    // The absent sentinel is a bare string: parsing it as JSON is exactly the
    // bug this split exists to prevent. It has to read as Absent, and every
    // real answer — always JSON.stringify output — as Json.
    #[test]
    fn absent_sentinel_is_not_parsed_as_json() {
        assert_eq!(parse_world_answer("absent").unwrap(), WorldAnswer::Absent);
        assert_eq!(
            parse_world_answer(r#"{"generation":"0.0.1.0","tree":"- button"}"#).unwrap(),
            WorldAnswer::Json(json!({ "generation": "0.0.1.0", "tree": "- button" }))
        );
    }

    #[test]
    fn non_json_answers_still_report_what_arrived() {
        // A page that answers something else entirely is a real failure, and
        // the message must name the parse problem rather than blame the engine.
        let error = parse_world_answer("<html>").unwrap_err();
        assert!(error.starts_with("engine answer is not json:"), "{error}");
        assert!(error.contains("column 1"), "{error}");
    }

    // One CDP conversation at a time: `call_and_wait` pumps the message loop,
    // so a second call taken while the first waits re-enters the WebView2
    // dispatch and freezes the main thread. The lease is process-wide because
    // the freeze reproduced across two different tabs.
    #[test]
    fn one_cdp_call_runs_at_a_time() {
        let first = CdpLease::take().expect("first call is free to run");
        let second = CdpLease::take();
        assert!(
            second.is_err(),
            "a concurrent call must be refused, not queued"
        );
        drop(first);
        let third = CdpLease::take();
        assert!(third.is_ok(), "the lease must free when the call ends");
    }

    // The default (flag unset) is lease-on: actions keep the epoch stable so
    // one snapshot serves consecutive actions. MARIONETTE_BROWSER_REF_LEASE=0
    // is the documented rollback to the per-action bump.
    #[test]
    fn actions_do_not_bump_the_generation_by_default() {
        assert!(!super::bump_generation_after_act());
        assert!(super::ref_lease::enabled());
    }

    // The dialog event payload is page-controlled: parsed defensively, the
    // message capped, an empty type refused.
    #[test]
    fn dialog_event_parses_defensively() {
        let dialog = dialog_from_event(
            &json!({ "type": "confirm", "message": "Delete all files?" }),
            1234,
        )
        .unwrap();
        assert_eq!(dialog.kind, "confirm");
        assert_eq!(dialog.message, "Delete all files?");
        assert_eq!(dialog.at_ms, 1234);
        assert_eq!(dialog_from_event(&json!({ "message": "no type" }), 0), None);
        let flood =
            dialog_from_event(&json!({ "type": "alert", "message": "x".repeat(500) }), 0).unwrap();
        assert!(flood.message.chars().count() <= DIALOG_MESSAGE_MAX_CHARS);
    }

    // While a dialog blocks the renderer, evals must fast-fail with the two
    // recovery paths named — never hang inside the CDP lease.
    #[test]
    fn dialog_gate_names_the_exits_and_stays_silent_when_clear() {
        let dialog = PendingDialog {
            kind: "prompt".to_string(),
            message: "Your token".to_string(),
            at_ms: 0,
        };
        let note = dialog_block_note_for(Some(&dialog)).unwrap();
        assert!(note.starts_with("dialog_blocked:"), "{note}");
        assert!(note.contains("browser_dialog"), "{note}");
        assert!(note.contains("user"), "{note}");
        assert!(dialog_block_note_for(None).is_none());
    }
}
