/**
 * Edge dock (docs/plan-edge-dock.md): the main window's "docked mode", not a
 * second window — docking is the same SPA narrowing into a vertical strip.
 *
 * Stage 0 — background residency: with `autoHideOnClose` on, closing the
 * window retracts it to the edge instead of exiting (engine, sessions and the
 * Telegram bridge keep running); true exit is the footer's 退出 button
 * (`quit_app`). The Rust side mirrors the flag so its CloseRequested branch
 * knows the path.
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
import { isTauriRuntime } from "./api";
import { readDetachedSessionId } from "./detachedWindow";

const STORAGE_KEY = "marionette-dock-prefs";

export type DockEdge = "left" | "right";

export type DockPrefs = {
  /** "dock" = window is docked to a screen edge (live state, reset on launch); "off" = floating. */
  mode: "off" | "dock";
  edge: DockEdge;
  /** Strip width in logical px, 320–480. */
  width: number;
  /** Stage 0: closing the window retracts to background instead of quitting. */
  autoHideOnClose: boolean;
};

const DEFAULT_PREFS: DockPrefs = {
  mode: "off",
  edge: "right",
  width: 400,
  autoHideOnClose: true,
};

const WIDTH_MIN = 320;
const WIDTH_MAX = 480;

/** On-screen part of the parked window, physical px (plan §1: 只留 4px). */
const SLIVER_PX = 4;
/** Slide-in/out: 9 equal set_position steps over ~160ms (plan §1: 整窗重绘不做更长). */
const ANIM_STEPS = 9;
const ANIM_MS = 160;
/** Retract grace after mouseleave, also used when focus is lost (plan §1). */
const RETRACT_DELAY_MS = 300;
/** Floating-window minimum, mirrors tauri.conf.json minWidth/minHeight. */
const FLOATING_MIN = { width: 960, height: 640 };
const DOCK_MIN = { width: WIDTH_MIN, height: 200 };

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

// ── Stage 1: dock engine ─────────────────────────────────────────────────

export type DockState = "floating" | "docked-hidden" | "docked-expanded";

type StateListener = (state: DockState, edge: DockEdge) => void;

const stateListeners = new Set<StateListener>();
let dockState: DockState = "floating";
let dockEdge: DockEdge = "right";

/** Work area (physical px) of the monitor the dock is bound to. */
type DockArea = { x: number; y: number; w: number; h: number; scale: number; name: string | null };
let dockArea: DockArea | null = null;
let dockWidthPhys = 0;

/** Floating geometry (physical) to restore on undock. */
let floating: { x: number; y: number; width: number; height: number; maximized: boolean } | null = null;

/** Newest slide wins; an older loop notices the bump and stops mid-flight. */
let animToken = 0;
let retractTimer: ReturnType<typeof setTimeout> | null = null;
let focused = true;
let pointerInside = false;

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
  };
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

/** Parked: whole strip off-screen except the sliver. */
function hiddenX(area: DockArea, widthPhys: number, edge: DockEdge): number {
  return edge === "right" ? area.x + area.w - SLIVER_PX : area.x - widthPhys + SLIVER_PX;
}

/** Expanded: flush against the work-area edge, taskbar never covered. */
function expandedX(area: DockArea, widthPhys: number, edge: DockEdge): number {
  return edge === "right" ? area.x + area.w - widthPhys : area.x;
}

const sleep = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));

async function slideTo(targetX: number): Promise<void> {
  const token = ++animToken;
  const win = getCurrentWindow();
  const y = dockArea?.y ?? 0;
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
 * Dock the main window to an edge and retract it to the sliver. Returns false
 * when docking is unavailable (non-tauri, detached SPA, no monitors) — callers
 * fall back to hiding (stage 0 behavior).
 */
export async function enterDock(edge?: DockEdge): Promise<boolean> {
  if (!isTauriRuntime() || readDetachedSessionId()) return false;
  if (dockState !== "floating") return true;
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
    const size = await win.outerSize();
    saved = { x: pos.x, y: pos.y, width: size.width, height: size.height, maximized: wasMaximized };
  } catch {
    return false;
  }
  const useEdge = edge ?? prefs.edge;
  const widthPhys = Math.round(prefs.width * area.scale);
  try {
    await win.setAlwaysOnTop(true);
    await win.setSkipTaskbar(true);
    await win.setResizable(false);
    await win.setMinSize(new LogicalSize(DOCK_MIN.width, DOCK_MIN.height));
    await win.setSize(new PhysicalSize(widthPhys, area.h));
  } catch {
    return false;
  }
  floating = saved;
  dockArea = area;
  dockWidthPhys = widthPhys;
  dockEdge = useEdge;
  patchDockPrefs({ mode: "dock", edge: useEdge });
  try {
    await win.setPosition(new PhysicalPosition(hiddenX(area, widthPhys, useEdge), area.y));
  } catch {
    // best effort — the strip is already docked-sized; expand() repositions
  }
  ensureSliver();
  setState("docked-hidden");
  return true;
}

/** Slide the strip into the work area. No setFocus — click inside to focus. */
export async function expand(): Promise<void> {
  if (dockState !== "docked-hidden" || !dockArea) return;
  try {
    if (await invoke<boolean>("fullscreen_active")) return; // D3D / presentation: stay retracted
  } catch {
    // command missing — fail open
  }
  await refreshWorkArea();
  if (!dockArea) return;
  setState("docked-expanded"); // optimistic: a re-enter mid-slide cancels back
  cancelRetract();
  try {
    await getCurrentWindow().setSize(new PhysicalSize(dockWidthPhys, dockArea.h));
  } catch {
    // ignore — position below is the visible part
  }
  await slideTo(expandedX(dockArea, dockWidthPhys, dockEdge));
}

/** Slide the strip back out, leaving only the sliver on screen. */
export async function retract(): Promise<void> {
  if (!dockArea || (dockState !== "docked-expanded" && dockState !== "docked-hidden")) return;
  setState("docked-hidden"); // optimistic: a re-enter mid-slide cancels back
  cancelRetract();
  await slideTo(hiddenX(dockArea, dockWidthPhys, dockEdge));
}

/** Leave dock mode: restore the floating geometry and default window params. */
export async function undock(): Promise<void> {
  if (dockState === "floating") return;
  animToken++; // stop any in-flight slide
  cancelRetract();
  const win = getCurrentWindow();
  try {
    await win.setAlwaysOnTop(false);
    await win.setSkipTaskbar(false);
    await win.setResizable(true);
    await win.setMinSize(new LogicalSize(FLOATING_MIN.width, FLOATING_MIN.height));
    if (floating) {
      await win.setSize(new PhysicalSize(floating.width, floating.height));
      await win.setPosition(new PhysicalPosition(floating.x, floating.y));
      if (floating.maximized) await win.maximize();
    }
  } catch {
    // best effort — params that did land still apply
  }
  floating = null;
  dockArea = null;
  patchDockPrefs({ mode: "off" });
  setState("floating");
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
    if (!pointerInside && !focused && dockState === "docked-expanded") void retract();
  }, RETRACT_DELAY_MS);
}

/** Rust CloseRequested in background mode lands here: close = retract (dock if still floating). */
async function handleCloseFromRust(): Promise<void> {
  if (dockState === "floating") {
    const docked = await enterDock();
    if (!docked) {
      try {
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

  return () => {
    for (const un of unlisteners) un();
    root.removeEventListener("mouseenter", onPointerEnter);
    root.removeEventListener("mouseleave", onPointerLeave);
    cancelRetract();
  };
}
