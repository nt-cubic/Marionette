/**
 * usageLedger — 跨会话 token 用量台账(纯前端,localStorage)。
 *
 * 每轮 turn/complete 时把 turnStats 增量记进按「本地日」聚合的台账
 * (turnStats 已盖在 assistant_message 上随转录持久化,这里另记一份聚合,
 * 供 ContextPanel 的「Token 总览」与 Token 用量分析面板跨会话展示)。
 * O(1) 写入、跨重启,绝不为了聚合去重载转录(内存压力红线)。
 * 保留 180 个自然日(分析页的 GitHub 式格子要铺出小半年),写入时修剪。
 *
 * 记账挂在 turn/complete 处理器里,与事件盖戳共用同一条单窗口不变量
 * (同一轮只被一个窗口处理,否则转录本身就会重复)。**多窗口合并**:
 * 撕出的 detached 窗口同样会记账,而每个窗口各持一份内存副本——落账前
 * 重新读盘、把自己的这笔并进去再写回(读-合并-写),并监听 `storage`
 * 事件让其它窗口的写入立刻反映到本窗 UI;整文件覆盖写仍可能丢最后一
 * 毫秒的对端写入,概率极低且只丢聚合展示,不碰转录真相。
 */

import { useSyncExternalStore } from "react";
import type { TurnStats } from "./types";

export type LedgerAgentUsage = { output: number; turns: number };

/** 每模型一行的四桶拆分(牌价折算按桶计价,模型间费率不同)。 */
export type LedgerModelUsage = {
  input: number;
  output: number;
  cached: number;
  cacheCreation: number;
  turns: number;
};

export type LedgerDay = {
  input: number;
  output: number;
  cached: number;
  cacheCreation: number;
  reasoning: number;
  turns: number;
  /** 有流式时长的轮次之和(本地实测,mm 不含排队)。 */
  ms: number;
  agents: Record<string, LedgerAgentUsage>;
  models: Record<string, LedgerModelUsage>;
  /** 24 桶:当天各小时落账的轮数(活动节奏热图的原料;按查看者本地钟)。 */
  hours: number[];
};

export type UsageLedger = {
  version: 1;
  /** key = 本地日期 YYYY-MM-DD。 */
  days: Record<string, LedgerDay>;
};

const LEDGER_KEY = "marionette-usage-ledger";
const RETAIN_DAYS = 180;

// ── 模块级 store(useSyncExternalStore 订阅,语言无关) ──────────────────

const HAS_WINDOW = typeof window !== "undefined";

function normalizeDay(raw: unknown): LedgerDay | null {
  if (typeof raw !== "object" || raw == null) return null;
  const d = raw as Partial<LedgerDay>;
  return {
    input: d.input ?? 0,
    output: d.output ?? 0,
    cached: d.cached ?? 0,
    cacheCreation: d.cacheCreation ?? 0,
    reasoning: d.reasoning ?? 0,
    turns: d.turns ?? 0,
    ms: d.ms ?? 0,
    agents: d.agents ?? {},
    models: d.models ?? {},
    hours: Array.isArray(d.hours) && d.hours.length === 24 ? d.hours : new Array(24).fill(0),
  };
}

function loadLedger(): UsageLedger {
  const empty: UsageLedger = { version: 1, days: {} };
  if (!HAS_WINDOW) return empty;
  try {
    const raw = window.localStorage.getItem(LEDGER_KEY);
    if (!raw) return empty;
    const parsed = JSON.parse(raw) as UsageLedger;
    if (parsed?.version !== 1 || typeof parsed.days !== "object" || !parsed.days) {
      return empty;
    }
    const days: Record<string, LedgerDay> = {};
    for (const [key, value] of Object.entries(parsed.days)) {
      const day = normalizeDay(value);
      if (day) days[key] = day;
    }
    return { version: 1, days };
  } catch {
    return empty;
  }
}

let ledger: UsageLedger = loadLedger();
let storageOk = true;
const listeners = new Set<() => void>();

function emit() {
  for (const listener of listeners) listener();
}

function persist() {
  if (!storageOk || !HAS_WINDOW) return;
  try {
    window.localStorage.setItem(LEDGER_KEY, JSON.stringify(ledger));
  } catch {
    storageOk = false;
  }
}

/** 另一个窗口写了台账 → 重读并通知订阅者(detached 窗同享一份账)。 */
if (HAS_WINDOW) {
  try {
    window.addEventListener("storage", (event) => {
      if (event.key !== LEDGER_KEY) return;
      ledger = loadLedger();
      emit();
    });
  } catch {
    /* storage 不可用(极端环境)——单窗口语义照常 */
  }
}

export function subscribeUsageLedger(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function getUsageLedger(): UsageLedger {
  return ledger;
}

/** React 侧订阅;record 替换整个 ledger 对象保证引用变化。 */
export function useUsageLedger(): UsageLedger {
  return useSyncExternalStore(subscribeUsageLedger, getUsageLedger);
}

// ── 记账(纯函数核心 + 薄封装) ───────────────────────────────────────

/** 本地日期 key:YYYY-MM-DD。 */
export function dayKeyOf(at: Date): string {
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${at.getFullYear()}-${pad(at.getMonth() + 1)}-${pad(at.getDate())}`;
}

function emptyDay(): LedgerDay {
  return {
    input: 0,
    output: 0,
    cached: 0,
    cacheCreation: 0,
    reasoning: 0,
    turns: 0,
    ms: 0,
    agents: {},
    models: {},
    hours: new Array(24).fill(0),
  };
}

/** 一轮是否真的报了数字(全 null 的 turnStats 不记账)。 */
function hasTokenFigures(stats: TurnStats): boolean {
  return stats.input != null || stats.output != null || stats.total != null;
}

/** 纯函数:把一轮记进台账(不改原对象),返回新台账。 */
export function recordTurnUsageInto(
  target: UsageLedger,
  entry: {
    agentId?: string | null;
    modelId?: string | null;
    stats: TurnStats;
    endedAt: number;
  },
  now: Date = new Date(entry.endedAt),
): UsageLedger {
  if (!hasTokenFigures(entry.stats)) return target;
  const key = dayKeyOf(now);
  const day = normalizeDay(target.days[key]) ?? emptyDay();
  const s = entry.stats;
  const next: LedgerDay = {
    input: day.input + (s.input ?? 0),
    output: day.output + (s.output ?? 0),
    cached: day.cached + (s.cached ?? 0),
    cacheCreation: day.cacheCreation + (s.cacheCreation ?? 0),
    reasoning: day.reasoning + (s.reasoning ?? 0),
    turns: day.turns + 1,
    ms: day.ms + Math.max(0, s.durationMs ?? 0),
    agents: { ...day.agents },
    models: { ...day.models },
    hours: [...day.hours],
  };
  const agentId = entry.agentId?.trim() || "unknown";
  const agent = next.agents[agentId] ?? { output: 0, turns: 0 };
  next.agents[agentId] = {
    output: agent.output + (s.output ?? 0),
    turns: agent.turns + 1,
  };
  const modelId = entry.modelId?.trim();
  if (modelId) {
    const model = next.models[modelId] ?? {
      input: 0,
      output: 0,
      cached: 0,
      cacheCreation: 0,
      turns: 0,
    };
    next.models[modelId] = {
      input: model.input + (s.input ?? 0),
      output: model.output + (s.output ?? 0),
      cached: model.cached + (s.cached ?? 0),
      cacheCreation: model.cacheCreation + (s.cacheCreation ?? 0),
      turns: model.turns + 1,
    };
  }
  next.hours[now.getHours()] = (next.hours[now.getHours()] ?? 0) + 1;
  return { version: 1, days: { ...target.days, [key]: next } };
}

/** 修剪超期日(保留最近 RETAIN_DAYS 个自然日,含今天),返回新台账。 */
export function pruneLedger(target: UsageLedger, now: Date = new Date()): UsageLedger {
  const cutoff = new Date(now);
  cutoff.setDate(cutoff.getDate() - (RETAIN_DAYS - 1));
  const cutoffKey = dayKeyOf(cutoff);
  const days = Object.fromEntries(
    Object.entries(target.days).filter(([key]) => key >= cutoffKey),
  );
  return { version: 1, days };
}

/** 落账入口:App 的 turn/complete 处理器调用。 */
export function recordTurnUsage(entry: {
  agentId?: string | null;
  modelId?: string | null;
  stats: TurnStats;
  endedAt: number;
}): void {
  // 读-合并-写:先拾起其它窗口可能刚写的账,再并进这一笔(detached 窗并存)。
  const fresh = storageOk ? loadLedger() : ledger;
  ledger = pruneLedger(recordTurnUsageInto(fresh, entry));
  persist();
  emit();
}

// ── 聚合(UI 消费) ───────────────────────────────────────────────────

export type LedgerAggregate = LedgerDay & { days: number };

function emptyAggregate(): LedgerAggregate {
  return { ...emptyDay(), days: 0 };
}

function addToAggregate(acc: LedgerAggregate, day: LedgerDay): void {
  acc.input += day.input;
  acc.output += day.output;
  acc.cached += day.cached;
  acc.cacheCreation += day.cacheCreation;
  acc.reasoning += day.reasoning;
  acc.turns += day.turns;
  acc.ms += day.ms;
  if (day.turns > 0) acc.days += 1;
  for (const [agentId, agent] of Object.entries(day.agents)) {
    const slot = acc.agents[agentId] ?? { output: 0, turns: 0 };
    acc.agents[agentId] = { output: slot.output + agent.output, turns: slot.turns + agent.turns };
  }
  for (const [modelId, model] of Object.entries(day.models)) {
    const slot =
      acc.models[modelId] ?? { input: 0, output: 0, cached: 0, cacheCreation: 0, turns: 0 };
    acc.models[modelId] = {
      input: slot.input + model.input,
      output: slot.output + model.output,
      cached: slot.cached + model.cached,
      cacheCreation: slot.cacheCreation + model.cacheCreation,
      turns: slot.turns + model.turns,
    };
  }
  for (let h = 0; h < 24; h += 1) {
    acc.hours[h] = (acc.hours[h] ?? 0) + (day.hours[h] ?? 0);
  }
}

/** 聚合 [endAt − days + 1, endAt] 这几个自然日(endAt 当天含)。 */
export function aggregateDays(
  target: UsageLedger,
  days: number,
  endAt: Date = new Date(),
): LedgerAggregate {
  const acc = emptyAggregate();
  const from = new Date(endAt);
  from.setDate(from.getDate() - (days - 1));
  const fromKey = dayKeyOf(from);
  const endKey = dayKeyOf(endAt);
  for (const [key, day] of Object.entries(target.days)) {
    if (key >= fromKey && key <= endKey) addToAggregate(acc, day);
  }
  return acc;
}

export type LedgerSummary = {
  today: LedgerAggregate;
  last7: LedgerAggregate;
  last30: LedgerAggregate;
  /** 上一周期(近 7 天窗口再往前数 7 天),分析页的环比。 */
  prev7: LedgerAggregate;
  prev30: LedgerAggregate;
};

/** 纯函数:按本地日聚合出今天 / 近 7 天 / 近 30 天 + 各自的上一周期。 */
export function summarizeLedger(target: UsageLedger, now: Date = new Date()): LedgerSummary {
  const at7 = new Date(now);
  at7.setDate(at7.getDate() - 7);
  const at30 = new Date(now);
  at30.setDate(at30.getDate() - 30);
  return {
    today: aggregateDays(target, 1, now),
    last7: aggregateDays(target, 7, now),
    last30: aggregateDays(target, 30, now),
    prev7: aggregateDays(target, 7, at7),
    prev30: aggregateDays(target, 30, at30),
  };
}

/** 近 7 天按输出 token 降序的助手清单(UI 的「按助手」明细)。 */
export function topAgents7(
  summary: LedgerSummary,
  limit = 5,
): Array<{ agentId: string; output: number; turns: number }> {
  return Object.entries(summary.last7.agents)
    .map(([agentId, a]) => ({ agentId, output: a.output, turns: a.turns }))
    .filter((row) => row.turns > 0)
    .sort((a, b) => b.output - a.output)
    .slice(0, limit);
}

/** 模型排名:按总 token 降序,超过 limit 折叠进「其它」(C11 的 Other 折叠)。 */
export function rankedModels(
  agg: LedgerAggregate,
  limit = 7,
): Array<{
  modelId: string;
  input: number;
  output: number;
  cached: number;
  cacheCreation: number;
  turns: number;
}> {
  const rows = Object.entries(agg.models)
    .map(([modelId, m]) => ({
      modelId,
      input: m.input,
      output: m.output,
      cached: m.cached,
      cacheCreation: m.cacheCreation,
      turns: m.turns,
      total: m.input + m.output + m.cached + m.cacheCreation,
    }))
    .filter((row) => row.turns > 0)
    .sort((a, b) => b.total - a.total);
  const head = rows.slice(0, limit).map(({ total: _total, ...row }) => row);
  const tail = rows.slice(limit);
  if (tail.length > 0) {
    const other = tail.reduce(
      (acc, row) => ({
        modelId: "other",
        input: acc.input + row.input,
        output: acc.output + row.output,
        cached: acc.cached + row.cached,
        cacheCreation: acc.cacheCreation + row.cacheCreation,
        turns: acc.turns + row.turns,
      }),
      { modelId: "other", input: 0, output: 0, cached: 0, cacheCreation: 0, turns: 0 },
    );
    head.push(other);
  }
  return head;
}

export type DailyPoint = {
  key: string;
  /** 周几(0=周日),GitHub 式格子与热图折叠用。 */
  weekday: number;
  input: number;
  output: number;
  cached: number;
  cacheCreation: number;
  turns: number;
};

/** 连续 days 天的逐日序列(含空日),趋势柱与格子的数据源。 */
export function dailySeries(
  target: UsageLedger,
  days: number,
  endAt: Date = new Date(),
): DailyPoint[] {
  const points: DailyPoint[] = [];
  for (let i = days - 1; i >= 0; i -= 1) {
    const d = new Date(endAt);
    d.setDate(d.getDate() - i);
    const key = dayKeyOf(d);
    const day = target.days[key];
    points.push({
      key,
      weekday: d.getDay(),
      input: day?.input ?? 0,
      output: day?.output ?? 0,
      cached: day?.cached ?? 0,
      cacheCreation: day?.cacheCreation ?? 0,
      turns: day?.turns ?? 0,
    });
  }
  return points;
}

/** 小时×星期折叠(7×24 的 turns 计数)——活动节奏热图的数据源。 */
export function foldHourWeek(
  target: UsageLedger,
  weeks: number,
  endAt: Date = new Date(),
): number[][] {
  const grid = Array.from({ length: 7 }, () => new Array(24).fill(0));
  const from = new Date(endAt);
  from.setDate(from.getDate() - (weeks * 7 - 1));
  const fromKey = dayKeyOf(from);
  const endKey = dayKeyOf(endAt);
  for (const [key, day] of Object.entries(target.days)) {
    if (key < fromKey || key > endKey) continue;
    const weekday = new Date(`${key}T00:00:00`).getDay();
    for (let h = 0; h < 24; h += 1) grid[weekday]![h]! += day.hours[h] ?? 0;
  }
  return grid;
}

/** 环比:当前 vs 上一周期,输出 token 的变化百分比(null = 上期为 0,无环比)。 */
export function deltaPercent(current: number, previous: number): number | null {
  if (previous <= 0) return null;
  return (current - previous) / previous;
}
