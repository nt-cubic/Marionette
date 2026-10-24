/**
 * The strip's bottom bar (edge dock): a separate row *below* the composer that
 * carries the buttons normally living at the bottom of the two sidebars.
 *
 * Docked, both rails become full-page overlays, so their footers are out of
 * reach. Rather than duplicating the proxy / Telegram / theme logic, the same
 * React subtrees are portaled into one body-level bar:
 *
 *   ProjectShelf  → `.sidebar-dock` (模型共享 entry + its footer icons)
 *   ContextPanel  → the update button (the right rail's only other control)
 *
 * A body-level host keeps the bar out of the transformed rail overlays (a
 * `position: fixed` popover inside a `translateX(100%)` rail would be parked
 * off-screen with it). CSS hides the host unless `.dock-mode` is on.
 */

import { useSyncExternalStore } from "react";
import { getDockState, subscribeDockState } from "./dockMode";

/** Re-render when the window docks/undocks (the state machine owns the truth). */
export function useDocked(): boolean {
  return useSyncExternalStore(
    subscribeDockState,
    () => getDockState() !== "floating",
    () => false,
  );
}

let host: HTMLDivElement | null = null;

/** Lazily created, stable, outside #root — React portals need it to exist. */
export function dockFooterHost(): HTMLElement {
  if (!host || !host.isConnected) {
    host = document.createElement("div");
    host.id = "dock-footer";
    host.className = "dock-footer";
    host.setAttribute("role", "toolbar");
    host.setAttribute("aria-label", "侧栏按钮");
    document.body.appendChild(host);
  }
  return host;
}
