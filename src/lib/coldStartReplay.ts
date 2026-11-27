/**
 * Cold-start replay planner for backend-driven worker sessions (P0-9, P0-2).
 *
 * A worker can run to completion while Marionette is closed. Its only record
 * is the append-only tee ledger; the canonical transcript on disk is older.
 * On the next open, this module decides exactly which tee rows still need to
 * be folded into that transcript. It is pure: no IO, no React, no clocks.
 *
 * Sole-writer / P0-2 contract:
 * - The frontend rails (`useSessionRails.persistSessionNow`) remain the only
 *   writer of the canonical transcript. This planner never writes anything.
 * - Apply `plan.actions` through the same live pipeline (`applyStreamPart` for
 *   `stream` parts, the user-message path for `user_echo`), then flush durably.
 * - Persist `plan.nextCursor` with the tee consumer ONLY after that flush
 *   succeeds. If the flush fails, keep the previous cursor: re-opening reloads
 *   the transcript and this planner returns the same batch again.
 * - An empty canonical transcript is never trusted as "already consumed";
 *   `startCursor` resets to 0 in that case.
 * - If `liveRailHasStream` is true, the live listener owns the stream. Do not
 *   replay; retry / let the live rail's durable write finish instead.
 *
 * Determinism / idempotency:
 * - Same inputs -> same plan.
 * - After a durable consume, the same tee rows sit at or before `startCursor`
 *   and the plan is empty (`nothing_after_cursor`).
 * - You cards and tool cards already in the canonical transcript are
 *   suppressed and counted, never re-applied.
 * - A failed persist is retried by re-planning from disk (or by keeping
 *   `liveRailHasStream` true for the still-open live session and retrying the
 *   flush, so the in-memory rail is not applied twice).
 */
import {
  extractAcpUpdateText,
  extractAcpUserText,
  getSessionUpdate,
  getSessionUpdateKind,
  type AcpTextPart,
} from "./acpTranscript";
import {
  maxSeqOf,
  mergeUserEcho,
  shouldFlushUserEcho,
  transcriptMaxCreatedAtMs,
  uncoveredTeeEvents,
  type ChiefTeeEvent,
} from "./chiefTee";
import type { SessionEvent } from "./types";

/**
 * How close (ms) a canonical `user_message` must be to a tee echo to count as
 * the same card. `sentAt`/`createdAt` is local send time, while the echo `ts`
 * is receipt time, so they differ by network + first-token latency. The window
 * also keeps two identical prompts sent far apart in the same worker session
 * from being collapsed into one.
 */
export const DEFAULT_USER_DEDUPE_WINDOW_MS = 120_000;

/** Machine-readable explanation for an empty plan (`null` when non-empty). */
export type ColdStartReplayReason =
  /** The live listener already owns this stream; replay would double-apply. */
  | "live_rail_owns_stream"
  /** The tee ledger has no rows at all. */
  | "no_tee_events"
  /** Every usable row is at or before the effective cursor. */
  | "nothing_after_cursor"
  /** Rows remain, but their `ts` is inside the transcript's covered span. */
  | "all_within_covered_span"
  /** Every renderable row was already present in the canonical transcript. */
  | "all_duplicates"
  /** Rows remain but none normalizes into a user/assistant/thought/tool/system part. */
  | "no_replayable_events";

/** A grouped user echo: one You card, rebuilt from one or more tee chunks. */
export type ColdStartReplayUserEchoAction = {
  kind: "user_echo";
  /** Merged text of the echo chunk group. */
  text: string;
  /** Seq of the last chunk in the group (the group's consume anchor). */
  seq: number;
  firstSeq: number;
  lastSeq: number;
  /** First finite tee timestamp in the group; null when the ledger has none. */
  firstTs: number | null;
};

/** One normalized stream part, ready for `applyStreamPart`. */
export type ColdStartReplayStreamAction = {
  kind: "stream";
  part: AcpTextPart;
  /** Seq of the tee row that produced this part. */
  seq: number;
  ts: number | null;
};

export type ColdStartReplayAction =
  | ColdStartReplayUserEchoAction
  | ColdStartReplayStreamAction;

/** Honest counters for the P0-9 duplicate / skip questions. */
export type ColdStartReplayCounts = {
  /** Raw tee rows handed in (including malformed rows). */
  teeTotal: number;
  /** Rows whose `seq` was not a finite number (ignored). */
  malformedSeq: number;
  /** Rows at or before the effective cursor (already consumed / covered). */
  alreadyCovered: number;
  /** Rows after the effective cursor. */
  afterCursor: number;
  /** Rows after the cursor but inside the transcript's covered `ts` span. */
  uncoveredSpanSkips: number;
  /** Raw rows that normalized into a user echo chunk or a stream part. */
  renderableEvents: number;
  /** Raw rows that survived every filter and are in `replayEvents`. */
  replayedEvents: number;
  /** User-echo cards emitted (a group of chunks counts once). */
  replayedUserEchoes: number;
  /** Stream parts emitted (one per tee row). */
  replayedStreamParts: number;
  /** User echoes suppressed because the transcript already has that card. */
  duplicateYouCards: number;
  /** Tool cards suppressed because the same toolCallId/signature already exists. */
  duplicateToolCalls: number;
};

export type ColdStartReplayInput = {
  /** Session the transcript and tee rows belong to. */
  sessionId: string;
  /** Canonical transcript `SessionEvent[]` already on disk. */
  transcriptEvents: ReadonlyArray<SessionEvent>;
  /** Raw tee ledger rows for this session (append-only, chronological). */
  teeEvents: ReadonlyArray<ChiefTeeEvent>;
  /** Last tee seq consumed for this session (0 = never consumed). */
  storedCursor: number;
  /**
   * Max canonical `createdAt` in ms. Omit to derive it from `transcriptEvents`.
   * Kept explicit because the caller may load only a transcript tail.
   */
  transcriptMaxMs?: number;
  /** True when the live rail already contains this stream (live listener won). */
  liveRailHasStream?: boolean;
  /** Override `DEFAULT_USER_DEDUPE_WINDOW_MS`; clamped to >= 0. */
  userDedupeWindowMs?: number;
};

export type ColdStartReplayPlan = {
  sessionId: string;
  /**
   * Raw tee rows that still need to be fed into the live pipeline, in order.
   * `user_message_chunk` rows belong to `user_echo` actions (append a
   * `userMessageEvent`); every other row goes through `applyStreamPart`.
   */
  replayEvents: ChiefTeeEvent[];
  /** Ordered, ready-to-apply actions (the useful surface for App wiring). */
  actions: ColdStartReplayAction[];
  /**
   * Highest tee seq to persist as consumed, and only after a durable flush.
   * Duplicate / covered rows are not consumed (they stay as tee evidence).
   */
  nextCursor: number;
  /** Highest finite seq in the whole tee ledger (diagnostics). */
  maxSeq: number;
  /** Highest finite seq among rows after the effective cursor (diagnostics). */
  scannedMaxSeq: number;
  /** Effective replay cursor (0 when the canonical transcript is empty). */
  startCursor: number;
  /** True when `nextCursor > storedCursor`. */
  advancesCursor: boolean;
  /** True when `actions` must be flushed before `nextCursor` is consumed. */
  requiresPersist: boolean;
  /** Transcript max `createdAt` used for the uncovered-span filter. */
  transcriptMaxMs: number;
  /** True when replay was skipped because the live rail owns the stream. */
  liveRailOwnsStream: boolean;
  counts: ColdStartReplayCounts;
  /** Non-null only when `actions` is empty (or the live rail owns the stream). */
  emptyReason: ColdStartReplayReason | null;
};

/**
 * Stable, whitespace-conservative text fingerprint.
 *
 * Only CRLF and trailing whitespace are normalized. Interior runs are kept:
 * two prompts that differ by meaningful whitespace (code snippets) must not be
 * collapsed into one card. The output is used as a map key, so it is
 * collision-free (no hashing).
 */
export function normalizeReplayText(text: string): string {
  return text
    .replace(/\r\n?/g, "\n")
    .split("\n")
    .map((line) => line.replace(/[ \t]+$/g, ""))
    .join("\n")
    .trim();
}

/** `user_message` fingerprint used to prevent duplicate You cards. */
export function userMessageFingerprint(text: string): string {
  return normalizeReplayText(text);
}

type TranscriptUser = { atMs: number; used: boolean };

const EMPTY_COUNTS: ColdStartReplayCounts = {
  teeTotal: 0,
  malformedSeq: 0,
  alreadyCovered: 0,
  afterCursor: 0,
  uncoveredSpanSkips: 0,
  renderableEvents: 0,
  replayedEvents: 0,
  replayedUserEchoes: 0,
  replayedStreamParts: 0,
  duplicateYouCards: 0,
  duplicateToolCalls: 0,
};

function finiteNumber(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

function finiteSeq(event: ChiefTeeEvent): number | null {
  return finiteNumber(event?.seq);
}

function finiteTs(event: ChiefTeeEvent): number | null {
  return finiteNumber(event?.ts);
}

function isoToMs(value: string | null | undefined): number | null {
  if (typeof value !== "string" || !value) return null;
  const ms = Date.parse(value);
  return Number.isFinite(ms) ? ms : null;
}

function sanitizeCursor(value: number): number {
  return typeof value === "number" && Number.isFinite(value) && value > 0 ? value : 0;
}

function resolveDedupeWindow(value: number | undefined): number {
  return typeof value === "number" && Number.isFinite(value) && value >= 0
    ? value
    : DEFAULT_USER_DEDUPE_WINDOW_MS;
}

function resolveTranscriptMaxMs(
  provided: number | undefined,
  events: ReadonlyArray<SessionEvent>,
): number {
  if (typeof provided === "number" && Number.isFinite(provided)) {
    return provided > 0 ? provided : 0;
  }
  return transcriptMaxCreatedAtMs(events);
}

function joinFingerprintParts(...parts: ReadonlyArray<string | null | undefined>): string {
  return normalizeReplayText(parts.map((part) => part ?? "").join("\u0000"));
}

function toolSignatureFromPart(part: AcpTextPart): string {
  return joinFingerprintParts(part.toolName, part.toolTitle, part.toolPath, part.text);
}

function toolSignatureFromEvent(event: Extract<SessionEvent, { type: "tool_call" }>): string {
  return joinFingerprintParts(event.toolName, event.title, event.path, event.text);
}

/** `tool_call` creates a card; `tool_call_update` merges into one. */
function isToolCreationKind(sessionUpdate: string): boolean {
  const key = sessionUpdate.toLowerCase().replace(/[^a-z0-9]/g, "");
  return !key.includes("update");
}

function buildTranscriptUserPool(
  events: ReadonlyArray<SessionEvent>,
  sessionId: string,
): Map<string, TranscriptUser[]> {
  const pool = new Map<string, TranscriptUser[]>();
  for (const event of events) {
    if (event.sessionId !== sessionId || event.type !== "user_message") continue;
    const fingerprint = userMessageFingerprint(event.text);
    if (!fingerprint) continue;
    const atMs = isoToMs(event.sentAt) ?? isoToMs(event.createdAt) ?? 0;
    const list = pool.get(fingerprint);
    if (list) list.push({ atMs, used: false });
    else pool.set(fingerprint, [{ atMs, used: false }]);
  }
  return pool;
}

/**
 * Multiset match: one canonical card can absorb at most one echo group. Uses
 * `sentAt` (wire time) first because it is the closest anchor to the echo `ts`;
 * falls back to `createdAt` for legacy rows.
 */
function consumeTranscriptUserMatch(
  pool: Map<string, TranscriptUser[]>,
  fingerprint: string,
  atMs: number | null,
  windowMs: number,
): boolean {
  const candidates = pool.get(fingerprint);
  if (!candidates || candidates.length === 0) return false;

  if (atMs != null) {
    let best = -1;
    let bestDelta = Number.POSITIVE_INFINITY;
    for (let i = 0; i < candidates.length; i += 1) {
      const candidate = candidates[i]!;
      if (candidate.used || candidate.atMs <= 0) continue;
      const delta = Math.abs(candidate.atMs - atMs);
      if (delta <= windowMs && delta < bestDelta) {
        best = i;
        bestDelta = delta;
      }
    }
    if (best >= 0) {
      candidates[best]!.used = true;
      return true;
    }
    // Unknown-time legacy rows: content is the only signal available.
    for (let i = 0; i < candidates.length; i += 1) {
      const candidate = candidates[i]!;
      if (!candidate.used && candidate.atMs <= 0) {
        candidate.used = true;
        return true;
      }
    }
    return false;
  }

  // No tee timestamp at all: match the first unused card by content.
  for (let i = 0; i < candidates.length; i += 1) {
    const candidate = candidates[i]!;
    if (!candidate.used) {
      candidate.used = true;
      return true;
    }
  }
  return false;
}

function buildTranscriptToolIndex(
  events: ReadonlyArray<SessionEvent>,
  sessionId: string,
): Set<string> {
  const index = new Set<string>();
  for (const event of events) {
    if (event.sessionId !== sessionId || event.type !== "tool_call") continue;
    const id = typeof event.toolCallId === "string" ? event.toolCallId.trim() : "";
    if (id) index.add(id);
  }
  return index;
}

/**
 * Multiset of no-id tool cards already persisted. Build lazily; compliant ACP
 * agents always send `toolCallId`, so this path is rare.
 */
function buildTranscriptNoIdToolIndex(
  events: ReadonlyArray<SessionEvent>,
  sessionId: string,
): Map<string, number> {
  const index = new Map<string, number>();
  for (const event of events) {
    if (event.sessionId !== sessionId || event.type !== "tool_call") continue;
    const id = typeof event.toolCallId === "string" ? event.toolCallId.trim() : "";
    if (id) continue;
    const key = `sig:${toolSignatureFromEvent(event)}`;
    index.set(key, (index.get(key) ?? 0) + 1);
  }
  return index;
}

type ToolDecision = "apply" | "duplicate";

/**
 * Tool dedupe. Only card *creations* are suppressed; updates always apply
 * (they survive the uncovered-span filter, so they are genuinely newer than
 * the persisted transcript and must never be hidden).
 *
 * `toolCallId` is authoritative: a creation whose id is already in the
 * transcript (or earlier in this batch) would mint a second card. Without an
 * id, only an exact rendered snapshot is treated as the same card.
 */
function classifyToolEvent(
  part: AcpTextPart,
  existingToolIds: Set<string>,
  seenToolIds: Set<string>,
  noIdIndex: () => Map<string, number>,
): ToolDecision {
  const id = typeof part.toolCallId === "string" ? part.toolCallId.trim() : "";
  const creation = isToolCreationKind(part.sessionUpdate);

  if (id) {
    if (!creation) return "apply";
    if (existingToolIds.has(id) || seenToolIds.has(id)) return "duplicate";
    seenToolIds.add(id);
    return "apply";
  }

  if (!creation) return "apply";

  // No id: absorb at most as many creations as there are identical persisted
  // cards. A genuinely repeated call (more creations than cards) still lands.
  const index = noIdIndex();
  const key = `sig:${toolSignatureFromPart(part)}`;
  const remaining = index.get(key) ?? 0;
  if (remaining > 0) {
    index.set(key, remaining - 1);
    return "duplicate";
  }
  return "apply";
}

function emptyReasonFor(args: {
  liveRailOwnsStream: boolean;
  teeTotal: number;
  afterCursor: number;
  uncovered: number;
  actions: number;
  renderableEvents: number;
}): ColdStartReplayReason | null {
  if (args.liveRailOwnsStream) return "live_rail_owns_stream";
  if (args.actions > 0) return null;
  if (args.teeTotal === 0) return "no_tee_events";
  if (args.afterCursor === 0) return "nothing_after_cursor";
  if (args.uncovered === 0) return "all_within_covered_span";
  return args.renderableEvents > 0 ? "all_duplicates" : "no_replayable_events";
}

/**
 * Plan one cold-start replay. Pure and deterministic: the caller owns every
 * mutation (rail apply, durable flush, tee consume).
 */
export function planColdStartReplay(input: ColdStartReplayInput): ColdStartReplayPlan {
  const sessionId = input.sessionId;
  const transcriptEvents: ReadonlyArray<SessionEvent> = input.transcriptEvents ?? [];
  const teeEvents: ReadonlyArray<ChiefTeeEvent> = input.teeEvents ?? [];
  const liveRailOwnsStream = input.liveRailHasStream === true;
  const dedupeWindowMs = resolveDedupeWindow(input.userDedupeWindowMs);

  const transcriptMaxMs = resolveTranscriptMaxMs(input.transcriptMaxMs, transcriptEvents);
  const transcriptEmpty = transcriptEvents.length === 0 && transcriptMaxMs <= 0;
  const storedCursor = sanitizeCursor(input.storedCursor);
  const startCursor = transcriptEmpty ? 0 : storedCursor;

  const counts: ColdStartReplayCounts = { ...EMPTY_COUNTS };
  counts.teeTotal = teeEvents.length;

  const afterCursor: ChiefTeeEvent[] = [];
  for (const event of teeEvents) {
    const seq = finiteSeq(event);
    if (seq == null) {
      counts.malformedSeq += 1;
      continue;
    }
    if (seq <= startCursor) {
      counts.alreadyCovered += 1;
      continue;
    }
    afterCursor.push(event);
  }
  counts.afterCursor = afterCursor.length;

  // The tee is append-only, but a stable seq order keeps the plan deterministic
  // even if a caller hands rows in file order with equal/overlapping seqs.
  afterCursor.sort((a, b) => (finiteSeq(a) ?? 0) - (finiteSeq(b) ?? 0));

  const scannedMaxSeq = maxSeqOf(afterCursor, startCursor);
  const maxSeq = maxSeqOf(teeEvents, 0);

  const uncovered: ChiefTeeEvent[] =
    afterCursor.length > 0 ? uncoveredTeeEvents(afterCursor, transcriptMaxMs) : [];
  counts.uncoveredSpanSkips = afterCursor.length - uncovered.length;

  const actions: ColdStartReplayAction[] = [];
  const replayEvents: ChiefTeeEvent[] = [];

  if (uncovered.length > 0) {
    const userPool = buildTranscriptUserPool(transcriptEvents, sessionId);
    const existingToolIds = buildTranscriptToolIndex(transcriptEvents, sessionId);
    const seenToolIds = new Set<string>();
    let noIdToolIndex: Map<string, number> | null = null;
    const getNoIdToolIndex = (): Map<string, number> => {
      if (!noIdToolIndex) {
        noIdToolIndex = buildTranscriptNoIdToolIndex(transcriptEvents, sessionId);
      }
      return noIdToolIndex;
    };

    let userEcho = "";
    let userEchoFirstSeq = 0;
    let userEchoLastSeq = 0;
    let userEchoFirstTs: number | null = null;
    let userEchoChunks: ChiefTeeEvent[] = [];

    const flushUserEcho = (): void => {
      if (!userEcho) {
        userEchoChunks = [];
        return;
      }
      const text = userEcho;
      const firstSeq = userEchoFirstSeq;
      const lastSeq = userEchoLastSeq;
      const firstTs = userEchoFirstTs;
      const chunks = userEchoChunks;

      userEcho = "";
      userEchoChunks = [];
      userEchoFirstSeq = 0;
      userEchoLastSeq = 0;
      userEchoFirstTs = null;

      const fingerprint = userMessageFingerprint(text);
      if (consumeTranscriptUserMatch(userPool, fingerprint, firstTs, dedupeWindowMs)) {
        counts.duplicateYouCards += 1;
        return;
      }
      actions.push({ kind: "user_echo", text, seq: lastSeq, firstSeq, lastSeq, firstTs });
      for (const chunk of chunks) replayEvents.push(chunk);
      counts.replayedUserEchoes += 1;
    };

    for (const event of uncovered) {
      const method = typeof event.method === "string" ? event.method : "";
      const update = getSessionUpdate(event.data);
      const kind = update ? getSessionUpdateKind(update) : "";

      if (method === "session/update" && kind === "user_message_chunk") {
        const user = extractAcpUserText(event.data);
        if (user && user.text) {
          counts.renderableEvents += 1;
          const ts = finiteTs(event);
          if (userEcho === "") {
            userEchoFirstSeq = event.seq;
            userEchoFirstTs = ts;
          } else if (userEchoFirstTs == null && ts != null) {
            userEchoFirstTs = ts;
          }
          userEchoLastSeq = event.seq;
          userEcho = mergeUserEcho(userEcho, { text: user.text, isDelta: user.isDelta });
          userEchoChunks.push(event);
        }
        continue;
      }

      if (userEcho && shouldFlushUserEcho(method, kind)) flushUserEcho();

      const part = extractAcpUpdateText(event.data);
      if (!part) continue;
      if (
        part.role !== "assistant" &&
        part.role !== "thought" &&
        part.role !== "tool" &&
        part.role !== "system"
      ) {
        continue;
      }
      counts.renderableEvents += 1;

      if (part.role === "tool") {
        const decision = classifyToolEvent(
          part,
          existingToolIds,
          seenToolIds,
          getNoIdToolIndex,
        );
        if (decision === "duplicate") {
          counts.duplicateToolCalls += 1;
          continue;
        }
      }

      actions.push({ kind: "stream", part, seq: event.seq, ts: finiteTs(event) });
      replayEvents.push(event);
      counts.replayedStreamParts += 1;
    }

    if (userEcho) flushUserEcho();
  }

  const uncoveredMaxSeq = maxSeqOf(uncovered, startCursor);
  const nextCursor = liveRailOwnsStream
    ? storedCursor
    : Math.max(storedCursor, uncoveredMaxSeq);
  const advancesCursor = !liveRailOwnsStream && nextCursor > storedCursor;
  const requiresPersist = !liveRailOwnsStream && actions.length > 0;

  counts.replayedEvents = replayEvents.length;

  const emptyReason = emptyReasonFor({
    liveRailOwnsStream,
    teeTotal: teeEvents.length,
    afterCursor: afterCursor.length,
    uncovered: uncovered.length,
    actions: actions.length,
    renderableEvents: counts.renderableEvents,
  });

  if (liveRailOwnsStream) {
    // The live listener owns the stream; nothing may be re-applied. Duplicate
    // counters stay meaningful (they describe the tee vs. the transcript), but
    // the replay counters describe events we deliberately did NOT emit.
    return {
      sessionId,
      replayEvents: [],
      actions: [],
      nextCursor,
      maxSeq,
      scannedMaxSeq,
      startCursor,
      advancesCursor,
      requiresPersist,
      transcriptMaxMs,
      liveRailOwnsStream,
      counts: {
        ...counts,
        replayedEvents: 0,
        replayedUserEchoes: 0,
        replayedStreamParts: 0,
      },
      emptyReason,
    };
  }

  return {
    sessionId,
    replayEvents,
    actions,
    nextCursor,
    maxSeq,
    scannedMaxSeq,
    startCursor,
    advancesCursor,
    requiresPersist,
    transcriptMaxMs,
    liveRailOwnsStream,
    counts,
    emptyReason,
  };
}
