/**
 * Desktop attention when the user is looking away.
 *
 * OpenCode's notifier uses short bundled WAV cues. Keep the same cues in the
 * frontend so Marionette does not depend on the user's OpenCode installation:
 * action-needed events are immediate, clean turn completion settles briefly,
 * and failures/stalls raise the error cue at once.
 *
 * Windows: taskbar flash + yellow/red progress overlay (QQ/WeChat-ish).
 * Cleared when the window is focused again.
 */

import {
  getCurrentWindow,
  ProgressBarStatus,
  UserAttentionType,
} from "@tauri-apps/api/window";
import { isTauriRuntime } from "./api";
import completeSoundUrl from "../assets/notifications/complete.wav";
import errorSoundUrl from "../assets/notifications/error.wav";
import permissionSoundUrl from "../assets/notifications/permission.wav";
import questionSoundUrl from "../assets/notifications/question.wav";
import subagentCompleteSoundUrl from "../assets/notifications/subagent_complete.wav";

const STORAGE_KEY = "marionette-desktop-notify";
const BASE_TITLE = "Marionette";
/** Same event debounce as OpenCode notifier's sound/notification paths. */
const DEDUPE_MS = 1000;
/** Match OpenCode notifier's session.idle settling window. */
export const DESKTOP_NOTIFY_SETTLE_MS = 350;

const SOUND_URLS: Partial<Record<NotifyKind, string>> = {
  reply: completeSoundUrl,
  permission: permissionSoundUrl,
  question: questionSoundUrl,
  // OpenCode 0.2.8 does not ship a separate plan cue; use its positive chime.
  plan: completeSoundUrl,
  error: errorSoundUrl,
  stuck: errorSoundUrl,
  process: errorSoundUrl,
  subagent_complete: subagentCompleteSoundUrl,
};

const ERROR_KINDS = new Set<NotifyKind>(["error", "process", "stuck"]);
const ACTION_KINDS = new Set<NotifyKind>(["permission", "question", "plan"]);

export type NotifyKind =
  | "reply"
  | "stuck"
  | "permission"
  | "question"
  | "plan"
  | "error"
  | "process"
  | "subagent_complete";

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
const scheduledRaises = new Map<string, ReturnType<typeof setTimeout>>();
const activeAudio = new Set<HTMLAudioElement>();

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
    cancelAllScheduledDesktopNotifies();
    for (const audio of activeAudio) {
      try {
        audio.pause();
        audio.currentTime = 0;
      } catch {
        // ignore
      }
    }
    activeAudio.clear();
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

/** Play the exact short WAV cue shipped by OpenCode's notifier. */
export function playNotifySound(kind: NotifyKind): void {
  const soundUrl = SOUND_URLS[kind];
  if (!soundUrl) return;

  try {
    const audio = new Audio(soundUrl);
    audio.preload = "auto";
    activeAudio.add(audio);
    const cleanup = () => {
      activeAudio.delete(audio);
      audio.removeEventListener("ended", cleanup);
    };
    audio.addEventListener("ended", cleanup, { once: true });
    // Keep an audio object alive for the duration of the cue, but do not let a
    // failed/blocked WebView playback accumulate objects forever.
    window.setTimeout(cleanup, 3000);
    void audio.play().catch(cleanup);
  } catch {
    // Autoplay policy / missing API / malformed asset — ignore.
  }
}

/** Schedule a completion-style alert after the stream has settled. */
export function scheduleDesktopNotify(
  key: string,
  kind: NotifyKind,
  detail?: string | null,
  delayMs = DESKTOP_NOTIFY_SETTLE_MS,
): void {
  cancelScheduledDesktopNotify(key);
  const timer = setTimeout(() => {
    scheduledRaises.delete(key);
    void raiseDesktopNotify(kind, detail);
  }, Math.max(0, delayMs));
  scheduledRaises.set(key, timer);
}

/** Cancel a delayed completion when the session starts another turn. */
export function cancelScheduledDesktopNotify(key: string): void {
  const timer = scheduledRaises.get(key);
  if (!timer) return;
  clearTimeout(timer);
  scheduledRaises.delete(key);
}

function cancelAllScheduledDesktopNotifies(): void {
  for (const timer of scheduledRaises.values()) clearTimeout(timer);
  scheduledRaises.clear();
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
  // Allow a higher-priority event to replace a previous one; throttle
  // same-kind spam (the OpenCode plugin uses a 1s event debounce).
  if (kind === pending && now - prev < DEDUPE_MS) return;
  lastRaisedAt[kind] = now;

  pending = kind;
  lastDetail = detail?.trim() || null;
  emit();

  playNotifySound(kind);

  // Title badge (visible on taskbar hover / alt-tab)
  const prefix = ERROR_KINDS.has(kind) ? "⚠ " : ACTION_KINDS.has(kind) ? "! " : "● ";
  const fallback =
    kind === "stuck"
      ? "Agent may be stuck"
      : kind === "reply"
        ? "Reply ready"
        : kind === "subagent_complete"
          ? "Subagent finished"
          : kind === "permission"
            ? "Permission required"
            : kind === "question"
              ? "Agent has a question"
              : kind === "plan"
                ? "Plan ready for review"
                : kind === "process"
                  ? "Agent process ended"
                  : "Agent error";
  const title = lastDetail
    ? `${prefix}${lastDetail} · ${BASE_TITLE}`
    : `${prefix}${fallback} · ${BASE_TITLE}`;
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
  //  - Paused  → yellow (reply / action needed)
  //  - Error   → red (failure / stuck)
  try {
    await w.setProgressBar({
      status: ERROR_KINDS.has(kind) ? ProgressBarStatus.Error : ProgressBarStatus.Paused,
      progress: 100,
    });
  } catch {
    // ignore
  }
}

/**
 * Clear flash / yellow bar / title when the user comes back.
 *
 * IMPORTANT — Windows / tao 0.35.3 (tao#1180 / tauri#14088):
 * Calling Tauri window APIs during focus / paint re-enters the Win32 pump and
 * trips `assert!(flush_paint_messages(..))` → process exit 101.
 *
 * Rules:
 *  - JS state (title, pending) updates immediately — no native.
 *  - Native clear only if we actually raised attention (`hadPending`).
 *  - Never call `requestUserAttention(null)` — worst offender on focus.
 *  - Defer remaining native work well after the focus paint settles.
 */
let clearInflight = false;
let clearTimer: ReturnType<typeof setTimeout> | null = null;

/** Delay before any native clear; focus paint must fully finish first. */
const NATIVE_CLEAR_DELAY_MS = 400;

export async function clearDesktopNotify(opts?: { silent?: boolean }): Promise<void> {
  // JS-side state can update immediately (cheap, no Win32 re-entry).
  const hadPending = pending != null;
  pending = null;
  lastDetail = null;
  emit();
  document.title = BASE_TITLE;

  if (clearTimer != null) {
    clearTimeout(clearTimer);
    clearTimer = null;
  }

  // No native work needed: we never raised attention, and this isn't a
  // "force reset native chrome" (feature toggle off) call.
  if (!hadPending && !opts?.silent) {
    return;
  }

  if (clearInflight) return;
  clearInflight = true;

  await new Promise<void>((resolve) => {
    clearTimer = setTimeout(() => {
      clearTimer = null;
      resolve();
    }, NATIVE_CLEAR_DELAY_MS);
  });

  try {
    const w = await win();
    if (!w) return;

    // Title only — cheap compared to attention/progress, still deferred.
    try {
      await w.setTitle(BASE_TITLE);
    } catch {
      // ignore
    }
    // Progress bar only. Do NOT call requestUserAttention(null): clearing
    // flash via null attention re-enters the message loop during focus and
    // panics tao 0.35.3. Flash stops on its own once the window is focused
    // (Critical attention) or when the process exits.
    try {
      await w.setProgressBar({ status: ProgressBarStatus.None });
    } catch {
      // ignore
    }
  } finally {
    clearInflight = false;
  }
}

/**
 * Call once from App: clear attention on focus; optional visibility fallback.
 */
export function bindDesktopNotifyFocusHandlers(): () => void {
  let unlistenFocus: (() => void) | undefined;
  let disposed = false;
  /** Coalesce focus + visibility + onFocusChanged into one clear. */
  let scheduled = false;
  let scheduleTimer: ReturnType<typeof setTimeout> | null = null;

  const scheduleClear = () => {
    if (disposed || scheduled) return;
    // Skip entirely when there is nothing to clear — avoids native Win32
    // calls on every alt-tab / click-back (the main crash path).
    if (pending == null) return;
    scheduled = true;
    // Defer past focus paint; do not use rAF alone (still inside paint frame).
    if (scheduleTimer != null) clearTimeout(scheduleTimer);
    scheduleTimer = setTimeout(() => {
      scheduleTimer = null;
      scheduled = false;
      if (disposed) return;
      if (pending == null) return;
      void clearDesktopNotify();
    }, NATIVE_CLEAR_DELAY_MS);
  };

  void (async () => {
    const w = await win();
    if (!w || disposed) return;
    try {
      unlistenFocus = await w.onFocusChanged(({ payload: focused }) => {
        if (focused) scheduleClear();
      });
    } catch {
      // fall through to document events
    }
  })();

  const onVis = () => {
    if (document.visibilityState === "visible" && document.hasFocus()) {
      scheduleClear();
    }
  };
  const onFocus = () => {
    scheduleClear();
  };
  window.addEventListener("focus", onFocus);
  document.addEventListener("visibilitychange", onVis);

  return () => {
    disposed = true;
    unlistenFocus?.();
    window.removeEventListener("focus", onFocus);
    document.removeEventListener("visibilitychange", onVis);
    if (scheduleTimer != null) {
      clearTimeout(scheduleTimer);
      scheduleTimer = null;
    }
    if (clearTimer != null) {
      clearTimeout(clearTimer);
      clearTimer = null;
    }
  };
}
