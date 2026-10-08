/**
 * Parking an idle dialog.
 *
 * Every warm ACP dialog holds a live agent process. Suspending stops it — the
 * transcript stays on disk and readable, and the next send reconnects and
 * re-injects this dialog's history, the same path an app restart takes.
 */

/** How long a warm, idle dialog may sit untouched before it is parked. */
export const SUSPEND_IDLE_MS = 30 * 60 * 1000;

export type SuspendableSession = {
  id: string;
  status: string;
  lastActiveAt: string;
};

export type SuspendRules = {
  /** The dialog this window is showing — never parked under the user. */
  currentSessionId?: string | null;
  /** Sessions with a question / plan / permission card still waiting. */
  pendingSessionIds?: readonly string[];
  /** Sessions with a queued follow-up that has not gone out yet. */
  queuedSessionIds?: readonly string[];
};

/** `lastActiveAt` is an epoch-ms string on new rows and ISO on legacy ones. */
export function lastActiveMs(session: { lastActiveAt: string }): number {
  const raw = (session.lastActiveAt ?? "").trim();
  if (!raw) return 0;
  const epoch = Number(raw);
  if (Number.isFinite(epoch) && epoch > 0) return epoch;
  const iso = Date.parse(raw);
  return Number.isFinite(iso) ? iso : 0;
}

/** A warm, idle, unblocked dialog — the only kind worth parking. */
export function isSuspendable(session: SuspendableSession, rules: SuspendRules = {}): boolean {
  if (session.status !== "waiting") return false;
  if (session.id === rules.currentSessionId) return false;
  if (rules.pendingSessionIds?.includes(session.id)) return false;
  if (rules.queuedSessionIds?.includes(session.id)) return false;
  return true;
}

/** True once a warm, idle dialog has been untouched for the idle window. */
export function shouldAutoSuspend(
  session: SuspendableSession,
  now: number,
  rules: SuspendRules & { idleMs?: number } = {},
): boolean {
  if (!isSuspendable(session, rules)) return false;
  const last = lastActiveMs(session);
  if (last === 0) return false;
  return now - last >= (rules.idleMs ?? SUSPEND_IDLE_MS);
}
