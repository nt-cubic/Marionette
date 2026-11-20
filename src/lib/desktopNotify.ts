/**
 * Desktop attention when the user is looking away.
 *
 * OpenCode's notifier uses short bundled WAV cues. Keep the same cues in the
 * frontend so Marionette does not depend on the user's OpenCode installation:
 * action-needed events are immediate, clean turn completion settles briefly,
 * failures/stalls raise the error cue at once, and completion / silent-stop
 * chimes also play while the window is focused.
 *
 * ACP reply/permission/question/plan alerts are owned by Rust
 * (`src-tauri/src/desktop_notify.rs`) so they still fire while WebView2 is
 * suspended. This module handles frontend-only signals and the shared toggle;
 * do not add a second ACP dispatch here.
 *
 * Windows: taskbar flash + yellow/red progress overlay (QQ/WeChat-ish).
 * Cleared when the window is focused again.
 */

import {
  getCurrentWindow,
  ProgressBarStatus,
  UserAttentionType,
} from "@tauri-apps/api/window";
import { invoke } from "@tauri-apps/api/core";
import { appendDebugLog, isTauriRuntime } from "./api";
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
  // Agent stopped on its own with nothing to show — same positive chime.
  idle: completeSoundUrl,
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
  | "idle"
  | "stuck"
  | "permission"
  | "question"
  | "plan"
  | "error"
  | "process"
  | "subagent_complete";

export type RaiseOptions = {
  /**
   * Play the cue even when the window is focused (no taskbar chrome then).
   * Used for turn completion and silent stop: the user may be in another
   * dialog, or watching the stream, and still wants the chime.
   */
  audibleWhenFocused?: boolean;
};

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

/**
 * Native chrome (taskbar bar + title badge) is tracked separately from
 * `pending`: a raise can land while the focus signal is stale, and the
 * renderer can be reloaded while the shell keeps the bar painted. Either way
 * the window still has chrome that `pending` alone would not clear.
 */
let nativeChrome = false;
/** Focus backstop while chrome is up — DOM focus events can be missed across
 *  minimize/restore and WebView2 suspension, and the bar must never stick. */
let chromeWatch: ReturnType<typeof setInterval> | null = null;
const CHROME_FOCUS_POLL_MS = 2000;

function syncChromeWatch(): void {
  if (nativeChrome) {
    if (chromeWatch != null) return;
    chromeWatch = setInterval(() => {
      if (!nativeChrome) {
        stopChromeWatch();
        return;
      }
      void isAppWindowFocused().then((focused) => {
        if (focused && nativeChrome) void clearDesktopNotify();
      });
    }, CHROME_FOCUS_POLL_MS);
    return;
  }
  stopChromeWatch();
}

function stopChromeWatch(): void {
  if (chromeWatch == null) return;
  clearInterval(chromeWatch);
  chromeWatch = null;
}

function logChrome(summary: string, detail: string): void {
  void appendDebugLog({ source: "notify", summary, detail });
}

function readEnabled(): boolean {
  try {
    const own = window.localStorage.getItem(STORAGE_KEY);
    if (own !== null) return own === "1" || own === "true";
    const legacy = window.localStorage.getItem("agentshell-desktop-notify");
    if (legacy === null) return true; // default on — useful out of the box
    window.localStorage.removeItem("agentshell-desktop-notify");
    return legacy === "1" || legacy === "true";
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
  syncDesktopNotifyBackend(value);
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

/**
 * The ACP dispatcher owns background alerts because WebView2 is suspended
 * while minimized/hidden. Keep its in-memory switch in lockstep with the UI
 * preference; older dev frontends without the command safely no-op.
 */
export function syncDesktopNotifyBackend(value = enabled): void {
  if (!isTauriRuntime()) return;
  void invoke("desktop_notify_set_enabled", { enabled: value }).catch(() => undefined);
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
  opts?: RaiseOptions,
): void {
  cancelScheduledDesktopNotify(key);
  const timer = setTimeout(() => {
    scheduledRaises.delete(key);
    void raiseDesktopNotify(kind, detail, opts);
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

/**
 * Is the user looking at this window?
 *
 * The page-level focus comes first. On Windows tao tracks the focus of the
 * host window, and after the webview is suspended and resumed (minimize →
 * `TrySuspend` → restore) that flag stays false while the page itself is
 * focused. Trusting it alone meant "you are away" for a user sitting in the
 * window: every completion raised the yellow bar and nothing ever cleared it,
 * because no further focus transition happened.
 */
export async function isAppWindowFocused(): Promise<boolean> {
  if (document.hasFocus()) return true;
  if (!isTauriRuntime()) return false;
  try {
    const w = await win();
    if (!w) return false;
    return await w.isFocused();
  } catch {
    return false;
  }
}

/**
 * Raise attention if the user is away and notifications are on.
 * No-op when already focused (you're watching the stream) — unless the caller
 * marks the cue `audibleWhenFocused`, which then plays sound only.
 */
export async function raiseDesktopNotify(
  kind: NotifyKind,
  detail?: string | null,
  opts?: RaiseOptions,
): Promise<void> {
  if (!enabled) return;

  const now = Date.now();
  const prev = lastRaisedAt[kind] ?? 0;
  // Allow a higher-priority event to replace a previous one; throttle
  // same-kind spam (the OpenCode plugin uses a 1s event debounce).
  if (kind === pending && now - prev < DEDUPE_MS) return;

  const focused = await isAppWindowFocused();
  if (focused) {
    // Taskbar chrome is pointless while the user is looking at the window, but
    // an audible cue still plays — with the same 1s same-kind throttle.
    if (nativeChrome) void clearDesktopNotify();
    if (!opts?.audibleWhenFocused) return;
    if (now - prev < DEDUPE_MS) return;
    lastRaisedAt[kind] = now;
    playNotifySound(kind);
    return;
  }

  lastRaisedAt[kind] = now;

  pending = kind;
  lastDetail = detail?.trim() || null;
  nativeChrome = true;
  syncChromeWatch();
  emit();
  logChrome("attention raised", `kind=${kind}`);

  playNotifySound(kind);

  // Title badge (visible on taskbar hover / alt-tab)
  const prefix = ERROR_KINDS.has(kind) ? "⚠ " : ACTION_KINDS.has(kind) ? "! " : "● ";
  const fallback =
    kind === "stuck"
      ? "Agent may be stuck"
      : kind === "reply"
        ? "Reply ready"
        : kind === "idle"
          ? "Agent stopped"
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

  // The user may have come back while those IPC calls were in flight — the
  // focus event already fired before `pending` existed, so nothing else would
  // clear the bar. Drop it now instead of leaving it until the next alt-tab.
  if (await isAppWindowFocused()) void clearDesktopNotify();
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
 *  - Native clear whenever this window may own chrome (`nativeChrome`), not
 *    just while `pending` happens to be set.
 *  - Never call `requestUserAttention(null)` — worst offender on focus.
 *  - Defer remaining native work well after the focus paint settles.
 */
let clearInflight = false;
let clearTimer: ReturnType<typeof setTimeout> | null = null;

/** Delay before any native clear; focus paint must fully finish first. */
const NATIVE_CLEAR_DELAY_MS = 400;

export async function clearDesktopNotify(opts?: { silent?: boolean }): Promise<void> {
  // JS-side state can update immediately (cheap, no Win32 re-entry).
  const clearedKind = pending;
  const hadChrome = nativeChrome;
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
  if (clearedKind == null && !hadChrome && !opts?.silent) {
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
    // A newer notification landed while we waited — it owns the chrome now.
    if (pending != null) return;

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
    if (hadChrome) logChrome("attention cleared", `kind=${clearedKind ?? "stale"}`);
  } finally {
    clearInflight = false;
    // Keep the bookkeeping when a newer notification took over the bar; a
    // failed native clear still drops it so the watcher cannot spin forever.
    if (pending == null) {
      nativeChrome = false;
      syncChromeWatch();
    }
  }
}

/**
 * Call once from App: clear attention on focus; optional visibility fallback.
 */
export function bindDesktopNotifyFocusHandlers(): () => void {
  syncDesktopNotifyBackend();
  let unlistenFocus: (() => void) | undefined;
  let disposed = false;
  /** Coalesce focus + visibility + onFocusChanged into one clear. */
  let scheduled = false;
  let scheduleTimer: ReturnType<typeof setTimeout> | null = null;

  const scheduleClear = () => {
    if (disposed || scheduled) return;
    // Skip entirely when there is nothing to clear — avoids native Win32
    // calls on every alt-tab / click-back (the main crash path). Compound
    // with `nativeChrome` so a stale bar (reload, missed event) still goes.
    if (pending == null && !nativeChrome) return;
    scheduled = true;
    // Defer past focus paint; do not use rAF alone (still inside paint frame).
    if (scheduleTimer != null) clearTimeout(scheduleTimer);
    scheduleTimer = setTimeout(() => {
      scheduleTimer = null;
      scheduled = false;
      if (disposed) return;
      if (pending == null && !nativeChrome) return;
      void clearDesktopNotify();
    }, NATIVE_CLEAR_DELAY_MS);
  };

  // A page reload (dev HMR, the crash screen's Reload button) loses the JS
  // bookkeeping while the shell keeps our bar painted. Reset it once here;
  // the call is a no-op when nothing was raised.
  void clearDesktopNotify({ silent: true });

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
