// Marionette patch (not upstream wry).
//
// Windows OLE drags that carry no CF_HDROP (selected text, links, images
// dragged from a browser) are silently dropped by the stock drag-drop target:
// `DragEnter` bails out before emitting anything, and WebView2's HTML5
// external drop is disabled while the OLE target is registered — so text
// drags never reach the page in any form.
//
// This module exposes a hook through which the embedding app receives those
// drags as text, alongside (not instead of) the normal file-drop events.
// See vendor/wry/MARIONETTE-PATCH.md.

use std::sync::atomic::{AtomicUsize, Ordering};

/// `kind` values passed to [`TextDragHook`].
pub const KIND_ENTER: u8 = 0;
pub const KIND_OVER: u8 = 1;
pub const KIND_DROP: u8 = 2;
pub const KIND_LEAVE: u8 = 3;

/// Called on the window thread that owns the drag target.
/// `hwnd` identifies the target window (cast from `HWND.0`), `x`/`y` are the
/// cursor in physical client pixels — same coordinate space as
/// `DragDropEvent::Over`.
pub type TextDragHook = fn(kind: u8, text: &str, x: f64, y: f64, hwnd: isize);

static HOOK: AtomicUsize = AtomicUsize::new(0);

pub fn set_text_drag_hook(hook: TextDragHook) {
  HOOK.store(hook as usize, Ordering::SeqCst);
}

/// Returns false when no hook is installed (caller falls back to stock behavior).
pub(crate) fn emit_text_drag(kind: u8, text: &str, x: f64, y: f64, hwnd: isize) -> bool {
  let ptr = HOOK.load(Ordering::SeqCst);
  if ptr == 0 {
    return false;
  }
  let hook: TextDragHook = unsafe { std::mem::transmute(ptr) };
  hook(kind, text, x, y, hwnd);
  true
}
