/**
 * Text/link drag-in bridge (vendor wry patch → Rust drag_text.rs).
 *
 * OLE drags without CF_HDROP (browser selections, links, image URLs) never
 * reach the page through the stock event stream: wry's DragEnter bails before
 * emitting anything and WebView2's HTML5 external drop is disabled while the
 * OLE target is registered. The vendored wry reads CF_UNICODETEXT instead and
 * the app forwards it as `marionette-drag-text` (routed to the window the
 * cursor is over). File drags keep using `onDragDropEvent` paths — untouched.
 *
 * One wrinkle both channels share: in-page HTML5 drags (memo rows onto the
 * composer) also reach the OLE target with text (the row sets text/plain).
 * Those must stay on the pure HTML5 path, so the app installs a drag-source
 * tracker and every drag-text listener bails while the source is this page.
 */

export type DragTextKind = "enter" | "over" | "drop" | "leave";

/** x/y are physical client pixels, same space as onDragDropEvent positions. */
export type DragTextEvent = { kind: DragTextKind; text: string; x: number; y: number };

let pageDragSource = false;

/**
 * Marks drags that START inside this page (HTML5). Call once from App.
 * `dragend` fires on the source element after any outcome (drop, Esc, leave);
 * `drop` is belt-and-braces for sources removed mid-drag.
 */
export function installPageDragSourceTracker(): () => void {
  if (typeof window === "undefined") return () => undefined;
  const onStart = () => {
    pageDragSource = true;
  };
  const onEnd = () => {
    pageDragSource = false;
  };
  window.addEventListener("dragstart", onStart);
  window.addEventListener("dragend", onEnd);
  window.addEventListener("drop", onEnd);
  return () => {
    window.removeEventListener("dragstart", onStart);
    window.removeEventListener("dragend", onEnd);
    window.removeEventListener("drop", onEnd);
    pageDragSource = false;
  };
}

export function isPageDragSource(): boolean {
  return pageDragSource;
}

/** Subscribe to text drags over THIS window. Resolve to an unsubscribe fn. */
export async function onDragText(cb: (event: DragTextEvent) => void): Promise<() => void> {
  const { listen } = await import("@tauri-apps/api/event");
  return listen<DragTextEvent>("marionette-drag-text", (event) => cb(event.payload));
}
