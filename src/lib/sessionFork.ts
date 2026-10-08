import type { SessionEvent } from "./types";

/** A reply the user forks from: the new dialog keeps everything up to it. */
export type ForkAnchor = {
  /** Reply text — the fallback matcher when the agent sent no message id. */
  text: string;
  messageId?: string;
};

/**
 * Index of the reply a fork starts from: the last assistant row that matches.
 *
 * Message ids are the primary key. Agents that send none — or churn a fresh id
 * per token — fall back to the reply text, where the *last* match is the row
 * the user clicked.
 */
export function forkCutIndex(events: SessionEvent[], anchor: ForkAnchor): number {
  for (let i = events.length - 1; i >= 0; i -= 1) {
    const event = events[i];
    if (event.type !== "assistant_message") continue;
    if (anchor.messageId ? event.messageId === anchor.messageId : event.text === anchor.text) {
      return i;
    }
  }
  return -1;
}

/** Rows that must not travel into a fork — they point at the old dialog. */
const FORK_SKIPPED = new Set(["subtask_started", "subtask_result", "handoff_prepared"]);

/**
 * The new dialog's transcript: every row up to and including that reply,
 * re-keyed to `targetSessionId`. Null when the anchor no longer exists.
 */
export function forkCopyEvents(
  events: SessionEvent[],
  anchor: ForkAnchor,
  targetSessionId: string,
): SessionEvent[] | null {
  const cut = forkCutIndex(events, anchor);
  if (cut < 0) return null;
  const kept = events
    .slice(0, cut + 1)
    .filter((event) => !FORK_SKIPPED.has(event.type))
    .map((event) => ({ ...event, sessionId: targetSessionId }) as SessionEvent);
  return kept.length > 0 ? kept : null;
}
