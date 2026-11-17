import type { ChiefTask } from "./api";

/** A stale running task remains running; staleness is an annotation, not a new state. */
export const DEFAULT_STAFF_STALE_AFTER_MS = 5 * 60_000;

/** Failed/interrupted tasks stay visible briefly so a staff row does not flash and vanish. */
export const DEFAULT_STAFF_TERMINAL_RETENTION_MS = 5 * 60_000;

export type StaffTaskStatus = ChiefTask["status"];

export type StaffDisplayState =
  | "waiting_user"
  | "failed"
  | "interrupted"
  | "running"
  | "thinking"
  | "idle";

export type StaffWaitingReason =
  | "permission"
  | "question"
  | "plan_approval"
  | "user_input"
  | "unknown";

/**
 * Structural input accepted by the projector. ChiefTask is assignable to this
 * shape; the extra progress/report fields are forward-compatible placeholders.
 */
export interface StaffTaskLike {
  taskId: string;
  status: StaffTaskStatus;
  createdAt?: number | null;
  updatedAt?: number | null;
  lastEventAt?: number | null;
  lastProgressAt?: number | null;
  progressAt?: number | null;
  lastReportAt?: number | null;
  waitingReason?: string | null;
}

export interface StaffTaskCounts {
  total: number;
  proposed: number;
  starting: number;
  running: number;
  waitingUser: number;
  completed: number;
  failed: number;
  interrupted: number;
  cancelled: number;
  active: number;
  terminal: number;
  stale: number;
}

export type StaffCommitmentStatus = "open" | "due" | "done" | "cancelled" | "expired";

export interface StaffCommitmentInput {
  commitmentId: string;
  status: StaffCommitmentStatus;
  dueAt?: number | null;
}

export type StaffNotificationKind =
  | "waiting_user"
  | "failed"
  | "interrupted"
  | "commitment_due"
  | "permission"
  | "question"
  | "plan"
  | "completed"
  | "progress";

export interface StaffNotificationGateInput {
  now: number;
  eventKind: StaffNotificationKind;
  pausedUntilUserResumes?: boolean;
  quietHours?: string | null;
  /** Minutes east of UTC. Omit to use the runtime's local timezone. */
  timezoneOffsetMinutes?: number;
  cooldownMs?: number;
  lastNotificationAt?: number | null;
  notificationsLastHour?: number;
  /** 0 or less means no independent hourly cap. */
  maxReportsPerHour?: number;
}

export type StaffNotificationGateReason =
  | "allowed"
  | "priority_bypass"
  | "persistent_pause"
  | "quiet_hours"
  | "cooldown"
  | "hourly_cap";

export interface StaffNotificationGateDecision {
  allowed: boolean;
  bypassed: boolean;
  reason: StaffNotificationGateReason;
}

export interface StaffNotificationProjectionInput
  extends Omit<StaffNotificationGateInput, "now"> {
  lastReportAt?: number | null;
}

export interface StaffStatusProjectionOptions {
  now?: number;
  staleAfterMs?: number;
  terminalRetentionMs?: number;
  notification?: StaffNotificationProjectionInput | null;
  commitments?: readonly StaffCommitmentInput[] | null;
}

export interface StaffStatusProjection {
  displayState: StaffDisplayState;
  primaryTaskId: string | null;
  waitingReason: StaffWaitingReason | null;
  counts: StaffTaskCounts;
  terminalTaskIds: string[];
  staleTaskIds: string[];
  dueCommitmentIds: string[];
  dueCommitmentCount: number;
  lastEventAt: number | null;
  lastProgressAt: number | null;
  lastReportAt: number | null;
  notificationGate: StaffNotificationGateDecision | null;
}

function finiteTimestamp(value: number | null | undefined): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

function latestTimestamp(...values: Array<number | null | undefined>): number | null {
  let latest: number | null = null;
  for (const value of values) {
    const timestamp = finiteTimestamp(value);
    if (timestamp !== null && (latest === null || timestamp > latest)) {
      latest = timestamp;
    }
  }
  return latest;
}

function taskEventAt(task: StaffTaskLike): number | null {
  return latestTimestamp(task.lastEventAt, task.updatedAt, task.createdAt);
}

function taskProgressAt(task: StaffTaskLike): number | null {
  return (
    latestTimestamp(task.lastProgressAt, task.progressAt) ??
    latestTimestamp(task.updatedAt, task.createdAt)
  );
}

function taskReportAt(task: StaffTaskLike): number | null {
  return finiteTimestamp(task.lastReportAt);
}

export function isTerminalTaskStatus(status: StaffTaskStatus): boolean {
  return (
    status === "completed" ||
    status === "failed" ||
    status === "cancelled" ||
    status === "interrupted"
  );
}

/** Missing timestamps retain the terminal fact rather than hiding it by assumption. */
export function shouldRetainTerminalTask(
  task: StaffTaskLike,
  now: number,
  terminalRetentionMs: number = DEFAULT_STAFF_TERMINAL_RETENTION_MS,
): boolean {
  if (!isTerminalTaskStatus(task.status)) return false;
  const eventAt = taskEventAt(task);
  if (eventAt === null) return true;
  if (!Number.isFinite(now) || terminalRetentionMs < 0) return false;
  if (terminalRetentionMs === Number.POSITIVE_INFINITY) return true;
  return now - eventAt <= terminalRetentionMs;
}

/** Missing timestamps and disabled/non-finite thresholds are not guessed as stale. */
export function isStaffTaskStale(
  task: StaffTaskLike,
  now: number,
  staleAfterMs: number = DEFAULT_STAFF_STALE_AFTER_MS,
): boolean {
  if (isTerminalTaskStatus(task.status)) return false;
  const eventAt = taskEventAt(task);
  if (eventAt === null || !Number.isFinite(now) || staleAfterMs < 0) return false;
  if (staleAfterMs === Number.POSITIVE_INFINITY) return false;
  return now - eventAt >= staleAfterMs;
}

export function isCommitmentDue(
  commitment: StaffCommitmentInput,
  now: number,
): boolean {
  if (commitment.status === "done" || commitment.status === "cancelled" || commitment.status === "expired") {
    return false;
  }
  if (commitment.status === "due") return true;
  const dueAt = finiteTimestamp(commitment.dueAt);
  return Number.isFinite(now) && dueAt !== null && dueAt <= now;
}

function displayStateForTask(
  task: StaffTaskLike,
  now: number,
  terminalRetentionMs: number,
): StaffDisplayState | null {
  switch (task.status) {
    case "waiting_user":
      return "waiting_user";
    case "failed":
    case "interrupted":
      return shouldRetainTerminalTask(task, now, terminalRetentionMs)
        ? task.status
        : null;
    case "running":
      return "running";
    case "starting":
    case "proposed":
      return "thinking";
    case "completed":
    case "cancelled":
      return null;
  }
}

function displayRank(state: StaffDisplayState): number {
  switch (state) {
    case "waiting_user":
      return 5;
    case "failed":
    case "interrupted":
      return 4;
    case "running":
      return 3;
    case "thinking":
      return 2;
    case "idle":
      return 0;
  }
}

function normalizeWaitingReason(value: string | null | undefined): StaffWaitingReason {
  switch (value) {
    case "permission":
    case "question":
    case "plan_approval":
    case "user_input":
      return value;
    default:
      return "unknown";
  }
}

function emptyTaskCounts(): StaffTaskCounts {
  return {
    total: 0,
    proposed: 0,
    starting: 0,
    running: 0,
    waitingUser: 0,
    completed: 0,
    failed: 0,
    interrupted: 0,
    cancelled: 0,
    active: 0,
    terminal: 0,
    stale: 0,
  };
}

function countTask(counts: StaffTaskCounts, task: StaffTaskLike): void {
  counts.total += 1;
  switch (task.status) {
    case "proposed":
      counts.proposed += 1;
      counts.active += 1;
      break;
    case "starting":
      counts.starting += 1;
      counts.active += 1;
      break;
    case "running":
      counts.running += 1;
      counts.active += 1;
      break;
    case "waiting_user":
      counts.waitingUser += 1;
      counts.active += 1;
      break;
    case "completed":
      counts.completed += 1;
      counts.terminal += 1;
      break;
    case "failed":
      counts.failed += 1;
      counts.terminal += 1;
      break;
    case "interrupted":
      counts.interrupted += 1;
      counts.terminal += 1;
      break;
    case "cancelled":
      counts.cancelled += 1;
      counts.terminal += 1;
      break;
  }
}

function parseClock(value: string): number | null {
  const match = /^(\d{1,2}):(\d{2})$/.exec(value.trim());
  if (!match) return null;
  const hour = Number(match[1]);
  const minute = Number(match[2]);
  if (!Number.isInteger(hour) || !Number.isInteger(minute)) return null;
  if (hour < 0 || hour > 23 || minute < 0 || minute > 59) return null;
  return hour * 60 + minute;
}

function parseQuietHours(value: string): { start: number; end: number } | null {
  const match = /^\s*(\d{1,2}:\d{2})\s*-\s*(\d{1,2}:\d{2})\s*$/.exec(value);
  if (!match) return null;
  const start = parseClock(match[1] ?? "");
  const end = parseClock(match[2] ?? "");
  if (start === null || end === null || start === end) return null;
  return { start, end };
}

function minuteOfDay(now: number, timezoneOffsetMinutes?: number): number {
  const date = new Date(
    Number.isFinite(timezoneOffsetMinutes)
      ? now + (timezoneOffsetMinutes as number) * 60_000
      : now,
  );
  return Number.isFinite(timezoneOffsetMinutes)
    ? date.getUTCHours() * 60 + date.getUTCMinutes()
    : date.getHours() * 60 + date.getMinutes();
}

/** Start is inclusive, end is exclusive; equal endpoints and invalid ranges are disabled. */
export function isWithinQuietHours(
  now: number,
  quietHours: string | null | undefined,
  timezoneOffsetMinutes?: number,
): boolean {
  if (quietHours === null || quietHours === undefined || !Number.isFinite(now)) return false;
  const range = parseQuietHours(quietHours);
  if (range === null) return false;
  const minute = minuteOfDay(now, timezoneOffsetMinutes);
  if (range.start < range.end) {
    return minute >= range.start && minute < range.end;
  }
  return minute >= range.start || minute < range.end;
}

export function isNotificationCooldownActive(
  now: number,
  lastNotificationAt: number | null | undefined,
  cooldownMs: number | null | undefined,
): boolean {
  const last = finiteTimestamp(lastNotificationAt);
  if (last === null || !Number.isFinite(now) || !Number.isFinite(cooldownMs)) return false;
  if ((cooldownMs as number) <= 0) return false;
  return now - last < (cooldownMs as number);
}

/** 0 or less means unlimited, matching StaffPolicy's "no independent cap" convention. */
export function isNotificationHourlyCapReached(
  notificationsLastHour: number | null | undefined,
  maxReportsPerHour: number | null | undefined,
): boolean {
  if (!Number.isFinite(notificationsLastHour) || !Number.isFinite(maxReportsPerHour)) return false;
  if ((maxReportsPerHour as number) <= 0) return false;
  return Math.max(0, Math.floor(notificationsLastHour as number)) >= (maxReportsPerHour as number);
}

export function shouldBypassNotificationGate(kind: StaffNotificationKind): boolean {
  return (
    kind === "waiting_user" ||
    kind === "failed" ||
    kind === "interrupted" ||
    kind === "commitment_due" ||
    kind === "permission" ||
    kind === "question" ||
    kind === "plan"
  );
}

export function isNotificationPersistentlyPaused(
  pausedUntilUserResumes: boolean | null | undefined,
): boolean {
  return pausedUntilUserResumes === true;
}

/** Persistent pause wins over every requested notification kind. */
export function evaluateStaffNotificationGate(
  input: StaffNotificationGateInput,
): StaffNotificationGateDecision {
  if (isNotificationPersistentlyPaused(input.pausedUntilUserResumes)) {
    return { allowed: false, bypassed: false, reason: "persistent_pause" };
  }
  if (shouldBypassNotificationGate(input.eventKind)) {
    return { allowed: true, bypassed: true, reason: "priority_bypass" };
  }
  if (isWithinQuietHours(input.now, input.quietHours, input.timezoneOffsetMinutes)) {
    return { allowed: false, bypassed: false, reason: "quiet_hours" };
  }
  if (
    isNotificationCooldownActive(
      input.now,
      input.lastNotificationAt,
      input.cooldownMs,
    )
  ) {
    return { allowed: false, bypassed: false, reason: "cooldown" };
  }
  if (
    isNotificationHourlyCapReached(
      input.notificationsLastHour,
      input.maxReportsPerHour,
    )
  ) {
    return { allowed: false, bypassed: false, reason: "hourly_cap" };
  }
  return { allowed: true, bypassed: false, reason: "allowed" };
}

/**
 * Derive the staff row from task facts without writing a second business state.
 * Defaults: retain failed/interrupted for 5 minutes; mark tasks stale after 5
 * minutes without an event. Timestamps are never synthesized from wall clock.
 */
export function projectStaffStatus(
  tasks: readonly StaffTaskLike[],
  options: StaffStatusProjectionOptions = {},
): StaffStatusProjection {
  const now = Number.isFinite(options.now) ? (options.now as number) : Date.now();
  const staleAfterMs = options.staleAfterMs ?? DEFAULT_STAFF_STALE_AFTER_MS;
  const terminalRetentionMs =
    options.terminalRetentionMs ?? DEFAULT_STAFF_TERMINAL_RETENTION_MS;
  const counts = emptyTaskCounts();
  const terminalTaskIds: string[] = [];
  const staleTaskIds: string[] = [];
  let lastEventAt: number | null = null;
  let lastProgressAt: number | null = null;
  let primary:
    | { task: StaffTaskLike; state: StaffDisplayState; rank: number; eventAt: number | null }
    | null = null;

  for (const task of tasks) {
    countTask(counts, task);
    if (isTerminalTaskStatus(task.status)) terminalTaskIds.push(task.taskId);
    if (isStaffTaskStale(task, now, staleAfterMs)) staleTaskIds.push(task.taskId);

    lastEventAt = latestTimestamp(lastEventAt, taskEventAt(task));
    lastProgressAt = latestTimestamp(lastProgressAt, taskProgressAt(task));

    const state = displayStateForTask(task, now, terminalRetentionMs);
    if (state === null) continue;
    const rank = displayRank(state);
    const eventAt = taskEventAt(task);
    if (
      primary === null ||
      rank > primary.rank ||
      (rank === primary.rank &&
        (eventAt ?? Number.NEGATIVE_INFINITY) >
          (primary.eventAt ?? Number.NEGATIVE_INFINITY))
    ) {
      primary = { task, state, rank, eventAt };
    }
  }

  counts.stale = staleTaskIds.length;

  const commitments = options.commitments ?? [];
  const dueCommitmentIds = commitments
    .filter((commitment) => isCommitmentDue(commitment, now))
    .map((commitment) => commitment.commitmentId);

  const latestTaskReportAt = tasks.reduce<number | null>(
    (latest, task) => latestTimestamp(latest, taskReportAt(task)),
    null,
  );
  const notification = options.notification;
  const notificationGate = notification
    ? evaluateStaffNotificationGate({ ...notification, now })
    : null;
  const lastReportAt = latestTimestamp(
    latestTaskReportAt,
    notification?.lastReportAt,
    notification?.lastNotificationAt,
  );

  return {
    displayState: primary?.state ?? "idle",
    primaryTaskId: primary?.task.taskId ?? null,
    waitingReason:
      primary?.state === "waiting_user"
        ? normalizeWaitingReason(primary.task.waitingReason)
        : null,
    counts,
    terminalTaskIds,
    staleTaskIds,
    dueCommitmentIds,
    dueCommitmentCount: dueCommitmentIds.length,
    lastEventAt,
    lastProgressAt,
    lastReportAt,
    notificationGate,
  };
}
