//! Built-in browser tabs (batch 2 of the browser/computer-use plan).
//!
//! A tab is a wry child webview of the main window (see `surface.rs`). This
//! module owns everything above it: tauri commands for the frontend, the
//! grant mutations the toolbar performs, and the in-conversation permission
//! flow Control-level actions go through. The agent-facing tool surface lives
//! in `crate::tools_mcp` and calls the same functions — MCP reads walk the
//! **same** grant checks as the toolbar, never a second set.

pub mod agent;
pub mod pick;
pub mod probe;
pub mod service;
#[cfg(target_os = "windows")]
pub mod surface;

/// How a newly opened tab should enter the workspace. This is an OPEN-time
/// instruction, not a persistent visibility state: `workspace` asks the UI to
/// show the browser beside the current Agent conversation once; later tab
/// switches behave normally.
pub fn normalize_open_display(raw: Option<&str>) -> Result<&'static str, String> {
    let display = raw.unwrap_or("background").trim().to_ascii_lowercase();
    match display.as_str() {
        "background" => Ok("background"),
        "workspace" => Ok("workspace"),
        other => Err(format!(
            "browser display must be background or workspace, got {other:?}"
        )),
    }
}

#[cfg(target_os = "windows")]
mod imp {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::mpsc;
    use std::sync::Mutex;
    use std::time::Duration;

    use serde_json::{json, Value};
    use tauri::{AppHandle, Emitter};

    use super::{agent, service, surface};
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
    struct PendingHandoff {
        tab_id: String,
        tx: mpsc::Sender<String>,
    }

    static PENDING_HANDOFFS: Mutex<Option<HashMap<String, PendingHandoff>>> = Mutex::new(None);
    static HANDOFF_SEQ: AtomicU64 = AtomicU64::new(1);

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

    struct PendingPermission {
        tab_id: String,
        tx: mpsc::Sender<bool>,
    }

    static PENDING_PERMISSIONS: Mutex<Option<HashMap<String, PendingPermission>>> =
        Mutex::new(None);

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
            guard.get_or_insert_with(HashMap::new).insert(
                request_id.clone(),
                PendingPermission {
                    tab_id: tab_id.to_string(),
                    tx,
                },
            );
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
        let _ = set_tab_status(app, tab_id, "waiting_user");
        let _ = app.emit("marionette-browser-permission", &request);
        let answer = rx.recv_timeout(PERMISSION_TIMEOUT).unwrap_or(false);
        if let Ok(mut guard) = PENDING_PERMISSIONS.lock() {
            if let Some(map) = guard.as_mut() {
                map.remove(&request_id);
            }
        }
        let _ = set_tab_status(app, tab_id, "idle");
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
            Some(pending) => pending.tx.send(allow).is_ok(),
            None => false,
        }
    }

    pub fn cancel_permissions_for_tab(tab_id: &str) -> usize {
        let pending = PENDING_PERMISSIONS
            .lock()
            .ok()
            .and_then(|mut guard| {
                let map = guard.as_mut()?;
                let ids: Vec<String> = map
                    .iter()
                    .filter(|(_, request)| request.tab_id == tab_id)
                    .map(|(id, _)| id.clone())
                    .collect();
                Some(
                    ids.into_iter()
                        .filter_map(|id| map.remove(&id))
                        .collect::<Vec<_>>(),
                )
            })
            .unwrap_or_default();
        pending
            .into_iter()
            .filter(|request| request.tx.send(false).is_ok())
            .count()
    }

    pub fn set_tab_status(app: &AppHandle, tab_id: &str, status: &str) -> Result<(), String> {
        let status = status.to_string();
        surface::with_surface(app, tab_id, move |s| {
            s.status.replace(status);
            s.last_used_at_ms.set(surface::now_ms_pub());
            surface::emit_tabs(&s.app);
        })
    }

    pub fn request_handoff(
        app: &AppHandle,
        session_id: &str,
        tab_id: &str,
        title: &str,
        message: &str,
        timeout_secs: u64,
    ) -> Result<Value, String> {
        let url = tab_url(app, tab_id)?;
        let origin = tab_origin(app, tab_id)?;
        let seq = HANDOFF_SEQ.fetch_add(1, Ordering::Relaxed);
        let request_id = format!("handoff-{seq}");
        let (tx, rx) = mpsc::channel();
        if let Ok(mut guard) = PENDING_HANDOFFS.lock() {
            guard
                .get_or_insert_with(HashMap::new)
                .insert(
                    request_id.clone(),
                    PendingHandoff {
                        tab_id: tab_id.to_string(),
                        tx,
                    },
                );
        }
        set_tab_status(app, tab_id, "waiting_user")?;
        let timeout_secs = timeout_secs.clamp(15, 300);
        let expires_at_ms = surface::now_ms_pub() + timeout_secs * 1000;
        let title = if title.trim().is_empty() {
            "Complete this step".to_string()
        } else {
            title.trim().chars().take(120).collect()
        };
        let message = message.trim().chars().take(2_000).collect::<String>();
        let _ = app.emit(
            "marionette-browser-handoff",
            json!({
                "requestId": request_id,
                "sessionId": session_id,
                "tabId": tab_id,
                "title": title,
                "message": message,
                "url": url,
                "origin": origin,
                "expiresAtMs": expires_at_ms,
            }),
        );
        let outcome = rx
            .recv_timeout(Duration::from_secs(timeout_secs))
            .unwrap_or_else(|_| "timeout".to_string());
        if let Ok(mut guard) = PENDING_HANDOFFS.lock() {
            if let Some(map) = guard.as_mut() {
                map.remove(&request_id);
            }
        }
        // The user may close the tab while the handoff is pending; the
        // cancellation still returns a denial, so a vanished tab is fine.
        let _ = set_tab_status(app, tab_id, "idle");
        Ok(json!({
            "requestId": request_id,
            "tabId": tab_id,
            "outcome": outcome,
            "url": url,
            "origin": origin,
        }))
    }

    pub fn resolve_handoff(request_id: &str, outcome: &str) -> bool {
        let outcome = if outcome == "continue" { "continue" } else { "denied" };
        let sender = PENDING_HANDOFFS
            .lock()
            .ok()
            .and_then(|mut guard| guard.as_mut().and_then(|map| map.remove(request_id)));
        match sender {
            Some(pending) => pending.tx.send(outcome.to_string()).is_ok(),
            None => false,
        }
    }

    /// A closed tab cannot receive the human's next click. Release every
    /// blocked handoff on it now, as a denial, instead of leaving the Agent
    /// waiting for the full timeout.
    pub fn cancel_handoffs_for_tab(tab_id: &str) -> usize {
        let pending = PENDING_HANDOFFS
            .lock()
            .ok()
            .and_then(|mut guard| {
                let map = guard.as_mut()?;
                let ids: Vec<String> = map
                    .iter()
                    .filter(|(_, request)| request.tab_id == tab_id)
                    .map(|(id, _)| id.clone())
                    .collect();
                Some(
                    ids.into_iter()
                        .filter_map(|id| map.remove(&id))
                        .collect::<Vec<_>>(),
                )
            })
            .unwrap_or_default();
        let mut delivered = 0;
        for request in pending {
            if request.tx.send("denied".to_string()).is_ok() {
                delivered += 1;
            }
        }
        delivered
    }

    // -----------------------------------------------------------------------
    // Tab operations (thread-safe wrappers over the main-thread registry)
    // -----------------------------------------------------------------------

    /// Open a tab. `opened_by_agent` decides the starting grant: agent tabs
    /// read on arrival (the agent chose the page); user tabs share nothing
    /// until the toolbar says so.
    pub fn open_tab(
        app: &AppHandle,
        url: &str,
        opened_by_agent: bool,
        display: &str,
        owner_session_id: Option<&str>,
    ) -> Result<String, String> {
        let (tx, rx) = mpsc::channel();
        let url = url.to_string();
        let display = display.to_string();
        let owner_session_id = owner_session_id.map(str::to_string);
        let app_for_job = app.clone();
        app.run_on_main_thread(move || {
            let result = surface::create_surface(
                &app_for_job,
                &url,
                opened_by_agent,
                &display,
                owner_session_id.as_deref(),
            );
            let _ = tx.send(result);
        })
        .map_err(|e| format!("main thread hop: {e}"))?;
        rx.recv().map_err(|_| "main thread did not answer".to_string())?
    }

    pub fn close_tab(app: &AppHandle, tab_id: &str) -> Result<(), String> {
        cancel_permissions_for_tab(tab_id);
        cancel_handoffs_for_tab(tab_id);
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

    /// Ownership is a privacy boundary: always-approve may skip the Control
    /// card for tabs the agent opened, but never for a user tab.
    pub fn tab_opened_by_agent(app: &AppHandle, tab_id: &str) -> Result<bool, String> {
        surface::with_surface(app, tab_id, |s| s.opened_by_agent)
    }

    /// Run one agent-side tool call and publish the tab's live activity state.
    /// The closure itself may take a CDP lease or wait on a permission card;
    /// the surrounding status is what the browser UI reflects.
    pub fn tool_call(
        app: &AppHandle,
        tab_id: &str,
        run: impl FnOnce(&Surface) -> Result<Value, String> + Send + 'static,
    ) -> Result<Value, String> {
        surface::with_surface(app, tab_id, move |s| {
            let running = s.busy_count.get().saturating_add(1);
            s.busy_count.set(running);
            s.status.replace("running".to_string());
            s.last_used_at_ms.set(surface::now_ms_pub());
            surface::emit_tabs(&s.app);
            let result = run(s);
            let remaining = s.busy_count.get().saturating_sub(1);
            s.busy_count.set(remaining);
            if remaining == 0 {
                s.status.replace(
                    if result.is_ok() {
                        "idle".to_string()
                    } else {
                        "failed".to_string()
                    },
                );
            }
            surface::emit_tabs(&s.app);
            result
        })?
    }

    /// Close every tab an agent session owns. User-opened tabs have no owner
    /// and are never touched by session deletion.
    pub fn close_tabs_for_session(app: &AppHandle, session_id: &str) -> Result<usize, String> {
        let (tx, rx) = mpsc::channel();
        let session_id = session_id.to_string();
        let app_for_job = app.clone();
        app.run_on_main_thread(move || {
            let ids: Vec<String> = surface::tabs_snapshot()
                .into_iter()
                .filter(|tab| tab.owner_session_id.as_deref() == Some(session_id.as_str()))
                .map(|tab| tab.id)
                .collect();
            let mut closed = 0usize;
            for id in ids {
                if surface::close_surface(&app_for_job, &id).is_ok() {
                    closed += 1;
                }
            }
            let _ = tx.send(closed);
        })
        .map_err(|e| format!("main thread hop: {e}"))?;
        rx.recv().map_err(|_| "main thread did not answer".to_string())
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
        tool_call(app, tab_id, run)
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
    pub fn browser_open_tab(
        app: AppHandle,
        url: String,
        display: Option<String>,
    ) -> Result<Value, String> {
        let display = super::normalize_open_display(display.as_deref())?;
        open_tab(&app, &url, false, display, None).map(|id| json!({ "tabId": id }))
    }

    #[tauri::command(async)]
    pub fn browser_close_tab(app: AppHandle, tab_id: String) -> Result<(), String> {
        close_tab(&app, &tab_id)
    }

    #[tauri::command(async)]
    pub fn browser_close_tabs_for_session(
        app: AppHandle,
        session_id: String,
    ) -> Result<usize, String> {
        close_tabs_for_session(&app, &session_id)
    }

    #[tauri::command(async)]
    pub fn browser_probe_loopback_url(url: String) -> Result<bool, String> {
        service::probe_loopback_url(&url)
    }

    #[tauri::command(async)]
    pub fn browser_navigate_tab(app: AppHandle, tab_id: String, url: String) -> Result<(), String> {
        let url = surface::normalize_url(&url)?;
        surface::with_surface(&app, &tab_id, move |s| {
            s.webview.load_url(&url).map_err(|e| format!("navigate: {e}"))?;
            surface::apply_document_navigation(s, url.clone(), None);
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

    // ── element picker (annotate mode) ─────────────────────────────────────
    // UI commands for the user driving their own browser — deliberately no
    // grant checks here: grants gate agent tool calls, and `tool_read` /
    // `control_gate` stay the one place that does it. Nothing in this batch
    // adds an agent tool; the annotation prompt tells the agent to use the
    // existing browser_snapshot / browser_click / browser_set_value.

    /// Toggle the picker, or switch its mode while running.
    #[tauri::command(async)]
    pub fn browser_pick_set(
        app: AppHandle,
        tab_id: String,
        active: bool,
        mode: String,
    ) -> Result<Value, String> {
        surface::with_surface(&app, &tab_id, move |s| {
            surface::pick_set(s, active, &mode)
        })?
    }

    /// Forget the current annotation batch after send/clear/abandon.
    #[tauri::command(async)]
    pub fn browser_pick_clear(app: AppHandle, tab_id: String) -> Result<Value, String> {
        surface::with_surface(&app, &tab_id, move |s| surface::pick_clear(s))?
    }

    /// Crop the viewport screenshot down to one pick, highlight excluded.
    #[tauri::command(async)]
    pub fn browser_pick_finalize(app: AppHandle, tab_id: String, pick_id: String) -> Result<Value, String> {
        surface::with_surface(&app, &tab_id, move |s| {
            surface::pick_finalize(s, &pick_id)
        })?
    }

    /// Map the retained picks to aria refs as of a fresh snapshot.
    #[tauri::command(async)]
    pub fn browser_pick_resolve(app: AppHandle, tab_id: String) -> Result<Value, String> {
        surface::with_surface(&app, &tab_id, move |s| surface::pick_resolve(s))?
    }


    /// Diagnostic (E2E harness): the CDP-level truth about one tab — what the
    /// renderer loaded vs what the window composited. Not for the product UI.
    #[tauri::command(async)]
    pub fn browser_debug_state(app: AppHandle, tab_id: String) -> Result<Value, String> {
        crate::debug_log::append("browser", "info", "", "debug state invoked", Some(&tab_id));
        surface::with_surface(&app, &tab_id, move |s| {
            let _lease = surface::cdp_lease()?;
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

    /// Diagnostic (E2E harness): run the real snapshot path over CDP so the
    /// aria engine's install-then-call sequence can be exercised without an
    /// agent in the loop. Not for the product UI.
    #[tauri::command(async)]
    pub fn browser_debug_snapshot(app: AppHandle, tab_id: String) -> Result<Value, String> {
        surface::with_surface(&app, &tab_id, move |s| {
            let request = agent::SnapshotRequest { max_chars: 4000 };
            let started = std::time::Instant::now();
            let outcome = surface::snapshot(s, &request);
            let elapsed_ms = started.elapsed().as_millis() as u64;
            match outcome {
                Ok(value) => {
                    crate::debug_log::append(
                        "browser",
                        "info",
                        "",
                        "debug snapshot ok",
                        Some(&format!("{elapsed_ms}ms")),
                    );
                    Ok(value)
                }
                Err(error) => {
                    crate::debug_log::append(
                        "browser",
                        "warn",
                        "",
                        "debug snapshot failed",
                        Some(&format!("{elapsed_ms}ms: {error}")),
                    );
                    Err(error)
                }
            }
        })?
    }

    /// Diagnostic (E2E harness): the whole probe flow against a real page —
    /// snapshot for a ref, read state, write the control, read state again —
    /// so the change counter can be shown to move. Not for the product UI.
    #[tauri::command(async)]
    pub fn browser_debug_probe(app: AppHandle, tab_id: String) -> Result<Value, String> {
        surface::with_surface(&app, &tab_id, move |s| {
            let before = surface::probe(s, &super::probe::state_call())?;
            let snapshot = surface::snapshot(s, &agent::SnapshotRequest { max_chars: 20000 })?;
            let tree = snapshot["tree"].as_str().unwrap_or("").to_string();
            let generation = snapshot["generation"].as_str().unwrap_or("").to_string();
            // The first checkbox the tree names is the one to write.
            let target = tree
                .lines()
                .find(|line| line.contains("checkbox"))
                .and_then(|line| {
                    let start = line.find("[ref=")? + 5;
                    let rest = &line[start..];
                    let end = rest.find(']')?;
                    Some(rest[..end].to_string())
                });
            let write = match &target {
                Some(target) => Some(surface::set_control_value(
                    s,
                    &generation,
                    target,
                    &serde_json::json!(true),
                )?),
                None => None,
            };
            let after = surface::probe(s, &super::probe::state_call())?;
            // A second snapshot is the honest check on the write: if the page's
            // own change listener ran, it rewrote #out, and the tree says so.
            // A write that only assigned `checked` leaves #out stale.
            let echo = surface::snapshot(s, &agent::SnapshotRequest { max_chars: 20000 })?;
            let echo_tree = echo["tree"].as_str().unwrap_or("").to_string();
            let summary = serde_json::json!({
                "tab": s.id,
                "seqBefore": before["seq"],
                "seqAfter": after["seq"],
                "title": before["title"],
                "url": before["url"],
                "target": target,
                "write": write,
                "checkboxLines": tree.lines().filter(|l| l.contains("checkbox")).count(),
                "listenerRan": echo_tree.contains("agree=true"),
                "echoLine": echo_tree
                    .lines()
                    .find(|line| line.contains("agree=") || line.contains("idle") || line.contains("tick"))
                    .map(str::to_string),
            });
            crate::debug_log::append(
                "browser",
                "info",
                "",
                "debug probe flow",
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
    pub fn browser_handoff_respond(request_id: String, outcome: String) -> Result<bool, String> {
        Ok(resolve_handoff(&request_id, &outcome))
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

#[cfg(test)]
mod display_tests {
    use super::normalize_open_display;

    #[test]
    fn browser_open_display_is_strict_and_defaults_to_background() {
        assert_eq!(normalize_open_display(None).unwrap(), "background");
        assert_eq!(
            normalize_open_display(Some("WORKSPACE")).unwrap(),
            "workspace"
        );
        assert!(normalize_open_display(Some("foreground")).is_err());
    }
}

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
    pub fn tab_opened_by_agent(_app: &AppHandle, _tab_id: &str) -> Result<bool, String> {
        unavailable()
    }
    pub fn close_tabs_for_session(_app: &AppHandle, _session_id: &str) -> Result<usize, String> {
        Ok(0)
    }

    #[tauri::command(async)]
    pub fn browser_open_tab(
        _app: AppHandle,
        _url: String,
        _display: Option<String>,
    ) -> Result<Value, String> {
        unavailable()
    }
    #[tauri::command(async)]
    pub fn browser_close_tab(_app: AppHandle, _tab_id: String) -> Result<(), String> {
        unavailable()
    }
    #[tauri::command(async)]
    pub fn browser_close_tabs_for_session(
        _app: AppHandle,
        _session_id: String,
    ) -> Result<usize, String> {
        Ok(0)
    }
    #[tauri::command(async)]
    pub fn browser_probe_loopback_url(url: String) -> Result<bool, String> {
        super::service::probe_loopback_url(&url)
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
    pub fn browser_pick_set(
        _app: AppHandle,
        _tab_id: String,
        _active: bool,
        _mode: String,
    ) -> Result<Value, String> {
        unavailable()
    }
    #[tauri::command(async)]
    pub fn browser_pick_clear(_app: AppHandle, _tab_id: String) -> Result<Value, String> {
        unavailable()
    }
    #[tauri::command(async)]
    pub fn browser_pick_finalize(
        _app: AppHandle,
        _tab_id: String,
        _pick_id: String,
    ) -> Result<Value, String> {
        unavailable()
    }
    #[tauri::command(async)]
    pub fn browser_pick_resolve(_app: AppHandle, _tab_id: String) -> Result<Value, String> {
        unavailable()
    }
    #[tauri::command(async)]
    pub fn browser_permission_respond(_request_id: String, _allow: bool) -> Result<bool, String> {
        Ok(false)
    }
    #[tauri::command(async)]
    pub fn browser_handoff_respond(
        _request_id: String,
        _outcome: String,
    ) -> Result<bool, String> {
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
