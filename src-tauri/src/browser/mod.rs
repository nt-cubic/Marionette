//! Built-in browser tabs (batch 2 of the browser/computer-use plan).
//!
//! A tab is a wry child webview of the main window (see `surface.rs`). This
//! module owns everything above it: tauri commands for the frontend, the
//! grant mutations the toolbar performs, and the in-conversation permission
//! flow Control-level actions go through. The agent-facing tool surface lives
//! in `crate::tools_mcp` and calls the same functions — MCP reads walk the
//! **same** grant checks as the toolbar, never a second set.

pub mod agent;
#[cfg(target_os = "windows")]
pub mod surface;

#[cfg(target_os = "windows")]
mod imp {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::sync::Mutex;
    use std::time::Duration;

    use serde_json::{json, Value};
    use tauri::{AppHandle, Emitter};

    use super::surface;
    use surface::{AgentGrant, GrantLevel, Surface, TabInfo};

    /// Global switch for the agent tool surface (the frontend settings page).
    static TOOLS_ENABLED: AtomicBool = AtomicBool::new(true);

    pub fn tools_enabled() -> bool {
        TOOLS_ENABLED.load(Ordering::Relaxed)
    }

    /// Sessions the user put into "always approve": a Control request from
    /// one of them is granted without a card, mirroring what the mode already
    /// means for tool permissions. Pushed by the frontend when the toggle
    /// moves and when a session is spawned.
    static SESSION_ALWAYS_APPROVE: Mutex<Option<HashMap<String, bool>>> = Mutex::new(None);

    pub fn session_auto_approves(session_id: &str) -> bool {
        SESSION_ALWAYS_APPROVE
            .lock()
            .ok()
            .and_then(|map| map.as_ref().and_then(|m| m.get(session_id).copied()))
            .unwrap_or(false)
    }

    // -----------------------------------------------------------------------
    // Permission requests (the "card" the frontend answers; refusal is a value)
    // -----------------------------------------------------------------------

    /// Timeout for an unanswered card: silence is a denial (codeg eval
    /// confirm semantics — never wait forever, never default to yes).
    const PERMISSION_TIMEOUT: Duration = Duration::from_secs(120);

    static PENDING_PERMISSIONS: Mutex<Option<HashMap<String, mpsc::Sender<bool>>>> = Mutex::new(None);

    #[derive(Clone, serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    pub struct PermissionRequest {
        pub request_id: String,
        pub session_id: String,
        pub tab_id: String,
        pub url: String,
        pub origin: String,
        pub summary: String,
    }

    /// Ask the user, through the frontend, for one Control-level action.
    /// Returns false on timeout — a refusal the agent can relay.
    pub fn request_permission(
        app: &AppHandle,
        session_id: &str,
        tab_id: &str,
        url: &str,
        origin: &str,
        summary: &str,
    ) -> bool {
        let request_id = format!(
            "bp{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let (tx, rx) = mpsc::channel();
        if let Ok(mut guard) = PENDING_PERMISSIONS.lock() {
            guard.get_or_insert_with(HashMap::new).insert(request_id.clone(), tx);
        } else {
            return false;
        }
        let request = PermissionRequest {
            request_id: request_id.clone(),
            session_id: session_id.to_string(),
            tab_id: tab_id.to_string(),
            url: url.to_string(),
            origin: origin.to_string(),
            summary: summary.to_string(),
        };
        let _ = app.emit("marionette-browser-permission", &request);
        let answer = rx.recv_timeout(PERMISSION_TIMEOUT).unwrap_or(false);
        if let Ok(mut guard) = PENDING_PERMISSIONS.lock() {
            if let Some(map) = guard.as_mut() {
                map.remove(&request_id);
            }
        }
        answer
    }

    /// The user answered a card. `allow` false covers the explicit deny and
    /// the dismiss; the timeout path never reaches here.
    pub fn resolve_permission(request_id: &str, allow: bool) -> bool {
        let sender = PENDING_PERMISSIONS
            .lock()
            .ok()
            .and_then(|mut guard| guard.as_mut().and_then(|map| map.remove(request_id)));
        match sender {
            Some(tx) => tx.send(allow).is_ok(),
            None => false,
        }
    }

    // -----------------------------------------------------------------------
    // Tab operations (thread-safe wrappers over the main-thread registry)
    // -----------------------------------------------------------------------

    /// Open a tab. `opened_by_agent` decides the starting grant: agent tabs
    /// read on arrival (the agent chose the page); user tabs share nothing
    /// until the toolbar says so.
    pub fn open_tab(app: &AppHandle, url: &str, opened_by_agent: bool) -> Result<String, String> {
        let (tx, rx) = mpsc::channel();
        let url = url.to_string();
        let app_for_job = app.clone();
        app.run_on_main_thread(move || {
            let result = surface::create_surface(&app_for_job, &url, opened_by_agent);
            let _ = tx.send(result);
        })
        .map_err(|e| format!("main thread hop: {e}"))?;
        rx.recv().map_err(|_| "main thread did not answer".to_string())?
    }

    pub fn close_tab(app: &AppHandle, tab_id: &str) -> Result<(), String> {
        let (tx, rx) = mpsc::channel();
        let tab_id = tab_id.to_string();
        let app_for_job = app.clone();
        app.run_on_main_thread(move || {
            let result = surface::close_surface(&app_for_job, &tab_id);
            let _ = tx.send(result);
        })
        .map_err(|e| format!("main thread hop: {e}"))?;
        rx.recv().map_err(|_| "main thread did not answer".to_string())?
    }

    pub fn list_tabs(app: &AppHandle) -> Result<Vec<TabInfo>, String> {
        let (tx, rx) = mpsc::channel();
        app.run_on_main_thread(move || {
            let _ = tx.send(surface::tabs_snapshot());
        })
        .map_err(|e| format!("main thread hop: {e}"))?;
        rx.recv().map_err(|_| "main thread did not answer".to_string())
    }

    /// Grant state of one tab, as the tool surface checks it.
    pub fn grant_level(app: &AppHandle, tab_id: &str) -> Result<GrantLevel, String> {
        surface::with_surface(app, tab_id, |s| {
            s.grant.borrow().as_ref().map(|g| g.level).unwrap_or(GrantLevel::None)
        })
    }

    pub fn tab_url(app: &AppHandle, tab_id: &str) -> Result<String, String> {
        surface::with_surface(app, tab_id, |s| s.url.borrow().clone())
    }

    pub fn tab_origin(app: &AppHandle, tab_id: &str) -> Result<String, String> {
        surface::with_surface(app, tab_id, |s| s.origin.borrow().clone())
    }

    /// Toolbar share: set the grant for the tab's CURRENT origin by hand.
    /// `None` revokes. Emits the tab list so every open UI updates.
    pub fn set_grant(app: &AppHandle, tab_id: &str, level: GrantLevel) -> Result<(), String> {
        let (tx, rx) = mpsc::channel();
        let tab_id = tab_id.to_string();
        let app_for_job = app.clone();
        app.run_on_main_thread(move || {
            let result = (|| {
                let tab = surface::find_surface(&tab_id)
                    .ok_or_else(|| format!("no such browser tab: {tab_id}"))?;
                match level {
                    GrantLevel::None => *tab.grant.borrow_mut() = None,
                    _ => {
                        let origin = tab.origin.borrow().clone();
                        *tab.grant.borrow_mut() = Some(AgentGrant {
                            level,
                            origin,
                            granted_at_ms: surface::now_ms_pub(),
                        });
                    }
                }
                surface::emit_tabs(&app_for_job);
                Ok(())
            })();
            let _ = tx.send(result);
        })
        .map_err(|e| format!("main thread hop: {e}"))?;
        rx.recv().map_err(|_| "main thread did not answer".to_string())?
    }

    /// A read-level tool call (snapshot / console / screenshot): grant check
    /// first, then run. A refusal is a value the agent can relay — the tool
    /// call itself succeeds.
    pub fn tool_read(
        app: &AppHandle,
        tab_id: &str,
        run: impl FnOnce(&Surface) -> Result<Value, String> + Send + 'static,
    ) -> Result<Value, String> {
        let level = grant_level(app, tab_id)?;
        if !level.allows(GrantLevel::Read) {
            return Ok(json!({
                "tabId": tab_id,
                "error": "browser_grant_required",
                "note": grant_required_note(tab_id),
            }));
        }
        surface::with_surface(app, tab_id, run).and_then(|inner| inner)
    }

    pub fn grant_required_note(tab_id: &str) -> String {
        format!(
            "Browser tab {tab_id} is not shared with agents. Ask the user to open that tab and press \
             \"Share\" in its toolbar, then try again. Sharing is theirs to give — there is no way to \
             take it, and no point retrying until they have."
        )
    }

    // ── tauri commands (frontend surface) ─────────────────────────────────

    #[tauri::command(async)]
    pub fn browser_open_tab(app: AppHandle, url: String) -> Result<Value, String> {
        open_tab(&app, &url, false).map(|id| json!({ "tabId": id }))
    }

    #[tauri::command(async)]
    pub fn browser_close_tab(app: AppHandle, tab_id: String) -> Result<(), String> {
        close_tab(&app, &tab_id)
    }

    #[tauri::command(async)]
    pub fn browser_navigate_tab(app: AppHandle, tab_id: String, url: String) -> Result<(), String> {
        let url = surface::normalize_url(&url)?;
        surface::with_surface(&app, &tab_id, move |s| {
            s.webview.load_url(&url).map_err(|e| format!("navigate: {e}"))?;
            surface::apply_main_frame_navigation(s, url.clone());
            Ok::<(), String>(())
        })??;
        let (tx, rx) = mpsc::channel();
        let app_for_job = app.clone();
        app.run_on_main_thread(move || {
            surface::emit_tabs(&app_for_job);
            let _ = tx.send(());
        })
        .map_err(|e| e.to_string())?;
        let _ = rx.recv();
        Ok(())
    }

    #[tauri::command(async)]
    pub fn browser_set_tab_bounds(
        app: AppHandle,
        tab_id: String,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    ) -> Result<(), String> {
        surface::with_surface(&app, &tab_id, move |s| {
            s.webview
                .set_bounds(wry::Rect {
                    position: wry::dpi::LogicalPosition::new(x, y).into(),
                    size: wry::dpi::LogicalSize::new(width, height).into(),
                })
                .map_err(|e| format!("set_bounds: {e}"))
        })?
    }

    #[tauri::command(async)]
    pub fn browser_set_tab_visible(app: AppHandle, tab_id: String, visible: bool) -> Result<(), String> {
        surface::with_surface(&app, &tab_id, move |s| {
            // Stock wry set_visible, exactly codeg's call: our vendored patch
            // pairs TrySuspend/Resume INSIDE it — a custom ShowWindow path
            // skips the Resume and the page never composites (2026-10-31).
            s.webview.set_visible(visible).map_err(|e| format!("set_visible: {e}"))
        })?
    }

    #[tauri::command(async)]
    pub fn browser_list_tabs(app: AppHandle) -> Result<Vec<TabInfo>, String> {
        list_tabs(&app)
    }

    #[tauri::command(async)]
    pub fn browser_share_tab(app: AppHandle, tab_id: String, level: String) -> Result<(), String> {
        set_grant(&app, &tab_id, GrantLevel::parse(&level))
    }


    /// Diagnostic (E2E harness): the CDP-level truth about one tab — what the
    /// renderer loaded vs what the window composited. Not for the product UI.
    #[tauri::command(async)]
    pub fn browser_debug_state(app: AppHandle, tab_id: String) -> Result<Value, String> {
        crate::debug_log::append("browser", "info", "", "debug state invoked", Some(&tab_id));
        surface::with_surface(&app, &tab_id, move |s| {
            let webview2 = surface::webview2_of(s);
            let url = s.webview.url().unwrap_or_default();
            let frame_tree = surface::debug_cdp(&webview2, "Page.getFrameTree", "{}");
            let doc = surface::debug_cdp(
                &webview2,
                "Runtime.evaluate",
                &serde_json::json!({
                    "expression": "JSON.stringify({ready: document.readyState, title: document.title, w: innerWidth, h: innerHeight})"
                })
                .to_string(),
            );
            let shot = surface::debug_cdp(
                &webview2,
                "Page.captureScreenshot",
                &serde_json::json!({"format": "jpeg", "quality": 60}).to_string(),
            );
            let summary = serde_json::json!({
                "url": url,
                "frameTree": frame_tree,
                "document": doc,
                "screenshotLen": shot.as_ref().map(|v| v.len()).unwrap_or(0),
            });
            crate::debug_log::append(
                "browser",
                "info",
                "",
                "debug state",
                Some(&summary.to_string()),
            );
            Ok(summary)
        })?
    }

    #[tauri::command(async)]
    pub fn browser_permission_respond(request_id: String, allow: bool) -> Result<bool, String> {
        Ok(resolve_permission(&request_id, allow))
    }

    #[tauri::command(async)]
    pub fn set_browser_tools_enabled(enabled: bool) -> Result<(), String> {
        TOOLS_ENABLED.store(enabled, Ordering::Relaxed);
        Ok(())
    }

    #[tauri::command(async)]
    pub fn set_session_approval(session_id: String, always_approve: bool) -> Result<(), String> {
        if let Ok(mut guard) = SESSION_ALWAYS_APPROVE.lock() {
            guard
                .get_or_insert_with(HashMap::new)
                .insert(session_id, always_approve);
        }
        Ok(())
    }
}

#[cfg(target_os = "windows")]
pub use imp::*;

/// Non-Windows: the commands exist (the frontend imports them unconditionally)
/// and answer "not here".
#[cfg(not(target_os = "windows"))]
mod imp {
    use serde_json::{json, Value};
    use tauri::AppHandle;

    fn unavailable<T>() -> Result<T, String> {
        Err("browser tabs are Windows-only in this build".to_string())
    }

    pub fn tools_enabled() -> bool {
        false
    }
    pub fn session_auto_approves(_session_id: &str) -> bool {
        false
    }
    pub fn request_permission(
        _app: &AppHandle,
        _session_id: &str,
        _tab_id: &str,
        _url: &str,
        _origin: &str,
        _summary: &str,
    ) -> bool {
        false
    }
    pub fn grant_required_note(_tab_id: &str) -> String {
        "browser tabs are unavailable on this platform".to_string()
    }

    #[tauri::command(async)]
    pub fn browser_open_tab(_app: AppHandle, _url: String) -> Result<Value, String> {
        unavailable()
    }
    #[tauri::command(async)]
    pub fn browser_close_tab(_app: AppHandle, _tab_id: String) -> Result<(), String> {
        unavailable()
    }
    #[tauri::command(async)]
    pub fn browser_navigate_tab(_app: AppHandle, _tab_id: String, _url: String) -> Result<(), String> {
        unavailable()
    }
    #[tauri::command(async)]
    pub fn browser_set_tab_bounds(
        _app: AppHandle,
        _tab_id: String,
        _x: f64,
        _y: f64,
        _width: f64,
        _height: f64,
    ) -> Result<(), String> {
        unavailable()
    }
    #[tauri::command(async)]
    pub fn browser_set_tab_visible(_app: AppHandle, _tab_id: String, _visible: bool) -> Result<(), String> {
        unavailable()
    }
    #[tauri::command(async)]
    pub fn browser_list_tabs(_app: AppHandle) -> Result<Vec<Value>, String> {
        Ok(Vec::new())
    }
    #[tauri::command(async)]
    pub fn browser_share_tab(_app: AppHandle, _tab_id: String, _level: String) -> Result<(), String> {
        unavailable()
    }
    #[tauri::command(async)]
    pub fn browser_permission_respond(_request_id: String, _allow: bool) -> Result<bool, String> {
        Ok(false)
    }
    #[tauri::command(async)]
    pub fn set_browser_tools_enabled(_enabled: bool) -> Result<(), String> {
        Ok(())
    }
    #[tauri::command(async)]
    pub fn set_session_approval(_session_id: String, _always_approve: bool) -> Result<(), String> {
        Ok(())
    }

    #[allow(dead_code)]
    fn _keep_json() -> Value {
        json!({})
    }
}

#[cfg(not(target_os = "windows"))]
pub use imp::*;
