/**
 * Native (Explorer) drop routing for the docked strip — 2026-10-26.
 *
 * While the strip is docked, App-level routing owns every OS file drop
 * (components/DockDropRouter.tsx): the top band means 收纳盒, the
 * conversation below means the composer. The composer's own Tauri drop
 * handler is switched off while docked, so chat-zone paths arrive here as a
 * DOM CustomEvent and reuse the composer's normal insert path. In-page memo
 * drags never cross the OLE boundary and stay pure HTML5 (lib/memos.ts).
 *
 * Text drags (browser selections/links, vendor wry patch) ride the same
 * zones: chat-zone text goes to the composer as a DOCK_DROP_TEXT_EVENT.
 */

export const DOCK_DROP_PATHS_EVENT = "marionette:dock-drop-paths";
export const DOCK_DROP_TEXT_EVENT = "marionette:dock-drop-text";

/** Hand chat-zone drop paths to the mounted composer (no-op if none). */
export function dispatchDockDropPaths(paths: string[]): void {
  window.dispatchEvent(new CustomEvent(DOCK_DROP_PATHS_EVENT, { detail: paths }));
}

/** Hand chat-zone dropped text to the mounted composer (no-op if none). */
export function dispatchDockDropText(text: string): void {
  window.dispatchEvent(new CustomEvent(DOCK_DROP_TEXT_EVENT, { detail: text }));
}
