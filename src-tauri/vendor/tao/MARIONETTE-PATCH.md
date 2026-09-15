# Marionette patch on tao 0.35.3

Vendored from crates.io `tao 0.35.3` with **one** behavior change:

`src/platform_impl/windows/event_loop.rs` — the thread-event-target
`WM_PAINT` handler used to `assert!(flush_paint_messages(..))`.
`flush_paint_messages` is documented to return `false` when re-entrant;
the assert assumed that never happens.

It does happen when a CJK IME (Chinese IME on a Japanese Windows locale
is the worst case) pumps nested messages on window focus / click. Release
builds use `panic = "abort"`, so the assert killed Marionette.

The patch treats a `false` return as "already flushing, skip this nested
paint" instead of aborting.

Drop this vendor (and the `[patch.crates-io]` entry) when Tauri ships
tao ≥ 0.36, which removed window subclassing and the assert (tao#1231).
