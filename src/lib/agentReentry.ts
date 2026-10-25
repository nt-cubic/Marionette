/**
 * Agent re-entry（再次进入对话）— the harness already answered (Marionette saw
 * `turn/complete`, sealed the Reply, freed the composer), but a background
 * command it started came back, so the agent resumes working on its own: no
 * client prompt is in flight, and for several adapters no turn boundary ever
 * follows. Measured on zcode-acp-server 0.76.2 (2026-10-24, session
 * 1792813095629): after the prompt response returned, a `bg_*` tool completed
 * 54 s later and the agent streamed a fresh Reply with no turnState and no
 * turn/complete — today that card never seals and the dialog never shows as
 * active again.
 *
 * Detection is wire-generic on purpose (every ACP walks the same path): a
 * transcript chunk (assistant / thought) arriving while Marionette considers
 * the session idle means the agent is producing again. Per-agent channels
 * (zcode `$/zcode/turnState`, grok `_meta.turnStartMs`) exist but are uneven —
 * zcode does not emit its own turnState around background continuations — so
 * nothing here keys on them.
 */

import type { SessionEvent } from "./types";
import { findLastIndexForSession } from "./acpTranscript";

/**
 * A chunk landing this soon after `turn/complete` is a late echo of the turn
 * that just ended (adapters race their final updates against the prompt
 * response), not a re-entry. Real background continuations take at least one
 * command round-trip.
 */
export const REENTRY_GRACE_MS = 2_500;

/**
 * No adapter guarantees an end signal for a re-entry segment, so it ends on
 * quiet: long tools and slow models are routinely silent for minutes
 * (activityHealth only starts calling that "quiet" at 5), and a chunk that
 * arrives later simply arms a fresh segment.
 */
export const REENTRY_IDLE_MS = 6 * 60_000;

/**
 * True when an assistant/thought chunk should re-arm an idle session.
 * `turnEndedAtMs == null` (restored dialogs, no live process) never arms —
 * a stray chunk there must not invent activity.
 */
export function shouldArmAgentReentry(
  status: string | undefined,
  turnEndedAtMs: number | null | undefined,
  now: number = Date.now(),
): boolean {
  if (status !== "waiting") return false;
  if (turnEndedAtMs == null || !Number.isFinite(turnEndedAtMs)) return false;
  return now - turnEndedAtMs >= REENTRY_GRACE_MS;
}

/**
 * Seal the segment the way `turn/complete` does: the session's last card gets
 * `durationMs` (Reply) or `endedAt` (Thought). Earlier mid-turn cards stay
 * unsealed, exactly like a normal multi-card turn — only the tail is the end.
 */
export function sealAgentSegmentForIdle(
  events: SessionEvent[],
  sessionId: string,
  endedAtMs: number = Date.now(),
): SessionEvent[] {
  const idx = findLastIndexForSession(events, sessionId);
  if (idx < 0) return events;
  const last = events[idx]!;
  if (last.type === "assistant_message" && last.durationMs == null) {
    const started = Date.parse(last.createdAt);
    const durationMs =
      Number.isFinite(started) && started > 0 ? Math.max(0, endedAtMs - started) : 0;
    const next = [...events];
    next[idx] = {
      ...last,
      durationMs,
      endedAt: new Date(endedAtMs).toISOString(),
    };
    return next;
  }
  if (last.type === "thought" && last.endedAt == null) {
    const next = [...events];
    next[idx] = {
      ...last,
      endedAt: new Date(endedAtMs).toISOString(),
    };
    return next;
  }
  return events;
}
