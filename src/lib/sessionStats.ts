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

import { t } from "./i18n";
import type { SessionEvent, TurnStats } from "./types";
import { generatedOutputTokens, type TurnTokens } from "./usage";

/** Disjoint token buckets, matching how providers bill a prompt. */
export type TokenBuckets = {
  /** Prompt tokens billed at full price. */
  uncachedInputTokens: number;
  /** Prompt tokens served from the provider's cache. */
  cacheReadTokens: number;
  /**
   * Prompt tokens spent populating the cache. OpenCode/Claude report this;
   * Grok folds it into the billed prompt and it stays 0 when the agent
   * doesn't split it out.
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
   * Prompt tokens the model actually had to process (fresh input + cache
   * writes; cache reads are served, not processed) over turns that also have a
   * first-token reading — the prefill-speed numerator.
   */
  prefillTokens: number;
  /** Summed first-token latency of those same turns' steps, ms — the denominator. */
  prefillMs: number;
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
    prefillTokens: 0,
    prefillMs: 0,
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
 * Split one turn's reported tokens into disjoint prompt buckets.
 *
 * `input` is fresh-only for every agent (Grok's billed-including-cache shape is
 * netted in `extractTurnTokens`). `total` settles the billed prompt when
 * present (total − output), which keeps a mislabelled adapter from silently
 * skewing the cache-hit share. Cache read/write are clamped to that prompt so
 * adapter noise cannot produce a share above 100%.
 */
export function tokenBuckets(tokens: TurnTokens): TokenBuckets {
  const output = nonNegative(tokens.output ?? 0);
  const cacheRead = nonNegative(tokens.cached ?? 0);
  const cacheWrite = nonNegative(tokens.cacheCreation ?? 0);
  const input = tokens.input;
  let billedPrompt: number | null = null;
  if (tokens.total != null && tokens.output != null) {
    const derived = tokens.total - output;
    if (Number.isFinite(derived) && derived >= 0) billedPrompt = derived;
  }
  if (billedPrompt == null && input != null) {
    billedPrompt = input + cacheRead + cacheWrite;
  }
  if (billedPrompt == null) {
    return {
      uncachedInputTokens: 0,
      cacheReadTokens: 0,
      cacheWriteTokens: cacheWrite,
      outputTokens: output,
    };
  }
  const read = Math.min(cacheRead, Math.max(0, billedPrompt));
  const write = Math.min(cacheWrite, Math.max(0, billedPrompt - read));
  return {
    uncachedInputTokens: Math.max(0, billedPrompt - read - write),
    cacheReadTokens: read,
    cacheWriteTokens: write,
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
    cacheCreation: stats.cacheCreation ?? null,
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
 * @param options.agentId - agent owning the dialog; kept for call-site
 *   compatibility (token buckets no longer branch on it).
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
  /** First-token latency summed over this turn's steps. */
  let turnTtftMs = 0;
  let turnTokens: TurnTokens | null = null;
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
        const ttft = Math.max(0, open.firstChunkMs - open.startMs);
        totals.ttftMs += ttft;
        totals.ttftSteps += 1;
        turnTtftMs += ttft;
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
      const buckets = reported != null ? tokenBuckets(reported) : null;
      if (buckets) {
        totals.uncachedInputTokens += buckets.uncachedInputTokens;
        totals.cacheReadTokens += buckets.cacheReadTokens;
        totals.cacheWriteTokens += buckets.cacheWriteTokens;
        totals.outputTokens += buckets.outputTokens;
        // Prefill pairs the turn's processed prompt with the time its model
        // calls spent before the first token. Only turns with both count.
        const processed = buckets.uncachedInputTokens + buckets.cacheWriteTokens;
        if (processed > 0 && turnTtftMs > 0) {
          totals.prefillTokens += processed;
          totals.prefillMs += turnTtftMs;
        }
      }
      // Only pair tokens with decode time we actually measured, so TPS stays a
      // ratio of two real numbers instead of a guess. A running turn has no
      // billed count yet, so its streamed estimate stands in for one.
      const billedGenerated = reported
        ? generatedOutputTokens(reported)
        : buckets?.outputTokens ?? 0;
      const generated = billedGenerated + liveTokens;
      if (turnDecodeMs > 0 && generated > 0) {
        totals.decodeMs += turnDecodeMs;
        totals.decodeTokens += billedGenerated;
        totals.liveOutputTokens += liveTokens;
      }
    }
    inTurn = false;
    turnSteps = 0;
    turnDecodeMs = 0;
    turnTtftMs = 0;
    turnTokens = null;
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
 * Generation window of the dialog's last turn (first token → step end, tool
 * waits excluded). Used to pair billed output with decode time for tok/s —
 * wall-clock first-chunk → turn end includes every tool wait and reads as a
 * few tok/s on a long tool loop.
 *
 * `endedAt` closes the last step: at `turn/complete` the tail card has not
 * been stamped yet, so the fold's `endedAt` on that card is still missing.
 */
export function measureTurnDecodeMs(
  events: readonly SessionEvent[],
  sessionId: string,
  endedAt: number,
): number | null {
  let lastUserIdx = -1;
  for (let i = events.length - 1; i >= 0; i -= 1) {
    const event = events[i];
    if (event && event.sessionId === sessionId && event.type === "user_message") {
      lastUserIdx = i;
      break;
    }
  }
  if (lastUserIdx < 0) return null;

  let decodeMs = 0;
  let stepFirstChunkMs: number | null = null;
  let lastCardEndMs: number | null = null;

  const closeStep = (endMs: number | null): void => {
    if (stepFirstChunkMs == null) return;
    const first = stepFirstChunkMs;
    stepFirstChunkMs = null;
    if (endMs == null) return;
    decodeMs += Math.max(0, endMs - first);
  };

  for (let i = lastUserIdx + 1; i < events.length; i += 1) {
    const event = events[i];
    if (!event || event.sessionId !== sessionId) continue;
    if (event.type === "user_message") break;

    if (event.type === "assistant_message" || event.type === "thought") {
      if (stepFirstChunkMs == null) stepFirstChunkMs = timeMs(event.createdAt);
      const ended = timeMs(event.endedAt);
      if (ended != null) lastCardEndMs = ended;
      continue;
    }

    if (event.type === "tool_call") {
      closeStep(timeMs(event.createdAt));
    }
  }
  closeStep(lastCardEndMs ?? endedAt);
  return decodeMs > 0 ? decodeMs : null;
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

/**
 * Prefill (prompt-processing) speed in tok/s. ACP exposes no prefill timer, so
 * this is derived: processed prompt tokens (fresh input + cache writes) of each
 * turn ÷ the first-token latency of that turn's model calls, summed over the
 * session. A multi-step turn reports one prompt total across all its calls, so
 * the time side is likewise the sum over those calls. null until a turn has
 * both numbers — never guessed.
 */
export function prefillTokensPerSecond(stats: SessionStats): number | null {
  if (stats.prefillMs <= 0 || stats.prefillTokens <= 0) return null;
  return stats.prefillTokens / (stats.prefillMs / 1000);
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
  if (seconds < 60) return t("stats.secondsCompact", { n: Math.round(seconds * 10) / 10 });
  const whole = Math.round(seconds);
  return t("stats.minutesSecondsCompact", { m: Math.floor(whole / 60), s: whole % 60 });
}
