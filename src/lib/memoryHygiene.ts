/**
 * Memory hygiene that must not change user-visible behaviour.
 *
 * Policy:
 * - Never drop events for open tabs, the active dialog, live ACP sessions,
 *   or in-flight @-delegate children.
 * - Cold sessions: after a successful transcript flush, drop their events from
 *   the in-memory array. Re-open reloads from disk (same content).
 */

import type { Session, SessionEvent } from "./types";

export type DelegateMetaLike = {
  finished?: boolean;
  parentId?: string;
};

export function collectHotSessionIds(args: {
  currentSessionId: string | undefined | null;
  openSessionIds: string[];
  sessions: Session[];
  delegateMeta: Iterable<[string, DelegateMetaLike]>;
}): Set<string> {
  const hot = new Set<string>();
  if (args.currentSessionId) hot.add(args.currentSessionId);
  for (const id of args.openSessionIds) {
    if (id) hot.add(id);
  }
  for (const s of args.sessions) {
    if (
      s.status === "running" ||
      s.status === "starting" ||
      s.status === "waiting"
    ) {
      hot.add(s.id);
    }
  }
  for (const [childId, meta] of args.delegateMeta) {
    if (meta && !meta.finished) {
      hot.add(childId);
      if (meta.parentId) hot.add(meta.parentId);
    }
  }
  // Child event streams under a hot parent stay addressable.
  for (const s of args.sessions) {
    if (s.parentSessionId && hot.has(s.id)) {
      hot.add(s.parentSessionId);
    }
  }
  return hot;
}

/** Session ids that currently have events in memory but are not hot. */
export function coldSessionIdsWithEvents(
  events: SessionEvent[],
  hot: Set<string>
): string[] {
  const present = new Set<string>();
  for (const e of events) {
    if (e.sessionId) present.add(e.sessionId);
  }
  return [...present].filter((id) => !hot.has(id));
}

export function dropEventsForSessions(
  events: SessionEvent[],
  dropIds: Set<string>
): SessionEvent[] {
  if (dropIds.size === 0) return events;
  return events.filter((e) => !dropIds.has(e.sessionId));
}
