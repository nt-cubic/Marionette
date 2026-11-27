/**
 * Worker stream projection (P0-4, docs/plans/plan-t3code-adoption.md).
 *
 * Delegated workers (origin `chief-worker` / `agent-worker`) do not flow
 * through the frontend rails while they run; their raw events are written to
 * a JSONL tee by Rust and replayed when the session is opened. Replay must NOT
 * grow a second card renderer: it has to use the same normalization chain as a
 * normal conversation -- `extractAcpUpdateText` turns an ACP session/update
 * into an `AcpTextPart`, and `applyAcpPartToEvents` (or the app-injected
 * `applyStreamPart`) folds that part into `SessionEvent` cards.
 *
 * This module is only the reusable projector:
 *   1. raw tee frame -> `extractAcpUpdateText` -> the same apply seam;
 *   2. preserve `agent_thought_chunk`, `agent_message_chunk`, `tool_call` and
 *      `tool_call_update`. Tool updates amend the existing card in place; they
 *      never collapse a whole run and never drop an event;
 *   3. deterministic duplicate guards: tee `seq` watermark plus a
 *      `toolCallId` content signature (repeating an identical tool update is
 *      skipped; any content change is still applied);
 *   4. an applied / skipped / unknown report for the P0-1 pipeline counters and
 *      the P0-8 fixture assertions.
 *
 * Deliberately out of scope:
 *   - `user_message_chunk` (the You card has its own path in the parent's
 *     existing replay: `mergeUserEcho` + `userMessageEvent`). Here it is only
 *     counted as `deferredUserEcho` and the rail is returned untouched, so a
 *     worker stream cannot accidentally mint a Reply card;
 *   - a second card renderer or status machine;
 *   - persistence, DOM reads, React dependencies.
 */

import {
  applyAcpPartToEvents,
  extractAcpUpdateText,
  getSessionUpdate,
  getSessionUpdateKind,
  type AcpTextPart,
} from "./acpTranscript";
import type { SessionEvent } from "./types";

/**
 * One raw worker tee line. Structurally identical to `ChiefTeeEvent` in
 * `chiefTee.ts`, but deliberately not imported by that name: this module is
 * meant for every delegated worker (agent-worker as well), not only chief.
 */
export interface WorkerTeeFrame {
  seq?: number;
  ts?: number;
  sessionId?: string;
  kind?: string;
  method?: string | null;
  data?: unknown;
}

/**
 * The seam that applies one normalized part onto the rail.
 *
 * Defaults to `applyAcpPartToEvents` (useful for scripts / tests). The app
 * should inject the `applyStreamPart` exposed by `useSessionRails`, so turn
 * start timing, adjacent thought coalescing and assistant fragment coalescing
 * behave exactly like the live path -- do not invent a second renderer.
 */
export type WorkerPartApplier = (
  events: SessionEvent[],
  sessionId: string,
  part: AcpTextPart,
) => SessionEvent[];

export interface WorkerStreamProjectorOptions {
  /** Marionette session id this stream belongs to (also passed to apply). */
  sessionId: string;
  /** Apply seam; defaults to `applyAcpPartToEvents`. The app passes `applyStreamPart`. */
  applyPart?: WorkerPartApplier;
  /** Starting watermark (exclusive): `seq <= startSeq` counts as already consumed. */
  startSeq?: number;
  /** Upper bound (inclusive): `seq > maxSeq` is out of the caller's window (optional). */
  maxSeq?: number;
  /**
   * Existing rail (e.g. a transcript already on screen). Tool ids found there
   * are treated as started, so a replay does not mistake an existing card for
   * a brand new start.
   */
  seedRail?: ReadonlyArray<SessionEvent>;
}

/**
 * Projection report. Every field is a count or a kind name; no message body,
 * so it can go straight into a `subagent-observe` dev.log line.
 */
export interface WorkerStreamReport {
  /** Total pushFrame calls (including skipped frames). */
  frames: number;
  /** Parts handed to the normalization / apply seam. */
  parts: number;
  /** Times apply returned a new rail (the card really changed). */
  applied: number;
  /** Valid parts that apply judged unchanged (merged duplicate / empty chunk). */
  noOp: number;
  /** Skipped because `seq <= watermark` (duplicate replay / stale out-of-order frame). */
  skippedSeq: number;
  /** Skipped because `seq > maxSeq` (outside the caller's window). */
  skippedOutOfWindow: number;
  /** Repeated start for a toolCallId whose content matches the last applied part. */
  skippedDuplicateToolStart: number;
  /** Repeated update for a toolCallId whose content matches the last applied part. */
  skippedDuplicateToolUpdate: number;
  /** Worker-tee user echo; the parent's You-card path owns it, not this module. */
  deferredUserEcho: number;
  /** Frames that did not normalize into agent activity (unknown kinds, control frames). */
  unknown: number;
  /** Unknown kind -> occurrence count. */
  unknownKinds: Record<string, number>;
  /** Parts that changed the rail, bucketed by raw sessionUpdate. */
  appliedKinds: Record<string, number>;
  /** Parts that changed the rail, bucketed by role (assistant / thought / tool / system). */
  appliedRoles: Record<string, number>;
  /** `tool_call` starts seen (including starts folded onto an existing card). */
  toolStarts: number;
  /** Tool-class updates seen (including identical duplicates that were skipped). */
  toolUpdates: number;
  /** Non-identical repeated starts for an existing card; applied as an amendment, no new card. */
  dedupedToolStarts: number;
  /** Updates seen for a tool id that has not started yet; still applied, apply decides shell/settle. */
  orphanToolUpdates: number;
  /** Distinct toolCallIds that have started. */
  distinctToolCalls: number;
  /** First seq accepted by this projector (null when frames carry no seq). */
  firstSeq: number | null;
  /** Latest seq watermark (null when frames carry no seq). */
  lastSeq: number | null;
}

type WorkerStreamCounters = Omit<WorkerStreamReport, "distinctToolCalls">;

interface ToolPartSnapshot {
  status: string;
  title: string;
  path: string;
  detail: string;
  input: string;
  images: readonly string[];
}

interface ToolGuardState {
  started: boolean;
  /** Content snapshot of the last applied part; null after seeding from a rail. */
  last: ToolPartSnapshot | null;
}

export interface WorkerStreamProjector {
  /**
   * Apply one raw tee frame. Returns the new rail; when nothing changed the
   * same array reference is returned, matching the `applyAcpPartToEvents` /
   * `applyStreamPart` contract (`updateSessionEvents` relies on it).
   */
  pushFrame(frame: WorkerTeeFrame, rail: SessionEvent[]): SessionEvent[];
  /**
   * Apply an already-extracted part. Useful when the caller extracted the part
   * itself and only wants the guards / report.
   */
  pushPart(part: AcpTextPart, rail: SessionEvent[]): SessionEvent[];
  /** Record tool ids from a rail that became available after creation. */
  seedFromRail(rail: ReadonlyArray<SessionEvent>): void;
  /** Replace the apply seam (the app's `applyStreamPart` may change identity). */
  setApplyPart(applyPart: WorkerPartApplier): void;
  /** Current report snapshot (nested tables are shallow copies, safe to keep/serialize). */
  report(): WorkerStreamReport;
  /** Started toolCallIds in insertion order (deterministic). */
  startedToolCallIds(): string[];
  /** Reset for a new span / session. Clears tool guards unless explicitly kept. */
  reset(options?: { startSeq?: number; maxSeq?: number; keepToolGuards?: boolean }): void;
}

const EMPTY_IMAGES: readonly string[] = [];

const isFiniteSeq = (value: unknown): value is number =>
  typeof value === "number" && Number.isFinite(value);

function emptyCounters(): WorkerStreamCounters {
  return {
    frames: 0,
    parts: 0,
    applied: 0,
    noOp: 0,
    skippedSeq: 0,
    skippedOutOfWindow: 0,
    skippedDuplicateToolStart: 0,
    skippedDuplicateToolUpdate: 0,
    deferredUserEcho: 0,
    unknown: 0,
    unknownKinds: {},
    appliedKinds: {},
    appliedRoles: {},
    toolStarts: 0,
    toolUpdates: 0,
    dedupedToolStarts: 0,
    orphanToolUpdates: 0,
    firstSeq: null,
    lastSeq: null,
  };
}

function bump(table: Record<string, number>, key: string): void {
  table[key] = (table[key] ?? 0) + 1;
}

function snapshotToolPart(part: AcpTextPart): ToolPartSnapshot {
  return {
    status: part.toolStatus ?? "",
    title: part.toolTitle ?? "",
    path: part.toolPath ?? "",
    detail: part.toolDetail ?? "",
    input: part.toolInput ?? "",
    images: part.toolImages ?? EMPTY_IMAGES,
  };
}

/** True when applying `part` could not change the card content at all. */
function sameToolContent(snapshot: ToolPartSnapshot, part: AcpTextPart): boolean {
  if (snapshot.status !== (part.toolStatus ?? "")) return false;
  if (snapshot.title !== (part.toolTitle ?? "")) return false;
  if (snapshot.path !== (part.toolPath ?? "")) return false;
  if (snapshot.detail !== (part.toolDetail ?? "")) return false;
  if (snapshot.input !== (part.toolInput ?? "")) return false;
  const images = part.toolImages ?? EMPTY_IMAGES;
  if (snapshot.images.length !== images.length) return false;
  for (let i = 0; i < images.length; i += 1) {
    if (snapshot.images[i] !== images[i]) return false;
  }
  return true;
}

/**
 * Deterministic tool content signature (exported for verify scripts that want
 * to assert "updates are not lost"). Covers only fields that affect the card,
 * never seq / ts.
 */
export function workerToolPartSignature(part: AcpTextPart): string {
  const snapshot = snapshotToolPart(part);
  // Array rather than object: field order is fixed and stable across runs.
  return JSON.stringify([
    snapshot.status,
    snapshot.title,
    snapshot.path,
    snapshot.detail,
    snapshot.input,
    snapshot.images,
  ]);
}

/**
 * Create a reusable worker projector. One projector belongs to one replay span
 * / one session; it owns the seq watermark and tool guards. Do not feed the
 * same instance from two independent streams at once.
 */
export function createWorkerStreamProjector(
  options: WorkerStreamProjectorOptions,
): WorkerStreamProjector {
  const sessionId = options.sessionId;
  let applyPart: WorkerPartApplier = options.applyPart ?? applyAcpPartToEvents;
  let counters = emptyCounters();
  const startSeqOption = options.startSeq;
  const maxSeqOption = options.maxSeq;
  let lastSeq: number | null = isFiniteSeq(startSeqOption) ? startSeqOption : null;
  let maxSeq: number | null = isFiniteSeq(maxSeqOption) ? maxSeqOption : null;
  const tools = new Map<string, ToolGuardState>();

  const seedToolsFromRail = (rail: ReadonlyArray<SessionEvent>): void => {
    for (const event of rail) {
      if (event.type !== "tool_call") continue;
      if (event.sessionId !== sessionId) continue;
      const id = event.toolCallId;
      if (!id) continue;
      const existing = tools.get(id);
      if (existing) {
        existing.started = true;
      } else {
        tools.set(id, { started: true, last: null });
      }
    }
  };

  if (options.seedRail) seedToolsFromRail(options.seedRail);

  const noteUnknown = (kind: string | null | undefined): void => {
    const key = kind && kind.length > 0 ? kind : "<none>";
    counters.unknown += 1;
    bump(counters.unknownKinds, key);
  };

  /**
   * Tool guard. `false` only means "this content is identical to the last
   * applied part, so applying it cannot change the card"; it never means
   * "tool updates may be dropped". Any content change passes through.
   */
  const considerToolPart = (part: AcpTextPart): boolean => {
    const id = part.toolCallId;
    if (!id) return true;
    const isStart = part.sessionUpdate === "tool_call";
    if (isStart) counters.toolStarts += 1;
    else counters.toolUpdates += 1;
    // Append / replace / patched-result parts change the card by *mode*, not
    // just by fields: two parts with identical fields can still produce
    // different bodies. Never let those become a dedupe baseline.
    const plainApply =
      part.toolDetailAppend !== true &&
      part.toolDetailReplace !== true &&
      !(part.subagentResults && part.subagentResults.length > 0);
    const existing = tools.get(id);
    if (plainApply && existing?.last && sameToolContent(existing.last, part)) {
      if (isStart) counters.skippedDuplicateToolStart += 1;
      else counters.skippedDuplicateToolUpdate += 1;
      return false;
    }
    const snapshot = snapshotToolPart(part);
    if (isStart) {
      if (existing?.started) counters.dedupedToolStarts += 1;
      tools.set(id, { started: true, last: plainApply ? snapshot : null });
    } else {
      if (!existing?.started) counters.orphanToolUpdates += 1;
      tools.set(id, {
        started: existing?.started ?? false,
        last: plainApply ? snapshot : null,
      });
    }
    return true;
  };

  const applyNormalized = (
    part: AcpTextPart,
    rail: SessionEvent[],
    sourceKind: string,
  ): SessionEvent[] => {
    counters.parts += 1;
    const role = part.role;
    if (role !== "assistant" && role !== "thought" && role !== "tool" && role !== "system") {
      // extractAcpUpdateText returns null for user echo; any remaining "other"
      // is genuinely unreadable for the card pipeline.
      noteUnknown(part.sessionUpdate || sourceKind);
      return rail;
    }
    if (role === "tool" && !considerToolPart(part)) return rail;
    const next = applyPart(rail, sessionId, part);
    const kindKey = part.sessionUpdate || sourceKind || "<none>";
    if (next !== rail) {
      counters.applied += 1;
      bump(counters.appliedKinds, kindKey);
      bump(counters.appliedRoles, role);
    } else {
      counters.noOp += 1;
    }
    return next;
  };

  return {
    pushFrame(frame, rail) {
      counters.frames += 1;
      const rawSeq = frame?.seq;
      const seq = isFiniteSeq(rawSeq) ? rawSeq : null;
      if (seq !== null) {
        if (maxSeq !== null && seq > maxSeq) {
          counters.skippedOutOfWindow += 1;
          return rail;
        }
        if (lastSeq !== null && seq <= lastSeq) {
          counters.skippedSeq += 1;
          return rail;
        }
        lastSeq = seq;
        if (counters.firstSeq === null) counters.firstSeq = seq;
        counters.lastSeq = seq;
      }

      const update = getSessionUpdate(frame?.data);
      const kind = update ? getSessionUpdateKind(update) : "";
      // The You card belongs to the parent's existing replay path
      // (mergeUserEcho + userMessageEvent). Count it and pass the rail through
      // so a worker stream cannot mint a Reply card.
      if (kind.includes("user_message")) {
        counters.deferredUserEcho += 1;
        return rail;
      }
      const part = extractAcpUpdateText(frame?.data);
      if (!part) {
        const rawMethod = frame?.method;
        const method = typeof rawMethod === "string" ? rawMethod : "";
        noteUnknown(kind || method || frame?.kind || "<none>");
        return rail;
      }
      return applyNormalized(part, rail, kind);
    },

    pushPart(part, rail) {
      return applyNormalized(part, rail, "");
    },

    seedFromRail(rail) {
      seedToolsFromRail(rail);
    },

    setApplyPart(next) {
      applyPart = next;
    },

    report() {
      let distinctToolCalls = 0;
      for (const state of tools.values()) {
        if (state.started) distinctToolCalls += 1;
      }
      return {
        ...counters,
        unknownKinds: { ...counters.unknownKinds },
        appliedKinds: { ...counters.appliedKinds },
        appliedRoles: { ...counters.appliedRoles },
        distinctToolCalls,
      };
    },

    startedToolCallIds() {
      const ids: string[] = [];
      for (const [id, state] of tools) {
        if (state.started) ids.push(id);
      }
      return ids;
    },

    reset(resetOptions) {
      counters = emptyCounters();
      const nextStart = resetOptions?.startSeq;
      const nextMax = resetOptions?.maxSeq;
      lastSeq = isFiniteSeq(nextStart) ? nextStart : null;
      maxSeq = isFiniteSeq(nextMax) ? nextMax : null;
      if (!resetOptions?.keepToolGuards) tools.clear();
    },
  };
}

/**
 * One-shot convenience (scripts / fixtures). The app's live replay should keep
 * a projector instance and push frames in batches instead of rebuilding it.
 */
export function projectWorkerFrames(
  frames: ReadonlyArray<WorkerTeeFrame>,
  options: WorkerStreamProjectorOptions & { rail?: SessionEvent[] },
): { rail: SessionEvent[]; report: WorkerStreamReport } {
  const projector = createWorkerStreamProjector(options);
  let rail = options.rail ?? [];
  for (const frame of frames) {
    rail = projector.pushFrame(frame, rail);
  }
  return { rail, report: projector.report() };
}
