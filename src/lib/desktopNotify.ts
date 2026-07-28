/**
 * Desktop attention when the user is looking away:
 *  - AI finished a reply
 *  - turn went quiet long enough to look stuck
 *
 * Windows: taskbar flash + yellow/red progress overlay (QQ/WeChat-ish).
 * Sound: short WebAudio chime (no asset file).
 * Cleared when the window is focused again.
 */

import {
  getCurrentWindow,
  ProgressBarStatus,
  UserAttentionType,
} from "@tauri-apps/api/window";
import { isTauriRuntime } from "./api";

const STORAGE_KEY = "marionette-desktop-notify";
const BASE_TITLE = "Marionette";

export type NotifyKind = "reply" | "stuck";

export type DesktopNotifyState = {
  enabled: boolean;
  /** Currently showing attention (until focus clears it). */
  pending: NotifyKind | null;
  lastDetail: string | null;
};

type Listener = (state: DesktopNotifyState) => void;

let enabled = readEnabled();
let pending: NotifyKind | null = null;
let lastDetail: string | null = null;
const listeners = new Set<Listener>();

/** Dedupe: don't re-chime the same kind within a short window. */
const lastRaisedAt: Partial<Record<NotifyKind, number>> = {};
const DEDUPE_MS = 2500;

function readEnabled(): boolean {
  try {
    const raw =
      window.localStorage.getItem(STORAGE_KEY) ??
      window.localStorage.getItem("agentshell-desktop-notify");
    if (raw === null) return true; // default on — useful out of the box
    return raw === "1" || raw === "true";
  } catch {
    return true;
  }
}

function writeEnabled(value: boolean): void {
  try {
    window.localStorage.setItem(STORAGE_KEY, value ? "1" : "0");
  } catch {
    // private mode
  }
}

function snapshot(): DesktopNotifyState {
  return { enabled, pending, lastDetail };
}

function emit(): void {
  const s = snapshot();
  for (const fn of listeners) {
    try {
      fn(s);
    } catch {
      // ignore subscriber errors
    }
  }
}

export function getDesktopNotifyState(): DesktopNotifyState {
  return snapshot();
}

export function isDesktopNotifyEnabled(): boolean {
  return enabled;
}

export function setDesktopNotifyEnabled(value: boolean): void {
  enabled = value;
  writeEnabled(value);
  if (!value) {
    void clearDesktopNotify({ silent: true });
  }
  emit();
}

export function subscribeDesktopNotify(fn: Listener): () => void {
  listeners.add(fn);
  return () => {
    listeners.delete(fn);
  };
}

/** Soft two-tone (reply) or lower warning (stuck). */
export function playNotifySound(kind: NotifyKind): void {
  try {
    const AudioCtx =
      window.AudioContext ||
      (window as unknown as { webkitAudioContext?: typeof AudioContext }).webkitAudioContext;
    if (!AudioCtx) return;
    const ctx = new AudioCtx();
    const now = ctx.currentTime;
    const gain = ctx.createGain();
    gain.connect(ctx.destination);
    gain.gain.setValueAtTime(0.0001, now);
    gain.gain.exponentialRampToValueAtTime(0.14, now + 0.02);

    const beep = (freq: number, start: number, dur: number) => {
      const osc = ctx.createOscillator();
      osc.type = "sine";
      osc.frequency.setValueAtTime(freq, start);
      osc.connect(gain);
      osc.start(start);
      osc.stop(start + dur);
    };

    if (kind === "reply") {
      // Short "done" chime
      beep(880, now, 0.1);
      beep(1174.66, now + 0.1, 0.16);
      gain.gain.exponentialRampToValueAtTime(0.0001, now + 0.32);
    } else {
      // Lower warning
      beep(392, now, 0.14);
      beep(311.13, now + 0.16, 0.22);
      gain.gain.exponentialRampToValueAtTime(0.0001, now + 0.45);
    }

    window.setTimeout(() => {
      void ctx.close().catch(() => undefined);
    }, 600);
  } catch {
    // Autoplay / missing API — ignore
  }
}

async function win() {
  if (!isTauriRuntime()) return null;
  try {
    return getCurrentWindow();
  } catch {
    return null;
  }
}

export async function isAppWindowFocused(): Promise<boolean> {
  // Browser / preview: document.hasFocus is good enough.
  if (!isTauriRuntime()) return document.hasFocus();
  try {
    const w = await win();
    if (!w) return document.hasFocus();
    return await w.isFocused();
  } catch {
    return document.hasFocus();
  }
}

/**
 * Raise attention if the user is away and notifications are on.
 * No-op when already focused (you're watching the stream).
 */
export async function raiseDesktopNotify(
  kind: NotifyKind,
  detail?: string | null,
): Promise<void> {
  if (!enabled) return;

  const focused = await isAppWindowFocused();
  if (focused) return;

  const now = Date.now();
  const prev = lastRaisedAt[kind] ?? 0;
  // Allow stuck → reply upgrade; throttle same-kind spam.
  if (kind === pending && now - prev < DEDUPE_MS) return;
  if (now - prev < DEDUPE_MS && pending === kind) return;
  lastRaisedAt[kind] = now;

  pending = kind;
  lastDetail = detail?.trim() || null;
  emit();

  playNotifySound(kind);

  // Title badge (visible on taskbar hover / alt-tab)
  const prefix = kind === "stuck" ? "⚠ " : "● ";
  const title = lastDetail
    ? `${prefix}${lastDetail} · ${BASE_TITLE}`
    : kind === "stuck"
      ? `${prefix}Agent may be stuck · ${BASE_TITLE}`
      : `${prefix}Reply ready · ${BASE_TITLE}`;
  document.title = title;

  const w = await win();
  if (!w) return;

  try {
    await w.setTitle(title);
  } catch {
    // permission / non-tauri
  }

  // Windows: flash taskbar until focused (Critical keeps flashing).
  try {
    await w.requestUserAttention(UserAttentionType.Critical);
  } catch {
    // ignore
  }

  // Windows taskbar progress overlay:
  //  - Paused  → yellow (reply)
  //  - Error   → red (stuck)
  try {
    await w.setProgressBar({
      status: kind === "stuck" ? ProgressBarStatus.Error : ProgressBarStatus.Paused,
      progress: 100,
    });
  } catch {
    // ignore
  }
}

/** Clear flash / yellow bar / title when the user comes back. */
export async function clearDesktopNotify(opts?: { silent?: boolean }): Promise<void> {
  if (pending == null && opts?.silent) {
    // still reset title/progress defensively when turning the feature off
  }
  pending = null;
  lastDetail = null;
  if (!opts?.silent) emit();
  else emit();

  document.title = BASE_TITLE;

  const w = await win();
  if (!w) return;

  try {
    await w.setTitle(BASE_TITLE);
  } catch {
    // ignore
  }
  try {
    await w.requestUserAttention(null);
  } catch {
    // ignore
  }
  try {
    await w.setProgressBar({ status: ProgressBarStatus.None });
  } catch {
    // ignore
  }
}

/**
 * Call once from App: clear attention on focus; optional visibility fallback.
 */
export function bindDesktopNotifyFocusHandlers(): () => void {
  let unlistenFocus: (() => void) | undefined;
  let disposed = false;

  void (async () => {
    const w = await win();
    if (!w || disposed) return;
    try {
      unlistenFocus = await w.onFocusChanged(({ payload: focused }) => {
        if (focused) void clearDesktopNotify();
      });
    } catch {
      // fall through to document events
    }
  })();

  const onVis = () => {
    if (document.visibilityState === "visible" && document.hasFocus()) {
      void clearDesktopNotify();
    }
  };
  const onFocus = () => {
    void clearDesktopNotify();
  };
  window.addEventListener("focus", onFocus);
  document.addEventListener("visibilitychange", onVis);

  return () => {
    disposed = true;
    unlistenFocus?.();
    window.removeEventListener("focus", onFocus);
    document.removeEventListener("visibilitychange", onVis);
  };
}
