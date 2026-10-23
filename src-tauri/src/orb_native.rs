//! Native floating orb — a Win32 layered topmost window with a tray icon.
//!
//! Zed-flat disc (no gloss / gradient). Left-click fans satellite orbs around
//! the hub; the quota satellite stretches the same window upward into stacked
//! rings. Memo / chat / tasks emit the existing orb command contract
//! (`src/lib/orbEvents.ts`) — the main window executes them.
//!
//! - left-click hub        → fan satellites (or collapse if already open);
//! - quota satellite       → stretch upward, QuotaBar-style rings;
//! - memo / chat / tasks   → open the matching surface in the main window;
//! - drag hub              → system move loop; click-vs-drag by position delta;
//! - right-click           → Win32 menu (open main / memo / new chat / collect
//!                           clipboard / quit);
//! - file drop (WM_DROPFILES) → capture command straight to the main window;
//! - tray icon             → resident presence (left-click: show main).
//!
//! The on-demand `orb-panel` webview is kept (never destroyed — tao paint-assert
//! rule) but is no longer the click target.

use std::collections::HashMap;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicIsize, Ordering};
use std::sync::{Mutex, OnceLock};

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

use crate::app_paths;
use crate::engine::EngineQuotaEntry;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC,
    ReleaseDC, SelectObject, ValidateRect, AC_SRC_ALPHA, AC_SRC_OVER, BLENDFUNCTION, BI_RGB,
    BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS,
};
use windows_sys::Win32::Graphics::GdiPlus::{
    GdipCreateBitmapFromScan0, GdipCreateFont, GdipCreateFontFamilyFromName, GdipCreateHICONFromBitmap,
    GdipCreatePen1, GdipCreateSolidFill, GdipCreateStringFormat,
    GdipDeleteBrush, GdipDeleteFont, GdipDeleteFontFamily, GdipDeleteGraphics, GdipDeletePen,
    GdipDeleteStringFormat, GdipDisposeImage, GdipDrawArcI, GdipDrawEllipseI, GdipDrawImageRectI,
    GdipDrawLineI, GdipDrawRectangleI, GdipDrawString, GdipFillEllipseI, GdipGetImageGraphicsContext,
    GdipGetImageHeight, GdipGetImageWidth, GdipLoadImageFromFile, GdipSetInterpolationMode,
    GdipSetPenLineCap197819, GdipSetSmoothingMode, GdipSetStringFormatAlign,
    GdipSetStringFormatLineAlign, GdiplusShutdown, GdiplusStartup, GdiplusStartupInput, GpBitmap,
    GpBrush, GpFont, GpFontFamily, GpGraphics, GpImage, GpPen, GpSolidFill, GpStringFormat, RectF,
    SmoothingModeAntiAlias, StringAlignmentCenter, UnitPixel, FontStyleBold,
};

/// GDI+ PixelFormat32bppARGB (0x26200A) — windows-sys ships only the flag bits.
const PIXEL_FORMAT_32BPP_ARGB: i32 = 0x26200A;
use windows_sys::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, OpenClipboard};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{GlobalLock, GlobalUnlock};
use windows_sys::Win32::System::Threading::Sleep;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Shell::{
    Shell_NotifyIconW, DragAcceptFiles, DragFinish, DragQueryFileW, NOTIFYICONDATAW, NIF_ICON,
    NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_DELETE, NIM_MODIFY,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu,
    DispatchMessageW, GetCursorPos, GetMessageW, GetSystemMetrics, GetWindowRect, LoadCursorW,
    PostMessageW, PostQuitMessage, RegisterClassW, SendMessageW, SetForegroundWindow, SetWindowPos,
    ShowWindow, SystemParametersInfoW, TrackPopupMenuEx, TranslateMessage, UpdateLayeredWindow,
    HICON, HMENU, MSG, SWP_NOACTIVATE, SWP_NOZORDER, SW_SHOWNOACTIVATE, TPM_NONOTIFY, TPM_RETURNCMD,
    TPM_RIGHTBUTTON, ULW_ALPHA, WNDCLASSW, WM_APP, WM_CLOSE, WM_DESTROY, WM_DPICHANGED,
    WM_DROPFILES, WM_ERASEBKGND, WM_LBUTTONDOWN, WM_NCLBUTTONDOWN, WM_NULL, WM_PAINT,
    WM_RBUTTONUP, WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

// ── constants (windows-sys leaves some menu/format flags unexported) ──────
const MF_STRING: u32 = 0x0000;
const MF_SEPARATOR: u32 = 0x0800;
const MF_GRAYED: u32 = 0x0001;
const HTCAPTION: usize = 0x2;
const SPI_GETWORKAREA: u32 = 0x0030;
const CF_UNICODETEXT: u32 = 13;
const CF_HDROP: u32 = 15;
const IDC_HAND: usize = 32649;
const SM_CXSMICON: i32 = 49;

const WM_ORB_TRAY: u32 = WM_APP + 1; // tray callback
const WM_ORB_BALLOON: u32 = WM_APP + 2; // show queued balloon (orb thread only)
// Layered windows driven by UpdateLayeredWindow never receive WM_PAINT —
// state changes ask for a repaint via this posted message instead.
const WM_ORB_REPAINT: u32 = WM_APP + 3;
const WM_ORB_QUOTA: u32 = WM_APP + 4; // quota fetch finished; resize + repaint
const WM_ORB_LAYOUT: u32 = WM_APP + 5; // recompute window size on the orb thread
const WM_LBUTTONUP_: u32 = 0x0202;
const WM_RBUTTONUP_: u32 = 0x0205;
const WM_ACTIVATE: u32 = 0x0006;

const CLASS_NAME: &str = "MarionetteOrbWnd";
const TRAY_ID: u32 = 1;
const PANEL_LABEL: &str = "orb-panel";
const PANEL_URL: &str = "index.html?orb=1";
const PANEL_W_LOGICAL: f64 = 380.0;
const PANEL_H_LOGICAL: f64 = 540.0;
/// Logical hub size — physical = logical × dpi/96. Dense, Zed-like (was 72).
const BALL_LOGICAL: i32 = 56;
const SAT_LOGICAL: i32 = 38;
/// QuotaBar ProviderRing default diameter is 46pt.
const RING_LOGICAL: i32 = 46;
const INTERPOLATION_HIGH_QUALITY_BICUBIC: i32 = 7;
const LINE_CAP_ROUND: i32 = 2;
const PANEL_BLANK_AFTER_MS: u32 = 180_000;

// Zed tokens (dark). The orb is a desktop overlay; it does not follow the
// in-app white theme — same idea as a status glyph sitting on the wallpaper.
const COL_PANEL: u32 = 0xFF252B32;
const COL_BORDER: u32 = 0xFF3D4650;
const COL_ACCENT: u32 = 0xFF67D7BD;
const COL_TEXT: u32 = 0xFFF0F3F6;
const COL_MUTED: u32 = 0xFF8B97A4;
const COL_ORANGE: u32 = 0xFFF0A15F;
const COL_RED: u32 = 0xFFEF6C73;
const COL_BLUE: u32 = 0xFF72A7FF;
/// QuotaBar dock disc fill is white@8% on black; we sit on wallpaper so the
/// plate has to be opaque. #2A3138 is one step above --panel.
const COL_DISC: u32 = 0xFF2A3138;
const COL_TRACK: u32 = 0xFF3D4650;

// Menu command ids.
const MENU_SHOW_MAIN: i32 = 100;
const MENU_NEW_CHAT: i32 = 101;
const MENU_COLLECT_CLIPBOARD: i32 = 102;
const MENU_OPEN_MEMOS: i32 = 103;
const MENU_QUIT: i32 = 200;

// ── shared state ───────────────────────────────────────────────────────────

#[derive(Clone)]
struct NoticeSummary {
    kind: String,
    label: String,
    at: u64,
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum OrbView {
    #[default]
    Idle,
    Fan,
    Quota,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SatKind {
    Memo,
    Quota,
    Chat,
    Tasks,
}

#[derive(Clone)]
struct QuotaRing {
    provider: String,
    title: String,
    remaining: Option<f64>,
    used: Option<f64>,
    balance: Option<String>,
}

struct SatPlace {
    kind: SatKind,
    x: i32,
    y: i32,
}

struct OrbLayout {
    win_x: i32,
    win_y: i32,
    win_w: i32,
    win_h: i32,
    hub_x: i32,
    hub_y: i32,
    ball: i32,
    sat: i32,
    sats: Vec<SatPlace>,
    rings: Vec<RECT>,
}

enum Hit {
    Hub,
    Sat(SatKind),
    Ring,
    Empty,
}

#[derive(Default)]
struct PaintState {
    main_visible: bool,
    running: usize,
    running_session_id: Option<String>,
    notices: Vec<NoticeSummary>,
    last_balloon_at: u64,
    view: OrbView,
    quota: Vec<QuotaRing>,
    quota_busy: bool,
}

struct Shared {
    app: AppHandle,
    paint: Mutex<PaintState>,
    /// Ball HWND as isize (HWND is a raw pointer — not sync).
    hwnd: AtomicIsize,
    tray_icon: AtomicIsize,
    /// Screen position of the hub's top-left. The window origin may sit
    /// up-left of this when satellites / quota rings are open.
    hub_x: AtomicI32,
    hub_y: AtomicI32,
}

static SHARED: OnceLock<Shared> = OnceLock::new();
static PANEL_BUSY: AtomicBool = AtomicBool::new(false);
static PANEL_BLANK_QUEUED: AtomicBool = AtomicBool::new(false);
/// Balloon text queued by another thread; drained on the orb thread.
static BALLOON_QUEUE: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn shared() -> &'static Shared {
    SHARED.get().expect("orb shared state used before start()")
}

fn ball_hwnd() -> HWND {
    shared().hwnd.load(Ordering::Relaxed) as HWND
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn debug(tag: &str, message: &str) {
    crate::debug_log::append("orb", tag, "", message, None);
}

// ── orb event contract (mirrors src/lib/orbEvents.ts) ─────────────────────

fn emit_command(payload: Value) {
    if let Some(sh) = SHARED.get() {
        let _ = sh.app.emit("marionette-orb-command", payload);
    }
}

fn emit_show_main() {
    emit_command(json!({ "kind": "showMain" }));
}

fn emit_new_chat() {
    emit_command(json!({ "kind": "newChat" }));
}

fn emit_open_memos() {
    emit_command(json!({ "kind": "openMemos" }));
}

fn emit_open_session(session_id: &str) {
    emit_command(json!({ "kind": "openSession", "sessionId": session_id }));
}

fn emit_capture(text: String, urls: Vec<String>, file_paths: Vec<String>) {
    emit_command(json!({
        "kind": "capture",
        "payload": { "text": text, "urls": urls, "filePaths": file_paths }
    }));
}

fn queue_balloon(text: String) {
    if let Ok(mut queue) = BALLOON_QUEUE.lock() {
        queue.push(text);
        if queue.len() > 3 {
            let excess = queue.len() - 3;
            queue.drain(..excess);
        }
    }
    let hwnd = ball_hwnd();
    if !hwnd.is_null() {
        unsafe {
            PostMessageW(hwnd, WM_ORB_BALLOON, 0, 0);
        }
    }
}

// ── geometry helpers ───────────────────────────────────────────────────────

fn work_area() -> RECT {
    unsafe {
        let mut rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut rect as *mut RECT as *mut core::ffi::c_void, 0);
        if rect.right > rect.left {
            rect
        } else {
            RECT {
                left: 0,
                top: 0,
                right: GetSystemMetrics(0),
                bottom: GetSystemMetrics(1),
            }
        }
    }
}

fn dpi_scale(hwnd: HWND) -> f64 {
    unsafe {
        let dpi = GetDpiForWindow(hwnd);
        if dpi == 0 {
            1.0
        } else {
            dpi as f64 / 96.0
        }
    }
}

fn ball_size_physical(hwnd: HWND) -> i32 {
    (BALL_LOGICAL as f64 * dpi_scale(hwnd)).round() as i32
}

fn default_ball_pos(size: i32) -> POINT {
    let work = work_area();
    let margin = (16.0 * (size as f64 / BALL_LOGICAL as f64)).round() as i32;
    POINT {
        x: work.right - size - margin,
        y: work.bottom - size - margin,
    }
}

fn pos_file() -> PathBuf {
    app_paths::global_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("orb-pos.json")
}

#[derive(serde::Serialize, serde::Deserialize)]
struct OrbPos {
    x: i32,
    y: i32,
}

fn load_ball_pos(size: i32) -> POINT {
    let default = default_ball_pos(size);
    let Ok(text) = std::fs::read_to_string(pos_file()) else {
        return default;
    };
    let Ok(pos) = serde_json::from_str::<OrbPos>(&text) else {
        return default;
    };
    let work = work_area();
    // Reject stale positions that fell off the current monitor layout.
    if pos.x < work.left - size || pos.y < work.top - size || pos.x > work.right || pos.y > work.bottom
    {
        return default;
    }
    POINT { x: pos.x, y: pos.y }
}

fn save_ball_pos(point: POINT) {
    let pos = OrbPos { x: point.x, y: point.y };
    let _ = std::fs::write(pos_file(), serde_json::to_string(&pos).unwrap_or_default());
}

fn px(logical: i32, scale: f64) -> i32 {
    (logical as f64 * scale).round() as i32
}

fn hub_screen() -> POINT {
    POINT {
        x: shared().hub_x.load(Ordering::Relaxed),
        y: shared().hub_y.load(Ordering::Relaxed),
    }
}

fn set_hub_screen(point: POINT) {
    shared().hub_x.store(point.x, Ordering::Relaxed);
    shared().hub_y.store(point.y, Ordering::Relaxed);
    save_ball_pos(point);
}

fn in_disc(px: i32, py: i32, x: i32, y: i32, d: i32) -> bool {
    let cx = x as f32 + d as f32 * 0.5;
    let cy = y as f32 + d as f32 * 0.5;
    let r = d as f32 * 0.5;
    let dx = px as f32 - cx;
    let dy = py as f32 - cy;
    dx * dx + dy * dy <= r * r
}

fn lparam_point(lparam: LPARAM) -> (i32, i32) {
    let x = (lparam & 0xFFFF) as u16 as i16 as i32;
    let y = ((lparam >> 16) & 0xFFFF) as u16 as i16 as i32;
    (x, y)
}

fn rings_from_entries(entries: &[EngineQuotaEntry]) -> Vec<QuotaRing> {
    let mut out = Vec::new();
    for entry in entries {
        let title = if entry.name.trim().is_empty() {
            entry.provider.clone()
        } else {
            entry.name.clone()
        };
        // QuotaBar paints one ring per provider. The arc is the hottest
        // window (least remaining); a money-only account shows the balance.
        let mut remaining = None;
        let mut used = None;
        for window in &entry.windows {
            let rem = window
                .remaining
                .or_else(|| window.used.map(|u| (100.0 - u).clamp(0.0, 100.0)));
            let usd = window
                .used
                .or_else(|| window.remaining.map(|r| (100.0 - r).clamp(0.0, 100.0)));
            match (remaining, rem) {
                (None, Some(r)) => {
                    remaining = Some(r);
                    used = usd;
                }
                (Some(best), Some(r)) if r < best => {
                    remaining = Some(r);
                    used = usd;
                }
                _ => {}
            }
        }
        let balance = entry
            .balance
            .as_ref()
            .filter(|s| !s.is_empty())
            .cloned();
        if remaining.is_none() && used.is_none() && balance.is_none() {
            continue;
        }
        out.push(QuotaRing {
            provider: entry.provider.clone(),
            title,
            remaining,
            used,
            balance,
        });
        if out.len() >= 8 {
            break;
        }
    }
    out
}

fn compute_layout(hwnd: HWND) -> OrbLayout {
    let scale = dpi_scale(hwnd);
    let ball = px(BALL_LOGICAL, scale);
    let sat = px(SAT_LOGICAL, scale);
    let hub = hub_screen();
    let (view, running, quota_n) = shared()
        .paint
        .lock()
        .map(|p| (p.view, p.running, p.quota.len().max(if p.quota_busy { 1 } else { 1 })))
        .unwrap_or((OrbView::Idle, 0, 1));
    match view {
        OrbView::Idle => OrbLayout {
            win_x: hub.x,
            win_y: hub.y,
            win_w: ball,
            win_h: ball,
            hub_x: 0,
            hub_y: 0,
            ball,
            sat,
            sats: Vec::new(),
            rings: Vec::new(),
        },
        OrbView::Fan => {
            let dist = px(54, scale);
            let pad = px(10, scale);
            let work = work_area();
            let cx = hub.x + ball / 2;
            let cy = hub.y + ball / 2;
            let sx = if cx >= (work.left + work.right) / 2 { -1 } else { 1 };
            let sy = if cy >= (work.top + work.bottom) / 2 { -1 } else { 1 };
            let offsets: [(SatKind, f32, f32); 4] = [
                (SatKind::Quota, 0.0, 1.0),
                (SatKind::Memo, 0.78, 0.78),
                (SatKind::Chat, 1.0, 0.0),
                (SatKind::Tasks, 0.78, 0.32),
            ];
            let mut sats_screen: Vec<(SatKind, i32, i32)> = Vec::new();
            let mut min_x = hub.x;
            let mut min_y = hub.y;
            let mut max_x = hub.x + ball;
            let mut max_y = hub.y + ball;
            for (kind, ox, oy) in offsets {
                if kind == SatKind::Tasks && running == 0 {
                    continue;
                }
                let scx = cx + (sx as f32 * ox * dist as f32).round() as i32;
                let scy = cy + (sy as f32 * oy * dist as f32).round() as i32;
                let x = scx - sat / 2;
                let y = scy - sat / 2;
                sats_screen.push((kind, x, y));
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x + sat);
                max_y = max_y.max(y + sat);
            }
            min_x -= pad;
            min_y -= pad;
            max_x += pad;
            max_y += pad;
            let sats = sats_screen
                .into_iter()
                .map(|(kind, x, y)| SatPlace {
                    kind,
                    x: x - min_x,
                    y: y - min_y,
                })
                .collect();
            OrbLayout {
                win_x: min_x,
                win_y: min_y,
                win_w: (max_x - min_x).max(ball),
                win_h: (max_y - min_y).max(ball),
                hub_x: hub.x - min_x,
                hub_y: hub.y - min_y,
                ball,
                sat,
                sats,
                rings: Vec::new(),
            }
        }
        OrbView::Quota => {
            let ring = px(RING_LOGICAL, scale);
            let caption = px(14, scale);
            let gap = px(8, scale);
            let pad = px(8, scale);
            let n = quota_n.max(1) as i32;
            let cell = ring + caption + gap;
            let win_w = ring.max(ball) + pad * 2;
            let win_h = pad + n * cell + pad + ball;
            let work = work_area();
            let mut win_x = hub.x - (win_w - ball) / 2;
            win_x = win_x.clamp(work.left, (work.right - win_w).max(work.left));
            let prefer_up_y = hub.y + ball - win_h;
            let grow_up = prefer_up_y >= work.top;
            let mut rings = Vec::new();
            if grow_up {
                for i in 0..n {
                    let x = (win_w - ring) / 2;
                    let y = pad + i * cell;
                    rings.push(RECT {
                        left: x,
                        top: y,
                        right: x + ring,
                        bottom: y + ring + caption,
                    });
                }
            } else {
                for i in 0..n {
                    let x = (win_w - ring) / 2;
                    let y = ball + pad + i * cell;
                    rings.push(RECT {
                        left: x,
                        top: y,
                        right: x + ring,
                        bottom: y + ring + caption,
                    });
                }
            }
            OrbLayout {
                win_x,
                win_y: if grow_up { prefer_up_y } else { hub.y },
                win_w,
                win_h,
                hub_x: (win_w - ball) / 2,
                hub_y: if grow_up { win_h - ball } else { 0 },
                ball,
                sat,
                sats: Vec::new(),
                rings,
            }
        }
    }
}

fn apply_layout(hwnd: HWND) {
    let layout = compute_layout(hwnd);
    unsafe {
        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            layout.win_x,
            layout.win_y,
            layout.win_w,
            layout.win_h,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
    unsafe {
        paint_ball(hwnd);
    }
}

fn set_view(hwnd: HWND, view: OrbView) {
    let enter_quota = {
        let Ok(mut paint) = shared().paint.lock() else {
            return;
        };
        paint.view = view;
        view == OrbView::Quota
    };
    if enter_quota {
        start_quota_fetch(hwnd);
    }
    apply_layout(hwnd);
}

fn start_quota_fetch(hwnd: HWND) {
    if let Ok(mut paint) = shared().paint.lock() {
        paint.quota_busy = true;
    }
    // HWND 不是 Send：以 usize 过线程边界，线程内还原（Win32 句柄的常规搬运法）。
    let hwnd = hwnd as usize;
    std::thread::spawn(move || {
        let hwnd = hwnd as HWND;
        let entries = crate::engine::quota(true);
        let rings = rings_from_entries(&entries);
        if let Ok(mut paint) = shared().paint.lock() {
            paint.quota = rings;
            paint.quota_busy = false;
        }
        if !hwnd.is_null() {
            unsafe {
                PostMessageW(hwnd, WM_ORB_QUOTA, 0, 0);
            }
        }
    });
}

fn hit_test(hwnd: HWND, cx: i32, cy: i32) -> Hit {
    let layout = compute_layout(hwnd);
    if in_disc(cx, cy, layout.hub_x, layout.hub_y, layout.ball) {
        return Hit::Hub;
    }
    for sat in &layout.sats {
        if in_disc(cx, cy, sat.x, sat.y, layout.sat) {
            return Hit::Sat(sat.kind);
        }
    }
    for rect in &layout.rings {
        let d = (rect.right - rect.left).min(rect.bottom - rect.top - 12).max(8);
        if in_disc(cx, cy, rect.left, rect.top, d) {
            return Hit::Ring;
        }
    }
    Hit::Empty
}

fn activate_sat(hwnd: HWND, kind: SatKind) {
    match kind {
        SatKind::Quota => set_view(hwnd, OrbView::Quota),
        SatKind::Memo => {
            set_view(hwnd, OrbView::Idle);
            emit_open_memos();
        }
        SatKind::Chat => {
            set_view(hwnd, OrbView::Idle);
            emit_new_chat();
        }
        SatKind::Tasks => {
            let session_id = shared()
                .paint
                .lock()
                .ok()
                .and_then(|p| p.running_session_id.clone());
            set_view(hwnd, OrbView::Idle);
            match session_id {
                Some(id) => emit_open_session(&id),
                None => emit_show_main(),
            }
        }
    }
}

// ── panel webview lifecycle ────────────────────────────────────────────────

fn panel_position(hwnd: HWND) -> (f64, f64) {
    unsafe {
        let scale = dpi_scale(hwnd);
        let panel_w = PANEL_W_LOGICAL * scale;
        let panel_h = PANEL_H_LOGICAL * scale;
        let mut ball = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        let mut anchor = default_ball_pos(ball_size_physical(hwnd));
        if !hwnd.is_null() && GetWindowRect(hwnd, &mut ball) != 0 {
            anchor.x = ball.left;
            anchor.y = ball.top;
        }
        let overlap = 28.0 * scale;
        let work = work_area();
        // Preferred: panel grows up-left from the ball's top-left corner.
        let mut x = anchor.x as f64 - panel_w + overlap;
        let mut y = anchor.y as f64 - panel_h + overlap;
        let left = work.left as f64;
        let top = work.top as f64;
        if x < left {
            x = (ball.right as f64 - overlap).max(left); // ball near the left edge → right side
        }
        if y < top {
            y = work.bottom as f64 - panel_h; // near the top edge → bottom
        }
        (x, y) // physical pixels
    }
}

fn eval_replace(win: &tauri::WebviewWindow, url: &str) {
    let js = format!(
        "try{{window.location.replace({})}}catch(e){{}}",
        serde_json::to_string(url).unwrap_or_else(|_| format!("\"{url}\""))
    );
    let _ = win.eval(&js);
}

fn panel_is_visible(app: &AppHandle) -> bool {
    app.get_webview_window(PANEL_LABEL)
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false)
}

#[allow(dead_code)]
fn show_panel(app: &AppHandle, hwnd: HWND) {
    let scale = dpi_scale(hwnd);
    let (px, py) = panel_position(hwnd);
    match app.get_webview_window(PANEL_LABEL) {
        Some(win) => {
            let _ = win.set_position(tauri::PhysicalPosition::new(
                px.round() as i32,
                py.round() as i32,
            ));
            // Fresh SPA state on every open (scenarios/quota reload, notices ack).
            eval_replace(&win, PANEL_URL);
            let _ = win.show();
            let _ = win.set_focus();
        }
        None => {
            let built = WebviewWindowBuilder::new(
                app,
                PANEL_LABEL,
                WebviewUrl::App(PANEL_URL.into()),
            )
            .inner_size(PANEL_W_LOGICAL, PANEL_H_LOGICAL)
            .position(px / scale, py / scale)
            .decorations(false)
            .skip_taskbar(true)
            .always_on_top(true)
            .resizable(false)
            .visible(false)
            .focused(false)
            // HTML5 DnD must survive — see orbWindow.ts history / Composer notes.
            .disable_drag_drop_handler()
            .build();
            match built {
                Ok(win) => {
                    let _ = win.show();
                    let _ = win.set_focus();
                }
                Err(err) => debug("warn", &format!("panel build failed: {err}")),
            }
        }
    }
}

fn hide_panel(app: &AppHandle) {
    if let Some(win) = app.get_webview_window(PANEL_LABEL) {
        let _ = win.hide();
        schedule_panel_blank(app.clone());
    }
}

fn schedule_panel_blank(app: AppHandle) {
    if PANEL_BLANK_QUEUED.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(move || {
        unsafe { Sleep(PANEL_BLANK_AFTER_MS) };
        PANEL_BLANK_QUEUED.store(false, Ordering::SeqCst);
        let Some(win) = app.get_webview_window(PANEL_LABEL) else {
            return;
        };
        if !panel_is_visible(&app) {
            eval_replace(&win, "about:blank");
        }
    });
}

/// JS-side collapse hook (Escape / ✕ / blur from the panel SPA).
pub fn panel_hide_from_ui(app: &AppHandle) {
    hide_panel(app);
}

#[allow(dead_code)]
fn toggle_panel() {
    let Some(sh) = SHARED.get() else { return };
    if PANEL_BUSY.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = sh.app.clone();
    // HWND is a raw pointer — cross the thread boundary as an integer.
    let hwnd_bits = sh.hwnd.load(Ordering::Relaxed);
    std::thread::spawn(move || {
        if panel_is_visible(&app) {
            hide_panel(&app);
        } else {
            show_panel(&app, hwnd_bits as HWND);
        }
        PANEL_BUSY.store(false, Ordering::SeqCst);
    });
}

// ── clipboard capture ──────────────────────────────────────────────────────

unsafe fn clipboard_utf16_text() -> Option<String> {
    let handle = GetClipboardData(CF_UNICODETEXT);
    if handle.is_null() {
        return None;
    }
    let locked = GlobalLock(handle);
    if locked.is_null() {
        return None;
    }
    let mut out = String::new();
    let mut cursor = locked as *const u16;
    let mut guard = 0usize;
    while guard < 1 << 20 {
        let ch = *cursor;
        if ch == 0 {
            break;
        }
        out.push(char::from_u32(ch as u32).unwrap_or('\u{FFFD}'));
        cursor = cursor.add(1);
        guard += 1;
    }
    GlobalUnlock(handle);
    let trimmed = out.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

unsafe fn clipboard_hdrop_files() -> Vec<String> {
    let handle = GetClipboardData(CF_HDROP);
    let mut files = Vec::new();
    if handle.is_null() {
        return files;
    }
    let count = DragQueryFileW(handle, 0xFFFF_FFFF, std::ptr::null_mut(), 0);
    for index in 0..count {
        let mut buffer = [0u16; 1024];
        let len = DragQueryFileW(handle, index, buffer.as_mut_ptr(), buffer.len() as u32);
        if len == 0 {
            continue;
        }
        let text = String::from_utf16_lossy(&buffer[..len as usize]);
        files.push(text);
    }
    files
}

fn collect_clipboard(hwnd: HWND) {
    unsafe {
        if OpenClipboard(hwnd) == 0 {
            queue_balloon("剪贴板暂时不可用".into());
            return;
        }
        let text = clipboard_utf16_text();
        let files = clipboard_hdrop_files();
        CloseClipboard();
        if text.is_none() && files.is_empty() {
            queue_balloon("剪贴板没有可收集的内容".into());
            return;
        }
        let mut combined = text.unwrap_or_default();
        for file in &files {
            if !combined.is_empty() {
                combined.push('\n');
            }
            combined.push_str(&format!("[文件] {file}"));
        }
        emit_capture(combined, Vec::new(), files);
        queue_balloon("已送入收集箱 ✓".into());
    }
}

// ── menu ───────────────────────────────────────────────────────────────────

unsafe fn append_menu_text(menu: HMENU, flags: u32, id: usize, text: &str) {
    let wide_text = wide(text);
    AppendMenuW(menu, flags, id, wide_text.as_ptr());
}

unsafe fn show_menu(hwnd: HWND) {
    let running = shared()
        .paint
        .lock()
        .map(|paint| paint.running)
        .unwrap_or(0);

    let menu = CreatePopupMenu();
    if menu.is_null() {
        return;
    }
    append_menu_text(menu, MF_STRING, MENU_SHOW_MAIN as usize, "打开主窗口(&M)");
    append_menu_text(menu, MF_STRING, MENU_OPEN_MEMOS as usize, "打开 Memo(&O)");
    append_menu_text(menu, MF_STRING, MENU_NEW_CHAT as usize, "新建对话(&N)");
    append_menu_text(menu, MF_SEPARATOR, 0, "");
    append_menu_text(menu, MF_STRING, MENU_COLLECT_CLIPBOARD as usize, "收集剪贴板(&C)");
    append_menu_text(menu, MF_SEPARATOR, 0, "");
    let info = if running > 0 {
        format!("运行中：{running} 个任务")
    } else {
        "无运行中任务".to_string()
    };
    append_menu_text(menu, MF_GRAYED, 0, &info);
    append_menu_text(menu, MF_SEPARATOR, 0, "");
    append_menu_text(menu, MF_STRING, MENU_QUIT as usize, "退出 Marionette(&X)");

    let mut cursor = POINT { x: 0, y: 0 };
    GetCursorPos(&mut cursor);
    SetForegroundWindow(hwnd);
    let command = TrackPopupMenuEx(
        menu,
        TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
        cursor.x,
        cursor.y,
        hwnd,
        std::ptr::null_mut(),
    );
    DestroyMenu(menu);
    PostMessageW(hwnd, WM_NULL, 0, 0);

    if command == 0 {
        return;
    }
    if command == MENU_SHOW_MAIN {
        emit_show_main();
    } else if command == MENU_NEW_CHAT {
        emit_new_chat();
    } else if command == MENU_OPEN_MEMOS {
        emit_open_memos();
    } else if command == MENU_COLLECT_CLIPBOARD {
        collect_clipboard(hwnd);
    } else if command == MENU_QUIT {
        if let Some(sh) = SHARED.get() {
            crate::shutdown_and_exit(&sh.app);
        }
    }
}

// ── tray icon ──────────────────────────────────────────────────────────────

unsafe fn tray_add(hwnd: HWND, hicon: HICON) {
    let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = TRAY_ID;
    nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    nid.uCallbackMessage = WM_ORB_TRAY;
    nid.hIcon = hicon;
    let tip = wide("Marionette");
    let copy_len = tip.len().min(nid.szTip.len() - 1);
    nid.szTip[..copy_len].copy_from_slice(&tip[..copy_len]);
    if Shell_NotifyIconW(NIM_ADD, &nid) == 0 {
        debug("warn", "Shell_NotifyIcon NIM_ADD failed");
    }
}

unsafe fn tray_remove(hwnd: HWND) {
    let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = TRAY_ID;
    Shell_NotifyIconW(NIM_DELETE, &nid);
}

unsafe fn tray_balloon(hwnd: HWND, message: &str) {
    let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = TRAY_ID;
    nid.uFlags = NIF_INFO;
    let info = wide(message);
    let copy_len = info.len().min(nid.szInfo.len() - 1);
    nid.szInfo[..copy_len].copy_from_slice(&info[..copy_len]);
    let title = wide("Marionette");
    let title_len = title.len().min(nid.szInfoTitle.len() - 1);
    nid.szInfoTitle[..title_len].copy_from_slice(&title[..title_len]);
    nid.dwInfoFlags = NIIF_INFO;
    Shell_NotifyIconW(NIM_MODIFY, &nid);
    // NIF_INFO sticks until cleared — reset so the next balloon can fire.
    nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    nid.szInfo = [0; 256];
    nid.szInfoTitle = [0; 64];
    Shell_NotifyIconW(NIM_MODIFY, &nid);
}

// ── procedural disc rendering (GDI+ flat API, Zed tokens + QuotaBar marks) ─

struct BallArt {
    bitmap: *mut GpBitmap,
    pixels: Vec<u32>,
}

fn logo_bytes(name: &str) -> Option<&'static [u8]> {
    Some(match name {
        "hub" => include_bytes!("../../src/marionette-logo.png"),
        "alibaba" => include_bytes!("../quota-logos/alibaba.png"),
        "antigravity" => include_bytes!("../quota-logos/antigravity.png"),
        "claude" => include_bytes!("../quota-logos/claude.png"),
        "codex" => include_bytes!("../quota-logos/codex.png"),
        "copilot" => include_bytes!("../quota-logos/copilot.png"),
        "cursor" => include_bytes!("../quota-logos/cursor.png"),
        "deepseek" => include_bytes!("../quota-logos/deepseek.png"),
        "gemini" => include_bytes!("../quota-logos/gemini.png"),
        "grok" => include_bytes!("../quota-logos/grok.png"),
        "kimi" => include_bytes!("../quota-logos/kimi-dark.png"),
        "kiro" => include_bytes!("../quota-logos/kiro.png"),
        "manus" => include_bytes!("../quota-logos/manus.png"),
        "mimo" => include_bytes!("../quota-logos/mimo.png"),
        "minimax" => include_bytes!("../quota-logos/minimax.png"),
        "opencode-go" => include_bytes!("../quota-logos/opencode-go.png"),
        "openrouter" => include_bytes!("../quota-logos/openrouter.png"),
        "qoder" => include_bytes!("../quota-logos/qoder.png"),
        "qwen" => include_bytes!("../quota-logos/qwen.png"),
        "volcengine" => include_bytes!("../quota-logos/volcengine.png"),
        "windsurf" => include_bytes!("../quota-logos/windsurf.png"),
        "zai" => include_bytes!("../quota-logos/zai.png"),
        "zhipu" => include_bytes!("../quota-logos/zhipu.png"),
        _ => return None,
    })
}

/// Magpie provider id / display name → QuotaBar logo file stem.
fn logo_key(provider: &str, title: &str) -> Option<&'static str> {
    let hay = format!("{provider} {title}").to_ascii_lowercase();
    const TABLE: &[(&str, &str)] = &[
        ("claude", "claude"),
        ("anthropic", "claude"),
        ("codex", "codex"),
        ("openai", "codex"),
        ("chatgpt", "codex"),
        ("cursor", "cursor"),
        ("deepseek", "deepseek"),
        ("antigravity", "antigravity"),
        ("gemini", "gemini"),
        ("google", "gemini"),
        ("grok", "grok"),
        ("xai", "grok"),
        ("x-ai", "grok"),
        ("kimi", "kimi"),
        ("moonshot", "kimi"),
        ("minimax", "minimax"),
        ("opencode", "opencode-go"),
        ("openrouter", "openrouter"),
        ("qwen", "qwen"),
        ("dashscope", "qwen"),
        ("bailian", "alibaba"),
        ("alibaba", "alibaba"),
        ("zhipu", "zhipu"),
        ("glm", "zhipu"),
        ("zai", "zai"),
        ("z.ai", "zai"),
        ("manus", "manus"),
        ("copilot", "copilot"),
        ("github", "copilot"),
        ("volcengine", "volcengine"),
        ("doubao", "volcengine"),
        ("ark", "volcengine"),
        ("kiro", "kiro"),
        ("windsurf", "windsurf"),
        ("qoder", "qoder"),
        ("mimo", "mimo"),
        ("xiaomi", "mimo"),
    ];
    for (needle, key) in TABLE {
        if hay.contains(needle) {
            return Some(*key);
        }
    }
    None
}

fn path_wide(path: &Path) -> Vec<u16> {
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

fn ensure_logo_file(name: &str, bytes: &[u8]) -> PathBuf {
    let dir = app_paths::global_dir()
        .unwrap_or_else(|_| std::env::temp_dir())
        .join("cache")
        .join("quota-logos");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(format!("{name}.png"));
    let stale = path
        .metadata()
        .map(|meta| meta.len() as usize != bytes.len())
        .unwrap_or(true);
    if stale {
        let _ = std::fs::write(&path, bytes);
    }
    path
}

fn image_cache() -> &'static Mutex<HashMap<String, usize>> {
    static CACHE: OnceLock<Mutex<HashMap<String, usize>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

unsafe fn image_named(name: &str) -> *mut GpImage {
    if let Ok(cache) = image_cache().lock() {
        if let Some(bits) = cache.get(name) {
            return *bits as *mut GpImage;
        }
    }
    let Some(bytes) = logo_bytes(name) else {
        return std::ptr::null_mut();
    };
    let path = ensure_logo_file(name, bytes);
    let wide_path = path_wide(&path);
    let mut image: *mut GpImage = std::ptr::null_mut();
    if GdipLoadImageFromFile(wide_path.as_ptr(), &mut image) != 0 {
        return std::ptr::null_mut();
    }
    if let Ok(mut cache) = image_cache().lock() {
        cache.insert(name.to_string(), image as usize);
    }
    image
}

unsafe fn draw_png(gfx: *mut GpGraphics, name: &str, x: i32, y: i32, size: i32) -> bool {
    let image = image_named(name);
    if image.is_null() || size <= 0 {
        return false;
    }
    GdipSetInterpolationMode(gfx, INTERPOLATION_HIGH_QUALITY_BICUBIC);
    let mut width = 0u32;
    let mut height = 0u32;
    GdipGetImageWidth(image, &mut width);
    GdipGetImageHeight(image, &mut height);
    let (dw, dh) = if width == 0 || height == 0 {
        (size, size)
    } else if width >= height {
        let h = (size as f32 * height as f32 / width as f32).round() as i32;
        (size, h.max(1))
    } else {
        let w = (size as f32 * width as f32 / height as f32).round() as i32;
        (w.max(1), size)
    };
    let dx = x + (size - dw) / 2;
    let dy = y + (size - dh) / 2;
    GdipDrawImageRectI(gfx, image, dx, dy, dw, dh);
    true
}

/// QuotaBar UsageRamp — colour of a **used** percentage (green plateau
/// through 50, then olive → amber → red, full red at 90).
fn usage_ramp_argb(used: f64) -> u32 {
    const STOPS: [u32; 9] = [
        0xFF34C759, 0xFF4FC447, 0xFF82B91F, 0xFFA8A81E, 0xFFBF961C, 0xFFD0801B, 0xFFDE6418,
        0xFFE83A1A, 0xFFDC2626,
    ];
    let p = used.clamp(0.0, 100.0);
    if p <= 50.0 {
        return STOPS[0];
    }
    if p >= 90.0 {
        return STOPS[8];
    }
    let x = (p - 50.0) / 5.0;
    let i = x.floor() as usize;
    let t = x - i as f64;
    let blend = |shift: u32| {
        let a = ((STOPS[i] >> shift) & 0xFF) as f64;
        let b = ((STOPS[i + 1] >> shift) & 0xFF) as f64;
        (a + (b - a) * t).round() as u32
    };
    0xFF000000 | (blend(16) << 16) | (blend(8) << 8) | blend(0)
}

unsafe fn draw_centered_text(
    gfx: *mut GpGraphics,
    text: &str,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    font_size: f32,
    bold: bool,
    color: u32,
) {
    let name = wide("Segoe UI");
    let mut family: *mut GpFontFamily = std::ptr::null_mut();
    if GdipCreateFontFamilyFromName(name.as_ptr(), std::ptr::null_mut(), &mut family) != 0 {
        return;
    }
    let mut font: *mut GpFont = std::ptr::null_mut();
    let style = if bold { FontStyleBold } else { 0 };
    if GdipCreateFont(family, font_size, style, UnitPixel, &mut font) != 0 {
        GdipDeleteFontFamily(family);
        return;
    }
    let mut format: *mut GpStringFormat = std::ptr::null_mut();
    GdipCreateStringFormat(0, 0, &mut format);
    GdipSetStringFormatAlign(format, StringAlignmentCenter);
    GdipSetStringFormatLineAlign(format, StringAlignmentCenter);
    let mut solid: *mut GpSolidFill = std::ptr::null_mut();
    GdipCreateSolidFill(color, &mut solid);
    let layout = RectF { X: x, Y: y, Width: w, Height: h };
    let wide_text = wide(text);
    GdipDrawString(
        gfx,
        wide_text.as_ptr(),
        (wide_text.len() - 1) as i32,
        font,
        &layout,
        format,
        solid as *mut GpBrush,
    );
    GdipDeleteBrush(solid as *mut GpBrush);
    GdipDeleteStringFormat(format);
    GdipDeleteFont(font);
    GdipDeleteFontFamily(family);
}

unsafe fn fill_disc(gfx: *mut GpGraphics, x: i32, y: i32, d: i32, argb: u32) {
    let mut brush: *mut GpSolidFill = std::ptr::null_mut();
    GdipCreateSolidFill(argb, &mut brush);
    GdipFillEllipseI(gfx, brush as *mut GpBrush, x, y, d, d);
    GdipDeleteBrush(brush as *mut GpBrush);
}

unsafe fn stroke_disc(gfx: *mut GpGraphics, x: i32, y: i32, d: i32, argb: u32, width: f32) {
    let mut pen: *mut GpPen = std::ptr::null_mut();
    GdipCreatePen1(argb, width.max(1.0), UnitPixel, &mut pen);
    GdipSetPenLineCap197819(pen, LINE_CAP_ROUND, LINE_CAP_ROUND, 0);
    GdipDrawEllipseI(gfx, pen, x, y, d, d);
    GdipDeletePen(pen);
}

unsafe fn stroke_arc(
    gfx: *mut GpGraphics,
    x: i32,
    y: i32,
    d: i32,
    argb: u32,
    width: f32,
    start: f32,
    sweep: f32,
) {
    let mut pen: *mut GpPen = std::ptr::null_mut();
    GdipCreatePen1(argb, width.max(1.0), UnitPixel, &mut pen);
    GdipSetPenLineCap197819(pen, LINE_CAP_ROUND, LINE_CAP_ROUND, 0);
    GdipDrawArcI(gfx, pen, x, y, d, d, start, sweep);
    GdipDeletePen(pen);
}

unsafe fn stroke_line(gfx: *mut GpGraphics, x1: i32, y1: i32, x2: i32, y2: i32, argb: u32, width: f32) {
    let mut pen: *mut GpPen = std::ptr::null_mut();
    GdipCreatePen1(argb, width.max(1.0), UnitPixel, &mut pen);
    GdipSetPenLineCap197819(pen, LINE_CAP_ROUND, LINE_CAP_ROUND, 0);
    GdipDrawLineI(gfx, pen, x1, y1, x2, y2);
    GdipDeletePen(pen);
}

unsafe fn stroke_rect(gfx: *mut GpGraphics, x: i32, y: i32, w: i32, h: i32, argb: u32, width: f32) {
    let mut pen: *mut GpPen = std::ptr::null_mut();
    GdipCreatePen1(argb, width.max(1.0), UnitPixel, &mut pen);
    GdipSetPenLineCap197819(pen, LINE_CAP_ROUND, LINE_CAP_ROUND, 0);
    GdipDrawRectangleI(gfx, pen, x, y, w, h);
    GdipDeletePen(pen);
}

unsafe fn draw_hub_disc(
    gfx: *mut GpGraphics,
    x: i32,
    y: i32,
    d: i32,
    notice: Option<&str>,
    running: usize,
) {
    // Steel plate + teal ring + marionette mark. The mascot is dark, so the
    // plate sits one step above --panel and the ring is the signature.
    fill_disc(gfx, x, y, d, COL_DISC);
    let ring_w = (d as f32 * 0.055).max(1.75);
    stroke_disc(
        gfx,
        x,
        y,
        d,
        if running > 0 { COL_ACCENT } else { COL_BORDER },
        ring_w,
    );
    if running > 0 {
        let inset = (d as f32 * 0.10).round() as i32;
        stroke_disc(
            gfx,
            x + inset,
            y + inset,
            d - inset * 2,
            COL_ACCENT,
            (d as f32 * 0.04).max(1.4),
        );
        let label = if running > 9 {
            "9+".to_string()
        } else {
            running.to_string()
        };
        draw_centered_text(
            gfx,
            &label,
            x as f32,
            y as f32,
            d as f32,
            d as f32,
            (d as f32 * 0.36).max(11.0),
            true,
            COL_TEXT,
        );
    } else {
        let mark = (d as f32 * 0.72).round() as i32;
        if !draw_png(gfx, "hub", x + (d - mark) / 2, y + (d - mark) / 2, mark) {
            let core = (d as f32 * 0.18).round().max(5.0) as i32;
            fill_disc(gfx, x + (d - core) / 2, y + (d - core) / 2, core, COL_ACCENT);
        }
    }
    if let Some(kind) = notice {
        let color = match kind {
            "error" => COL_RED,
            "permission" => COL_BLUE,
            _ => COL_ORANGE,
        };
        let badge = (d as f32 * 0.20).round().max(6.0) as i32;
        fill_disc(gfx, x + d - badge, y, badge, color);
    }
}

unsafe fn draw_sat_icon(gfx: *mut GpGraphics, x: i32, y: i32, d: i32, kind: SatKind, running: usize) {
    let w = (d as f32 * 0.085).max(1.6);
    let cx = x + d / 2;
    let cy = y + d / 2;
    match kind {
        SatKind::Memo => {
            let rw = (d as f32 * 0.42).round() as i32;
            let rh = (d as f32 * 0.50).round() as i32;
            let rx = cx - rw / 2;
            let ry = cy - rh / 2;
            stroke_rect(gfx, rx, ry, rw, rh, COL_TEXT, w);
            let pad = (rw as f32 * 0.22).round() as i32;
            for i in 0..3 {
                let ly = ry + (rh as f32 * (0.28 + i as f32 * 0.22)).round() as i32;
                stroke_line(gfx, rx + pad, ly, rx + rw - pad, ly, COL_TEXT, w * 0.85);
            }
        }
        SatKind::Quota => {
            let inner = (d as f32 * 0.46).round() as i32;
            stroke_disc(gfx, cx - inner / 2, cy - inner / 2, inner, COL_TRACK, w);
            stroke_arc(
                gfx,
                cx - inner / 2,
                cy - inner / 2,
                inner,
                COL_ACCENT,
                w,
                -90.0,
                220.0,
            );
        }
        SatKind::Chat => {
            let arm = (d as f32 * 0.18).round() as i32;
            stroke_line(gfx, cx - arm, cy, cx + arm, cy, COL_TEXT, w * 1.15);
            stroke_line(gfx, cx, cy - arm, cx, cy + arm, COL_TEXT, w * 1.15);
        }
        SatKind::Tasks => {
            let n = if running > 9 {
                "9+".to_string()
            } else {
                running.to_string()
            };
            draw_centered_text(
                gfx,
                &n,
                x as f32,
                y as f32,
                d as f32,
                d as f32,
                (d as f32 * 0.40).max(10.0),
                true,
                COL_ACCENT,
            );
        }
    }
}

unsafe fn draw_sat_disc(gfx: *mut GpGraphics, x: i32, y: i32, d: i32, kind: SatKind, running: usize) {
    fill_disc(gfx, x, y, d, COL_PANEL);
    stroke_disc(gfx, x, y, d, COL_BORDER, (d as f32 * 0.05).max(1.2));
    draw_sat_icon(gfx, x, y, d, kind, running);
}

unsafe fn draw_quota_ring(gfx: *mut GpGraphics, rect: RECT, ring: Option<&QuotaRing>, busy: bool) {
    let d = (rect.right - rect.left)
        .min(rect.bottom - rect.top - 12)
        .max(8);
    let x = rect.left;
    let y = rect.top;
    // QuotaBar ProviderRing: 3pt stroke on a 46pt disc ≈ 0.065 of diameter.
    let track_w = (d as f32 * 0.065).max(2.4);
    fill_disc(gfx, x, y, d, COL_PANEL);
    stroke_disc(gfx, x, y, d, COL_TRACK, track_w);

    if busy && ring.is_none() {
        draw_centered_text(
            gfx,
            "…",
            x as f32,
            y as f32,
            d as f32,
            d as f32,
            (d as f32 * 0.28).max(9.0),
            true,
            COL_MUTED,
        );
        draw_centered_text(
            gfx,
            "读取中",
            x as f32,
            (y + d) as f32,
            d as f32,
            12.0,
            9.0,
            false,
            COL_MUTED,
        );
        return;
    }
    let Some(ring) = ring else {
        draw_centered_text(
            gfx,
            "—",
            x as f32,
            y as f32,
            d as f32,
            d as f32,
            (d as f32 * 0.28).max(9.0),
            true,
            COL_MUTED,
        );
        draw_centered_text(
            gfx,
            "引擎未运行",
            x as f32,
            (y + d) as f32,
            d as f32,
            12.0,
            9.0,
            false,
            COL_MUTED,
        );
        return;
    };

    let remaining = ring
        .remaining
        .or_else(|| ring.used.map(|u| (100.0 - u).clamp(0.0, 100.0)));
    let used = ring
        .used
        .or_else(|| remaining.map(|r| (100.0 - r).clamp(0.0, 100.0)));
    if let Some(rem) = remaining {
        let tint = used.map(usage_ramp_argb).unwrap_or(COL_ACCENT);
        let sweep = (rem.clamp(0.0, 100.0) as f32 * 3.6).max(4.3);
        stroke_arc(gfx, x, y, d, tint, track_w, -90.0, sweep);
    }

    let mark = (d as f32 * 0.64).round() as i32;
    let drew = if let Some(key) = logo_key(&ring.provider, &ring.title) {
        draw_png(gfx, key, x + (d - mark) / 2, y + (d - mark) / 2, mark)
    } else {
        false
    };
    if !drew {
        let letter = ring
            .title
            .chars()
            .next()
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_else(|| "?".into());
        draw_centered_text(
            gfx,
            &letter,
            x as f32,
            y as f32,
            d as f32,
            d as f32,
            (d as f32 * 0.36).max(11.0),
            true,
            COL_TEXT,
        );
    }

    let caption = if let Some(rem) = remaining {
        format!("{}", rem.round() as i32)
    } else if let Some(balance) = ring.balance.as_ref().filter(|s| !s.is_empty()) {
        balance.clone()
    } else {
        "—".into()
    };
    draw_centered_text(
        gfx,
        &caption,
        x as f32,
        (y + d) as f32,
        d as f32,
        13.0,
        10.0,
        false,
        COL_MUTED,
    );
}

fn alloc_art(w: i32, h: i32) -> Option<(BallArt, *mut GpGraphics)> {
    if w <= 0 || h <= 0 {
        return None;
    }
    let mut pixels = vec![0u32; (w as usize) * (h as usize)];
    let mut bitmap: *mut GpBitmap = std::ptr::null_mut();
    let stride = w * 4;
    unsafe {
        if GdipCreateBitmapFromScan0(
            w,
            h,
            stride,
            PIXEL_FORMAT_32BPP_ARGB,
            pixels.as_mut_ptr() as *const u8,
            &mut bitmap,
        ) != 0
        {
            return None;
        }
        let mut gfx: *mut GpGraphics = std::ptr::null_mut();
        if GdipGetImageGraphicsContext(bitmap as *mut GpImage, &mut gfx) != 0 {
            GdipDisposeImage(bitmap as *mut GpImage);
            return None;
        }
        GdipSetSmoothingMode(gfx, SmoothingModeAntiAlias);
        Some((BallArt { bitmap, pixels }, gfx))
    }
}

/// Idle / tray glyph — hub disc only.
unsafe fn draw_ball(size: i32, notice: Option<&str>, running: usize) -> Option<BallArt> {
    let (art, gfx) = alloc_art(size, size)?;
    draw_hub_disc(gfx, 0, 0, size, notice, running);
    GdipDeleteGraphics(gfx);
    Some(art)
}

unsafe fn draw_scene(hwnd: HWND) -> Option<BallArt> {
    let layout = compute_layout(hwnd);
    let (view, notice, running, quota, quota_busy) = {
        let paint = shared().paint.lock().ok()?;
        let worst = worst_notice(&paint.notices).map(|s| s.to_string());
        (
            paint.view,
            worst,
            paint.running,
            paint.quota.clone(),
            paint.quota_busy,
        )
    };
    let (art, gfx) = alloc_art(layout.win_w, layout.win_h)?;
    draw_hub_disc(
        gfx,
        layout.hub_x,
        layout.hub_y,
        layout.ball,
        notice.as_deref(),
        running,
    );
    match view {
        OrbView::Idle => {}
        OrbView::Fan => {
            for sat in &layout.sats {
                draw_sat_disc(gfx, sat.x, sat.y, layout.sat, sat.kind, running);
            }
        }
        OrbView::Quota => {
            if layout.rings.is_empty() {
                // keep hub only
            } else if quota.is_empty() {
                draw_quota_ring(gfx, layout.rings[0], None, quota_busy);
            } else {
                for (i, rect) in layout.rings.iter().enumerate() {
                    draw_quota_ring(gfx, *rect, quota.get(i), false);
                }
            }
        }
    }
    GdipDeleteGraphics(gfx);
    Some(art)
}

fn premultiply(pixels: &mut [u32]) {
    for pixel in pixels.iter_mut() {
        let argb = *pixel;
        let a = (argb >> 24) & 0xFF;
        if a == 0 {
            *pixel = 0;
            continue;
        }
        if a == 0xFF {
            continue;
        }
        let r = ((argb >> 16) & 0xFF) * a / 255;
        let g = ((argb >> 8) & 0xFF) * a / 255;
        let b = (argb & 0xFF) * a / 255;
        *pixel = (a << 24) | (r << 16) | (g << 8) | b;
    }
}

unsafe fn paint_ball(hwnd: HWND) {
    let layout = compute_layout(hwnd);
    let Some(art) = draw_scene(hwnd) else {
        return;
    };
    let w = layout.win_w;
    let h = layout.win_h;

    unsafe {
        let hdc_screen = GetDC(std::ptr::null_mut());
        let hdc_mem = CreateCompatibleDC(hdc_screen);
        let mut header = std::mem::zeroed::<BITMAPINFOHEADER>();
        header.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        header.biWidth = w;
        header.biHeight = -h; // top-down
        header.biPlanes = 1;
        header.biBitCount = 32;
        header.biCompression = BI_RGB;
        let mut info: BITMAPINFO = std::mem::zeroed();
        info.bmiHeader = header;
        let mut bits = std::ptr::null_mut();
        let dib = CreateDIBSection(
            hdc_mem,
            &info,
            DIB_RGB_COLORS,
            &mut bits,
            std::ptr::null_mut(),
            0,
        );
        if dib.is_null() || bits.is_null() {
            DeleteDC(hdc_mem);
            ReleaseDC(std::ptr::null_mut(), hdc_screen);
            GdipDisposeImage(art.bitmap as *mut GpImage);
            return;
        }
        let mut premut = art.pixels.clone();
        premultiply(&mut premut);
        std::ptr::copy_nonoverlapping(premut.as_ptr(), bits as *mut u32, premut.len());
        let old = SelectObject(hdc_mem, dib);
        let size_sz = SIZE { cx: w, cy: h };
        let src = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        UpdateLayeredWindow(
            hwnd,
            hdc_screen,
            std::ptr::null_mut(),
            &size_sz,
            hdc_mem,
            &src,
            0,
            &blend,
            ULW_ALPHA,
        );
        SelectObject(hdc_mem, old);
        DeleteObject(dib);
        DeleteDC(hdc_mem);
        ReleaseDC(std::ptr::null_mut(), hdc_screen);
        GdipDisposeImage(art.bitmap as *mut GpImage);
    }
}

fn worst_notice(notices: &[NoticeSummary]) -> Option<&str> {
    let mut best: Option<&NoticeSummary> = None;
    for notice in notices {
        let rank = |kind: &str| if kind == "error" { 2 } else if kind == "permission" { 1 } else { 0 };
        let better = match best {
            None => true,
            Some(current) => rank(&notice.kind) > rank(&current.kind),
        };
        if better {
            best = Some(notice);
        }
    }
    best.map(|notice| notice.kind.as_str())
}

fn tray_icon_from_art() -> Option<HICON> {
    unsafe {
        let size = GetSystemMetrics(SM_CXSMICON).max(24);
        let art = draw_ball(size, None, 0)?;
        let mut hicon = std::ptr::null_mut();
        let status = GdipCreateHICONFromBitmap(art.bitmap, &mut hicon);
        GdipDisposeImage(art.bitmap as *mut GpImage);
        if status == 0 && !hicon.is_null() {
            Some(hicon)
        } else {
            None
        }
    }
}

// ── orb state listener (payload mirrors OrbState in src/lib/orbEvents.ts) ──

fn parse_notices(value: &Value) -> Vec<NoticeSummary> {
    let mut out = Vec::new();
    let Some(entries) = value.get("notices").and_then(Value::as_array) else {
        return out;
    };
    for entry in entries.iter().take(20) {
        let kind = entry
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("reply")
            .to_string();
        let label = entry
            .get("label")
            .and_then(Value::as_str)
            .unwrap_or("Session")
            .to_string();
        let at = entry.get("at").and_then(Value::as_u64).unwrap_or(0);
        out.push(NoticeSummary { kind, label, at });
    }
    out
}

fn install_state_listener(app: &AppHandle) {
    use tauri::Listener;
    let app = app.clone();
    app.listen("marionette-orb-state", move |event| {
        let Ok(value) = serde_json::from_str::<Value>(event.payload()) else {
            return;
        };
        let main_visible = value
            .get("mainVisible")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let running_entries = value.get("running").and_then(Value::as_array);
        let running = running_entries.map(|entries| entries.len()).unwrap_or(0);
        let running_session_id = running_entries.and_then(|entries| {
            entries
                .first()
                .and_then(|e| e.get("sessionId"))
                .and_then(Value::as_str)
                .map(|s| s.to_string())
        });
        let notices = parse_notices(&value);

        let balloon = {
            let Ok(mut paint) = shared().paint.lock() else {
                return;
            };
            paint.main_visible = main_visible;
            paint.running = running;
            paint.running_session_id = running_session_id;
            paint.notices = notices.clone();
            let mut message = None;
            if !main_visible {
                if let Some(newest) = notices.iter().max_by_key(|notice| notice.at) {
                    if newest.at > paint.last_balloon_at && newest.at > 0 {
                        paint.last_balloon_at = newest.at;
                        let prefix = match newest.kind.as_str() {
                            "error" => "出错了",
                            "permission" => "需要授权",
                            _ => "任务完成",
                        };
                        message = Some(format!("{prefix}：{}", newest.label));
                    }
                }
            }
            message
        };

        if let Some(message) = balloon {
            queue_balloon(message);
        }
        let hwnd = ball_hwnd();
        if !hwnd.is_null() {
            let view = shared()
                .paint
                .lock()
                .map(|p| p.view)
                .unwrap_or(OrbView::Idle);
            unsafe {
                PostMessageW(
                    hwnd,
                    if view == OrbView::Idle {
                        WM_ORB_REPAINT
                    } else {
                        WM_ORB_LAYOUT
                    },
                    0,
                    0,
                );
            }
        }
    });
}

// ── window procedure ───────────────────────────────────────────────────────

unsafe extern "system" fn orb_wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            ValidateRect(hwnd, std::ptr::null());
            paint_ball(hwnd);
            0
        }
        WM_ERASEBKGND => 1,
        WM_LBUTTONDOWN => {
            let (cx, cy) = lparam_point(lparam);
            match hit_test(hwnd, cx, cy) {
                Hit::Hub => {
                    // System move loop; a click that didn't move is expand/collapse.
                    let mut before = RECT {
                        left: 0,
                        top: 0,
                        right: 0,
                        bottom: 0,
                    };
                    GetWindowRect(hwnd, &mut before);
                    SendMessageW(hwnd, WM_NCLBUTTONDOWN, HTCAPTION, 0);
                    let mut after = RECT {
                        left: 0,
                        top: 0,
                        right: 0,
                        bottom: 0,
                    };
                    GetWindowRect(hwnd, &mut after);
                    let dx = after.left - before.left;
                    let dy = after.top - before.top;
                    if dx != 0 || dy != 0 {
                        let hub = hub_screen();
                        set_hub_screen(POINT {
                            x: hub.x + dx,
                            y: hub.y + dy,
                        });
                    } else {
                        let view = shared()
                            .paint
                            .lock()
                            .map(|p| p.view)
                            .unwrap_or(OrbView::Idle);
                        match view {
                            OrbView::Idle => set_view(hwnd, OrbView::Fan),
                            OrbView::Fan | OrbView::Quota => set_view(hwnd, OrbView::Idle),
                        }
                    }
                }
                Hit::Sat(kind) => activate_sat(hwnd, kind),
                Hit::Ring => {}
                Hit::Empty => {
                    let view = shared()
                        .paint
                        .lock()
                        .map(|p| p.view)
                        .unwrap_or(OrbView::Idle);
                    if view != OrbView::Idle {
                        set_view(hwnd, OrbView::Idle);
                    }
                }
            }
            0
        }
        WM_RBUTTONUP => {
            show_menu(hwnd);
            0
        }
        WM_ORB_REPAINT => {
            paint_ball(hwnd);
            0
        }
        WM_ORB_LAYOUT | WM_ORB_QUOTA => {
            apply_layout(hwnd);
            0
        }
        WM_ACTIVATE => {
            // Collapse when the user clicks elsewhere. Right-click menu also
            // deactivates — collapsing to idle before the menu is fine.
            if (wparam & 0xFFFF) == 0 {
                let view = shared()
                    .paint
                    .lock()
                    .map(|p| p.view)
                    .unwrap_or(OrbView::Idle);
                if view != OrbView::Idle {
                    set_view(hwnd, OrbView::Idle);
                }
            }
            0
        }
        WM_ORB_BALLOON => {
            if let Ok(mut queue) = BALLOON_QUEUE.lock() {
                for message in queue.drain(..) {
                    tray_balloon(hwnd, &message);
                }
            }
            0
        }
        WM_ORB_TRAY => {
            let mouse = (lparam & 0xFFFF) as u32;
            if mouse == WM_LBUTTONUP_ {
                emit_show_main();
            } else if mouse == WM_RBUTTONUP_ {
                show_menu(hwnd);
            }
            0
        }
        WM_DROPFILES => {
            let drop = wparam as *mut core::ffi::c_void;
            let count = DragQueryFileW(drop, 0xFFFF_FFFF, std::ptr::null_mut(), 0);
            let mut files = Vec::new();
            for index in 0..count {
                let mut buffer = [0u16; 1024];
                let len = DragQueryFileW(drop, index, buffer.as_mut_ptr(), buffer.len() as u32);
                if len > 0 {
                    files.push(String::from_utf16_lossy(&buffer[..len as usize]));
                }
            }
            DragFinish(drop);
            if !files.is_empty() {
                let text = files
                    .iter()
                    .map(|file| format!("[文件] {file}"))
                    .collect::<Vec<_>>()
                    .join("\n");
                emit_capture(text, Vec::new(), files);
                queue_balloon("已送入收集箱 ✓".into());
            }
            0
        }
        WM_DPICHANGED => {
            apply_layout(hwnd);
            0
        }
        WM_CLOSE => 0, // hide-only rule: never destroy the ball
        WM_DESTROY => {
            tray_remove(hwnd);
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

// ── entry point ────────────────────────────────────────────────────────────

/// Spawn the orb thread. Called once from tauri setup (main.rs).
pub fn start(app: AppHandle) {
    // Seed the shared state with the real app handle before anything uses it.
    if SHARED.get().is_none() {
        let _ = SHARED.set(Shared {
            app: app.clone(),
            paint: Mutex::new(PaintState::default()),
            hwnd: AtomicIsize::new(0),
            tray_icon: AtomicIsize::new(0),
            hub_x: AtomicI32::new(0),
            hub_y: AtomicI32::new(0),
        });
    }
    std::thread::Builder::new()
        .name("orb-native".into())
        .spawn(move || {
            if let Err(err) = run(app) {
                debug("error", &format!("native orb failed: {err}"));
            }
        })
        .ok();
}

fn run(app: AppHandle) -> Result<(), String> {
    unsafe {
        let mut token = 0usize;
        let input = GdiplusStartupInput {
            GdiplusVersion: 1,
            DebugEventCallback: 0,
            SuppressBackgroundThread: 0,
            SuppressExternalCodecs: 0,
        };
        if GdiplusStartup(&mut token, &input, std::ptr::null_mut()) != 0 {
            return Err("GdiplusStartup failed".into());
        }

        let instance = GetModuleHandleW(std::ptr::null());
        let class_name = wide(CLASS_NAME);
        let class = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(orb_wnd_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: instance,
            hIcon: std::ptr::null_mut(),
            hCursor: LoadCursorW(std::ptr::null_mut(), IDC_HAND as *const u16),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: class_name.as_ptr(),
        };
        if RegisterClassW(&class) == 0 {
            GdiplusShutdown(token);
            return Err("RegisterClassW failed".into());
        }

        // No probe window: a temporary 0x0 window would hit WM_DESTROY on this
        // very thread and PostQuitMessage would kill the loop below before it
        // starts. Create at logical size, then rescale from the real DPI.
        let pos = load_ball_pos(BALL_LOGICAL);
        shared().hub_x.store(pos.x, Ordering::Relaxed);
        shared().hub_y.store(pos.y, Ordering::Relaxed);
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            class_name.as_ptr(),
            wide("Marionette Orb").as_ptr(),
            WS_POPUP,
            pos.x,
            pos.y,
            BALL_LOGICAL,
            BALL_LOGICAL,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            std::ptr::null_mut(),
        );
        if hwnd.is_null() {
            GdiplusShutdown(token);
            return Err("CreateWindowExW failed".into());
        }
        let size = ball_size_physical(hwnd);
        if size != BALL_LOGICAL {
            SetWindowPos(
                hwnd,
                std::ptr::null_mut(),
                pos.x,
                pos.y,
                size,
                size,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
        shared().hwnd.store(hwnd as isize, Ordering::Relaxed);

        DragAcceptFiles(hwnd, 1);
        paint_ball(hwnd);
        ShowWindow(hwnd, SW_SHOWNOACTIVATE);

        if let Some(hicon) = tray_icon_from_art() {
            shared().tray_icon.store(hicon as isize, Ordering::Relaxed);
            tray_add(hwnd, hicon);
        }

        install_state_listener(&app);

        let mut message = std::mem::zeroed::<MSG>();
        loop {
            let result = GetMessageW(&mut message, std::ptr::null_mut(), 0, 0);
            if result <= 0 {
                break;
            }
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }

        tray_remove(hwnd);
        GdiplusShutdown(token);
        Ok(())
    }
}
