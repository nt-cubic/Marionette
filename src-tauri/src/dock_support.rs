//! Edge-dock support (docs/plan-edge-dock.md).
//!
//! Stage 0 — background residency: while `background_mode` is on, closing the
//! main window hides it instead of exiting; the engine, all sessions and the
//! Telegram bridge keep running. The only legitimate exit remains
//! main.rs `shutdown_and_exit` (tao 0.35 paint assert, tao#1180) — the
//! frontend's 退出 button reaches it through `quit_app` here.
//!
//! Stage 1 will add `monitors_work_areas` + `fullscreen_active` to this module.

use std::sync::atomic::{AtomicBool, Ordering};
use tauri::AppHandle;

static BACKGROUND_MODE: AtomicBool = AtomicBool::new(false);

/// Close-path branch: true → CloseRequested hides, false → exit as before.
pub fn background_mode_enabled() -> bool {
    BACKGROUND_MODE.load(Ordering::Relaxed)
}

/// The frontend mirrors its persisted dock prefs (autoHideOnClose) into this
/// flag at startup and on every toggle, so no guesswork at CloseRequested time.
#[tauri::command]
pub fn set_background_mode(enabled: bool) {
    BACKGROUND_MODE.store(enabled, Ordering::Relaxed);
    crate::debug_log::append(
        "dock",
        "info",
        "",
        "background mode toggled",
        Some(if enabled {
            "closing the window now hides instead of exiting"
        } else {
            "closing the window exits as before"
        }),
    );
}

/// Real exit from the UI (footer 退出): engine kill + agent stop + exit(0).
/// Same path as closing the last window with background mode off.
#[tauri::command]
pub fn quit_app(app: AppHandle) {
    crate::shutdown_and_exit(&app);
}
