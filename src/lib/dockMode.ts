/**
 * Edge dock (docs/plan-edge-dock.md): the main window's "docked mode", not a
 * second window — docking is the same SPA narrowing into a vertical strip.
 *
 * Stage 0 — background residency: with `autoHideOnClose` on, closing the
 * window retracts it to the edge instead of exiting (engine, sessions and the
 * Telegram bridge keep running); true exit is 退出 in the strip's ⋯ menu or
 * the Ctrl K palette (`quit_app`). The Rust side mirrors the flag so its
 * CloseRequested branch knows the path.
 *
 * Stage 1 — the dock engine, a three-state machine over the main window:
 *
 *   floating ──(dock / close)──► docked-hidden ──(sliver mouseenter)──► docked-expanded
 *      ▲                             ▲                                       │
 *      └──(undock)───────────────────┘◄──(mouseleave 300ms + unfocused)──────┘
 *
 * The 4px sliver is real client area of the (W-wide) window parked mostly
 * off-screen: the pointer entering the viewport is the expand signal; pointer
 * leaving for 300ms while unfocused retracts. Expansion only moves the window
 * — never `setFocus` (QQ-style: click inside to focus, typing elsewhere is
 * never interrupted). All geometry is physical px (Tauri monitor work areas
 * are physical) so mixed-DPI multi-monitor placement stays exact. The window
 * is never hidden and never destroyed while docked — only set_position /
 * set_size / show (tao 0.35 paint assert red line, plan §7).
 *
 * Prefs live in localStorage under `marionette-dock-prefs`; `mode: "dock"` is
 * live window state only — normalized back to "off" on every launch.
 */

import { invoke } from "@tauri-apps/api/core";
import {
  availableMonitors,
  currentMonitor,
  cursorPosition,
  getCurrentWindow,
  LogicalSize,
  PhysicalPosition,
  PhysicalSize,
  type Monitor,
} from "@tauri-apps/api/window";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { isTauriRuntime } from "./api";
import { isPageDragSource, onDragText } from "./dragText";
import { readDetachedSessionId } from "./detachedWindow";

const STORAGE_KEY = "marionette-dock-prefs";

export type DockEdge = "left" | "right";

export type DockPrefs = {
  /** "dock" = window is docked to a screen edge (live state, reset on launch); "off" = floating. */
  mode: "off" | "dock";
  edge: DockEdge;
  /** Strip width in logical px, 320–560. */
  width: number;
  /** Stage 0: closing the window retracts to background instead of quitting. */
  autoHideOnClose: boolean;
  /**
   * 侧边栏常驻 (2026-10-25): while expanded, mouseleave / focus loss no longer
   * retracts the strip. Manual retract (close path, drag-out, undock) still
   * works; a re-entered sliver expands and then stays.
   */
  pinned: boolean;
};

const DEFAULT_PREFS: DockPrefs = {
  mode: "off",
  edge: "right",
  width: 400,
  autoHideOnClose: true,
  pinned: false,
};

const WIDTH_MIN = 320;
const WIDTH_MAX = 560;

/**
 * On-screen part of the parked window, logical px (plan §1 said 4 physical;
 * at 150% that is under 3 CSS px — too thin to see or hit). The handle pill
 * drawn in it is #dock-sliver in dock.css.
 */
const SLIVER_LOGICAL = 6;
/** Slide-in/out: 9 equal set_position steps over ~160ms (plan §1: 整窗重绘不做更长). */
const ANIM_STEPS = 9;
const ANIM_MS = 160;
/** Retract grace after mouseleave, also used when focus is lost (plan §1). */
const RETRACT_DELAY_MS = 300;
/** Floating-window minimum, mirrors tauri.conf.json minWidth/minHeight. */
const FLOATING_MIN = { width: 960, height: 640 };
const DOCK_MIN = { width: WIDTH_MIN, height: 200 };
/** How close to a screen side a dragged window has to come to dock (logical). */
const SNAP_LOGICAL = 20;
/** Movement must settle this long before we look (onMoved fires per frame). */
const SNAP_IDLE_MS = 160;
/** Then hold at the edge this long before committing the dock. */
const SNAP_CONFIRM_MS = 220;
/** Dragging the strip this far off its edge undocks it (QQ-style). */
const DOCK_OUT_LOGICAL = 48;

type Listener = (prefs: DockPrefs) => void;

const listeners = new Set<Listener>();

function clampWidth(value: unknown): number {
  const n = typeof value === "number" && Number.isFinite(value) ? Math.round(value) : NaN;
  if (Number.isNaN(n)) return DEFAULT_PREFS.width;
  return Math.min(WIDTH_MAX, Math.max(WIDTH_MIN, n));
}

export function readDockPrefs(): DockPrefs {
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (!raw) return { ...DEFAULT_PREFS };
    const parsed = JSON.parse(raw) as Partial<DockPrefs>;
    return {
      mode: parsed.mode === "dock" ? "dock" : "off",
      edge: parsed.edge === "left" ? "left" : "right",
      width: clampWidth(parsed.width),
      autoHideOnClose:
        typeof parsed.autoHideOnClose === "boolean" ? parsed.autoHideOnClose : DEFAULT_PREFS.autoHideOnClose,
      pinned: typeof parsed.pinned === "boolean" ? parsed.pinned : DEFAULT_PREFS.pinned,
    };
  } catch {
    return { ...DEFAULT_PREFS };
  }
}

function writePrefs(prefs: DockPrefs): void {
  try {
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(prefs));
  } catch {
    // private mode / storage disabled — prefs stay for this session only
  }
}

function emitPrefs(prefs: DockPrefs): void {
  for (const fn of listeners) {
    try {
      fn(prefs);
    } catch {
      // ignore subscriber errors
    }
  }
}

export function subscribeDockPrefs(fn: Listener): () => void {
  listeners.add(fn);
  return () => {
    listeners.delete(fn);
  };
}

export function patchDockPrefs(patch: Partial<DockPrefs>): DockPrefs {
  const next = { ...readDockPrefs(), ...patch };
  next.width = clampWidth(next.width);
  writePrefs(next);
  emitPrefs(next);
  return next;
}

/**
 * Mirror `autoHideOnClose` into the Rust close-path branch. Call at app start
 * and after every toggle — the Rust flag resets on relaunch.
 */
export async function syncBackgroundMode(): Promise<void> {
  if (!isTauriRuntime()) return;
  try {
    await invoke("set_background_mode", { enabled: readDockPrefs().autoHideOnClose });
  } catch {
    // command missing (older binary) or shutdown race — prefs still persisted
  }
}

/** Toggle + persist + sync in one step; returns the next value. */
export function toggleAutoHideOnClose(): boolean {
  const next = patchDockPrefs({ autoHideOnClose: !readDockPrefs().autoHideOnClose });
  void syncBackgroundMode();
  return next.autoHideOnClose;
}

/** 侧边栏常驻 toggle (⋯ menu). Returns the next value. */
export function toggleDockPinned(): boolean {
  return patchDockPrefs({ pinned: !readDockPrefs().pinned }).pinned;
}

// ── Stage 1: dock engine ─────────────────────────────────────────────────

export type DockState = "floating" | "docked-hidden" | "docked-expanded";

type StateListener = (state: DockState, edge: DockEdge) => void;

const stateListeners = new Set<StateListener>();
let dockState: DockState = "floating";
let dockEdge: DockEdge = "right";

/** Work area + full bounds (physical px) of the monitor the dock is bound to. */
type DockArea = {
  x: number;
  y: number;
  w: number;
  h: number;
  scale: number;
  name: string | null;
  mx: number;
  my: number;
  mw: number;
  mh: number;
};
let dockArea: DockArea | null = null;
let dockWidthPhys = 0;

/**
 * Invisible frame around the client area, physical px. tao keeps a resize /
 * shadow border on undecorated windows (WM_NCCALCSIZE insets ~8px left, right
 * and bottom), and outerPosition/setPosition address that outer rect. Placing
 * by the outer rect parked the client fully off-screen — nothing to see and
 * no pixel for the pointer to enter — and left a dead gap when expanded. All
 * placement below targets the client rect instead.
 */
type Insets = { left: number; top: number; right: number; bottom: number };
let insets: Insets = { left: 0, top: 0, right: 0, bottom: 0 };

async function measureInsets(): Promise<Insets> {
  const win = getCurrentWindow();
  try {
    const [outerPos, innerPos, outerSize, innerSize] = await Promise.all([
      win.outerPosition(),
      win.innerPosition(),
      win.outerSize(),
      win.innerSize(),
    ]);
    const left = Math.max(0, innerPos.x - outerPos.x);
    const top = Math.max(0, innerPos.y - outerPos.y);
    return {
      left,
      top,
      right: Math.max(0, outerSize.width - innerSize.width - left),
      bottom: Math.max(0, outerSize.height - innerSize.height - top),
    };
  } catch {
    return { left: 0, top: 0, right: 0, bottom: 0 };
  }
}

/**
 * Client size of the strip. Tauri's `setSize` maps to tao's `set_inner_size`
 * (the client rect), so this is exactly the strip width × work-area height —
 * adding the invisible frame here made every docked window 16px too wide and
 * pushed its right edge past the screen (2026-10-24).
 */
function clientSize(area: DockArea): PhysicalSize {
  return new PhysicalSize(dockWidthPhys, area.h);
}

function outerY(area: DockArea): number {
  return area.y - insets.top;
}

/** Floating geometry (physical) to restore on undock. */
let floating: { x: number; y: number; width: number; height: number; maximized: boolean } | null = null;

/** Newest slide wins; an older loop notices the bump and stops mid-flight. */
let animToken = 0;
let retractTimer: ReturnType<typeof setTimeout> | null = null;
let focused = true;
let pointerInside = false;
/** Our own setPosition calls also emit onMoved — never read them as a drag. */
let suppressAutoDockUntil = 0;
/** Latest outer size, kept for edge detection without an IPC on every move. */
let outerSizeCache: PhysicalSize | null = null;
let snapTimer: ReturnType<typeof setTimeout> | null = null;
let snapConfirmTimer: ReturnType<typeof setTimeout> | null = null;
let snapHintEdge: DockEdge | null = null;
let dockOutTimer: ReturnType<typeof setTimeout> | null = null;
/** Bumped whenever the window moves; stale async edge checks bail out. */
let snapSeq = 0;
/**
 * Set only by `beginDockBarDrag` (a press on the strip's title bar). Undock-on-
 * drag is driven by this positive signal instead of "the window moved away from
 * its parked spot", because our own expand/retract slides also move it and a
 * timing-based suppression could read a half-finished slide as a drag.
 */
let dockBarDrag = false;
let dockBarDragTimer: ReturnType<typeof setTimeout> | null = null;

export function getDockState(): DockState {
  return dockState;
}

export function getDockEdge(): DockEdge {
  return dockEdge;
}

export function subscribeDockState(fn: StateListener): () => void {
  stateListeners.add(fn);
  return () => {
    stateListeners.delete(fn);
  };
}

function setState(next: DockState): void {
  dockState = next;
  applyRootClasses();
  for (const fn of stateListeners) {
    try {
      fn(dockState, dockEdge);
    } catch {
      // ignore subscriber errors
    }
  }
}

function applyRootClasses(): void {
  const root = document.documentElement;
  const docked = dockState !== "floating";
  root.classList.toggle("dock-mode", docked);
  root.classList.toggle("dock-mode--left", docked && dockEdge === "left");
  root.classList.toggle("dock-mode--right", docked && dockEdge === "right");
  root.classList.toggle("edge-hidden", dockState === "docked-hidden");
}

/** The visible edge indicator; pointer enter/leave is read from the viewport itself. */
function ensureSliver(): void {
  if (document.getElementById("dock-sliver")) return;
  const el = document.createElement("div");
  el.id = "dock-sliver";
  el.setAttribute("aria-hidden", "true");
  document.body.appendChild(el);
}

// Monitors report physical px; everything below stays physical until the
// final setPosition/setSize calls.
function monitorContains(monitor: Monitor, x: number, y: number): boolean {
  const p = monitor.position;
  const s = monitor.size;
  return x >= p.x && x < p.x + s.width && y >= p.y && y < p.y + s.height;
}

function monitorToArea(monitor: Monitor): DockArea {
  const wa = monitor.workArea;
  return {
    x: wa.position.x,
    y: wa.position.y,
    w: wa.size.width,
    h: wa.size.height,
    scale: monitor.scaleFactor,
    name: monitor.name,
    mx: monitor.position.x,
    my: monitor.position.y,
    mw: monitor.size.width,
    mh: monitor.size.height,
  };
}

/**
 * Which screen edge the taskbar occupies on this monitor ("left"/"right", or
 * null for bottom/top/auto-hidden). Used to suggest the opposite edge when
 * picking a default (plan §5 阶段 4: 同边不拦，默认对边).
 */
function taskbarSide(area: DockArea): DockEdge | null {
  if (area.x - area.mx >= Math.max(8, area.mw * 0.02)) return "left";
  if (area.mx + area.mw - (area.x + area.w) >= Math.max(8, area.mw * 0.02)) return "right";
  return null;
}

/** Default edge avoids a lateral taskbar; an explicit choice always wins. */
function resolveEdge(requested: DockEdge | undefined, prefs: DockPrefs, area: DockArea): DockEdge {
  if (requested) return requested;
  const taskbar = taskbarSide(area);
  if (taskbar && prefs.edge === taskbar) return taskbar === "left" ? "right" : "left";
  return prefs.edge;
}

/**
 * The dock binds to the monitor the pointer is on when the dock command fires
 * (plan §5 阶段 1); falls back to the window's current monitor.
 */
async function pickWorkArea(): Promise<DockArea | null> {
  try {
    const monitors = await availableMonitors();
    if (monitors.length === 0) return null;
    const cursor = await cursorPosition().catch(() => null);
    if (cursor) {
      const hit = monitors.find((m) => monitorContains(m, cursor.x, cursor.y));
      if (hit) return monitorToArea(hit);
    }
    const cur = await currentMonitor();
    return monitorToArea(cur ?? monitors[0]!);
  } catch {
    return null;
  }
}

/** Re-lookup the bound monitor (taskbar moved, display replugged); keeps the old area if it is gone. */
async function refreshWorkArea(): Promise<void> {
  if (!dockArea) return;
  try {
    const monitors = await availableMonitors();
    const hit = dockArea.name ? monitors.find((m) => m.name === dockArea!.name) : undefined;
    if (hit) dockArea = monitorToArea(hit);
  } catch {
    // keep last known area
  }
}

/** Parked: the client is off-screen except the sliver (outer x returned). */
function hiddenX(area: DockArea, widthPhys: number, edge: DockEdge): number {
  const sliver = Math.max(4, Math.round(SLIVER_LOGICAL * area.scale));
  const clientX = edge === "right" ? area.x + area.w - sliver : area.x - widthPhys + sliver;
  return clientX - insets.left;
}

/** Expanded: client flush against the work-area edge, taskbar never covered. */
function expandedX(area: DockArea, widthPhys: number, edge: DockEdge): number {
  const clientX = edge === "right" ? area.x + area.w - widthPhys : area.x;
  return clientX - insets.left;
}

const sleep = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));

async function slideTo(targetX: number): Promise<void> {
  const token = ++animToken;
  const win = getCurrentWindow();
  const y = dockArea ? outerY(dockArea) : 0;
  try {
    const from = (await win.outerPosition()).x;
    const delta = targetX - from;
    if (Math.abs(delta) < 2) {
      await win.setPosition(new PhysicalPosition(targetX, y));
      return;
    }
    for (let i = 1; i <= ANIM_STEPS; i++) {
      if (token !== animToken) return; // a newer slide took over
      const x = Math.round(from + (delta * i) / ANIM_STEPS);
      await win.setPosition(new PhysicalPosition(x, y));
      await sleep(ANIM_MS / ANIM_STEPS);
    }
  } catch {
    // window gone / shutdown race — the next command recomputes from scratch
  }
}

/**
 * Dock the main window to an edge. By default it retracts to the sliver;
 * `expanded` leaves it flush against the edge instead (the drag-to-edge
 * gesture: the strip stays visible under the pointer, then retracts on blur).
 * Returns false when docking is unavailable (non-tauri, detached SPA, no
 * monitors) — callers fall back to hiding (stage 0 behavior).
 */
export async function enterDock(edge?: DockEdge, options?: { expanded?: boolean }): Promise<boolean> {
  if (!isTauriRuntime() || readDetachedSessionId()) return false;
  if (dockState !== "floating") return true;
  suppressAutoDockUntil = Date.now() + 1200; // our own setPosition moves are not a drag
  cancelSnap();
  cancelDockBarDrag();
  const win = getCurrentWindow();
  const prefs = readDockPrefs();
  const area = await pickWorkArea();
  if (!area) return false;
  let saved: NonNullable<typeof floating>;
  try {
    // Capture AFTER unmaximize — a maximized outer size would restore the
    // whole screen on undock.
    const wasMaximized = await win.isMaximized();
    if (wasMaximized) await win.unmaximize();
    const pos = await win.outerPosition();
    const size = await win.innerSize();
    saved = { x: pos.x, y: pos.y, width: size.width, height: size.height, maximized: wasMaximized };
  } catch {
    return false;
  }
  const useEdge = resolveEdge(edge, prefs, area);
  const widthPhys = Math.round(prefs.width * area.scale);
  dockWidthPhys = widthPhys;
  try {
    await win.setAlwaysOnTop(true);
    await win.setSkipTaskbar(true);
    await win.setResizable(false);
    await win.setMinSize(new LogicalSize(DOCK_MIN.width, DOCK_MIN.height));
    // Insets depend on DPI, not size: measure after the style changes land.
    insets = await measureInsets();
    await win.setSize(clientSize(area));
  } catch {
    return false;
  }
  floating = saved;
  dockArea = area;
  dockEdge = useEdge;
  patchDockPrefs({ mode: "dock", edge: useEdge });
  const expanded = options?.expanded === true;
  try {
    await win.setPosition(
      new PhysicalPosition(
        expanded ? expandedX(area, widthPhys, useEdge) : hiddenX(area, widthPhys, useEdge),
        outerY(area),
      ),
    );
  } catch {
    // best effort — the strip is already docked-sized; expand() repositions
  }
  ensureSliver();
  if (expanded) pointerInside = true;
  cancelRetract();
  setState(expanded ? "docked-expanded" : "docked-hidden");
  return true;
}

/** Slide the strip into the work area. No setFocus — click inside to focus. */
export async function expand(): Promise<void> {
  if (dockState !== "docked-hidden" || !dockArea) return;
  cancelDockBarDrag();
  try {
    if (await invoke<boolean>("fullscreen_active")) return; // D3D / presentation: stay retracted
  } catch {
    // command missing — fail open
  }
  await refreshWorkArea();
  if (!dockArea) return;
  suppressAutoDockUntil = Date.now() + 320; // slide moves are not a drag (slide ≈ 160ms)
  setState("docked-expanded"); // optimistic: a re-enter mid-slide cancels back
  cancelRetract();
  try {
    await getCurrentWindow().setSize(clientSize(dockArea));
  } catch {
    // ignore — position below is the visible part
  }
  await slideTo(expandedX(dockArea, dockWidthPhys, dockEdge));
}

/** Slide the strip back out, leaving only the sliver on screen. */
export async function retract(): Promise<void> {
  if (!dockArea || (dockState !== "docked-expanded" && dockState !== "docked-hidden")) return;
  cancelDockBarDrag();
  setState("docked-hidden"); // optimistic: a re-enter mid-slide cancels back
  cancelRetract();
  await slideTo(hiddenX(dockArea, dockWidthPhys, dockEdge));
}

// ── Strip width: drag the inner edge (2026-10-25) ─────────────────────────
// The grip turns pointer clientX into a new logical width. A right-docked
// strip drags its left (inner) edge, so the width is "current width −
// clientX" measured against the client rect the event saw; left-docked it is
// plain clientX (its left edge is fixed). Both are relative to the geometry
// of the moment, which is why the grip re-reads getDockWidth() per move:
// every apply lands the edge back on the pointer, so the next clientX starts
// near 0 again and rounding never accumulates.

let widthApplyChain: Promise<unknown> = Promise.resolve();

/** Current strip width in logical px (live while docked, prefs otherwise). */
export function getDockWidth(): number {
  if (dockArea && dockWidthPhys > 0) return dockWidthPhys / dockArea.scale;
  return readDockPrefs().width;
}

/**
 * Live-apply a new width while docked-expanded. Serialized — one step's
 * setSize + flush setPosition must not interleave with the next; each step
 * reads the latest dockWidthPhys so the chain converges on the last request.
 */
export function applyDockWidth(logical: number): void {
  if (dockState !== "docked-expanded" || !dockArea) return;
  const area = dockArea;
  dockWidthPhys = Math.max(1, Math.round(clampWidth(logical) * area.scale));
  const run = widthApplyChain.then(async () => {
    try {
      const win = getCurrentWindow();
      await win.setSize(clientSize(area));
      await win.setPosition(
        new PhysicalPosition(expandedX(area, dockWidthPhys, dockEdge), outerY(area)),
      );
    } catch {
      // window gone mid-drag — the next move recomputes from live geometry
    }
  });
  widthApplyChain = run.catch(() => undefined);
}

/** Persist the live width (grip pointerup / undock). */
export function persistDockWidth(): void {
  patchDockPrefs({ width: Math.round(getDockWidth()) });
}

/**
 * Leave dock mode: restore the floating geometry and default window params.
 *
 * `keepPosition` is the drag-out gesture: the user already placed the strip
 * with the pointer, so the restored window keeps that spot (clamped into the
 * work area) instead of teleporting back to where it was docked from.
 */
export async function undock(options?: { keepPosition?: boolean }): Promise<void> {
  if (dockState === "floating") return;
  suppressAutoDockUntil = Date.now() + 1200; // restoring geometry is not a drag
  cancelSnap();
  cancelDockBarDrag();
  animToken++; // stop any in-flight slide
  cancelRetract();
  const win = getCurrentWindow();
  const area = dockArea;
  let anchor: PhysicalPosition | null = null;
  if (options?.keepPosition) {
    try {
      anchor = await win.outerPosition();
    } catch {
      anchor = null;
    }
  }
  try {
    await win.setAlwaysOnTop(false);
    await win.setSkipTaskbar(false);
    await win.setResizable(true);
    await win.setMinSize(new LogicalSize(FLOATING_MIN.width, FLOATING_MIN.height));
    if (floating) {
      await win.setSize(new PhysicalSize(floating.width, floating.height));
      if (anchor && area) {
        // Place by the *outer* rect (what setPosition addresses) and clamp with
        // the measured outer size — using the client width here would push the
        // invisible frame past the work area again (the 2026-10-24 bug).
        let outer = new PhysicalSize(floating.width, floating.height);
        try {
          outer = await win.outerSize();
        } catch {
          // fall back to the client size
        }
        const x = Math.min(
          Math.max(anchor.x, area.x),
          Math.max(area.x, area.x + area.w - outer.width),
        );
        const y = Math.min(
          Math.max(anchor.y, area.y),
          Math.max(area.y, area.y + area.h - outer.height),
        );
        await win.setPosition(new PhysicalPosition(x, y));
      } else {
        await win.setPosition(new PhysicalPosition(floating.x, floating.y));
        if (floating.maximized) await win.maximize();
      }
    }
  } catch {
    // best effort — params that did land still apply
  }
  floating = null;
  dockArea = null;
  patchDockPrefs({ mode: "off" });
  setState("floating");
}

function cancelDockBarDrag(): void {
  dockBarDrag = false;
  if (dockBarDragTimer != null) {
    clearTimeout(dockBarDragTimer);
    dockBarDragTimer = null;
  }
}

/**
 * DockChrome calls this just before `startDragging()` on the strip's title bar.
 * Arms the drag-out check; a click that never moves disarms itself.
 */
export function beginDockBarDrag(): void {
  if (dockState !== "docked-expanded") return;
  dockBarDrag = true;
  armDockBarDragSafety();
}

function armDockBarDragSafety(): void {
  if (dockBarDragTimer != null) clearTimeout(dockBarDragTimer);
  dockBarDragTimer = setTimeout(() => {
    dockBarDragTimer = null;
    dockBarDrag = false;
  }, 15_000);
}

function cancelRetract(): void {
  if (retractTimer != null) {
    clearTimeout(retractTimer);
    retractTimer = null;
  }
}

function scheduleRetract(): void {
  cancelRetract();
  retractTimer = setTimeout(() => {
    retractTimer = null;
    // 常驻: an explicitly pinned strip never auto-retracts; manual paths
    // (close, drag-out, undock) call retract() directly and still work.
    if (readDockPrefs().pinned) return;
    if (!pointerInside && !focused && dockState === "docked-expanded") void retract();
  }, RETRACT_DELAY_MS);
}

// ── Drag-to-edge snap (QQ-style) ─────────────────────────────────────────
// The titlebar drag is native (DockChrome calls startDragging; the pointerup
// happens inside Windows' move loop), so JS never sees the release. `onMoved`
// is the signal we do get: when movement settles with a window side on a
// screen side, show the edge hint, then dock. The strip is left expanded so
// the user sees where it went; focus/pointer tracking then retracts it like
// any other expanded dock.

function setSnapHint(edge: DockEdge | null): void {
  if (edge === snapHintEdge) return;
  snapHintEdge = edge;
  const root = document.documentElement;
  root.classList.toggle("dock-snap-left", edge === "left");
  root.classList.toggle("dock-snap-right", edge === "right");
}

function cancelSnap(): void {
  snapSeq += 1;
  if (snapTimer != null) {
    clearTimeout(snapTimer);
    snapTimer = null;
  }
  if (snapConfirmTimer != null) {
    clearTimeout(snapConfirmTimer);
    snapConfirmTimer = null;
  }
  if (dockOutTimer != null) {
    clearTimeout(dockOutTimer);
    dockOutTimer = null;
  }
  setSnapHint(null);
}

/** Which screen side (if any) the floating window is resting against. */
async function edgeNear(pos: PhysicalPosition): Promise<DockEdge | null> {
  if (!outerSizeCache || outerSizeCache.width <= 0) return null;
  try {
    if (await getCurrentWindow().isMaximized()) return null;
    const monitors = await availableMonitors();
    if (monitors.length === 0) return null;
    // Pick the monitor the window overlaps most (its center can be off-screen
    // while it is parked against an edge).
    let monitor = monitors[0]!;
    let bestOverlap = -1;
    for (const m of monitors) {
      const left = Math.max(pos.x, m.position.x);
      const right = Math.min(pos.x + outerSizeCache.width, m.position.x + m.size.width);
      const top = Math.max(pos.y, m.position.y);
      const bottom = Math.min(pos.y + outerSizeCache.height, m.position.y + m.size.height);
      const overlap = Math.max(0, right - left) * Math.max(0, bottom - top);
      if (overlap > bestOverlap) {
        bestOverlap = overlap;
        monitor = m;
      }
    }
    // A near-full-screen window is not a strip; docking it would just trap it.
    if (outerSizeCache.width >= monitor.size.width * 0.92) return null;
    const threshold = Math.max(14, Math.round(SNAP_LOGICAL * monitor.scaleFactor));
    const leftGap = pos.x - monitor.position.x;
    const rightGap = monitor.position.x + monitor.size.width - (pos.x + outerSizeCache.width);
    const leftHit = leftGap >= -threshold && leftGap <= threshold;
    const rightHit = rightGap >= -threshold && rightGap <= threshold;
    if (leftHit && rightHit) return Math.abs(leftGap) <= Math.abs(rightGap) ? "left" : "right";
    return leftHit ? "left" : rightHit ? "right" : null;
  } catch {
    return null;
  }
}

async function considerSnap(): Promise<void> {
  if (dockState !== "floating" || Date.now() < suppressAutoDockUntil) {
    setSnapHint(null);
    return;
  }
  const seq = snapSeq;
  let pos: PhysicalPosition;
  try {
    pos = await getCurrentWindow().outerPosition();
  } catch {
    return;
  }
  if (seq !== snapSeq || dockState !== "floating") return;
  const edge = await edgeNear(pos);
  if (seq !== snapSeq || dockState !== "floating") return;
  setSnapHint(edge);
  if (!edge) return;
  snapConfirmTimer = setTimeout(() => {
    snapConfirmTimer = null;
    if (seq !== snapSeq || dockState !== "floating") {
      setSnapHint(null);
      return;
    }
    setSnapHint(null);
    void enterDock(edge, { expanded: true });
  }, SNAP_CONFIRM_MS);
}

async function considerDockOut(): Promise<void> {
  if (dockState !== "docked-expanded" || !dockArea) return;
  const area = dockArea;
  if (!dockBarDrag) return;
  let pos: PhysicalPosition;
  try {
    pos = await getCurrentWindow().outerPosition();
  } catch {
    return;
  }
  if (dockState !== "docked-expanded") return;
  const threshold = Math.max(48, Math.round(DOCK_OUT_LOGICAL * area.scale));
  const expectedX = expandedX(area, dockWidthPhys, dockEdge);
  const expectedY = outerY(area);
  const movedAway =
    Math.abs(pos.x - expectedX) > threshold || Math.abs(pos.y - expectedY) > threshold;
  cancelDockBarDrag(); // one drag, one judgement
  if (movedAway) void undock({ keepPosition: true });
}

/** Rust CloseRequested in background mode lands here: close = retract (dock if still floating). */
async function handleCloseFromRust(): Promise<void> {
  if (dockState === "floating") {
    const docked = await enterDock();
    if (!docked) {
      try {
        // Webview-level hide first — arms WebView2 suspension for the
        // background stay (vendor/wry patch); the Rust handoff wake path
        // shows the webview again before focusing.
        const { getCurrentWebview } = await import("@tauri-apps/api/webview");
        await getCurrentWebview().hide();
        await getCurrentWindow().hide(); // stage-0 fallback, awaiting second-launch handoff
      } catch {
        // ignore
      }
    }
  } else if (dockState === "docked-expanded") {
    await retract();
  }
  // docked-hidden: already retracted
}

/**
 * Bind the engine: close handoff, focus and pointer tracking. Main window
 * only — detached shells never dock. Call once from App's mount effect.
 */
export function initDockMode(): () => void {
  if (!isTauriRuntime() || readDetachedSessionId()) return () => {};
  const win = getCurrentWindow();
  // The first layout/position events at launch are not a drag.
  suppressAutoDockUntil = Date.now() + 1500;
  // Docking is live window state, not a launch state — a stale "dock" from
  // the previous run must not shrink the fresh 1280×820 window.
  if (readDockPrefs().mode === "dock") patchDockPrefs({ mode: "off" });

  const unlisteners: Array<() => void> = [];
  void (async () => {
    try {
      unlisteners.push(await win.listen("marionette-main-close", () => void handleCloseFromRust()));
      unlisteners.push(
        await win.onFocusChanged(({ payload: nowFocused }) => {
          focused = nowFocused;
          if (nowFocused) {
            cancelRetract();
            // Second-launch handoff (or anything else) focusing the parked
            // sliver would show a 4px nothing — pull the strip out instead.
            if (dockState === "docked-hidden") void expand();
          } else {
            scheduleRetract();
          }
        }),
      );
      outerSizeCache = await win.outerSize();
      unlisteners.push(
        await win.onResized(async () => {
          try {
            outerSizeCache = await win.outerSize();
          } catch {
            // window gone — keep the last known size
          }
        }),
      );
      // Dragging a file over the parked sliver must open the strip: OLE
      // drags suppress mouse events, so the sliver's mouseenter never fires —
      // tao's drag-drop channel is the only expand signal while hidden
      // (2026-10-25). MemoBox / Composer listen on the same event for their
      // own hit-testing; this one only drives the dock engine.
      unlisteners.push(
        await getCurrentWebview().onDragDropEvent((event) => {
          const type = event.payload.type;
          if (type === "enter" || type === "over") {
            pointerInside = true;
            cancelRetract();
            if (dockState === "docked-hidden") void expand();
          } else if (type === "leave") {
            // Mouse events stayed suppressed through the drag; resync the
            // pointer flag so an unfocused strip can retract again.
            pointerInside = false;
            scheduleRetract();
          }
        }),
      );
      // Text/link drags (browser selections, URLs) never reach the channel
      // above — they ride the vendor wry patch event. Same expand signal.
      // Page-sourced HTML5 drags (memo rows) must not expand the strip.
      unlisteners.push(
        await onDragText((event) => {
          if (isPageDragSource()) return;
          if (event.kind === "enter" || event.kind === "over") {
            pointerInside = true;
            cancelRetract();
            if (dockState === "docked-hidden") void expand();
          } else if (event.kind === "leave") {
            pointerInside = false;
            scheduleRetract();
          }
        }),
      );
      unlisteners.push(
        await win.onMoved(() => {
          if (dockState === "docked-expanded") {
            // Only a drag that started on the title bar can undock; our own
            // expand/retract slides also emit Moved and must be ignored.
            if (!dockBarDrag) return;
            armDockBarDragSafety(); // a long drag keeps the flag alive
            if (dockOutTimer != null) clearTimeout(dockOutTimer);
            dockOutTimer = setTimeout(() => {
              dockOutTimer = null;
              void considerDockOut();
            }, SNAP_IDLE_MS);
            return;
          }
          if (Date.now() < suppressAutoDockUntil) return;
          if (dockState !== "floating") return;
          snapSeq += 1;
          setSnapHint(null);
          if (snapTimer != null) clearTimeout(snapTimer);
          if (snapConfirmTimer != null) {
            clearTimeout(snapConfirmTimer);
            snapConfirmTimer = null;
          }
          // Position is re-read in considerSnap; this only marks "moved".
          snapTimer = setTimeout(() => {
            snapTimer = null;
            void considerSnap();
          }, SNAP_IDLE_MS);
        }),
      );
    } catch {
      // window APIs unavailable — pointer tracking below still works
    }
  })();

  const root = document.documentElement;
  const onPointerEnter = () => {
    pointerInside = true;
    cancelRetract();
    if (dockState === "docked-hidden") void expand();
  };
  const onPointerLeave = () => {
    pointerInside = false;
    scheduleRetract();
  };
  root.addEventListener("mouseenter", onPointerEnter);
  root.addEventListener("mouseleave", onPointerLeave);

  // Strip tabs side-scroll on a plain wheel too (design: `.tabs` wheel rule) —
  // the equal-width floating tabs never overflow, so gate by dock state.
  const onWheel = (event: WheelEvent) => {
    if (dockState === "floating" || Math.abs(event.deltaY) <= Math.abs(event.deltaX)) return;
    const target = event.target instanceof Element ? event.target : null;
    const strip = target?.closest(".editor-tabs__tablist");
    if (strip) {
      strip.scrollLeft += event.deltaY;
      event.preventDefault();
    }
  };
  window.addEventListener("wheel", onWheel, { passive: false });

  return () => {
    for (const un of unlisteners) un();
    root.removeEventListener("mouseenter", onPointerEnter);
    root.removeEventListener("mouseleave", onPointerLeave);
    window.removeEventListener("wheel", onWheel);
    cancelRetract();
    cancelSnap();
  };
}
