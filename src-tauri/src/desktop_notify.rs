//! Native desktop attention that survives a suspended WebView2.
//!
//! The UI's HTMLAudio path is fine while a window is awake, but Marionette
//! deliberately suspends the main WebView when it is minimized or hidden to
//! the tray. ACP keeps running on Rust threads, so reply/permission/question
//! notifications must be raised here instead of waiting for the frontend to
//! resume. The WAV files are the same assets the web UI ships.

use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::window::{ProgressBarState, ProgressBarStatus};
use tauri::{AppHandle, Manager, UserAttentionType};

const BASE_TITLE: &str = "Marionette";
const DEDUPE: Duration = Duration::from_secs(1);
/// A focus change re-enters the Win32 paint loop. Never touch native window
/// chrome until the focus frame has fully settled (tao#1180 / tauri#14088).
const CLEAR_AFTER_FOCUS: Duration = Duration::from_millis(400);

static ENABLED: AtomicBool = AtomicBool::new(true);
static CLEAR_GENERATION: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Kind {
    Reply,
    Permission,
    Question,
    Plan,
    Error,
}

struct PendingAttention {
    window_label: String,
    previous_title: Option<String>,
    alert_title: String,
}

fn last_raised() -> &'static Mutex<HashMap<Kind, Instant>> {
    static VALUE: OnceLock<Mutex<HashMap<Kind, Instant>>> = OnceLock::new();
    VALUE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn pending_attention() -> &'static Mutex<Option<PendingAttention>> {
    static VALUE: OnceLock<Mutex<Option<PendingAttention>>> = OnceLock::new();
    VALUE.get_or_init(|| Mutex::new(None))
}

fn probe_turns() -> &'static Mutex<HashSet<String>> {
    static VALUE: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    VALUE.get_or_init(|| Mutex::new(HashSet::new()))
}

/// 本轮是否出现过助手正文（agent_message_chunk）。没有 = 「agent 静默停了」：
/// 空回合或只跑工具就收尾，屏幕上没有任何可读内容，所以聚焦时也要响。
fn replied_turns() -> &'static Mutex<HashSet<String>> {
    static VALUE: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    VALUE.get_or_init(|| Mutex::new(HashSet::new()))
}

/// 会话标题（通知正文里的会话名，旧前端实现就带）。`try_lock`：本函数跑在
/// ACP 派发线程上，storage 锁被占就放弃标题，绝不为一个通知等锁。
fn session_label(app: &AppHandle, session_id: &str) -> Option<String> {
    let state = app.try_state::<crate::AppState>()?;
    let storage = state.storage.try_lock().ok()?;
    let session = storage.find_session(session_id).ok().flatten()?;
    let label = compact_detail(&session.label);
    (!label.is_empty()).then_some(label)
}

fn with_label(label: Option<&str>, detail: &str) -> String {
    match label {
        Some(l) if detail.is_empty() => l.to_string(),
        Some(l) => format!("{l} · {detail}"),
        None => detail.to_string(),
    }
}

fn silent_turns() -> &'static Mutex<HashSet<String>> {
    static VALUE: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    VALUE.get_or_init(|| Mutex::new(HashSet::new()))
}

pub fn set_enabled(app: &AppHandle, enabled: bool) {
    ENABLED.store(enabled, Ordering::Release);
    if !enabled {
        clear_attention(app);
    }
}

pub fn mark_probe_turn(session_id: &str) {
    if let Ok(mut turns) = probe_turns().lock() {
        turns.insert(session_id.to_string());
    }
}

pub fn mark_silent_turn(session_id: &str) {
    if let Ok(mut turns) = silent_turns().lock() {
        turns.insert(session_id.to_string());
    }
}

pub fn clear_turn_marks(session_id: &str) {
    if let Ok(mut turns) = probe_turns().lock() {
        turns.remove(session_id);
    }
    if let Ok(mut turns) = silent_turns().lock() {
        turns.remove(session_id);
    }
    if let Ok(mut turns) = replied_turns().lock() {
        turns.remove(session_id);
    }
}

fn consume_suppressed_turn(session_id: &str) -> bool {
    let probe = probe_turns()
        .lock()
        .map(|mut turns| turns.remove(session_id))
        .unwrap_or(false);
    let silent = silent_turns()
        .lock()
        .map(|mut turns| turns.remove(session_id))
        .unwrap_or(false);
    probe || silent
}

/// Observe the single ACP event fan-out. This runs on the backend dispatcher,
/// independent of whether the target WebView is currently suspended.
pub fn observe_acp_event(
    app: &AppHandle,
    session_id: &str,
    target_window: &str,
    kind: &str,
    method: Option<&str>,
    data: &Value,
    delegate_child: bool,
) {
    let gate_kind = match method {
        Some("turn/complete") if kind == "error" => Some("task.failed"),
        Some("turn/complete") => Some("completed"),
        Some("permission/prompt") => Some("permission"),
        Some("question/prompt") => Some("question"),
        Some("plan/approval") => Some("plan"),
        _ => None,
    };
    if gate_kind.is_some_and(|gate| {
        !crate::chief::staff_commands::notification_would_allow(session_id, gate)
    }) {
        clear_turn_marks(session_id);
        return;
    }
    let label = if matches!(
        method,
        Some("turn/complete") | Some("permission/prompt") | Some("question/prompt") | Some("plan/approval")
    ) {
        session_label(app, session_id)
    } else {
        None
    };
    let label = label.as_deref();
    match method {
        Some("session/update") => {
            if data.pointer("/params/update/sessionUpdate").and_then(Value::as_str)
                == Some("agent_message_chunk")
            {
                if let Ok(mut turns) = replied_turns().lock() {
                    if !turns.contains(session_id) {
                        turns.insert(session_id.to_string());
                    }
                }
            }
        }
        Some("turn/complete") => {
            let replied = replied_turns()
                .lock()
                .map(|mut turns| turns.remove(session_id))
                .unwrap_or(false);
            let suppressed = consume_suppressed_turn(session_id);
            let stop_reason = data
                .get("stopReason")
                .and_then(Value::as_str)
                .unwrap_or("end_turn");
            if suppressed || delegate_child || stop_reason == "cancelled" {
                return;
            }
            let failed = kind == "error" || matches!(stop_reason, "error" | "refusal");
            if failed {
                let detail = error_detail(data).unwrap_or_else(|| "Agent error".to_string());
                raise(app, target_window, Kind::Error, &with_label(label, &detail), false);
            } else if !replied {
                raise(app, target_window, Kind::Reply, &with_label(label, "agent stopped"), true);
            } else {
                raise(app, target_window, Kind::Reply, &with_label(label, "Reply ready"), true);
            }
        }
        Some("permission/prompt") => {
            let is_plan = data
                .get("planMarkdown")
                .map(|value| !value.is_null())
                .unwrap_or(false);
            if is_plan {
                raise(
                    app,
                    target_window,
                    Kind::Plan,
                    &with_label(label, "Plan ready for review"),
                    false,
                );
            } else {
                let detail = string_field(data, "detail")
                    .or_else(|| string_field(data, "title"))
                    .unwrap_or_else(|| "Permission required".to_string());
                raise(app, target_window, Kind::Permission, &with_label(label, &detail), false);
            }
        }
        Some("question/prompt") => {
            let detail = data
                .pointer("/questions/0/question")
                .and_then(Value::as_str)
                .map(compact_detail)
                .or_else(|| string_field(data, "title"))
                .unwrap_or_else(|| "Agent has a question".to_string());
            raise(app, target_window, Kind::Question, &with_label(label, &detail), false);
        }
        Some("plan/approval") => {
            raise(
                app,
                target_window,
                Kind::Plan,
                &with_label(label, "Plan ready for review"),
                false,
            );
        }
        // A mid-turn process failure already emits `turn/complete(error)`, so
        // this is only bookkeeping. Idle CLI exits must not produce an alert.
        Some("process/ended") | Some("process/stopped") => clear_turn_marks(session_id),
        _ => {}
    }
}

/// Is any Marionette window the OS foreground window?
///
/// `window.is_focused()` alone is not trustworthy on Windows: after the
/// webview is suspended and resumed (minimize → TrySuspend → restore),
/// tao's host-focus flag can stay `false` for minutes while the user sits in
/// the window (2026-11-20). A notification raised on that stale signal paints
/// a yellow taskbar bar and the focus clear never runs, so the bar sticks.
/// The Win32 foreground window is the same truth the page's `document.hasFocus()`
/// sees, minus tao's bookkeeping.
fn app_in_foreground(app: &AppHandle) -> bool {
    app.webview_windows().values().any(|window| {
        if window.is_focused().unwrap_or(false) {
            return true;
        }
        window_is_foreground(window)
    })
}

#[cfg(windows)]
fn window_is_foreground(window: &tauri::WebviewWindow) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

    let Ok(hwnd) = window.hwnd() else {
        return false;
    };
    let foreground = unsafe { GetForegroundWindow() };
    !foreground.is_null() && foreground as isize == hwnd.0 as isize
}

#[cfg(not(windows))]
fn window_is_foreground(_window: &tauri::WebviewWindow) -> bool {
    false
}

fn raise(
    app: &AppHandle,
    preferred_window: &str,
    kind: Kind,
    detail: &str,
    audible_when_focused: bool,
) {
    if !ENABLED.load(Ordering::Acquire) || duplicate(kind) {
        return;
    }

    if app_in_foreground(app) {
        if audible_when_focused {
            play_sound(kind);
        }
        return;
    }

    // Invalidate any delayed clear started by an earlier focus transition.
    CLEAR_GENERATION.fetch_add(1, Ordering::AcqRel);
    play_sound(kind);

    let Some(window) = attention_window(app, preferred_window) else {
        return;
    };
    let window_label = window.label().to_string();
    let title = format!("{} {detail} · {BASE_TITLE}", title_prefix(kind));
    let pending_previous = pending_attention().lock().ok().and_then(|pending| {
        pending
            .as_ref()
            .filter(|current| current.window_label == window_label)
            .map(|current| current.previous_title.clone())
    });
    let previous_title = pending_previous.unwrap_or_else(|| window.title().ok());
    let _ = window.set_title(&title);
    let _ = window.request_user_attention(Some(UserAttentionType::Critical));
    let _ = window.set_progress_bar(ProgressBarState {
        status: Some(if kind == Kind::Error {
            ProgressBarStatus::Error
        } else {
            ProgressBarStatus::Paused
        }),
        progress: Some(100),
    });

    if let Ok(mut pending) = pending_attention().lock() {
        *pending = Some(PendingAttention {
            window_label: window_label.clone(),
            previous_title,
            alert_title: title,
        });
    }
    crate::debug_log::append(
        "notify",
        "info",
        "",
        "attention raised",
        Some(&format!("kind={kind:?} window={window_label}")),
    );
    ensure_chrome_watch(app);
}

fn duplicate(kind: Kind) -> bool {
    let now = Instant::now();
    let Ok(mut raised) = last_raised().lock() else {
        return false;
    };
    if raised
        .get(&kind)
        .is_some_and(|previous| now.duration_since(*previous) < DEDUPE)
    {
        return true;
    }
    raised.insert(kind, now);
    false
}

fn attention_window(app: &AppHandle, preferred_window: &str) -> Option<tauri::WebviewWindow> {
    if let Some(window) = app.get_webview_window(preferred_window) {
        if window.is_visible().unwrap_or(false) {
            return Some(window);
        }
    }
    if let Some(main) = app.get_webview_window("main") {
        if main.is_visible().unwrap_or(false) || app.webview_windows().len() == 1 {
            return Some(main);
        }
    }
    app.webview_windows()
        .into_values()
        .find(|window| window.is_visible().unwrap_or(false))
        .or_else(|| app.get_webview_window("main"))
}

/// Called from the native focus event. The delay is the crash guard noted in
/// `desktopNotify.ts`; the generation check makes a newer alert win the race.
pub fn schedule_clear_on_focus(app: &AppHandle) {
    if pending_attention()
        .lock()
        .map(|pending| pending.is_none())
        .unwrap_or(true)
    {
        return;
    }
    let generation = CLEAR_GENERATION.fetch_add(1, Ordering::AcqRel) + 1;
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(CLEAR_AFTER_FOCUS);
        if generation != CLEAR_GENERATION.load(Ordering::Acquire) {
            return;
        }
        if app_in_foreground(&app) {
            clear_attention(&app);
        }
    });
}

/// Backstop for the same reason the web UI polls: a raise can land after the
/// only focus event already fired, and tao can miss focus transitions across
/// minimize/restore. While Rust owns chrome, re-check the real foreground
/// window every few seconds and drop the bar once the user is back.
static CHROME_WATCHING: AtomicBool = AtomicBool::new(false);
const CHROME_POLL: Duration = Duration::from_secs(2);

fn ensure_chrome_watch(app: &AppHandle) {
    if CHROME_WATCHING.swap(true, Ordering::AcqRel) {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(CHROME_POLL);
        let pending = pending_attention()
            .lock()
            .map(|pending| pending.is_some())
            .unwrap_or(false);
        if !pending {
            CHROME_WATCHING.store(false, Ordering::Release);
            // A raise may have slipped in between the check and the store; if
            // so it already saw `true` and did not spawn, so take the watch on.
            let still_pending = pending_attention()
                .lock()
                .map(|pending| pending.is_some())
                .unwrap_or(false);
            if !still_pending || CHROME_WATCHING.swap(true, Ordering::AcqRel) {
                return;
            }
            continue;
        }
        if !app_in_foreground(&app) {
            continue;
        }
        // Same paint-settle guard as the focus handler: never touch native
        // chrome inside the focus transition.
        std::thread::sleep(CLEAR_AFTER_FOCUS);
        if app_in_foreground(&app) {
            clear_attention(&app);
        }
    });
}

fn clear_attention(app: &AppHandle) {
    let pending = pending_attention()
        .lock()
        .ok()
        .and_then(|mut slot| slot.take());
    let Some(pending) = pending else {
        return;
    };
    let Some(window) = app.get_webview_window(&pending.window_label) else {
        return;
    };
    if window.title().ok().as_deref() == Some(pending.alert_title.as_str()) {
        let _ = window.set_title(pending.previous_title.as_deref().unwrap_or(BASE_TITLE));
    }
    // Never call `request_user_attention(None)` here: on Windows/tao it can
    // re-enter the paint pump during focus and abort the process.
    let _ = window.set_progress_bar(ProgressBarState {
        status: None,
        progress: None,
    });
    crate::debug_log::append(
        "notify",
        "info",
        "",
        "attention cleared",
        Some(&format!("window={}", pending.window_label)),
    );
}

fn title_prefix(kind: Kind) -> &'static str {
    match kind {
        Kind::Error => "⚠",
        Kind::Permission | Kind::Question | Kind::Plan => "!",
        Kind::Reply => "●",
    }
}

fn string_field(data: &Value, field: &str) -> Option<String> {
    data.get(field)
        .and_then(Value::as_str)
        .map(compact_detail)
        .filter(|value| !value.is_empty())
}

fn error_detail(data: &Value) -> Option<String> {
    string_field(data, "message")
        .or_else(|| {
            data.pointer("/error/message")
                .and_then(Value::as_str)
                .map(compact_detail)
        })
        .filter(|value| !value.is_empty())
}

fn compact_detail(value: &str) -> String {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = normalized.chars();
    let mut short: String = chars.by_ref().take(96).collect();
    if chars.next().is_some() {
        short.push('…');
    }
    short
}

#[cfg(windows)]
fn play_sound(kind: Kind) {
    use windows_sys::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};

    let wav: &[u8] = match kind {
        Kind::Error => include_bytes!("../../src/assets/notifications/error.wav"),
        Kind::Permission | Kind::Plan => {
            include_bytes!("../../src/assets/notifications/permission.wav")
        }
        Kind::Question => include_bytes!("../../src/assets/notifications/question.wav"),
        Kind::Reply => include_bytes!("../../src/assets/notifications/complete.wav"),
    };
    // The byte slices are static for the process lifetime, which satisfies
    // PlaySoundW's asynchronous SND_MEMORY lifetime requirement.
    unsafe {
        PlaySoundW(
            wav.as_ptr().cast(),
            std::ptr::null_mut(),
            SND_ASYNC | SND_MEMORY | SND_NODEFAULT,
        );
    }
}

#[cfg(not(windows))]
fn play_sound(_kind: Kind) {}

#[tauri::command(async)]
pub fn desktop_notify_set_enabled(app: AppHandle, enabled: bool) {
    set_enabled(&app, enabled);
}

/// Frontend focus knowledge clears Rust-owned chrome too.
///
/// The page's `document.hasFocus()` is the most reliable "user is here"
/// signal; the native focus event can be missed across suspend/resume. This
/// command is a no-op when Rust owns nothing.
#[tauri::command(async)]
pub fn desktop_notify_clear(app: AppHandle) {
    clear_attention(&app);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_detail_collapses_and_caps_text() {
        let text = format!("first\n second {}", "x".repeat(120));
        let short = compact_detail(&text);
        assert!(short.starts_with("first second "));
        assert!(short.ends_with('…'));
        assert_eq!(short.chars().count(), 97);
    }

    #[test]
    fn probe_and_silent_marks_are_consumed_once() {
        let session = "desktop-notify-test";
        clear_turn_marks(session);
        mark_probe_turn(session);
        mark_silent_turn(session);
        assert!(consume_suppressed_turn(session));
        assert!(!consume_suppressed_turn(session));
    }
}
