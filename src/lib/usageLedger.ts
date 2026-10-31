/**
 * usageLedger — 跨会话 token 用量台账(纯前端,localStorage)。
 *
 * 每轮 turn/complete 时把 turnStats 增量记进按「本地日」聚合的台账
 * (turnStats 已盖在 assistant_message 上随转录持久化,这里另记一份聚合,
 * 供 ContextPanel 的「Token 总览」跨会话展示)。O(1) 写入、跨重启,
 * 绝不为了聚合去重载转录(内存压力红线)。保留 30 天,写入时修剪。
 *
 * 记账挂在 turn/complete 处理器里,与事件盖戳共用同一条单窗口不变量
 * (同一轮只被一个窗口处理,否则转录本身就会重复)。
 */

import { useSyncExternalStore } from "react";
import type { TurnStats } from "./types";

export type LedgerAgentUsage = { output: number; turns: number };

export type LedgerDay = {
  input: number;
  output: number;
  cached: number;
  reasoning: number;
  turns: number;
  /** 有流式时长的轮次之和(本地实测,mm 不含排队)。 */
  ms: number;
  agents: Record<string, LedgerAgentUsage>;
};

export type UsageLedger = {
  version: 1;
  /** key = 本地日期 YYYY-MM-DD。 */
  days: Record<string, LedgerDay>;
};

const LEDGER_KEY = "marionette-usage-ledger";
const RETAIN_DAYS = 30;

// ── 模块级 store(useSyncExternalStore 订阅,语言无关) ──────────────────

function loadLedger(): UsageLedger {
  try {
    const raw = window.localStorage.getItem(LEDGER_KEY);
    if (!raw) return { version: 1, days: {} };
    const parsed = JSON.parse(raw) as UsageLedger;
    if (parsed?.version !== 1 || typeof parsed.days !== "object" || !parsed.days) {
      return { version: 1, days: {} };
    }
    return parsed;
  } catch {
    return { version: 1, days: {} };
  }
}

let ledger: UsageLedger = { version: 1, days: {} };
/** 模块加载即读盘;localStorage 被拒时台账只在内存里活到本次会话。 */
try {
  ledger = loadLedger();
} catch {
  /* window 不可见(verify 脚本)——保持空台账,测试直接喂纯函数 */
}
let storageOk = true;
const listeners = new Set<() => void>();

function emit() {
  for (const listener of listeners) listener();
}

function persist() {
  if (!storageOk) return;
  try {
    window.localStorage.setItem(LEDGER_KEY, JSON.stringify(ledger));
  } catch {
    storageOk = false;
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
  return { input: 0, output: 0, cached: 0, reasoning: 0, turns: 0, ms: 0, agents: {} };
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
    stats: TurnStats;
    endedAt: number;
  },
  now: Date = new Date(entry.endedAt),
): UsageLedger {
  if (!hasTokenFigures(entry.stats)) return target;
  const key = dayKeyOf(now);
  const day = target.days[key] ?? emptyDay();
  const next: LedgerDay = {
    input: day.input + (entry.stats.input ?? 0),
    output: day.output + (entry.stats.output ?? 0),
    cached: day.cached + (entry.stats.cached ?? 0),
    reasoning: day.reasoning + (entry.stats.reasoning ?? 0),
    turns: day.turns + 1,
    ms: day.ms + Math.max(0, entry.stats.durationMs ?? 0),
    agents: { ...day.agents },
  };
  const agentId = entry.agentId?.trim() || "unknown";
  const agent = next.agents[agentId] ?? { output: 0, turns: 0 };
  next.agents[agentId] = {
    output: agent.output + (entry.stats.output ?? 0),
    turns: agent.turns + 1,
  };
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
  stats: TurnStats;
  endedAt: number;
}): void {
  ledger = pruneLedger(recordTurnUsageInto(ledger, entry));
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
  acc.reasoning += day.reasoning;
  acc.turns += day.turns;
  acc.ms += day.ms;
  acc.days += 1;
  for (const [agentId, agent] of Object.entries(day.agents)) {
    const slot = acc.agents[agentId] ?? { output: 0, turns: 0 };
    acc.agents[agentId] = { output: slot.output + agent.output, turns: slot.turns + agent.turns };
  }
}

export type LedgerSummary = {
  today: LedgerAggregate;
  last7: LedgerAggregate;
  last30: LedgerAggregate;
};

/** 纯函数:按本地日聚合出今天 / 近 7 天 / 近 30 天三个窗口。 */
export function summarizeLedger(target: UsageLedger, now: Date = new Date()): LedgerSummary {
  const todayKey = dayKeyOf(now);
  const d7 = new Date(now);
  d7.setDate(d7.getDate() - 6);
  const d30 = new Date(now);
  d30.setDate(d30.getDate() - 29);
  const weekKey = dayKeyOf(d7);
  const monthKey = dayKeyOf(d30);

  const summary: LedgerSummary = { today: emptyAggregate(), last7: emptyAggregate(), last30: emptyAggregate() };
  for (const [key, day] of Object.entries(target.days)) {
    if (key === todayKey) addToAggregate(summary.today, day);
    if (key >= weekKey) addToAggregate(summary.last7, day);
    if (key >= monthKey) addToAggregate(summary.last30, day);
  }
  return summary;
}

/** 近 7 天按输出 token 降序的助手清单(UI 的「按助手」明细)。 */
export function topAgents7(summary: LedgerSummary, limit = 5): Array<{ agentId: string; output: number; turns: number }> {
  return Object.entries(summary.last7.agents)
    .map(([agentId, a]) => ({ agentId, output: a.output, turns: a.turns }))
    .filter((row) => row.turns > 0)
    .sort((a, b) => b.output - a.output)
    .slice(0, limit);
}
