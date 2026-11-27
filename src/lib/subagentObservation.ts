/**
 * Subagent observability model (P0-5 / P0-7). Pure data, no IO, no React, no i18n.
 *
 * Two inputs only:
 * - the same `SessionEvent[]` the worker conversation renders from (counts and
 *   the parent card must never be derived by a second transcript parser);
 * - explicit task lifecycle facts (task-ledger status / start / end times).
 *
 * The output is a compact parent-card snapshot: current action, step/tool
 * counts, last thought, final reply, start/end/duration. All user-facing words
 * come from the caller - this module never invents copy.
 */
import type { SessionEvent } from "./types";
import { isToolInProgress } from "./activityHealth";

/** The six states a parent card may show. */
export type SubagentObservationState =
  | "starting"
  | "running"
  | "waiting_user"
  | "completed"
  | "failed"
  | "cancelled";

/** Lifecycle facts the caller already knows (all optional). */
export type SubagentLifecycleInput = {
  /**
   * Raw task-ledger status. Accepts `proposed` / `queued` / `interrupted` and
   * similar aliases in addition to the six modelled states - see STATUS_ALIASES.
   */
  status?: string | null;
  /** Wall-clock start (ms since epoch); falls back to the earliest worker event. */
  startedAtMs?: number | null;
  /** Wall-clock end (ms since epoch); terminal states fall back to the latest event. */
  endedAtMs?: number | null;
  /** Why the worker is waiting for a person (ledger free text, passed through). */
  waitingReason?: string | null;
};

export type SubagentObservationInput = {
  /** The worker conversation's own event array. */
  events: readonly SessionEvent[];
  /** Worker session id; when given, only that session's events are counted. */
  sessionId?: string | null;
  lifecycle?: SubagentLifecycleInput | null;
};

export type SubagentCurrentAction = {
  toolCallId: string | null;
  /** Latest tool frame's title (title -> tool name -> first body line). */
  title: string | null;
  /** Raw ACP tool status (in_progress / completed / ...), untranslated. */
  status: string | null;
  /** Status is not terminal (same predicate the whole app uses). */
  inProgress: boolean;
  /** File the tool is working on (ACP locations[0]) when the event carried one. */
  path: string | null;
  /** Stable tool name (read / bash / task ...); updates rename `title`, not this. */
  toolName: string | null;
  /** Event time of that tool frame, ms since epoch (null when unparsable). */
  atMs: number | null;
};

/** Everything the parent card needs; it never touches the event array again. */
export type SubagentCardSnapshot = {
  state: SubagentObservationState;
  /** starting / running / waiting_user. */
  active: boolean;
  actionTitle: string | null;
  actionStatus: string | null;
  actionPath: string | null;
  actionInProgress: boolean;
  stepCount: number;
  toolCount: number;
  startedAtMs: number | null;
  endedAtMs: number | null;
  durationMs: number | null;
  /** Latest assistant text, clipped. "Final" only once the task is terminal. */
  summaryText: string | null;
  /** Latest thought, clipped (live context hint only). */
  thoughtText: string | null;
  waitingReason: string | null;
};

export type SubagentObservation = {
  sessionId: string | null;
  state: SubagentObservationState;
  /** Raw lifecycle status as passed in (keeps `interrupted` visible to callers). */
  lifecycleStatus: string | null;
  currentAction: SubagentCurrentAction | null;
  /**
   * Distinct tool invocations started - the parent card's step count.
   *
   * Phase 0 definition: an observable worker step is a tool call (P0-10 compares
   * the parent step count against the worker's tool cards). So today
   * `stepCount === toolCount`; the fields stay separate because the parent card
   * says "steps" while the summary card says "tool calls". If an agent ever
   * reports non-tool steps, change the definition HERE, not in each consumer.
   */
  stepCount: number;
  /** Distinct tool invocations started - the summary card's tool-call count. */
  toolCount: number;
  lastThought: string | null;
  finalAssistantText: string | null;
  startedAtMs: number | null;
  endedAtMs: number | null;
  durationMs: number | null;
  waitingReason: string | null;
  active: boolean;
  card: SubagentCardSnapshot;
};

/** Clip limits for the card snapshot (exported so verify scripts can pin them). */
export const SUBAGENT_CARD_TITLE_MAX = 160;
export const SUBAGENT_CARD_THOUGHT_MAX = 240;
export const SUBAGENT_CARD_SUMMARY_MAX = 480;
export const SUBAGENT_CARD_REASON_MAX = 160;

/**
 * Status aliases. Only literal words are accepted - never guess with
 * `includes`/regex. `interrupted` maps to cancelled ("stopped"), not failed.
 */
const STATUS_ALIASES: Record<string, SubagentObservationState> = {
  proposed: "starting",
  queued: "starting",
  pending: "starting",
  starting: "starting",
  running: "running",
  active: "running",
  working: "running",
  in_progress: "running",
  waiting_user: "waiting_user",
  waiting: "waiting_user",
  needs_input: "waiting_user",
  input_required: "waiting_user",
  completed: "completed",
  complete: "completed",
  done: "completed",
  success: "completed",
  succeeded: "completed",
  failed: "failed",
  failure: "failed",
  error: "failed",
  errored: "failed",
  timeout: "failed",
  cancelled: "cancelled",
  canceled: "cancelled",
  stopped: "cancelled",
  interrupted: "cancelled",
  aborted: "cancelled",
};

/** Raw status -> six-state model; unknown returns null (keep the derived state). */
export function subagentObservationStateFromStatus(
  status: string | null | undefined,
): SubagentObservationState | null {
  if (!status) return null;
  const key = status.trim().toLowerCase();
  if (!key) return null;
  return STATUS_ALIASES[key] ?? null;
}

export function isSubagentObservationActive(state: SubagentObservationState): boolean {
  return state === "starting" || state === "running" || state === "waiting_user";
}

type ToolCallEvent = Extract<SessionEvent, { type: "tool_call" }>;

type Latest<T> = { event: T; atMs: number | null; index: number };

function timeMs(iso: string | null | undefined): number | null {
  if (!iso) return null;
  const ms = Date.parse(iso);
  return Number.isFinite(ms) ? ms : null;
}

function finiteOrNull(value: number | null | undefined): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

/** Collapse whitespace and clip short fields (titles, paths, reasons). */
function oneLine(text: string, max: number): string {
  const value = text.replace(/\s+/g, " ").trim();
  if (value.length <= max) return value;
  return `${value.slice(0, Math.max(0, max - 1))}\u2026`;
}

/** Clip multi-line text without collapsing its newlines (the card clamps lines). */
function clipText(text: string, max: number): string {
  const value = text.trim();
  if (value.length <= max) return value;
  return `${value.slice(0, Math.max(0, max - 1))}\u2026`;
}

function trimmedOrNull(value: string | null | undefined): string | null {
  const text = value?.trim();
  return text ? text : null;
}

function firstLine(text: string | null | undefined): string | null {
  if (!text) return null;
  const line = text.replace(/\r\n/g, "\n").split("\n", 1)[0] ?? "";
  const value = oneLine(line, SUBAGENT_CARD_TITLE_MAX);
  return value || null;
}

/** Same instant -> array order decides (later rows are newer). */
function isLater(
  atMs: number | null,
  index: number,
  previous: { atMs: number | null; index: number } | null,
): boolean {
  if (!previous) return true;
  if (atMs != null && previous.atMs != null && atMs !== previous.atMs) {
    return atMs > previous.atMs;
  }
  return index > previous.index;
}

/**
 * Dedupe key for a tool start.
 *
 * With a toolCallId that is the identity. Without one, only an exact fingerprint
 * (tool name + title + path + createdAt) collapses: a replayed start repeats all
 * four, while two genuinely separate calls cannot share the same millisecond,
 * title and path. An id-less update cannot be told apart from a start, so it
 * counts once more - matching the extra card the renderer mints, which is the
 * consistent answer (stay in sync with the rail, never guess).
 */
function toolKey(event: ToolCallEvent): string {
  const id = event.toolCallId?.trim();
  if (id) return `id:${id}`;
  return `fp:${event.toolName ?? ""}|${event.title ?? ""}|${event.path ?? ""}|${event.createdAt ?? ""}`;
}

/** Derive the parent-card model from worker events + explicit lifecycle facts. */
export function observeSubagent(input: SubagentObservationInput): SubagentObservation {
  const scope = input.sessionId ?? null;
  const lifecycle = input.lifecycle ?? null;

  const seenTools = new Set<string>();
  let toolCount = 0;

  let firstEventMs: number | null = null;
  let lastEventMs: number | null = null;

  let latestTool: Latest<ToolCallEvent> | null = null;
  let latestThought: Latest<Extract<SessionEvent, { type: "thought" }>> | null = null;
  let latestReply: Latest<Extract<SessionEvent, { type: "assistant_message" }>> | null = null;
  let latestAsk: Latest<Extract<SessionEvent, { type: "ask_prompt" }>> | null = null;
  let latestPlan: Latest<Extract<SessionEvent, { type: "plan_prompt" }>> | null = null;
  let sawEvent = false;

  for (let index = 0; index < input.events.length; index += 1) {
    const event = input.events[index];
    if (!event) continue;
    if (scope != null && event.sessionId !== scope) continue;

    sawEvent = true;
    const atMs = timeMs(event.createdAt);
    if (atMs != null) {
      if (firstEventMs == null || atMs < firstEventMs) firstEventMs = atMs;
      if (lastEventMs == null || atMs > lastEventMs) lastEventMs = atMs;
    }

    if (event.type === "tool_call") {
      seenTools.add(toolKey(event));
      toolCount = seenTools.size;
      if (isLater(atMs, index, latestTool)) latestTool = { event, atMs, index };
      continue;
    }
    if (event.type === "thought") {
      if (!event.text || !/\S/.test(event.text)) continue;
      if (isLater(atMs, index, latestThought)) latestThought = { event, atMs, index };
      continue;
    }
    if (event.type === "assistant_message") {
      if (!event.text || !/\S/.test(event.text)) continue;
      if (isLater(atMs, index, latestReply)) latestReply = { event, atMs, index };
      continue;
    }
    if (event.type === "ask_prompt") {
      if (isLater(atMs, index, latestAsk)) latestAsk = { event, atMs, index };
      continue;
    }
    if (event.type === "plan_prompt") {
      if (isLater(atMs, index, latestPlan)) latestPlan = { event, atMs, index };
    }
  }

  const waitingReasonText = lifecycle?.waitingReason;
  const waitingReason = waitingReasonText
    ? oneLine(waitingReasonText, SUBAGENT_CARD_REASON_MAX) || null
    : null;

  // Explicit lifecycle wins. Events never invent terminal states - a quiet
  // worker is not a finished one; only the ledger may say completed/failed.
  const lifecycleState = subagentObservationStateFromStatus(lifecycle?.status);
  const pendingInput =
    latestAsk?.event.status === "pending" || latestPlan?.event.status === "pending";
  let state: SubagentObservationState;
  if (lifecycleState) {
    state = lifecycleState;
  } else if (!sawEvent) {
    state = "starting";
  } else if (pendingInput) {
    state = "waiting_user";
  } else {
    state = "running";
  }
  const active = isSubagentObservationActive(state);

  let currentAction: SubagentCurrentAction | null = null;
  if (latestTool) {
    const event = latestTool.event;
    const title =
      oneLine(event.title ?? "", SUBAGENT_CARD_TITLE_MAX) ||
      oneLine(event.toolName ?? "", SUBAGENT_CARD_TITLE_MAX) ||
      firstLine(event.text);
    const path = trimmedOrNull(event.path);
    const status = trimmedOrNull(event.status);
    currentAction = {
      toolCallId: trimmedOrNull(event.toolCallId),
      title: title || null,
      status,
      inProgress: isToolInProgress(status),
      path,
      toolName: trimmedOrNull(event.toolName),
      atMs: latestTool.atMs,
    };
  }

  const startedAtMs = finiteOrNull(lifecycle?.startedAtMs) ?? firstEventMs ?? null;

  const terminal = state === "completed" || state === "failed" || state === "cancelled";
  const endedAtMs = finiteOrNull(lifecycle?.endedAtMs) ?? (terminal ? lastEventMs : null);

  const durationMs =
    startedAtMs != null && endedAtMs != null && endedAtMs >= startedAtMs
      ? endedAtMs - startedAtMs
      : null;

  const stepCount = toolCount;
  const lastThought = latestThought?.event.text ?? null;
  const finalAssistantText = latestReply?.event.text ?? null;

  const card: SubagentCardSnapshot = {
    state,
    active,
    actionTitle: currentAction?.title ?? null,
    actionStatus: currentAction?.status ?? null,
    actionPath: currentAction?.path ?? null,
    actionInProgress: currentAction?.inProgress ?? false,
    stepCount,
    toolCount,
    startedAtMs,
    endedAtMs,
    durationMs,
    summaryText: finalAssistantText
      ? clipText(finalAssistantText, SUBAGENT_CARD_SUMMARY_MAX)
      : null,
    thoughtText: lastThought ? clipText(lastThought, SUBAGENT_CARD_THOUGHT_MAX) : null,
    waitingReason,
  };

  return {
    sessionId: scope,
    state,
    lifecycleStatus: lifecycle?.status?.trim() || null,
    currentAction,
    stepCount,
    toolCount,
    lastThought,
    finalAssistantText,
    startedAtMs,
    endedAtMs,
    durationMs,
    waitingReason,
    active,
    card,
  };
}
