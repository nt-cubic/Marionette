/**
 * Pop a dialog into its own Tauri window for multi-monitor / OS snap layouts.
 *
 * Secondary windows load the same SPA with `?detached=<sessionId>`. The main
 * window keeps the shared ACP backend; closing main still shuts the process.
 *
 * Drag tear-off uses a lightweight DOM "ghost" chip in the main window; the
 * real WebView is only created on mouse-up (see SessionTabs).
 */

import { LogicalPosition } from "@tauri-apps/api/dpi";
import { cursorPosition, getCurrentWindow } from "@tauri-apps/api/window";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import { isTauriRuntime } from "./api";
import type { Session } from "./types";

export type DetachWindowOpts = {
  /** Open near the OS cursor (drag drop / optional click). */
  atCursor?: boolean;
};

/** Cursor offset into the new window so the titlebar sits under the drop point. */
const GRAB_OFFSET_X = 72;
const GRAB_OFFSET_Y = 14;

/** Stable webview label for a session (must match Tauri label charset). */
export function detachedWindowLabel(sessionId: string): string {
  return `detached-${sessionId}`;
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
      if (opts.atCursor) {
        await placeWindowAtCursor(existing);
      }
      try {
        await existing.setFocus();
      } catch {
        /* optional */
      }
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

  const url = `index.html?detached=${encodeURIComponent(session.id)}`;
  const webview = new WebviewWindow(label, {
    url,
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
    // Spawn may lag the cursor — snap again after create.
    await placeWindowAtCursor(webview);
  }

  try {
    await webview.setFocus();
  } catch {
    /* optional */
  }

  return true;
}
