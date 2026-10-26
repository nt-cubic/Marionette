//! 文字/链接拖放的进程内事件桥（vendor wry 补丁的接收端）。
//!
//! wry 的 OLE 层对无 CF_HDROP 的拖拽（浏览器选区、链接、图片 URL）默认静默
//! 吞掉，而 WebView2 的 HTML5 外部拖放又被关死——前端任何监听都收不到。
//! vendor/wry 补丁在 OLE 层把这些拖拽读成文字（CF_UNICODETEXT），经这里
//! 注册的钩子转成 `marionette-drag-text` 事件发给对应窗口的前端。
//! 事件只在「该窗口自己的拖放目标」命中时才路由过去，普通文件拖放的
//! paths 事件流完全不受影响。

use std::sync::OnceLock;

use tauri::{AppHandle, Emitter, Manager};

static APP: OnceLock<AppHandle> = OnceLock::new();

pub fn init(app: &AppHandle) {
    let _ = APP.set(app.clone());
    wry::marionette_drag::set_text_drag_hook(on_text_drag);
}

fn on_text_drag(kind: u8, text: &str, x: f64, y: f64, hwnd: isize) {
    let Some(app) = APP.get() else {
        return;
    };
    let kind = match kind {
        wry::marionette_drag::KIND_ENTER => "enter",
        wry::marionette_drag::KIND_OVER => "over",
        wry::marionette_drag::KIND_DROP => "drop",
        _ => "leave",
    };
    // hwnd 是拖放目标窗口（含 detached），只把事件发给它自己的 webview，
    // 别的窗口不会收到别人头顶上的拖拽。
    let windows = app.webview_windows();
    let target = windows
        .values()
        .find(|win| win.hwnd().map(|h| h.0 as isize == hwnd).unwrap_or(false));
    let payload = serde_json::json!({ "kind": kind, "text": text, "x": x, "y": y });
    match target {
        Some(win) => {
            let _ = win.emit("marionette-drag-text", payload);
        }
        None => {
            let _ = app.emit("marionette-drag-text", payload);
        }
    }
}
