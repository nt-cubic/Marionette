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
  display: "background" | "workspace";
  ownerSessionId: string | null;
  chosenId: string | null;
  pinned: boolean;
  busy: boolean;
  status: "idle" | "running" | "waiting_user" | "failed";
  lastUsedAtMs: number;
};

export type BrowserPermissionRequest = {
  requestId: string;
  sessionId: string;
  tabId: string;
  url: string;
  origin: string;
  summary: string;
};

export type BrowserHandoffRequest = {
  requestId: string;
  sessionId: string;
  tabId: string;
  title: string;
  message: string;
  url: string;
  origin: string;
  expiresAtMs: number;
};

export async function browserOpenTab(
  url: string,
  display: "background" | "workspace" = "background",
  chosenId?: string,
): Promise<{ tabId: string; existed?: boolean } | null> {
  if (!isTauriRuntime()) return null;
  return invoke<{ tabId: string; existed?: boolean }>("browser_open_tab", {
    url,
    display,
    chosenId: chosenId ?? null,
  });
}

export async function browserCloseTab(tabId: string): Promise<void> {
  if (!isTauriRuntime()) return;
  await invoke("browser_close_tab", { tabId });
}

export async function browserCloseTabsForSession(sessionId: string): Promise<number> {
  if (!isTauriRuntime()) return 0;
  return invoke<number>("browser_close_tabs_for_session", { sessionId });
}

export async function browserSetTabPinned(tabId: string, pinned: boolean): Promise<void> {
  if (!isTauriRuntime()) return;
  await invoke("browser_set_tab_pinned", { tabId, pinned });
}

export async function browserClaimTab(tabId: string): Promise<void> {
  if (!isTauriRuntime()) return;
  await invoke("browser_claim_tab", { tabId });
}

export async function browserTouchTab(tabId: string): Promise<void> {
  if (!isTauriRuntime()) return;
  await invoke("browser_touch_tab", { tabId });
}

export async function browserCleanupIdleTabs(
  minutes: number,
  keepTabIds: readonly string[],
): Promise<string[]> {
  if (!isTauriRuntime() || minutes <= 0) return [];
  return invoke<string[]>("browser_cleanup_idle_tabs", {
    minutes,
    keepTabIds,
  });
}

export async function browserProbeLoopbackUrl(url: string): Promise<boolean> {
  if (!isTauriRuntime()) return false;
  try {
    return await invoke<boolean>("browser_probe_loopback_url", { url });
  } catch {
    return false;
  }
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

// Keep rapid overlay/tab changes in order even when IPC calls complete out of order.
const visibilityQueues = new Map<string, Promise<void>>();

export async function browserSetTabVisible(tabId: string, visible: boolean): Promise<void> {
  if (!isTauriRuntime()) return;
  const previous = visibilityQueues.get(tabId) ?? Promise.resolve();
  const next = previous.catch(() => undefined).then(() => invoke<void>("browser_set_tab_visible", { tabId, visible }));
  visibilityQueues.set(tabId, next);
  try {
    await next;
  } finally {
    if (visibilityQueues.get(tabId) === next) visibilityQueues.delete(tabId);
  }
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

export async function browserHandoffRespond(
  requestId: string,
  outcome: "continue" | "denied",
): Promise<boolean> {
  if (!isTauriRuntime()) return false;
  return invoke<boolean>("browser_handoff_respond", { requestId, outcome });
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

export function onBrowserHandoff(
  handler: (request: BrowserHandoffRequest) => void,
): Promise<UnlistenFn> {
  return listen<BrowserHandoffRequest>("marionette-browser-handoff", (event) =>
    handler(event.payload),
  );
}

// ── Element picker (annotate mode) ──────────────────────────────────────────
// The toolbar's crosshair turns the page into a DevTools-style picker; the
// page reports picks over the `marionette-browser-pick` event and everything
// else (crop the screenshot, resolve the aria refs) is frontend-driven
// commands — the Rust event path never issues CDP calls.

export type BrowserPickRect = { x: number; y: number; width: number; height: number };

/** What the page says about one picked element (js/picker.js, capped there). */
export type BrowserPickInfo = {
  tag: string;
  id: string;
  classes: string[];
  text: string;
  rect: BrowserPickRect;
  dpr: number;
  selector: string;
  href?: string;
  placeholder?: string;
};

export type BrowserPickMessage =
  | { kind: "pick"; pickId: string; info: BrowserPickInfo; url?: string }
  | {
      kind: "region";
      pickId: string;
      rect: BrowserPickRect;
      dpr: number;
      elements: BrowserPickInfo[];
      scanCapped?: true;
      url?: string;
    }
  | { kind: "cancel" }
  | { kind: "limit"; limit: number };

export type BrowserPickEvent = { tabId: string; message: BrowserPickMessage };

export type BrowserPickMode = "element" | "region";

/** One resolved aria ref (`null` = the pick has none usable right now). */
export type BrowserPickRef = { ref: string; role: string; name: string } | null;

export async function browserPickSet(
  tabId: string,
  active: boolean,
  mode: BrowserPickMode,
): Promise<{ ok: boolean; mode?: BrowserPickMode; error?: string }> {
  if (!isTauriRuntime()) return { ok: false, error: "desktop runtime required" };
  try {
    return await invoke("browser_pick_set", { tabId, active, mode });
  } catch (error) {
    return { ok: false, error: String(error) };
  }
}

export async function browserPickClear(tabId: string): Promise<boolean> {
  if (!isTauriRuntime()) return false;
  try {
    await invoke("browser_pick_clear", { tabId });
    return true;
  } catch {
    return false;
  }
}

export async function browserPickFinalize(
  tabId: string,
  pickId: string,
): Promise<{
  pickId: string;
  /** Small drawer preview. The full JPEG is already written to `path`. */
  dataUrl: string;
  path: string;
  mimeType: string;
  width: number;
  height: number;
} | null> {
  if (!isTauriRuntime()) return null;
  try {
    return await invoke("browser_pick_finalize", { tabId, pickId });
  } catch {
    return null;
  }
}

export async function browserPickResolve(
  tabId: string,
): Promise<{ generation: string; refs: Record<string, BrowserPickRef> } | null> {
  if (!isTauriRuntime()) return null;
  try {
    return await invoke("browser_pick_resolve", { tabId });
  } catch {
    return null;
  }
}

export function onBrowserPick(
  handler: (event: BrowserPickEvent) => void,
): Promise<UnlistenFn> {
  return listen<BrowserPickEvent>("marionette-browser-pick", (event) => handler(event.payload));
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
