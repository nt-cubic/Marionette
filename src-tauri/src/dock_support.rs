//! Edge-dock support (docs/plan-edge-dock.md).
//!
//! 2026-10-31 定稿：贴边与关闭解耦——贴边走标题栏专属按钮 + 拖拽吸附，
//! × 的行为由用户在 Ctrl K 面板选择（默认直接退出；「后台常驻」= × 隐藏
//! 窗口、引擎/会话继续跑，二次启动交接唤回）。曾经「× 顺手贴边 + 前端交接
//! + 2s 看门狗」的链路已移除——后台常驻由 Rust 在 CloseRequested 里直接
//! hide（webview 级先行，配对 TrySuspend 补丁），前端冻死也拦不住关窗。

use std::sync::atomic::{AtomicBool, Ordering};
use tauri::AppHandle;

static BACKGROUND_MODE: AtomicBool = AtomicBool::new(false);

/// Close-path branch: true → CloseRequested hides the main window, false → exit.
pub fn background_mode_enabled() -> bool {
    BACKGROUND_MODE.load(Ordering::Relaxed)
}

/// 前端把 Ctrl K 面板里的选择（dock prefs 的 autoHideOnClose）镜像过来；
/// 标志随进程重置，启动时同步一次。
#[tauri::command]
pub fn set_background_mode(enabled: bool) {
    BACKGROUND_MODE.store(enabled, Ordering::Relaxed);
    crate::debug_log::append(
        "dock",
        "info",
        "",
        "background mode toggled",
        Some(if enabled {
            "closing the main window now hides instead of exiting"
        } else {
            "closing the main window exits"
        }),
    );
}

/// Real exit from the UI (⋯ 退出 / Ctrl K 面板): engine kill + agent stop + exit(0).
#[tauri::command]
pub fn quit_app(app: AppHandle) {
    crate::shutdown_and_exit(&app);
}

/// True while a fullscreen D3D app or a presentation mode owns the screen —
/// the dock sliver must not slide its strip over those (plan §1 全屏抑制).
/// Query failures read as "not fullscreen" (fail open for expansion).
#[tauri::command]
pub fn fullscreen_active() -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::Shell::{
            QUNS_BUSY, QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN,
            QUERY_USER_NOTIFICATION_STATE, SHQueryUserNotificationState,
        };
        let mut state: QUERY_USER_NOTIFICATION_STATE = 0;
        // S_OK == 0; any failure means "unknown", treat as not fullscreen.
        if unsafe { SHQueryUserNotificationState(&mut state) } != 0 {
            return false;
        }
        matches!(
            state,
            QUNS_RUNNING_D3D_FULL_SCREEN | QUNS_PRESENTATION_MODE | QUNS_BUSY
        )
    }
    #[cfg(not(windows))]
    false
}
