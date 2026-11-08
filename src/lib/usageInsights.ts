/**
 * usageInsights — the Token usage page's one data source (v2, 2026-11-08).
 *
 * `aggregateInsights(ledger, opts)` folds the local usage ledger into everything
 * the page draws: hero figures with the previous-period delta, the daily series
 * with cost, where tokens went (composition), where time went, per agent / model
 * rows, tool / command / skill rankings, edits, the hour × weekday grid, the
 * end-of-turn context distribution and recent sessions.
 *
 * Provenance travels with the numbers (`source`): "measured" = the agent's own
 * usage report or our local clock; "estimated" = visible text ÷ 4; "derived" =
 * a measured total minus the estimated parts; costs are list-price conversions.
 * Estimated parts are clamped so they never exceed the measured total.
 *
 * Days written before v2 have token figures but no `detail`: they still count in
 * the hero / trend / cost (unfiltered view), and `hasDetail` says whether the
 * richer sections have anything to show for the range.
 */

import {
  TPS_EDGES,
  TTFT_EDGES_MS,
  dayKeyOf,
  foldHourDays,
  mondayFirstHourGrid,
  type AgentDayDetail,
  type LedgerDay,
  type LedgerSession,
  type UsageLedger,
} from "./usageLedger";
import type { Tally } from "./usageDigest";
import { costOfBuckets, rateForModel, type RateIndex } from "./tokenPrices";

export type InsightRange = 1 | 7 | 30 | 90 | 180;

export type Buckets = { input: number; output: number; cached: number; cacheCreation: number; reasoning: number };

export type Source = "measured" | "estimated" | "derived";

export type DayPoint = {
  key: string;
  /** `11/6` */
  label: string;
  weekday: number;
  buckets: Buckets;
  total: number;
  /** List-price cost in USD; null when nothing that day could be priced. */
  usd: number | null;
  turns: number;
};

export type ModelRow = {
  modelId: string;
  buckets: Buckets;
  total: number;
  hit: number | null;
  usd: number | null;
  turns: number;
};

export type AgentRow = {
  agentId: string;
  buckets: Buckets;
  total: number;
  /** Cache hit = cached ÷ (input + cacheCreation + cached). */
  hit: number | null;
  usd: number | null;
  turns: number;
  ttftP50: number | null;
  tpsP50: number | null;
  /** Daily totals aligned with `UsageInsights.daily` (sparkline). */
  series: number[];
  models: ModelRow[];
};

export type RankRow = { key: string; label: string; kind: "tool" | "cmd" | "skill" | "mcp"; n: number; fail: number; avgMs: number };

export type CompositionKey = "cached" | "toolIo" | "agentInternal" | "user" | "thinking" | "answer";
export type CompositionPart = { key: CompositionKey; tokens: number; share: number; source: Source };

export type TimingKey = "ttft" | "think" | "answer" | "tool" | "other";
export type TimingPart = { key: TimingKey; ms: number; share: number };

export type SessionRow = LedgerSession & { id: string; total: number; usd: number | null };

export type Percentiles = { p50: number; p95: number };

export type UsageInsights = {
  range: InsightRange;
  agent: string | null;
  hasData: boolean;
  /** Any v2 detail (timing / tools / composition) in the range. */
  hasDetail: boolean;
  totals: {
    buckets: Buckets;
    total: number;
    turns: number;
    sessions: number;
    agents: number;
    models: number;
    /** Sum of send → turn-end wall time (v2 days only). */
    wallMs: number;
    activeDays: number;
  };
  previous: { total: number; turns: number; usd: number | null; hit: number | null };
  /** Change vs the previous period of the same length; null when that period was empty. */
  delta: { total: number | null; turns: number | null; usd: number | null };
  usd: { value: number | null; coverage: number };
  hit: number | null;
  ttft: Percentiles | null;
  tps: Percentiles | null;
  daily: DayPoint[];
  composition: CompositionPart[];
  timing: TimingPart[];
  /** Per-day time split aligned with `daily` (the time ribbon). */
  timingDaily: Record<TimingKey, number>[];
  agents: AgentRow[];
  tools: RankRow[];
  cmds: RankRow[];
  skills: RankRow[];
  toolTotals: { n: number; fail: number };
  cmdTotals: { n: number; fail: number };
  skillTotals: { n: number; fail: number };
  edits: { ops: number; add: number; del: number };
  /** 7 × 24, Monday first (not agent-filtered: hours are recorded per day). */
  hourGrid: number[][];
  /** Turns by end-of-turn context occupancy, 10 buckets of 10%. */
  ctxHist: number[];
  /** Turns that ended at ≥ 80% context. */
  ctxHot: number;
  sessions: SessionRow[];
};

// ── small helpers ────────────────────────────────────────────────────

const zero = (): Buckets => ({ input: 0, output: 0, cached: 0, cacheCreation: 0, reasoning: 0 });

function addB(a: Buckets, b: Partial<Buckets>): void {
  a.input += b.input ?? 0;
  a.output += b.output ?? 0;
  a.cached += b.cached ?? 0;
  a.cacheCreation += b.cacheCreation ?? 0;
  a.reasoning += b.reasoning ?? 0;
}

export function bucketTotal(b: Buckets): number {
  return b.input + b.output + b.cached + b.cacheCreation;
}

export function hitRate(b: Pick<Buckets, "input" | "cached" | "cacheCreation">): number | null {
  const denom = b.input + b.cached + b.cacheCreation;
  return denom > 0 ? b.cached / denom : null;
}

function keysBack(days: number, now: Date): { key: string; date: Date }[] {
  const out: { key: string; date: Date }[] = [];
  for (let i = days - 1; i >= 0; i--) {
    const d = new Date(now);
    d.setDate(d.getDate() - i);
    out.push({ key: dayKeyOf(d), date: d });
  }
  return out;
}

/** Percentile from a bucket histogram, linear inside the bucket. */
export function histPercentile(hist: readonly number[], edges: readonly number[], q: number): number | null {
  const total = hist.reduce((a, b) => a + b, 0);
  if (total <= 0) return null;
  const target = q * total;
  let seen = 0;
  for (let i = 0; i < hist.length; i++) {
    const n = hist[i] ?? 0;
    if (n <= 0) continue;
    if (seen + n >= target) {
      const lo = i === 0 ? 0 : edges[i - 1]!;
      const hi = i < edges.length ? edges[i]! : lo * 1.5;
      return lo + ((target - seen) / n) * (hi - lo);
    }
    seen += n;
  }
  return edges[edges.length - 1] ?? null;
}

function addHist(into: number[], from: readonly number[]): void {
  for (let i = 0; i < from.length; i++) into[i] = (into[i] ?? 0) + (from[i] ?? 0);
}

function mergeTallies(into: Record<string, Tally>, from: Record<string, Tally>): void {
  for (const [k, v] of Object.entries(from)) {
    const cur = into[k] ?? { n: 0, fail: 0, ms: 0 };
    into[k] = { n: cur.n + v.n, fail: cur.fail + v.fail, ms: cur.ms + v.ms };
  }
}

function rankTallies(map: Record<string, Tally>, kindOf: (key: string) => RankRow["kind"], labelOf: (key: string) => string): RankRow[] {
  return Object.entries(map)
    .map(([key, v]) => ({ key, label: labelOf(key), kind: kindOf(key), n: v.n, fail: v.fail, avgMs: v.n > 0 ? v.ms / v.n : 0 }))
    .sort((a, b) => b.n - a.n || a.label.localeCompare(b.label));
}

function totalsOf(rows: RankRow[]): { n: number; fail: number } {
  return rows.reduce((acc, r) => ({ n: acc.n + r.n, fail: acc.fail + r.fail }), { n: 0, fail: 0 });
}

type Priced = { usd: number | null; priced: number; unpriced: number };

function priceModels(day: LedgerDay, agent: string | null, prices: RateIndex | null): Priced {
  let usd = 0;
  let any = false;
  let priced = 0;
  let unpriced = 0;
  for (const [modelId, m] of Object.entries(day.models)) {
    if (agent && m.agent !== agent) continue;
    const cost = costOfBuckets(m, prices ? rateForModel(modelId, prices) : null);
    priced += cost.pricedTokens;
    unpriced += cost.unpricedTokens;
    if (cost.usd != null) {
      usd += cost.usd;
      any = true;
    }
  }
  return { usd: any ? usd : null, priced, unpriced };
}

/** The agent-scoped view of one day: detail when present, else the legacy figures. */
function dayScope(day: LedgerDay, agent: string | null): { buckets: Buckets; turns: number; details: AgentDayDetail[] } {
  const detail = day.detail ?? {};
  if (agent) {
    const d = detail[agent];
    if (d) return { buckets: { input: d.input, output: d.output, cached: d.cached, cacheCreation: d.cacheCreation, reasoning: d.reasoning }, turns: d.turns, details: [d] };
    const legacy = day.agents[agent];
    return { buckets: { ...zero(), output: legacy?.output ?? 0 }, turns: legacy?.turns ?? 0, details: [] };
  }
  return {
    buckets: { input: day.input, output: day.output, cached: day.cached, cacheCreation: day.cacheCreation, reasoning: day.reasoning },
    turns: day.turns,
    details: Object.values(detail),
  };
}

// ── the fold ─────────────────────────────────────────────────────────

export function aggregateInsights(
  ledger: UsageLedger,
  opts: { range: InsightRange; agent?: string | null; now?: Date; prices?: RateIndex | null },
): UsageInsights {
  const now = opts.now ?? new Date();
  const agent = opts.agent ?? null;
  const prices = opts.prices ?? null;
  const span = keysBack(opts.range, now);
  const prevEnd = new Date(now);
  prevEnd.setDate(prevEnd.getDate() - opts.range);
  const prevSpan = keysBack(opts.range, prevEnd);

  const totals = zero();
  let turns = 0;
  let wallMs = 0;
  let activeDays = 0;
  let usdSum = 0;
  let usdAny = false;
  let priced = 0;
  let unpriced = 0;
  const sessionIds = new Set<string>();
  const ttftHist = new Array(TTFT_EDGES_MS.length + 1).fill(0);
  const tpsHist = new Array(TPS_EDGES.length + 1).fill(0);
  const ctxHist = new Array(10).fill(0);
  const timing: Record<TimingKey, number> = { ttft: 0, think: 0, answer: 0, tool: 0, other: 0 };
  const est = { user: 0, attach: 0, toolIo: 0 };
  const detailBuckets = zero();
  const tools: Record<string, Tally> = {};
  const cmds: Record<string, Tally> = {};
  const skills: Record<string, Tally> = {};
  const edits = { ops: 0, add: 0, del: 0 };
  let hasDetail = false;

  type AgentAcc = { buckets: Buckets; turns: number; ttft: number[]; tps: number[]; series: number[]; models: Map<string, { buckets: Buckets; turns: number; usd: number; usdAny: boolean }>; usd: number; usdAny: boolean };
  const agentAcc = new Map<string, AgentAcc>();
  const agentOf = (id: string): AgentAcc => {
    let a = agentAcc.get(id);
    if (!a) {
      a = { buckets: zero(), turns: 0, ttft: new Array(ttftHist.length).fill(0), tps: new Array(tpsHist.length).fill(0), series: new Array(span.length).fill(0), models: new Map(), usd: 0, usdAny: false };
      agentAcc.set(id, a);
    }
    return a;
  };

  const daily: DayPoint[] = [];
  const timingDaily: Record<TimingKey, number>[] = [];
  const modelIds = new Set<string>();

  span.forEach(({ key, date }, idx) => {
    const day = ledger.days[key];
    const dayTiming: Record<TimingKey, number> = { ttft: 0, think: 0, answer: 0, tool: 0, other: 0 };
    if (!day) {
      daily.push({ key, label: `${date.getMonth() + 1}/${date.getDate()}`, weekday: date.getDay(), buckets: zero(), total: 0, usd: null, turns: 0 });
      timingDaily.push(dayTiming);
      return;
    }
    const scope = dayScope(day, agent);
    const cost = priceModels(day, agent, prices);
    addB(totals, scope.buckets);
    turns += scope.turns;
    if (scope.turns > 0) activeDays += 1;
    priced += cost.priced;
    unpriced += cost.unpriced;
    if (cost.usd != null) {
      usdSum += cost.usd;
      usdAny = true;
    }
    for (const id of day.sessionIds ?? []) sessionIds.add(id);
    for (const d of scope.details) {
      hasDetail = true;
      wallMs += d.wallMs;
      addHist(ttftHist, d.ttftHist);
      addHist(tpsHist, d.tpsHist);
      addHist(ctxHist, d.ctxHist);
      for (const k of Object.keys(timing) as TimingKey[]) {
        timing[k] += d.timing[k];
        dayTiming[k] += d.timing[k];
      }
      est.user += d.est.user;
      est.attach += d.est.attach;
      est.toolIo += d.est.toolIo;
      addB(detailBuckets, d);
      mergeTallies(tools, d.tools);
      mergeTallies(cmds, d.cmds);
      mergeTallies(skills, d.skills);
      edits.ops += d.edits.ops;
      edits.add += d.edits.add;
      edits.del += d.edits.del;
    }
    // Agent rows: detail where present, legacy output/turns otherwise.
    const agentIds = new Set([...Object.keys(day.detail ?? {}), ...Object.keys(day.agents)]);
    for (const id of agentIds) {
      if (agent && id !== agent) continue;
      const a = agentOf(id);
      const d = day.detail?.[id];
      if (d) {
        addB(a.buckets, d);
        a.turns += d.turns;
        addHist(a.ttft, d.ttftHist);
        addHist(a.tps, d.tpsHist);
        a.series[idx] = (a.series[idx] ?? 0) + d.input + d.output + d.cached + d.cacheCreation;
      } else {
        const l = day.agents[id]!;
        a.buckets.output += l.output;
        a.turns += l.turns;
        a.series[idx] = (a.series[idx] ?? 0) + l.output;
      }
    }
    for (const [modelId, m] of Object.entries(day.models)) {
      if (agent && m.agent !== agent) continue;
      modelIds.add(modelId);
      const a = agentOf(m.agent ?? "unknown");
      const row = a.models.get(modelId) ?? { buckets: zero(), turns: 0, usd: 0, usdAny: false };
      addB(row.buckets, m);
      row.turns += m.turns;
      const c = costOfBuckets(m, prices ? rateForModel(modelId, prices) : null);
      if (c.usd != null) {
        row.usd += c.usd;
        row.usdAny = true;
        a.usd += c.usd;
        a.usdAny = true;
      }
      a.models.set(modelId, row);
    }
    daily.push({
      key,
      label: `${date.getMonth() + 1}/${date.getDate()}`,
      weekday: date.getDay(),
      buckets: scope.buckets,
      total: bucketTotal(scope.buckets),
      usd: cost.usd,
      turns: scope.turns,
    });
    timingDaily.push(dayTiming);
  });

  // previous period (same length, immediately before)
  const prev = zero();
  let prevTurns = 0;
  let prevUsd = 0;
  let prevUsdAny = false;
  for (const { key } of prevSpan) {
    const day = ledger.days[key];
    if (!day) continue;
    const scope = dayScope(day, agent);
    addB(prev, scope.buckets);
    prevTurns += scope.turns;
    const c = priceModels(day, agent, prices);
    if (c.usd != null) {
      prevUsd += c.usd;
      prevUsdAny = true;
    }
  }
  const prevTotal = bucketTotal(prev);
  const total = bucketTotal(totals);
  const change = (cur: number, before: number): number | null => (before > 0 ? (cur - before) / before : null);

  // composition: measured total of the v2 days, split by provenance
  const composition: CompositionPart[] = [];
  const compTotal = bucketTotal(detailBuckets);
  if (compTotal > 0) {
    const freshIn = detailBuckets.input + detailBuckets.cacheCreation;
    let user = est.user + est.attach;
    let toolIo = est.toolIo;
    const seen = user + toolIo;
    if (seen > freshIn && seen > 0) {
      // Estimates may never exceed what the agent says it sent.
      user = (user / seen) * freshIn;
      toolIo = (toolIo / seen) * freshIn;
    }
    const thinking = Math.min(detailBuckets.reasoning, detailBuckets.output);
    const parts: [CompositionKey, number, Source][] = [
      ["cached", detailBuckets.cached, "measured"],
      ["toolIo", toolIo, "estimated"],
      ["agentInternal", Math.max(0, freshIn - user - toolIo), "derived"],
      ["user", user, "estimated"],
      ["thinking", thinking, "measured"],
      ["answer", Math.max(0, detailBuckets.output - thinking), "measured"],
    ];
    for (const [key, tokens, source] of parts) composition.push({ key, tokens, share: tokens / compTotal, source });
  }

  const timingTotal = Object.values(timing).reduce((a, b) => a + b, 0);
  const timingParts: TimingPart[] = (Object.keys(timing) as TimingKey[]).map((key) => ({
    key,
    ms: timing[key],
    share: timingTotal > 0 ? timing[key] / timingTotal : 0,
  }));

  const agents: AgentRow[] = [...agentAcc.entries()]
    .map(([agentId, a]) => {
      const models: ModelRow[] = [...a.models.entries()]
        .map(([modelId, m]) => ({ modelId, buckets: m.buckets, total: bucketTotal(m.buckets), hit: hitRate(m.buckets), usd: m.usdAny ? m.usd : null, turns: m.turns }))
        .sort((x, y) => y.total - x.total);
      return {
        agentId,
        buckets: a.buckets,
        total: bucketTotal(a.buckets),
        hit: hitRate(a.buckets),
        usd: a.usdAny ? a.usd : null,
        turns: a.turns,
        ttftP50: histPercentile(a.ttft, TTFT_EDGES_MS, 0.5),
        tpsP50: histPercentile(a.tps, TPS_EDGES, 0.5),
        series: a.series,
        models,
      };
    })
    .filter((a) => a.turns > 0 || a.total > 0)
    .sort((x, y) => y.total - x.total);

  const toolRows = rankTallies(tools, () => "tool", (k) => k);
  const cmdRows = rankTallies(cmds, () => "cmd", (k) => k);
  const skillRows = rankTallies(
    skills,
    (k) => (k.startsWith("mcp:") ? "mcp" : "skill"),
    (k) => k.replace(/^(mcp|skill):/, ""),
  );

  const fromMs = span.length > 0 ? new Date(`${span[0]!.key}T00:00:00`).getTime() : 0;
  const sessions: SessionRow[] = Object.entries(ledger.sessions ?? {})
    .filter(([, s]) => s.last >= fromMs && (!agent || s.agentId === agent))
    .map(([id, s]) => {
      const b = { input: s.input, output: s.output, cached: s.cached, cacheCreation: s.cacheCreation };
      const c = s.modelId && prices ? costOfBuckets(b, rateForModel(s.modelId, prices)) : null;
      return { ...s, id, total: b.input + b.output + b.cached + b.cacheCreation, usd: c?.usd ?? null };
    })
    .sort((a, b) => b.last - a.last);

  const hourGrid = mondayFirstHourGrid(foldHourDays(ledger, opts.range, now));
  const hit = hitRate(totals);
  const prevHit = hitRate(prev);
  const ttftP50 = histPercentile(ttftHist, TTFT_EDGES_MS, 0.5);
  const tpsP50 = histPercentile(tpsHist, TPS_EDGES, 0.5);

  return {
    range: opts.range,
    agent,
    hasData: turns > 0 || total > 0,
    hasDetail,
    totals: {
      buckets: totals,
      total,
      turns,
      sessions: sessionIds.size,
      agents: agents.length,
      models: modelIds.size,
      wallMs,
      activeDays,
    },
    previous: { total: prevTotal, turns: prevTurns, usd: prevUsdAny ? prevUsd : null, hit: prevHit },
    delta: {
      total: change(total, prevTotal),
      turns: change(turns, prevTurns),
      usd: usdAny && prevUsdAny ? change(usdSum, prevUsd) : null,
    },
    usd: { value: usdAny ? usdSum : null, coverage: priced + unpriced > 0 ? priced / (priced + unpriced) : 0 },
    hit,
    ttft: ttftP50 != null ? { p50: ttftP50, p95: histPercentile(ttftHist, TTFT_EDGES_MS, 0.95) ?? ttftP50 } : null,
    tps: tpsP50 != null ? { p50: tpsP50, p95: histPercentile(tpsHist, TPS_EDGES, 0.95) ?? tpsP50 } : null,
    daily,
    composition,
    timing: timingParts,
    timingDaily,
    agents,
    tools: toolRows,
    cmds: cmdRows,
    skills: skillRows,
    toolTotals: totalsOf(toolRows),
    cmdTotals: totalsOf(cmdRows),
    skillTotals: totalsOf(skillRows),
    edits,
    hourGrid,
    ctxHist,
    ctxHot: (ctxHist[8] ?? 0) + (ctxHist[9] ?? 0),
    sessions,
  };
}
