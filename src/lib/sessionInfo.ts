/**
 * Figures behind the composer's session-info ring (docs/ui-language.md v2,
 * "输入框 → 信息圆圈"). Pure helpers: the popover and the copy-as-text button
 * share them so the two never disagree.
 */

import type { SessionEvent } from "./types";
import {
  averageTtftMs,
  cacheHitPercent,
  formatDurationCompact,
  formatTokenCount,
  formatTokensPerSecond,
  hasTokenActivity,
  outputTokensPerSecond,
  prefillTokensPerSecond,
  type SessionStats,
} from "./sessionStats";

/** What the popover reads besides the context window itself. */
export type SessionInfoData = {
  stats: SessionStats;
  /** Git line counts summed over the session's turns; null when no turn had any. */
  changeAdd: number | null;
  changeDel: number | null;
};

export const RING_WARN_AT = 80;
export const RING_FULL_AT = 95;

export type RingTone = "ok" | "warn" | "full";

/** Context occupancy in percent (0–100, one decimal), null without both numbers. */
export function contextPercent(
  used: number | null | undefined,
  size: number | null | undefined,
): number | null {
  if (used == null || size == null || !(size > 0) || !(used >= 0)) return null;
  return Math.min(100, Math.round((used / size) * 1000) / 10);
}

export function ringTone(pct: number | null): RingTone {
  if (pct == null) return "ok";
  if (pct >= RING_FULL_AT) return "full";
  if (pct >= RING_WARN_AT) return "warn";
  return "ok";
}

/** `48k` / `1.2M` — lowercase, for the context line and rates. */
export function formatContextCount(value: number): string {
  if (value < 1_000) return String(Math.round(value));
  const scaled = (n: number): string =>
    n >= 100 ? String(Math.round(n)) : String(Math.round(n * 10) / 10);
  if (value < 1_000_000) return `${scaled(value / 1_000)}k`;
  return `${scaled(value / 1_000_000)}M`;
}

/**
 * Lines added / removed over the whole dialog. Each turn's `file_change` rows
 * carry git's own counts for that turn's window; within one turn a file that
 * changed again replaces its earlier row, so only the last row per path counts.
 * Rows without counts (pre-2026-10-23 transcripts, non-git workspaces, binary
 * files) contribute nothing — null when no turn had any, never a guess.
 */
export function sessionChangeLines(
  events: readonly SessionEvent[],
  sessionId: string,
): { add: number; del: number } | null {
  let add = 0;
  let del = 0;
  let known = false;
  let turn = new Map<string, { add: number; del: number }>();
  const flush = (): void => {
    for (const row of turn.values()) {
      add += row.add;
      del += row.del;
      known = true;
    }
    turn = new Map();
  };
  for (const event of events) {
    if (event.sessionId !== sessionId) continue;
    if (event.type === "user_message" || event.type === "handoff_prepared") {
      flush();
      continue;
    }
    if (event.type !== "file_change" || event.binary) continue;
    if (typeof event.add !== "number" || typeof event.del !== "number") continue;
    turn.set(event.path, { add: event.add, del: event.del });
  }
  flush();
  return known ? { add, del } : null;
}

export const DASH = "—";

export type InfoFigures = {
  decode: string;
  prefill: string;
  firstToken: string;
  /** Plain text; the popover colours the two halves itself. */
  changeAdd: string | null;
  changeDel: string | null;
  turnsSteps: { turns: number; steps: number } | null;
  cacheHit: string;
  uncachedInput: string;
  output: string;
  cacheRead: string;
  cacheWrite: string;
};

/** Every displayed figure as a string, `—` where the data does not exist. */
export function buildInfoFigures(info: SessionInfoData | null): InfoFigures {
  const stats = info?.stats ?? null;
  const active = stats != null && hasTokenActivity(stats);
  const live = stats != null && stats.liveOutputTokens > 0;
  const tps = stats ? outputTokensPerSecond(stats) : null;
  const pp = stats ? prefillTokensPerSecond(stats) : null;
  const ttft = stats ? averageTtftMs(stats) : null;
  const hit = stats ? cacheHitPercent(stats) : null;
  const zeroDash = (n: number): string => (n > 0 ? formatTokenCount(n) : DASH);
  return {
    decode: tps == null ? DASH : `${live ? "~" : ""}${formatTokensPerSecond(tps)} tok/s`,
    prefill: pp == null ? DASH : `${formatContextCount(pp)}/s`,
    firstToken: ttft == null ? DASH : formatDurationCompact(ttft),
    changeAdd: info && info.changeAdd != null ? `+${info.changeAdd}` : null,
    changeDel: info && info.changeDel != null ? `−${info.changeDel}` : null,
    turnsSteps:
      stats && (stats.turns > 0 || stats.steps > 0)
        ? { turns: stats.turns, steps: stats.steps }
        : null,
    cacheHit: hit == null ? DASH : `${hit}%`,
    uncachedInput: active ? formatTokenCount(stats!.uncachedInputTokens) : DASH,
    output: active
      ? `${live ? "~" : ""}${formatTokenCount(stats!.outputTokens + stats!.liveOutputTokens)}`
      : DASH,
    // 0 means "not reported" for these two (agents that don't split the cache
    // out), indistinguishable from a real zero — so it reads as no data.
    cacheRead: stats && active ? zeroDash(stats.cacheReadTokens) : DASH,
    cacheWrite: stats && active ? zeroDash(stats.cacheWriteTokens) : DASH,
  };
}
