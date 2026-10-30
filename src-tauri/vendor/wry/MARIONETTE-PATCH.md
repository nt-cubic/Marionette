# Marionette patch on wry 0.55.1

Vendored from crates.io `wry 0.55.1` with **four** behavior changes:

## 1. Text/link OLE drag fallback (`src/webview2/drag_drop.rs` + `src/marionette_drag.rs`)

`src/webview2/drag_drop.rs` — while the OLE drop target is registered,
WebView2's HTML5 external drop is disabled (`SetAllowExternalDrop(false)`),
so the page can only receive drags through OLE. But the stock `DragEnter`
calls `GetData(CF_HDROP)` first and bails out without emitting anything when
the drag carries no files. Result: drags of **text** — browser selections,
links, image URLs — never reach the page in any form (no event, no HTML5
fallback, cursor aside).

The patch adds a text fallback on that path: when a drag has no `CF_HDROP`
but offers `CF_UNICODETEXT`, the payload is read once at `DragEnter`
(capped at 1M chars) and forwarded through a process-wide hook
(`src/marionette_drag.rs`, installed via `wry::marionette_drag::set_text_drag_hook`),
with `enter` / `over` / `drop` / `leave` semantics, the target `HWND`, and
the cursor in the same physical client-pixel space as `DragDropEvent`.
Marionette's `src-tauri/src/drag_text.rs` turns the hook into the
`marionette-drag-text` Tauri event, routed to the window that owns the HWND.

Details that matter:

- File drags (`CF_HDROP` present) take the stock path **unchanged** — the
  normal `DragDropEvent` stream and its `paths` payloads are untouched; the
  extra `Win32_System_Memory` feature only feeds the text reader
  (`GlobalLock`/`GlobalSize`).
- The text is captured at `DragEnter` and reused for `Drop` (OLE data
  objects are immutable snapshots; `Drop` re-reads nothing).
- No hook installed → stock behavior exactly.
- Drags that offer neither files nor text (or render text lazily past the
  Enter query) behave as before.
- In-page HTML5 drags that WebView2 forwards to the host target do carry
  `CF_UNICODETEXT` — Marionette's frontend filters those out with a
  drag-source tracker (`src/lib/dragText.ts`), keeping memo-row drags on
  the pure HTML5 path.

Drop this vendor (and the `[patch.crates-io]` entry) when upstream wry/tauri
ships drag-drop events that carry text payloads.

## 2. Memory-usage target Low at creation (`src/webview2/mod.rs`, new_in_hwnd)

Every webview is created with `SetMemoryUsageTargetLevel(1)` (Low) —
WebView2 trims renderer caches harder (measured ~20-40% less webview
memory), designed for webviews that idle most of their life. Escape
hatch: `MARIONETTE_WEBVIEW_MEM=NORMAL` in the environment restores the
stock default. The stock runtime API `WebView::set_memory_usage_level`
is untouched.

## 3. Hidden webviews suspend (`src/webview2/mod.rs`, set_visible)

`set_visible(false)` additionally calls `ICoreWebView2_3::TrySuspend` —
a hidden WebView2 frees most of its renderer memory (measured
~150-250MB while Marionette sits in background mode). `set_visible(true)`
calls `Resume()`. Both arms live in the same function, so a suspension
can never outlive the next show. `TrySuspend` quietly fails when the
controller is still visible — harmless.

Tauri's window-level `hide()`/`show()` do NOT reach the webview-level
`set_visible`, so Marionette pairs every window hide/show with a
webview-level hide/show:

- frontend: `lib/detachedWindow.ts` (`withDetachedWebview`), `lib/dockMode.ts`
  `handleCloseFromRust` fallback, `app/App.tsx` merge-back show
- Rust: `main.rs` detached-window close, `shell_integration.rs`
  second-instance handoff wake (`win.as_ref().show()` before `win.show()`)

capabilities gained `core:webview:allow-webview-show` / `allow-webview-hide`
for the JS `Webview.hide()/show()` calls.

## 4. focus() defensively resumes (`src/webview2/mod.rs`, focus)

`focus()` first sets the controller visible and calls `Resume()` before
`MoveFocus` — a wake path that shows the window without the paired
webview-level show would otherwise render blank. No-ops when nothing is
suspended. Belt-and-braces for path 3.

Known interaction: Marionette's detached-shell reaper navigates hidden
shells to `about:blank` after a grace period; a suspended shell can't run
that navigation, the best-effort reaper skips it, and the suspended shell
has already freed more memory than the blanking would — resume-on-show
brings the full SPA back instead of a reload.
