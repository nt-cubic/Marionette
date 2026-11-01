// Built-in browser tabs (batch 2): thin typed wrappers over the Rust
// commands and the `marionette-browser-tabs` event. The backend owns the tab
// list — every mutation emits the full list and this module just mirrors it.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { isTauriRuntime } from "./api";

export type BrowserTab = {
  id: string;
  url: string;
  title: string;
  origin: string;
  grant: "none" | "read" | "control";
  openedByAgent: boolean;
};

export type BrowserPermissionRequest = {
  requestId: string;
  sessionId: string;
  tabId: string;
  url: string;
  origin: string;
  summary: string;
};

export async function browserOpenTab(url: string): Promise<{ tabId: string } | null> {
  if (!isTauriRuntime()) return null;
  return invoke<{ tabId: string }>("browser_open_tab", { url });
}

export async function browserCloseTab(tabId: string): Promise<void> {
  if (!isTauriRuntime()) return;
  await invoke("browser_close_tab", { tabId });
}

export async function browserNavigateTab(tabId: string, url: string): Promise<void> {
  if (!isTauriRuntime()) return;
  await invoke("browser_navigate_tab", { tabId, url });
}

export async function browserSetTabBounds(
  tabId: string,
  x: number,
  y: number,
  width: number,
  height: number,
): Promise<void> {
  if (!isTauriRuntime() || width <= 0 || height <= 0) return;
  await invoke("browser_set_tab_bounds", { tabId, x, y, width, height });
}

export async function browserSetTabVisible(tabId: string, visible: boolean): Promise<void> {
  if (!isTauriRuntime()) return;
  await invoke("browser_set_tab_visible", { tabId, visible });
}

export async function browserListTabs(): Promise<BrowserTab[]> {
  if (!isTauriRuntime()) return [];
  try {
    return await invoke<BrowserTab[]>("browser_list_tabs");
  } catch {
    return [];
  }
}

export async function browserShareTab(tabId: string, level: "none" | "read" | "control"): Promise<void> {
  if (!isTauriRuntime()) return;
  await invoke("browser_share_tab", { tabId, level });
}

export async function browserPermissionRespond(requestId: string, allow: boolean): Promise<void> {
  if (!isTauriRuntime()) return;
  await invoke("browser_permission_respond", { requestId, allow });
}

export function onBrowserTabs(handler: (tabs: BrowserTab[]) => void): Promise<UnlistenFn> {
  return listen<BrowserTab[]>("marionette-browser-tabs", (event) => handler(event.payload));
}

export function onBrowserPermission(
  handler: (request: BrowserPermissionRequest) => void,
): Promise<UnlistenFn> {
  return listen<BrowserPermissionRequest>("marionette-browser-permission", (event) =>
    handler(event.payload),
  );
}

// ── Session approval mirror ────────────────────────────────────────────────
// "Always approve" lives in the Composer's per-session state; the Rust tool
// channel needs it to decide between a permission card and an auto-grant.
// The module-level map remembers every session's mode (composers mount only
// for the active one) and App re-pushes the map after engine restarts.

const approvalBySession = new Map<string, boolean>();

export async function setSessionApproval(sessionId: string, alwaysApprove: boolean): Promise<void> {
  approvalBySession.set(sessionId, alwaysApprove);
  if (!isTauriRuntime()) return;
  try {
    await invoke("set_session_approval", { sessionId, alwaysApprove });
  } catch {
    // Older backend in dev — the map still tracks it for the next push.
  }
}

export async function pushAllSessionApprovals(): Promise<void> {
  for (const [sessionId, alwaysApprove] of approvalBySession) {
    await setSessionApproval(sessionId, alwaysApprove);
  }
}
