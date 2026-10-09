/**
 * Parking a dialog.
 *
 * Every warm ACP dialog holds a live agent process. Parking stops it — the
 * transcript stays on disk and readable, and the next send reconnects and
 * re-injects this dialog's history, the same path an app restart takes.
 *
 * Parking by hand is a stop, not a tidy-up: any dialog still holding a process
 * can be killed, including one mid-turn. `shouldAutoSuspend` below is the
 * gentler half of this feature and stays limited to warm, idle dialogs.
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

/** Statuses whose dialog still holds a live agent process worth stopping. */
const PARKABLE_STATUSES = new Set(["starting", "running", "waiting", "error"]);

/**
 * Whether the dialog holds an agent process right now — the only kind that can
 * be parked. `error` counts: the last turn failed, the process stayed up.
 * `running` / `starting` count too: parking a working dialog is how the user
 * says stop, and the caller seals the open turn before the kill.
 */
export function canPark(session: SuspendableSession): boolean {
  return PARKABLE_STATUSES.has(session.status);
}

export type SuspendControl = {
  /** Whether the row draws a park control at all. A drawn control is always live. */
  shown: boolean;
  title: string;
  ariaLabel: string;
};

/**
 * What a row's park control should be. Only a dialog that still holds a process
 * gets the button: a parked dialog has nothing left to stop, so drawing one
 * would be a dead affordance. Every control that is drawn can be pressed.
 */
export function suspendControl(
  session: SuspendableSession,
  label = session.id,
): SuspendControl {
  if (!canPark(session)) {
    return {
      shown: false,
      title: `${label} 已挂起 — 下次发消息自动接上`,
      ariaLabel: `${label} is already parked`,
    };
  }
  const title =
    session.status === "running"
      ? `挂起 ${label} — 立刻中断它正在做的事并结束进程，下次发消息自动接上`
      : session.status === "starting"
        ? `挂起 ${label} — 结束它刚起来的 agent 进程，下次发消息自动接上`
        : `挂起 ${label} — 结束它的 agent 进程，下次发消息自动接上`;
  return { shown: true, title, ariaLabel: `Suspend ${label}` };
}

/**
 * A warm, idle, unblocked dialog — the only kind the idle timer may park. The
 * hand-driven park above is deliberately wider: this runs unattended.
 */
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
