/**
 * Edge dock (docs/plan-edge-dock.md): the main window's "docked mode", not a
 * second window — docking is the same SPA narrowing into a vertical strip.
 *
 * Stage 0 — background residency: with `autoHideOnClose` on, closing the
 * window hides it instead of exiting (engine, sessions and the Telegram
 * bridge keep running); true exit is the footer's 退出 button (`quit_app`).
 * The Rust side mirrors the flag so its CloseRequested branch knows the path.
 *
 * Prefs live in localStorage under `marionette-dock-prefs` and already carry
 * the full stage-1+ shape (mode/edge/width), so later stages extend them
 * instead of migrating.
 */

import { invoke } from "@tauri-apps/api/core";
import { isTauriRuntime } from "./api";

const STORAGE_KEY = "marionette-dock-prefs";

export type DockEdge = "left" | "right";

export type DockPrefs = {
  /** "dock" = window is docked to a screen edge (stage 1+); "off" = floating. */
  mode: "off" | "dock";
  edge: DockEdge;
  /** Strip width in logical px, 320–480 (stage 1+). */
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
      autoHideOnClose: typeof parsed.autoHideOnClose === "boolean" ? parsed.autoHideOnClose : DEFAULT_PREFS.autoHideOnClose,
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
