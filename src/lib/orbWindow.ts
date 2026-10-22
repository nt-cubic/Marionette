/**
 * The floating orb window: an always-on-top, skip-taskbar, transparent
 * 72×72 circle pinned to the bottom-right of the desktop (input-method
 * style). Clicking it expands a ~380×540 panel *in the same window* via
 * setSize + reposition (the bottom-right corner stays anchored).
 *
 * Created from the main window (`ensureOrbWindow`) — never destroyed, only
 * shown/hidden/resized (tao 0.35 paint-assert rule: no WebView teardown).
 * Position is persisted in localStorage (shared origin across windows).
 *
 * dragDropEnabled is OFF for this window so WebView2 keeps native HTML5
 * drag-and-drop: that is the only way to receive text/uri-list, text/html
 * and text/plain drags coming from a browser (Tauri's file-drop events
 * only carry absolute file paths — see Composer's dual handling).
 */

import { LogicalPosition, LogicalSize } from "@tauri-apps/api/dpi";
import { currentMonitor, getCurrentWindow } from "@tauri-apps/api/window";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import { isTauriRuntime } from "./api";
import { ORB_WINDOW_LABEL } from "./orbEvents";

export const ORB_CIRCLE_SIZE = 72;
export const ORB_PANEL_WIDTH = 380;
export const ORB_PANEL_HEIGHT = 540;
const ORB_MARGIN = 16;
/** The monitor size includes the taskbar area — keep the orb above it. */
const ORB_TASKBAR_ALLOWANCE = 56;

const ORB_POS_STORAGE_KEY = "marionette-orb-pos";

export function orbPageUrl(): string {
  return "index.html?orb=1";
}

type LogicalPoint = { x: number; y: number };

function readSavedPosition(): LogicalPoint | null {
  try {
    const raw = window.localStorage.getItem(ORB_POS_STORAGE_KEY);
    if (!raw) return null;
    const pos = JSON.parse(raw) as LogicalPoint;
    if (
      typeof pos?.x !== "number" ||
      typeof pos?.y !== "number" ||
      !Number.isFinite(pos.x) ||
      !Number.isFinite(pos.y)
    ) {
      return null;
    }
    return pos;
  } catch {
    return null;
  }
}

function savePosition(pos: LogicalPoint): void {
  try {
    window.localStorage.setItem(ORB_POS_STORAGE_KEY, JSON.stringify(pos));
  } catch {
    /* private mode */
  }
}

async function defaultOrbPosition(): Promise<LogicalPoint> {
  try {
    const monitor = await currentMonitor();
    if (monitor) {
      const scale = monitor.scaleFactor || 1;
      const width = monitor.size.width / scale;
      const height = monitor.size.height / scale;
      return {
        x: Math.max(0, Math.round(width - ORB_CIRCLE_SIZE - ORB_MARGIN)),
        y: Math.max(0, Math.round(height - ORB_CIRCLE_SIZE - ORB_TASKBAR_ALLOWANCE)),
      };
    }
  } catch {
    /* fall through to a sane default */
  }
  return { x: 240, y: 240 };
}

/**
 * Create or re-show the orb window. Called from the main window on mount.
 * Never destroys or navigates an existing shell (tao paint-assert rule).
 */
export async function ensureOrbWindow(): Promise<boolean> {
  if (!isTauriRuntime()) return false;

  try {
    const existing = await WebviewWindow.getByLabel(ORB_WINDOW_LABEL);
    if (existing) {
      try {
        await existing.show();
      } catch {
        /* may already be visible */
      }
      return true;
    }
  } catch {
    /* fall through to create */
  }

  const pos = await defaultOrbPosition();
  const webview = new WebviewWindow(ORB_WINDOW_LABEL, {
    url: orbPageUrl(),
    title: "Marionette",
    width: ORB_CIRCLE_SIZE,
    height: ORB_CIRCLE_SIZE,
    x: pos.x,
    y: pos.y,
    transparent: true,
    decorations: false,
    alwaysOnTop: true,
    skipTaskbar: true,
    shadow: false,
    focus: false,
    resizable: false,
    dragDropEnabled: false,
  });

  return new Promise<boolean>((resolve) => {
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
}

/** Orb side: restore the persisted position on mount (before first paint). */
export async function applySavedOrbPosition(): Promise<void> {
  if (!isTauriRuntime()) return;
  const saved = readSavedPosition();
  if (!saved) return;
  try {
    await getCurrentWindow().setPosition(
      new LogicalPosition(Math.round(saved.x), Math.round(saved.y))
    );
  } catch {
    /* best-effort */
  }
}

/** Orb side: persist position whenever the user finishes dragging the ball. */
export async function bindOrbMovePersistence(): Promise<() => void> {
  if (!isTauriRuntime()) return () => undefined;
  let timer: number | undefined;
  const persist = () => {
    if (timer) window.clearTimeout(timer);
    timer = window.setTimeout(() => {
      void (async () => {
        try {
          const win = getCurrentWindow();
          const [pos, scale] = await Promise.all([
            win.outerPosition(),
            win.scaleFactor(),
          ]);
          savePosition({
            x: pos.x / (scale || 1),
            y: pos.y / (scale || 1),
          });
        } catch {
          /* best-effort */
        }
      })();
    }, 400);
  };
  try {
    const unlisten = await getCurrentWindow().onMoved(() => persist());
    return () => {
      if (timer) window.clearTimeout(timer);
      unlisten();
    };
  } catch {
    return () => undefined;
  }
}

/**
 * Resize the orb window between circle and panel, keeping the current
 * bottom-right corner anchored and clamped on-screen. Works from inside
 * the orb window (default) or from the main window for a specific label.
 */
export async function resizeOrbWindow(
  target: "circle" | "panel",
  win?: WebviewWindow
): Promise<void> {
  if (!isTauriRuntime()) return;
  try {
    const window_ = win ?? (getCurrentWindow() as unknown as WebviewWindow);
    const [physPos, physSize, scale] = await Promise.all([
      window_.outerPosition(),
      window_.outerSize(),
      window_.scaleFactor(),
    ]);
    const factor = scale || 1;
    const width = target === "panel" ? ORB_PANEL_WIDTH : ORB_CIRCLE_SIZE;
    const height = target === "panel" ? ORB_PANEL_HEIGHT : ORB_CIRCLE_SIZE;
    // Bottom-right anchor in logical coordinates.
    const anchorX = (physPos.x + physSize.width) / factor;
    const anchorY = (physPos.y + physSize.height) / factor;
    const x = Math.max(0, Math.round(anchorX - width));
    const y = Math.max(0, Math.round(anchorY - height));
    await window_.setSize(new LogicalSize(width, height));
    await window_.setPosition(new LogicalPosition(x, y));
  } catch {
    /* best-effort — collapse/expand is cosmetic */
  }
}
