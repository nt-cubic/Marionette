/**
 * usageSchedule - the reset-aware "send after this limit resets" request.
 *
 * This module builds an explicit payload; it is **not** a scheduler and it
 * never retries anything. The payload is shaped so the host can hand it to the
 * existing scheduled-send path:
 *
 *   handleScheduleSend(payload.atMs, payload.text)
 *
 * When no host callback is wired yet, the request is stored under a dedicated
 * localStorage key so the action is not a silent no-op. The conversation's
 * existing scheduled-send strip remains the only place to cancel it.
 */

export type ScheduleAfterResetLimit = {
  provider: string;
  entryName: string;
  windowName: string;
  resetsAtMs: number;
  usedPct: number | null;
};

export type ScheduleAfterResetPayload = {
  /** When the existing scheduler should send; one minute after the reset. */
  atMs: number;
  /** The explicit text the user typed; there is no canned auto-retry prompt. */
  text: string;
  source: "usage-limit-reset";
  limit: ScheduleAfterResetLimit;
};

export const USAGE_SCHEDULE_REQUEST_KEY = "marionette-usage-schedule-request";

const MIN_LEAD_MS = 6_000;
const MAX_FUTURE_MS = 60 * 24 * 60 * 60 * 1000;
const RESET_BUFFER_MS = 60_000;
const MAX_TEXT = 4_000;
const MAX_REQUESTS = 20;

/** A reset time we are willing to act on: parseable, future, not absurdly far. */
export function parseTrustworthyReset(
  value: string | number | null | undefined,
  now = Date.now(),
): number | null {
  if (value == null) return null;
  const ms = typeof value === "number" ? value : Date.parse(value);
  if (!Number.isFinite(ms)) return null;
  if (ms <= now + MIN_LEAD_MS) return null;
  if (ms > now + MAX_FUTURE_MS) return null;
  return Math.floor(ms);
}

export function buildScheduleAfterResetPayload(
  limit: Omit<ScheduleAfterResetLimit, "resetsAtMs"> & { resetsAtMs: number },
  text: string,
  now = Date.now(),
): ScheduleAfterResetPayload | null {
  const trimmed = text.trim().slice(0, MAX_TEXT);
  if (!trimmed) return null;
  const reset = parseTrustworthyReset(limit.resetsAtMs, now);
  if (reset == null) return null;
  const atMs = Math.max(reset + RESET_BUFFER_MS, now + MIN_LEAD_MS);
  return {
    atMs,
    text: trimmed,
    source: "usage-limit-reset",
    limit: {
      provider: limit.provider,
      entryName: limit.entryName,
      windowName: limit.windowName,
      resetsAtMs: reset,
      usedPct: limit.usedPct,
    },
  };
}

/** The argument pair the existing scheduled-send handler takes. */
export function schedulePayloadSendArgs(payload: ScheduleAfterResetPayload): { atMs: number; text: string } {
  return { atMs: payload.atMs, text: payload.text };
}

function isPayload(value: unknown): value is ScheduleAfterResetPayload {
  if (!value || typeof value !== "object") return false;
  const row = value as ScheduleAfterResetPayload;
  return (
    typeof row.atMs === "number" &&
    Number.isFinite(row.atMs) &&
    typeof row.text === "string" &&
    row.text.trim() !== "" &&
    row.source === "usage-limit-reset" &&
    !!row.limit &&
    typeof row.limit.provider === "string" &&
    typeof row.limit.windowName === "string" &&
    typeof row.limit.resetsAtMs === "number"
  );
}

export function readUsageScheduleRequests(): ScheduleAfterResetPayload[] {
  if (typeof window === "undefined") return [];
  try {
    const raw = window.localStorage.getItem(USAGE_SCHEDULE_REQUEST_KEY);
    if (!raw) return [];
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed.filter(isPayload).slice(0, MAX_REQUESTS);
  } catch {
    return [];
  }
}

/**
 * Persist one pending request, replacing an older request for the same
 * provider/window. This is a hand-off record, not a running timer.
 */
export function stashUsageScheduleRequest(payload: ScheduleAfterResetPayload): boolean {
  if (typeof window === "undefined") return false;
  try {
    const key = `${payload.limit.provider}\u0000${payload.limit.windowName}`;
    const rest = readUsageScheduleRequests().filter(
      (row) => `${row.limit.provider}\u0000${row.limit.windowName}` !== key,
    );
    window.localStorage.setItem(
      USAGE_SCHEDULE_REQUEST_KEY,
      JSON.stringify([payload, ...rest].slice(0, MAX_REQUESTS)),
    );
    return true;
  } catch {
    return false;
  }
}

/** Host-side helper: remove a request once it has really been enqueued. */
export function clearUsageScheduleRequest(payload: ScheduleAfterResetPayload): void {
  if (typeof window === "undefined") return;
  try {
    const rest = readUsageScheduleRequests().filter(
      (row) =>
        !(
          row.limit.provider === payload.limit.provider &&
          row.limit.windowName === payload.limit.windowName &&
          row.limit.resetsAtMs === payload.limit.resetsAtMs
        ),
    );
    if (rest.length === 0) window.localStorage.removeItem(USAGE_SCHEDULE_REQUEST_KEY);
    else window.localStorage.setItem(USAGE_SCHEDULE_REQUEST_KEY, JSON.stringify(rest));
  } catch {
    /* the hand-off record is best effort */
  }
}
