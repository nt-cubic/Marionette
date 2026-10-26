# Marionette patch on wry 0.55.1

Vendored from crates.io `wry 0.55.1` with **one** behavior change:

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
