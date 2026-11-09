/**
 * usageBackfill — the agent CLIs' own on-disk session stores, folded by
 * usage_backfill.rs (Rust) into per-session, per-UTC-hour usage facts.
 *
 * The scan runs off the main thread and caches per file on disk, so a repeat
 * scan is cheap. Types mirror the Rust contract exactly (serde camelCase):
 * buckets use the ledger's semantics (input = fresh, cached = cache read,
 * cacheCreation = cache write, output includes reasoning).
 */

import { invoke } from "@tauri-apps/api/core";
import { isTauriRuntime } from "./api";
import { dayKeyOf } from "./usageLedger";

export type BackfillUsageHour = {
  /** Hours since the Unix epoch, UTC: floor(t_ms / 3_600_000). */
  hour: number;
  model: string | null;
  turns: number;
  requests: number;
  input: number;
  output: number;
  cached: number;
  cacheCreation: number;
  reasoning: number;
  /** The agent's own cost for this hour's requests (USD), when the store has one. */
  reportedUsd: number | null;
};

export type BackfillSession = {
  /** Marionette agent id: grok-build | claude-code | codex | deepseek | opencode | zcode. */
  agent: string;
  nativeId: string;
  parentNativeId: string | null;
  title: string | null;
  cwd: string | null;
  firstAt: number;
  lastAt: number;
  hours: BackfillUsageHour[];
  ctxWindow: number | null;
  ctxSeries: Array<[number, number]>;
  compactions: number[];
  /** Conversation composition (grok today); absent for stores without the
   * raw request material. */
  composition?: BackfillComposition | null;
};

export type BackfillSource = {
  agent: string;
  root: string | null;
  found: boolean;
  files: number;
  sessions: number;
  turns: number;
  error: string | null;
  elapsedMs: number;
};

/** Conversation composition (grok today): token estimates per category over
 * the session's real request material, plus the measured final-context anchor. */
export type BackfillComposition = {
  system: number;
  toolDefs: number;
  inject: number;
  user: number;
  attach: number;
  assistant: number;
  reasoning: number;
  toolCalls: number;
  toolResults: number;
  /** Tokens in context at the end (grok signals `contextTokensUsed`); the UI
   * scales the estimates to it. Absent in sessions without the reading. */
  measuredPrompt: number | null;
};

export type BackfillReport = {
  version: number;
  generatedAt: number;
  sources: BackfillSource[];
  sessions: BackfillSession[];
};

export function emptyBackfillReport(): BackfillReport {
  return { version: 1, generatedAt: 0, sources: [], sessions: [] };
}

/** On-demand composition of one live Grok session (`nativeId` = the sessionId
 * the adapter reports; Marionette stores it in `Session.nativeSessionIds`). */
export async function fetchGrokComposition(nativeId: string): Promise<BackfillComposition | null> {
  if (!isTauriRuntime()) return null;
  try {
    return await invoke<BackfillComposition | null>("usage_grok_composition", { nativeId });
  } catch {
    return null;
  }
}

export type CompositionKey =
  | "system" | "toolDefs" | "inject" | "user" | "attach"
  | "assistant" | "reasoning" | "toolCalls" | "toolResults";

export const COMPOSITION_ORDER: readonly CompositionKey[] = [
  "system", "toolDefs", "inject", "user", "attach", "assistant", "reasoning", "toolCalls", "toolResults",
];

export type ScaledComposition = {
  parts: Array<{ key: CompositionKey; tokens: number; share: number }>;
  total: number;
  /** True when the shares were scaled to the measured final-context anchor. */
  calibrated: boolean;
  /** How far the raw estimate was from the anchor (e.g. 0.79); null when not calibrated. */
  coverage: number | null;
};

/**
 * Scale the per-category estimates so they sum to the measured final-context
 * anchor when present (each part keeps its proportion); without an anchor the
 * shares are the raw estimate's own. Nothing exceeds the anchor either way.
 */
export function scaleComposition(comp: BackfillComposition): ScaledComposition {
  const raw = COMPOSITION_ORDER.map((key) => ({ key, tokens: comp[key] as number }));
  const estTotal = raw.reduce((sum, part) => sum + part.tokens, 0);
  const anchor = comp.measuredPrompt != null && comp.measuredPrompt > 0 ? comp.measuredPrompt : null;
  if (estTotal <= 0) {
    return { parts: raw.map((part) => ({ ...part, share: 0 })), total: anchor ?? 0, calibrated: false, coverage: null };
  }
  if (anchor == null) {
    return { parts: raw.map((part) => ({ ...part, share: part.tokens / estTotal })), total: estTotal, calibrated: false, coverage: null };
  }
  const scale = anchor / estTotal;
  const scaled = raw.map((part) => ({ ...part, tokens: Math.round(part.tokens * scale) }));
  return {
    parts: scaled.map((part) => ({ ...part, share: part.tokens / anchor })),
    total: anchor,
    calibrated: true,
    coverage: Math.min(1, estTotal / anchor),
  };
}

export async function scanBackfill(force = false): Promise<BackfillReport> {
  if (!isTauriRuntime()) return emptyBackfillReport();
  return invoke<BackfillReport>("usage_backfill_scan", { force: force || null });
}

/** A backfill session folded into the page's window (local days, like the ledger). */
export type BackfillRow = {
  session: BackfillSession;
  turns: number;
  tokens: number;
  reportedUsd: number;
  /** True when the live ledger already counts this session (matched by native id). */
  duplicate: boolean;
};

export type BackfillTotals = {
  rows: BackfillRow[];
  turns: number;
  tokens: number;
  reportedUsd: number;
  sessions: number;
  /** Turns whose hour fell inside DeepSeek's peak window (×2 applies at display). */
  peakTokens: number;
};

/**
 * Fold the report into the [now − days + 1, now] local-day window (and an agent
 * filter). Duplicate sessions (native id known to a Marionette session) are
 * listed but not summed — their turns are already in the live ledger.
 */
export function foldBackfill(
  report: BackfillReport,
  opts: { days: number; agent: string | null; nativeIds: ReadonlySet<string>; now?: Date },
): BackfillTotals {
  const now = opts.now ?? new Date();
  const from = new Date(now);
  from.setDate(from.getDate() - (opts.days - 1));
  const fromMs = new Date(from.getFullYear(), from.getMonth(), from.getDate()).getTime();
  const endMs = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime() + 86_399_999;

  const rows: BackfillRow[] = [];
  let turns = 0;
  let tokens = 0;
  let reportedUsd = 0;
  let peakTokens = 0;
  for (const session of report.sessions) {
    if (opts.agent && session.agent !== opts.agent) continue;
    const duplicate = opts.nativeIds.has(`${session.agent}\u0000${session.nativeId}`) || opts.nativeIds.has(session.nativeId);
    let rowTurns = 0;
    let rowTokens = 0;
    let rowUsd = 0;
    let rowPeak = 0;
    for (const h of session.hours) {
      const t = h.hour * 3_600_000;
      if (t < fromMs || t > endMs) continue;
      rowTurns += h.turns;
      rowTokens += h.input + h.output + h.cached + h.cacheCreation;
      rowUsd += h.reportedUsd ?? 0;
      if (isPeakHour(h.hour)) rowPeak += h.input + h.output + h.cached + h.cacheCreation;
    }
    if (rowTurns === 0 && rowTokens === 0 && session.lastAt < fromMs) continue;
    rows.push({ session, turns: rowTurns, tokens: rowTokens, reportedUsd: rowUsd, duplicate });
    if (!duplicate) {
      turns += rowTurns;
      tokens += rowTokens;
      reportedUsd += rowUsd;
      peakTokens += rowPeak;
    }
  }
  rows.sort((a, b) => b.tokens - a.tokens || b.session.lastAt - a.session.lastAt);
  return { rows, turns, tokens, reportedUsd, sessions: rows.filter((row) => !row.duplicate).length, peakTokens };
}

/**
 * DeepSeek peak window in Beijing time (UTC+8): Mon–Fri 09:00–12:00 and
 * 14:00–18:00 — mirrors tokenPrices.isDeepSeekPeak, on UTC epoch hours.
 */
export function isPeakHour(epochHour: number): boolean {
  const beijing = new Date(epochHour * 3_600_000 + 8 * 3_600_000);
  const weekday = beijing.getUTCDay();
  if (weekday === 0 || weekday === 6) return false;
  const hour = beijing.getUTCHours();
  return (hour >= 9 && hour < 12) || (hour >= 14 && hour < 18);
}

/** Local day key of a UTC epoch hour (for row tooltips). */
export function hourDayKey(epochHour: number): string {
  return dayKeyOf(new Date(epochHour * 3_600_000));
}
