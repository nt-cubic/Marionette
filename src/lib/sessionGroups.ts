/**
 * Shelf grouping for Chat-store dialogs (2026-10-26).
 *
 * Chat sessions with `origin: "memo"` are conversations grown out of the
 * 收纳盒 ("开新对话" on picked entries). They are ordinary Chat sessions for
 * every runtime path (spawn cwd, prefs, tabs) — only the shelf splits them
 * into their own group below 聊天, mirroring karakeep's "collected →
 * organized" flow.
 */

import type {
  Project,
  Session,
  SessionPresentation,
  SessionPresentationOverride,
} from "./types";

/**
 * Mirrors `api::CHAT_PROJECT_ID`. Kept local so this pure grouping module (and
 * its verify scripts) does not import the Tauri/i18n/mock chain.
 */
const CHAT_PROJECT_ID = "project-chat";

/**
 * P1-11 session presentation states.
 *
 * Four states, but only three are ever written to disk: `working` is derived
 * from real signals (running process, delegated worker, pending approval or
 * question). `settled`, `snoozed` and `archived` are shelf-only overrides and
 * must never stop, park or otherwise touch the agent process.
 */
export type SessionPresentationState = "working" | SessionPresentationOverride;

/**
 * Everything App must supply for the pure helpers below. The process state
 * (`status`) is one signal among several; presentation never writes it back.
 */
export type SessionPresentationSignals = {
  status?: Session["status"];
  /** A delegated worker is still running for this parent dialog. */
  delegated?: boolean;
  /** Completed turn the user has not looked at yet. Blocks auto-settle. */
  unreadAttention?: boolean;
  /** Permission / plan approval card is pending. */
  approvalPending?: boolean;
  /** Ask-question card is pending. */
  questionPending?: boolean;
  /** ACP terminal, browser agent, child worker, etc. is still working. */
  activeBackgroundWork?: boolean;
  /** A scheduled send is queued for this dialog. */
  scheduledSend?: boolean;
  /** Last real activity; defaults to the session's stored lastActiveAt. */
  lastActivityMs?: number;
};

export type SessionPresentationPolicy = {
  /** Quiet period before an idle dialog may auto-settle. */
  quietMs?: number;
};

/** Default shelf quiet period. Configurable by the caller (App preferences). */
export const DEFAULT_SESSION_QUIET_MS = 15 * 60_000;

const STORED_PRESENTATION_STATES: ReadonlySet<string> = new Set([
  "settled",
  "snoozed",
  "archived",
]);

function stampMs(raw: string | null | undefined): number | null {
  if (!raw) return null;
  const asNumber = Number(raw);
  if (Number.isFinite(asNumber) && (asNumber > 1e11 || asNumber === 0)) return asNumber;
  const parsed = Date.parse(raw);
  return Number.isFinite(parsed) ? parsed : null;
}

/** Validate a persisted override; unknown or corrupt rows read as "no override". */
export function sessionPresentationOverride(session: Pick<Session, "presentation">): {
  state: SessionPresentationOverride;
  updatedAtMs: number | null;
  snoozedUntilMs: number | null;
} | null {
  const presentation = session.presentation;
  if (!presentation || !STORED_PRESENTATION_STATES.has(presentation.state)) return null;
  return {
    state: presentation.state,
    updatedAtMs: stampMs(presentation.updatedAt),
    snoozedUntilMs: stampMs(presentation.snoozedUntil),
  };
}

function sessionLastActivityMs(session: Session): number {
  const raw = session.lastActiveAt || session.startedAt || "";
  return stampMs(raw) ?? 0;
}

/** Stored status is always a signal; callers may add delegation/attention on top. */
function signalsWithSessionStatus(
  session: Session,
  signals: SessionPresentationSignals | undefined,
): SessionPresentationSignals {
  return { ...(signals ?? {}), status: signals?.status ?? session.status };
}

/** A human is being waited on right now (approval / ask / plan). */
export function sessionIsAttentionWaiting(signals?: SessionPresentationSignals): boolean {
  return Boolean(signals?.approvalPending || signals?.questionPending);
}

/** Working = process running, delegated worker, or attention waiting. */
export function sessionIsWorking(signals?: SessionPresentationSignals): boolean {
  if (!signals) return false;
  return (
    signals.status === "starting" ||
    signals.status === "running" ||
    Boolean(signals.delegated) ||
    sessionIsAttentionWaiting(signals)
  );
}

/**
 * Anything that must keep a dialog out of auto-settle. Unread completion and
 * background work are not "working" but still need a human look.
 */
export function sessionHasAutoSettleBlocker(
  signals?: SessionPresentationSignals,
): boolean {
  if (!signals) return false;
  return Boolean(
    signals.unreadAttention ||
      signals.approvalPending ||
      signals.questionPending ||
      signals.activeBackgroundWork ||
      signals.scheduledSend ||
      signals.delegated ||
      signals.status === "starting" ||
      signals.status === "running",
  );
}

/**
 * A snoozed row wakes when the chosen time passes or a real attention / work
 * event lands. App should then clear the override (`working`) so the dialog
 * does not fall asleep again after the badge is read.
 */
export function snoozeShouldWake(
  session: Session,
  signals: SessionPresentationSignals | undefined,
  now: number = Date.now(),
): boolean {
  const override = sessionPresentationOverride(session);
  if (override?.state !== "snoozed") return false;
  const effective = signalsWithSessionStatus(session, signals);
  if (sessionIsWorking(effective) || effective.unreadAttention) return true;
  return override.snoozedUntilMs != null && override.snoozedUntilMs <= now;
}

/**
 * Resolve what the shelf should show now.
 *
 * Precedence: archived (always hidden) > working > active snooze > settled.
 * A running or attention-waiting dialog wakes a snooze because hiding live
 * work would be a lie; archived stays hidden until explicitly restored.
 */
export function resolveSessionPresentation(
  session: Session,
  signals: SessionPresentationSignals | undefined,
  policy: SessionPresentationPolicy = {},
  now: number = Date.now(),
): SessionPresentationState {
  const override = sessionPresentationOverride(session);
  const effective = signalsWithSessionStatus(session, signals);
  if (override?.state === "archived") return "archived";
  if (sessionIsWorking(effective)) return "working";
  if (override?.state === "snoozed") {
    if (!snoozeShouldWake(session, signals, now)) return "snoozed";
    return "settled";
  }
  // `settled` is explicit, and idle rows are also settled for display; the
  // quiet period only gates whether App may *persist* the auto transition.
  void policy;
  return "settled";
}

/**
 * Snoozed sessions App should wake (clear the override) on this tick.
 * Expired wake time or live attention both count.
 */
export function autoWakeSnoozedSessionIds(
  sessions: ReadonlyArray<Session>,
  signalsById: ReadonlyMap<string, SessionPresentationSignals> | undefined,
  now: number = Date.now(),
): string[] {
  return sessions
    .filter((session) => snoozeShouldWake(session, signalsById?.get(session.id), now))
    .map((session) => session.id);
}

/**
 * Should App persist `settled` for this dialog now?
 *
 * Only after the configured quiet period and only with zero unread attention,
 * approvals/questions, active background work, delegated workers or scheduled
 * sends. Process state is a signal, never a write target.
 */
export function shouldAutoSettle(
  session: Session,
  signals: SessionPresentationSignals | undefined,
  policy: SessionPresentationPolicy = {},
  now: number = Date.now(),
): boolean {
  const override = sessionPresentationOverride(session);
  const effective = signalsWithSessionStatus(session, signals);
  if (override?.state === "archived" || override?.state === "snoozed") return false;
  if (sessionIsWorking(effective) || sessionHasAutoSettleBlocker(effective)) return false;
  const quietMs = policy.quietMs ?? DEFAULT_SESSION_QUIET_MS;
  const lastActivity = effective.lastActivityMs ?? sessionLastActivityMs(session);
  return lastActivity > 0 && now - lastActivity >= quietMs;
}

/**
 * Sessions App should auto-settle on this tick (usually once per minute).
 * Idempotent: already-settled rows and anything blocked by attention are left
 * alone. App persists each id through storage, then refreshes the shelf.
 */
export function autoSettleSessionIds(
  sessions: ReadonlyArray<Session>,
  signalsById: ReadonlyMap<string, SessionPresentationSignals> | undefined,
  policy: SessionPresentationPolicy = {},
  now: number = Date.now(),
): string[] {
  const ids: string[] = [];
  for (const session of sessions) {
    if (sessionPresentationOverride(session)?.state === "settled") continue;
    if (shouldAutoSettle(session, signalsById?.get(session.id), policy, now)) {
      ids.push(session.id);
    }
  }
  return ids;
}

/** Archive/snooze rows leave the normal shelf, search and status counts. */
export function isSessionHiddenFromShelf(
  session: Session,
  signals?: SessionPresentationSignals,
  policy: SessionPresentationPolicy = {},
  now: number = Date.now(),
): boolean {
  const state = resolveSessionPresentation(session, signals, policy, now);
  return state === "archived" || state === "snoozed";
}

export type ShelfPresentationPartition = {
  visible: Session[];
  working: Session[];
  settled: Session[];
  snoozed: Session[];
  archived: Session[];
};

/**
 * One pass for the shelf: visible rows keep project grouping, while
 * snoozed/archived are collected for their own restore sections.
 */
export function partitionSessionsByPresentation(
  sessions: ReadonlyArray<Session>,
  signalsById: ReadonlyMap<string, SessionPresentationSignals> | undefined,
  policy: SessionPresentationPolicy = {},
  now: number = Date.now(),
): ShelfPresentationPartition {
  const partition: ShelfPresentationPartition = {
    visible: [],
    working: [],
    settled: [],
    snoozed: [],
    archived: [],
  };
  for (const session of sessions) {
    const state = resolveSessionPresentation(
      session,
      signalsById?.get(session.id),
      policy,
      now,
    );
    switch (state) {
      case "working":
        partition.working.push(session);
        partition.visible.push(session);
        break;
      case "snoozed":
        partition.snoozed.push(session);
        break;
      case "archived":
        partition.archived.push(session);
        break;
      case "settled":
      default:
        partition.settled.push(session);
        partition.visible.push(session);
        break;
    }
  }
  return partition;
}

/**
 * Build the optimistic Session patch for a UI action. `working` clears the
 * override (null). Snoozed requires a wake time; an invalid one returns null
 * so callers can fail closed instead of hiding a row forever.
 */
export function sessionPresentationPatch(
  state: SessionPresentationState,
  now: number = Date.now(),
  snoozedUntil?: string | number | null,
): SessionPresentation | null {
  if (state === "working") return null;
  const updatedAt = new Date(now).toISOString();
  if (state === "snoozed") {
    const untilMs =
      typeof snoozedUntil === "number"
        ? snoozedUntil
        : typeof snoozedUntil === "string"
          ? stampMs(snoozedUntil)
          : null;
    if (untilMs == null || untilMs <= now) return null;
    return {
      state,
      updatedAt,
      snoozedUntil: new Date(untilMs).toISOString(),
    };
  }
  return { state, updatedAt, snoozedUntil: null };
}

export type ShelfChatSplit = {
  staff: Session[];
  chat: Session[];
  memo: Session[];
  workers: Session[];
};

/**
 * Worker conversations are an implementation detail of a parent task. They
 * can still be opened from the task card or background row, but they must not
 * occupy the user's normal thread list (T3 Code hides subagent threads the
 * same way).
 */
export function isDelegatedWorkerSession(
  session: Pick<Session, "origin" | "parentSessionId" | "lineage">,
): boolean {
  return (
    session.origin === "agent-worker" ||
    session.origin === "chief-worker" ||
    Boolean(session.parentSessionId) ||
    session.lineage?.relationship === "subagent"
  );
}

/** Split the virtual Chat project's sessions from hidden worker subtrees. */
export function splitShelfChatSessions(sessions: ReadonlyArray<Session>): ShelfChatSplit {
  const staff: Session[] = [];
  const chat: Session[] = [];
  const memo: Session[] = [];
  const workers: Session[] = [];
  for (const session of sessions) {
    if (session.projectId !== CHAT_PROJECT_ID) continue;
    if (isDelegatedWorkerSession(session)) workers.push(session);
    else if (session.origin === "chief") staff.push(session);
    else if (session.origin === "memo") memo.push(session);
    else chat.push(session);
  }
  return { staff, chat, memo, workers };
}

/**
 * Ghost project left behind by the abandoned 2026-10-22 orb-assistant
 * "capture inbox" scheme: a project whose root lives under Marionette's own
 * data dir (`~/.marionette/inbox`). Nothing creates it anymore; hide it so
 * users stop seeing an empty "inbox / No sessions" row.
 */
export function isLegacyGhostProject(project: Project): boolean {
  const root = project.rootPath.replace(/\\/g, "/").toLowerCase().replace(/\/+$/, "");
  return root.endsWith(".marionette/inbox");
}

/** Projects list minus ghost rows (defensive at every load site). */
export function withoutGhostProjects(projects: ReadonlyArray<Project>): Project[] {
  return projects.filter((project) => !isLegacyGhostProject(project));
}
