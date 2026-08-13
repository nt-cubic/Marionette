/**
 * Pop a dialog into its own Tauri window for multi-monitor / OS snap layouts.
 *
 * Secondary windows load the same SPA with `?detached=<sessionId>`. The main
 * window keeps the shared ACP backend; closing main still shuts the process.
 *
 * Drag tear-off uses a lightweight DOM "ghost" chip; the real WebView is only
 * created on mouse-up (see SessionTabs).
 *
 * Close on secondary windows only *hides* them (Windows tao WebView teardown
 * panics). Hidden shells are reaped: excess ones are navigated to about:blank
 * to drop the React tree; reopening reloads the detached URL.
 */

import { invoke } from "@tauri-apps/api/core";
import { LogicalPosition } from "@tauri-apps/api/dpi";
import { emit, listen, type UnlistenFn } from "@tauri-apps/api/event";
import { cursorPosition, getCurrentWindow } from "@tauri-apps/api/window";
import { WebviewWindow, getAllWebviewWindows } from "@tauri-apps/api/webviewWindow";
import { isTauriRuntime } from "./api";
import type { Session } from "./types";

export type DetachWindowOpts = {
  /** Open near the OS cursor (drag drop / optional click). */
  atCursor?: boolean;
};

/** Cursor offset into the new window so the titlebar sits under the drop point. */
const GRAB_OFFSET_X = 72;
const GRAB_OFFSET_Y = 14;

/**
 * Hidden shells stay fully warm for this long (instant re-open).
 * After that, only MAX_HIDDEN_DETACHED aged shells keep their SPA loaded;
 * older excess go to about:blank (host stays — no tao destroy).
 */
export const HIDDEN_DETACHED_GRACE_MS = 10 * 60 * 1000;
/** How many *aged* (past grace) hidden shells may keep a full SPA resident. */
export const MAX_HIDDEN_DETACHED = 2;

const DETACHED_HIDDEN_EVENT = "marionette-detached-hidden";

/** Detached window asks the main window to re-adopt its dialog tab. */
export const MERGE_BACK_EVENT = "marionette-merge-back";
/** Detached drag hovering over main → main lights up its tab strip. */
export const MERGE_HIGHLIGHT_EVENT = "marionette-merge-highlight";

export type MergeBackPayload = {
  sessionId: string;
  /** Origin window label — main ignores echoes from itself. */
  origin?: string;
};

export type MergeHighlightPayload = {
  active: boolean;
  origin?: string;
};

/** label → last hidden / last used ms (for LRU reap). */
const detachedTouch = new Map<string, number>();

/** Stable webview label for a session (must match Tauri label charset). */
export function detachedWindowLabel(sessionId: string): string {
  return `detached-${sessionId}`;
}

export function sessionIdFromDetachedLabel(label: string): string | null {
  if (!label.startsWith("detached-")) return null;
  return label.slice("detached-".length) || null;
}

export function readDetachedSessionId(): string | null {
  try {
    const raw = new URLSearchParams(window.location.search).get("detached");
    const id = raw?.trim();
    return id ? id : null;
  } catch {
    return null;
  }
}

function touch(label: string) {
  detachedTouch.set(label, Date.now());
}

function detachedPageUrl(sessionId: string): string {
  return `index.html?detached=${encodeURIComponent(sessionId)}`;
}

async function cursorLogical(): Promise<{ x: number; y: number } | null> {
  try {
    const [pos, scale] = await Promise.all([
      cursorPosition(),
      getCurrentWindow().scaleFactor(),
    ]);
    return { x: pos.x / scale, y: pos.y / scale };
  } catch {
    return null;
  }
}

async function placeWindowAtCursor(win: WebviewWindow): Promise<void> {
  const cur = await cursorLogical();
  if (!cur) return;
  try {
    await win.setPosition(
      new LogicalPosition(
        Math.round(cur.x - GRAB_OFFSET_X),
        Math.round(cur.y - GRAB_OFFSET_Y)
      )
    );
  } catch {
    /* ignore */
  }
}

/** Best-effort navigate a secondary webview (reload detached SPA or blank it). */
async function navigateDetached(win: WebviewWindow, href: string): Promise<void> {
  try {
    // Rust-side eval — JS API has no public WebviewWindow.eval in Tauri 2.
    await invoke("navigate_webview", { label: win.label, url: href });
  } catch {
    /* optional */
  }
}

/**
 * Drop React trees only for *aged* hidden detached windows beyond MAX.
 * Recently closed windows stay fully warm (no blank) so re-tear-off is instant.
 * Does NOT destroy the WebView host (tao paint assert on Windows).
 */
export async function reapHiddenDetachedWindows(keepLabel?: string): Promise<void> {
  if (!isTauriRuntime()) return;
  try {
    const all = await getAllWebviewWindows();
    const hidden: WebviewWindow[] = [];
    for (const w of all) {
      if (!w.label.startsWith("detached-")) continue;
      if (keepLabel && w.label === keepLabel) continue;
      let visible = true;
      try {
        visible = await w.isVisible();
      } catch {
        visible = false;
      }
      if (!visible) hidden.push(w);
    }
    if (hidden.length === 0) return;

    const now = Date.now();
    const aged = hidden
      .filter((w) => now - (detachedTouch.get(w.label) ?? now) >= HIDDEN_DETACHED_GRACE_MS)
      .sort(
        (a, b) => (detachedTouch.get(a.label) ?? 0) - (detachedTouch.get(b.label) ?? 0)
      );
    // Keep the newest MAX aged shells loaded; blank only the older excess.
    if (aged.length <= MAX_HIDDEN_DETACHED) return;
    const excess = aged.slice(0, aged.length - MAX_HIDDEN_DETACHED);
    for (const w of excess) {
      await navigateDetached(w, "about:blank");
      touch(w.label);
    }
  } catch {
    /* best-effort */
  }
}

/** Listen for Rust hide events and run the reaper. */
export async function bindDetachedWindowReaper(): Promise<UnlistenFn> {
  if (!isTauriRuntime()) return () => undefined;
  const unlisten = await listen<string>(DETACHED_HIDDEN_EVENT, (event) => {
    const label = event.payload;
    if (label) touch(label);
    void reapHiddenDetachedWindows();
  });
  // Grace period expiry — blank aged shells without waiting for another hide.
  const interval = window.setInterval(() => {
    void reapHiddenDetachedWindows();
  }, 60_000);
  return () => {
    unlisten();
    window.clearInterval(interval);
  };
}

/**
 * Detached window: ask the main window to re-adopt this dialog's tab.
 * Main re-adds the tab, focuses it, then hides this shell.
 */
export async function requestMergeBack(sessionId: string): Promise<void> {
  if (!isTauriRuntime()) return;
  try {
    const origin = getCurrentWindow().label;
    await emit(MERGE_BACK_EVENT, { sessionId, origin } satisfies MergeBackPayload);
  } catch {
    /* optional */
  }
}

/** Main window: handle merge-back requests from detached shells. */
export async function listenMergeBackRequests(
  onMerge: (sessionId: string) => void | Promise<void>
): Promise<UnlistenFn> {
  if (!isTauriRuntime()) return () => undefined;
  let origin: string | null = null;
  try {
    origin = getCurrentWindow().label;
  } catch {
    /* browser preview */
  }
  return listen<MergeBackPayload>(MERGE_BACK_EVENT, (event) => {
    const payload = event.payload;
    if (!payload?.sessionId) return;
    if (payload.origin && origin && payload.origin === origin) return;
    void onMerge(payload.sessionId);
  });
}

/** Detached window: tell main whether a drag is hovering over it (drop hint). */
export async function setMergeHighlight(active: boolean): Promise<void> {
  if (!isTauriRuntime()) return;
  try {
    const origin = getCurrentWindow().label;
    await emit(MERGE_HIGHLIGHT_EVENT, { active, origin } satisfies MergeHighlightPayload);
  } catch {
    /* optional */
  }
}

/** Main window: tab-strip glow while a detached tab drag hovers it. */
export async function listenMergeHighlights(
  onChange: (active: boolean) => void
): Promise<UnlistenFn> {
  if (!isTauriRuntime()) return () => undefined;
  let origin: string | null = null;
  try {
    origin = getCurrentWindow().label;
  } catch {
    /* browser preview */
  }
  return listen<MergeHighlightPayload>(MERGE_HIGHLIGHT_EVENT, (event) => {
    const payload = event.payload;
    if (!payload || typeof payload.active !== "boolean") return;
    if (payload.origin && origin && payload.origin === origin) return;
    onChange(payload.active);
  });
}

/** Physical cursor inside the outer bounds of a labeled window? (drop target check) */
export async function isCursorOverWindow(label: string): Promise<boolean> {
  if (!isTauriRuntime()) return false;
  try {
    const mod = await import("@tauri-apps/api/window");
    const [pos, windows] = await Promise.all([mod.cursorPosition(), mod.getAllWindows()]);
    const target = windows.find((w) => w.label === label);
    if (!target) return false;
    const [winPos, winSize] = await Promise.all([
      target.outerPosition(),
      target.outerSize(),
    ]);
    return (
      pos.x >= winPos.x &&
      pos.x < winPos.x + winSize.width &&
      pos.y >= winPos.y &&
      pos.y < winPos.y + winSize.height
    );
  } catch {
    return false;
  }
}

/** Main window: hide a detached shell after its tab merged back. */
export async function hideDetachedWindowForSession(sessionId: string): Promise<void> {
  if (!isTauriRuntime()) return;
  try {
    const win = await WebviewWindow.getByLabel(detachedWindowLabel(sessionId));
    await win?.hide();
  } catch {
    /* optional */
  }
}

/**
 * Create or re-show a detached dialog window.
 * Drag tear-off should call this only on mouse-up (ghost follows until then).
 */
export async function openDetachedSessionWindow(
  session: Session,
  opts: DetachWindowOpts = {}
): Promise<boolean> {
  if (!isTauriRuntime()) return false;

  const label = detachedWindowLabel(session.id);
  const pageUrl = detachedPageUrl(session.id);

  try {
    const existing = await WebviewWindow.getByLabel(label);
    if (existing) {
      // Secondary close only hides (avoid tao WebView teardown crash).
      try {
        await existing.show();
      } catch {
        /* may already be visible */
      }
      try {
        await existing.unminimize();
      } catch {
        /* optional */
      }
      try {
        await existing.setTitle(session.label || "Marionette");
      } catch {
        /* title is best-effort */
      }
      // Reload SPA in case the shell was reaped to about:blank.
      await navigateDetached(existing, pageUrl);
      if (opts.atCursor) {
        await placeWindowAtCursor(existing);
      }
      try {
        await existing.setFocus();
      } catch {
        /* optional */
      }
      touch(label);
      void reapHiddenDetachedWindows(label);
      return true;
    }
  } catch {
    /* fall through to create */
  }

  let x: number | undefined;
  let y: number | undefined;
  if (opts.atCursor) {
    const cur = await cursorLogical();
    if (cur) {
      x = Math.round(cur.x - GRAB_OFFSET_X);
      y = Math.round(cur.y - GRAB_OFFSET_Y);
    }
  }

  const webview = new WebviewWindow(label, {
    url: pageUrl,
    title: session.label || "Marionette",
    width: 960,
    height: 720,
    minWidth: 640,
    minHeight: 480,
    resizable: true,
    decorations: false,
    focus: true,
    ...(x != null && y != null ? { x, y } : {}),
  });

  const created = await new Promise<boolean>((resolve) => {
    let settled = false;
    const done = (ok: boolean) => {
      if (settled) return;
      settled = true;
      resolve(ok);
    };
    void webview.once("tauri://created", () => done(true));
    void webview.once("tauri://error", () => done(false));
    window.setTimeout(() => done(true), 2500);
  });

  if (!created) return false;

  if (opts.atCursor) {
    await placeWindowAtCursor(webview);
  }

  try {
    await webview.setFocus();
  } catch {
    /* optional */
  }

  touch(label);
  void reapHiddenDetachedWindows(label);
  return true;
}
