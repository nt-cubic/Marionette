/**
 * Whole-dialog statistics folded from the transcript log — the figures in the
 * Usage card's collapsible groups.
 *
 * Every number is derived from durable events, never from a live ref, so a
 * dialog restored from disk reports what it reported while it ran. ACP carries
 * no step boundaries of its own (unlike a harness that emits `step/start`), so
 * the timings here are the ones Marionette can honestly measure:
 *
 * - A **step** is one model call's visible output: a run of Reply/Thinking
 *   cards with no tool call between them. A tool call closes the step at its
 *   dispatch, and the next step is dispatched when that tool returns.
 * - **First token** is the first card of the step; **step start** is the wire
 *   send (first step) or the previous tool's completion (later steps), which is
 *   the closest thing to a request dispatch we can observe.
 * - A step's **end** is the next tool's dispatch or, for a turn's last step,
 *   the end stamped on its final card at turn completion.
 *
 * Figures degrade by omission: a step whose end we cannot bound is still
 * counted, but contributes no time — never an estimate.
 */

import type { SessionEvent, TurnStats } from "./types";
import type { TurnTokens } from "./usage";

/** Disjoint token buckets, matching how providers bill a prompt. */
export type TokenBuckets = {
  /** Prompt tokens billed at full price. */
  uncachedInputTokens: number;
  /** Prompt tokens served from the provider's cache. */
  cacheReadTokens: number;
  /**
   * Prompt tokens spent populating the cache. No agent Marionette talks to
   * reports this separately (Grok folds it into the billed input), so it stays
   * 0 and its row never renders — a figure we cannot prove is not shown.
   */
  cacheWriteTokens: number;
  outputTokens: number;
};

export type SessionStats = TokenBuckets & {
  /** Turns with at least one closed step. */
  turns: number;
  /** Closed steps. */
  steps: number;
  /** Summed model wall time (step start → step end) over bounded steps, ms. */
  llmMs: number;
  /** Summed tool wall time (dispatch → terminal status), ms. */
  toolMs: number;
  /** Summed first-token latency over steps that recorded one, ms. */
  ttftMs: number;
  /** Steps carrying a first-token reading. */
  ttftSteps: number;
  /**
   * Summed decode wall time (first token → step end) over turns whose reported
   * output tokens pair with it. Tool waits are excluded: each step contributes
   * only its own generation window. A step still streaming contributes its
   * window up to the fold's `now`.
   */
  decodeMs: number;
  /** Output tokens of the same turns — the TPS numerator. */
  decodeTokens: number;
  /**
   * Output tokens of the step still streaming, estimated from its text. ACP
   * reports token counts only when a call ends, so this is the one figure here
   * that is a guess; the UI marks it `~` and the provider's own count replaces
   * it the moment the turn is billed.
   */
  liveOutputTokens: number;
};

export function emptySessionStats(): SessionStats {
  return {
    turns: 0,
    steps: 0,
    llmMs: 0,
    toolMs: 0,
    ttftMs: 0,
    ttftSteps: 0,
    decodeMs: 0,
    decodeTokens: 0,
    liveOutputTokens: 0,
    uncachedInputTokens: 0,
    cacheReadTokens: 0,
    cacheWriteTokens: 0,
    outputTokens: 0,
  };
}

function timeMs(value: string | undefined | null): number | null {
  if (!value) return null;
  const t = Date.parse(value);
  return Number.isFinite(t) ? t : null;
}

function nonNegative(value: number): number {
  return Number.isFinite(value) && value > 0 ? value : 0;
}

/**
 * Grok reports each model call with `input_tokens` already net of nothing —
 * its billed prompt is `input + cache_read + cache_creation`, and the turn
 * accumulator folds all three into `input`. Every other adapter follows the
 * ACP/Anthropic shape, where `input` is the uncached remainder.
 */
function reportsBilledInput(agentId: string | null | undefined): boolean {
  return agentId === "grok" || agentId === "grok-build";
}

/**
 * Split one turn's reported tokens into disjoint prompt buckets.
 *
 * `total` settles the shape when present (total − output is the billed prompt),
 * which keeps a mislabelled adapter from silently skewing the cache-hit share;
 * otherwise the agent's convention decides. Cache read is clamped to the billed
 * prompt so adapter noise cannot produce a share above 100%.
 */
export function tokenBuckets(tokens: TurnTokens, agentId?: string | null): TokenBuckets {
  const output = nonNegative(tokens.output ?? 0);
  const cacheRead = nonNegative(tokens.cached ?? 0);
  const input = tokens.input;
  let billedPrompt: number | null = null;
  if (tokens.total != null && tokens.output != null) {
    const derived = tokens.total - output;
    if (Number.isFinite(derived) && derived >= 0) billedPrompt = derived;
  }
  if (billedPrompt == null && input != null) {
    billedPrompt = reportsBilledInput(agentId) ? input : input + cacheRead;
  }
  if (billedPrompt == null) {
    return { uncachedInputTokens: 0, cacheReadTokens: 0, cacheWriteTokens: 0, outputTokens: output };
  }
  const read = Math.min(cacheRead, Math.max(0, billedPrompt));
  return {
    uncachedInputTokens: Math.max(0, billedPrompt - read),
    cacheReadTokens: read,
    cacheWriteTokens: 0,
    outputTokens: output,
  };
}

/** Prompt tokens the provider billed — the cache-hit denominator. */
export function billedPromptTokens(buckets: TokenBuckets): number {
  return buckets.uncachedInputTokens + buckets.cacheReadTokens + buckets.cacheWriteTokens;
}

export function hasTokenActivity(stats: SessionStats): boolean {
  return billedPromptTokens(stats) > 0 || stats.outputTokens > 0 || stats.liveOutputTokens > 0;
}

/**
 * Tokens a piece of streamed text is likely to have cost.
 *
 * ACP reports token counts only when a model call ends, so a step that is still
 * streaming carries none — this estimate is what keeps the live rate moving.
 * Everything the step writes counts, thinking included, since that is how
 * Claude and Codex bill output. Wide characters (CJK, kana, full-width forms)
 * cost about a token each; Latin script about a quarter of one.
 */
export function estimateTokensFromText(text: string): number {
  let wide = 0;
  let ascii = 0;
  let rest = 0;
  for (const char of text) {
    const code = char.codePointAt(0) ?? 0;
    if (
      (code >= 0x2e80 && code <= 0x9fff) ||
      (code >= 0xac00 && code <= 0xd7af) ||
      (code >= 0xf900 && code <= 0xfaff) ||
      (code >= 0xff00 && code <= 0xffef)
    ) {
      wide += 1;
    } else if (code < 0x80) {
      ascii += 1;
    } else {
      rest += 1;
    }
  }
  return wide + ascii / 4 + rest / 2;
}

/** Billed prompt + output, the number the usage pill shows. */
export function totalTokens(stats: SessionStats): number {
  return billedPromptTokens(stats) + stats.outputTokens;
}

function statsToTokens(stats: TurnStats): TurnTokens {
  return {
    input: stats.input ?? null,
    output: stats.output ?? null,
    cached: stats.cached ?? null,
    reasoning: stats.reasoning ?? null,
    total: stats.total ?? null,
  };
}

type OpenStep = {
  /** Request dispatch anchor; null when we cannot observe one. */
  startMs: number | null;
  /** First card of the step. */
  firstChunkMs: number | null;
};

/**
 * Fold one dialog's events into its session totals.
 *
 * @param events - the dialog's events in transcript order.
 * @param sessionId - owning dialog (callers may pass a shared rail).
 * @param options.settled - the dialog is known idle, so its last turn is over
 *   even when no card end was stamped (transcripts written before cards carried
 *   `endedAt`).
 * @param options.now - the dialog's last turn is still running: count what is
 *   in flight as of this instant, so the elapsed windows and the rate move
 *   while it runs. Its open step's tokens are estimated from the streamed text
 *   (`liveOutputTokens`); everything else stays a measured number.
 * @param options.liveTokens - provider-reported tokens for the turn in flight.
 *   Grok reports usage once per model call, so a multi-step turn has real
 *   numbers before it ends; the estimate then only has to cover the step still
 *   streaming. Every other agent reports at turn end, so this stays null.
 * @param options.agentId - agent owning the dialog; decides how a reported
 *   `input` is read when no card has carried an id yet.
 * @returns totals; all zero for a dialog that never ran a step.
 */
export function foldSessionStats(
  events: readonly SessionEvent[],
  sessionId: string,
  options?: {
    settled?: boolean;
    now?: number;
    liveTokens?: TurnTokens | null;
    agentId?: string | null;
  },
): SessionStats {
  const totals = emptySessionStats();
  let inTurn = false;
  /** Dispatch anchor for the next step to open. */
  let anchorMs: number | null = null;
  let step: OpenStep | null = null;
  /** Text the running turn has written, for the live estimate. */
  let turnText = "";
  /** Text the open step alone has written — what a per-call report misses. */
  let stepText = "";
  let turnSteps = 0;
  let turnDecodeMs = 0;
  let turnTokens: TurnTokens | null = null;
  let turnAgentId: string | null = options?.agentId ?? null;
  /** End of the turn's last card, when the turn stamped one. */
  let lastCardEndMs: number | null = null;
  /** Tool that has not reported a terminal status yet. */
  let openToolStartMs: number | null = null;

  const closeStep = (endMs: number | null, countWithoutEnd: boolean): void => {
    if (!step) return;
    const open = step;
    step = null;
    if (endMs == null && !countWithoutEnd) return;
    totals.steps += 1;
    turnSteps += 1;
    if (endMs == null) return;
    if (open.startMs != null) totals.llmMs += Math.max(0, endMs - open.startMs);
    if (open.firstChunkMs != null) {
      if (open.startMs != null) {
        totals.ttftMs += Math.max(0, open.firstChunkMs - open.startMs);
        totals.ttftSteps += 1;
      }
      turnDecodeMs += Math.max(0, endMs - open.firstChunkMs);
    }
  };

  const closeTurn = (
    countOpenStep: boolean,
    liveTokens: number,
    liveCounts: TurnTokens | null,
  ): void => {
    if (!inTurn) return;
    closeStep(lastCardEndMs, countOpenStep);
    // A turn that reported tokens counts even before it has closed a step —
    // Grok's first model call can be a bare tool call with no text.
    if (turnSteps > 0 || liveCounts != null) {
      totals.turns += 1;
      const reported = turnTokens ?? liveCounts;
      const buckets = reported != null ? tokenBuckets(reported, turnAgentId) : null;
      if (buckets) {
        totals.uncachedInputTokens += buckets.uncachedInputTokens;
        totals.cacheReadTokens += buckets.cacheReadTokens;
        totals.cacheWriteTokens += buckets.cacheWriteTokens;
        totals.outputTokens += buckets.outputTokens;
      }
      // Only pair tokens with decode time we actually measured, so TPS stays a
      // ratio of two real numbers instead of a guess. A running turn has no
      // billed count yet, so its streamed estimate stands in for one.
      const output = (buckets?.outputTokens ?? 0) + liveTokens;
      if (turnDecodeMs > 0 && output > 0) {
        totals.decodeMs += turnDecodeMs;
        totals.decodeTokens += buckets?.outputTokens ?? 0;
        totals.liveOutputTokens += liveTokens;
      }
    }
    inTurn = false;
    turnSteps = 0;
    turnDecodeMs = 0;
    turnTokens = null;
    turnAgentId = options?.agentId ?? null;
    lastCardEndMs = null;
    step = null;
    stepText = "";
    turnText = "";
    openToolStartMs = null;
  };

  for (const event of events) {
    if (event.sessionId !== sessionId) continue;

    if (event.type === "user_message") {
      // A following turn proves the previous one ended; its final step is
      // counted even when no card end was stamped (transcripts written before
      // cards carried `endedAt`).
      closeTurn(true, 0, null);
      inTurn = true;
      anchorMs = timeMs(event.sentAt) ?? timeMs(event.createdAt);
      lastCardEndMs = null;
      openToolStartMs = null;
      continue;
    }

    if (event.type === "assistant_message" || event.type === "thought") {
      if (!inTurn) continue;
      const at = timeMs(event.createdAt);
      if (!step) {
        step = { startMs: anchorMs, firstChunkMs: at };
        stepText = "";
      }
      stepText += event.text ?? "";
      // The whole turn's text too: when nothing reports tokens per call, the
      // earlier steps of a live turn contribute decode time with no count of
      // their own, so the estimate has to cover them or the rate reads low.
      turnText += event.text ?? "";
      if (event.type === "assistant_message") {
        if (event.agentId) turnAgentId = event.agentId;
        if (event.turnStats) turnTokens = statsToTokens(event.turnStats);
      }
      const ended = timeMs(event.endedAt);
      if (ended != null) lastCardEndMs = ended;
      continue;
    }

    if (event.type === "tool_call") {
      if (!inTurn) continue;
      const dispatched = timeMs(event.createdAt);
      const completed = timeMs(event.completedAt);
      // The model stopped writing when the tool was dispatched.
      closeStep(dispatched, false);
      if (dispatched != null && completed != null) {
        totals.toolMs += Math.max(0, completed - dispatched);
      }
      openToolStartMs = completed == null ? dispatched : null;
      // The next step is dispatched when this tool returns.
      anchorMs = completed;
      continue;
    }
  }

  // A running turn is closed at `now`: the elapsed windows are real, and the
  // turn still streaming contributes an estimated token count so the rate
  // moves. An idle dialog instead settles on the end stamps its cards carry —
  // a step with no end stamp is counted, but contributes no time.
  let liveTokens = 0;
  let liveCounts: TurnTokens | null = null;
  const liveNow = options?.settled === true ? null : (options?.now ?? null);
  if (liveNow != null) {
    // A turn that already stamped its end or its tokens belongs to the real
    // numbers below — estimating it too would count it twice.
    const inFlight = turnTokens == null && lastCardEndMs == null;
    if (inFlight) {
      liveCounts = options?.liveTokens ?? null;
      liveTokens =
        liveCounts != null
          ? step != null
            ? estimateTokensFromText(stepText)
            : 0
          : estimateTokensFromText(turnText);
    }
    if (openToolStartMs != null && liveNow > openToolStartMs) {
      totals.toolMs += liveNow - openToolStartMs;
    }
    if (step != null && inFlight) closeStep(liveNow, true);
  }
  closeTurn(options?.settled === true, liveTokens, liveCounts);
  return totals;
}

/**
 * Cache-hit share of the billed prompt, `null` before anything was billed.
 * A partial hit is never rounded up to a full 100%.
 */
export function cacheHitPercent(stats: SessionStats): string | null {
  const denominator = billedPromptTokens(stats);
  if (denominator <= 0) return null;
  const read = Math.min(stats.cacheReadTokens, denominator);
  if (read >= denominator) return "100";
  const exact = (read / denominator) * 100;
  if (Math.round(exact) < 100) return String(Math.round(exact));
  // 99.6% must not read as a full hit.
  for (let digits = 1; digits <= 3; digits += 1) {
    const value = Number(exact.toFixed(digits));
    if (value < 100) return value.toFixed(digits);
  }
  return "100";
}

/**
 * Output speed over the measured decode windows; null until both exist. A
 * running step's estimate counts toward the numerator, so the reading moves
 * while the turn runs and settles onto the billed number afterwards.
 */
export function outputTokensPerSecond(stats: SessionStats): number | null {
  const tokens = stats.decodeTokens + stats.liveOutputTokens;
  if (stats.decodeMs <= 0 || tokens <= 0) return null;
  return tokens / (stats.decodeMs / 1000);
}

/** Average first-token latency; null until a step recorded one. */
export function averageTtftMs(stats: SessionStats): number | null {
  if (stats.ttftSteps <= 0 || stats.ttftMs <= 0) return null;
  return stats.ttftMs / stats.ttftSteps;
}

/** `15.8K` / `517` / `1.2M` — compact token count for the pill. */
export function formatTokenCount(value: number): string {
  const scaled = (candidate: number): string =>
    candidate >= 100 ? String(Math.round(candidate)) : String(Math.round(candidate * 10) / 10);
  if (value < 1_000) return String(Math.round(value));
  if (value < 1_000_000) return `${scaled(value / 1_000)}K`;
  return `${scaled(value / 1_000_000)}M`;
}

/** `15,832` — exact count with digit grouping, for the dialog rows. */
export function formatExactTokenCount(value: number): string {
  return Math.round(value).toLocaleString("en-US");
}

/** Whole tokens from ten up, one decimal below. */
export function formatTokensPerSecond(tps: number): string {
  const clamped = Math.max(0, tps);
  return clamped >= 10 ? String(Math.round(clamped)) : String(Math.round(clamped * 10) / 10);
}

/** `45.2秒` under a minute, `2分42秒` from there on. */
export function formatDurationCompact(ms: number): string {
  const seconds = Math.max(0, ms) / 1000;
  if (seconds < 60) return `${Math.round(seconds * 10) / 10}秒`;
  const whole = Math.round(seconds);
  return `${Math.floor(whole / 60)}分${whole % 60}秒`;
}
