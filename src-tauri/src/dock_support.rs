//! Edge-dock support (docs/plan-edge-dock.md).
//!
//! 2026-10-31 定稿：贴边与关闭解耦——贴边走标题栏专属按钮 + 拖拽吸附，
//! × 的行为由用户在 Ctrl K 面板选择（默认直接退出；「后台常驻」= × 隐藏
//! 窗口、引擎/会话继续跑，二次启动交接唤回）。曾经「× 顺手贴边 + 前端交接
//! + 2s 看门狗」的链路已移除——后台常驻由 Rust 在 CloseRequested 里直接
//! hide（webview 级先行，配对 TrySuspend 补丁），前端冻死也拦不住关窗。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tauri::{
    AppHandle, Manager,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent},
};

static BACKGROUND_MODE: AtomicBool = AtomicBool::new(false);

/// 托盘句柄：TrayIcon 被 drop 即移除图标，用 Option 持有做「常驻开/关 ↔
/// 图标有/无」的生命周期（2026-10-31）。菜单文案取创建时刻的语言，切换
/// 语言后下次开常驻时重建生效——托盘是低频入口，不值得追着重译。
static TRAY: Mutex<Option<TrayIcon>> = Mutex::new(None);

/// Close-path branch: true → CloseRequested hides the main window, false → exit.
pub fn background_mode_enabled() -> bool {
    BACKGROUND_MODE.load(Ordering::Relaxed)
}

/// 唤回主窗：webview 级 show 先行（TrySuspend 配对红线——后台驻留的 hide
/// 走了 `as_ref().hide()`，只 show 窗口级会得到一个白屏窗），与
/// shell_integration 二次启动交接同款顺序。
fn show_main_window(app: &AppHandle) {
    use tauri::Manager as _;
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.as_ref().show();
        let _ = main.show();
        let _ = main.set_focus();
    }
}

fn tray_show(app: &AppHandle) {
    let mut guard = match TRAY.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    if guard.is_some() {
        return;
    }
    let show_item = match MenuItem::with_id(
        app,
        "tray-show",
        crate::ts!("显示 Marionette", "Show Marionette"),
        true,
        None::<&str>,
    ) {
        Ok(item) => item,
        Err(error) => {
            crate::debug_log::append(
                "dock",
                "warn",
                "",
                "tray menu item build failed",
                Some(&error.to_string()),
            );
            return;
        }
    };
    let quit_item = match MenuItem::with_id(
        app,
        "tray-quit",
        crate::ts!("退出", "Quit"),
        true,
        None::<&str>,
    ) {
        Ok(item) => item,
        Err(error) => {
            crate::debug_log::append(
                "dock",
                "warn",
                "",
                "tray menu item build failed",
                Some(&error.to_string()),
            );
            return;
        }
    };
    let menu = match Menu::with_items(app, &[&show_item, &quit_item]) {
        Ok(menu) => menu,
        Err(error) => {
            crate::debug_log::append(
                "dock",
                "warn",
                "",
                "tray menu build failed",
                Some(&error.to_string()),
            );
            return;
        }
    };
    let icon = app.default_window_icon().cloned();
    let builder = TrayIconBuilder::with_id("marionette-residency")
        .tooltip("Marionette")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|tray, event| {
            let app = tray.app_handle();
            if event.id() == "tray-quit" {
                crate::shutdown_and_exit(app);
            } else if event.id() == "tray-show" {
                show_main_window(app);
            }
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        });
    let builder = match icon {
        Some(icon) => builder.icon(icon),
        None => builder,
    };
    match builder.build(app) {
        Ok(tray) => {
            *guard = Some(tray);
        }
        Err(error) => {
            crate::debug_log::append(
                "dock",
                "warn",
                "",
                "tray icon build failed",
                Some(&error.to_string()),
            );
        }
    }
}

fn tray_hide() {
    let mut guard = match TRAY.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    // drop 即移除托盘图标。
    *guard = None;
}

/// 前端把 Ctrl K 面板里的选择（dock prefs 的 autoHideOnClose）镜像过来；
/// 标志随进程重置，启动时同步一次。开着后台常驻 → 右下角托盘驻留（点击
/// 唤回 / 右键 退出）；关掉常驻 → 托盘移除。
#[tauri::command]
pub fn set_background_mode(app: AppHandle, enabled: bool) {
    BACKGROUND_MODE.store(enabled, Ordering::Relaxed);
    if enabled {
        tray_show(&app);
    } else {
        tray_hide();
    }
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
