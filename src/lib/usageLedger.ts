/**
 * usageLedger — 跨会话 token 用量台账。
 *
 * 每轮 turn/complete 时把 turnStats 增量记进按「本地日」聚合的台账
 * (turnStats 已盖在 assistant_message 上随转录持久化,这里另记一份聚合,
 * 供 Token 用量分析伪标签页跨会话展示)。
 * O(1) 写入、跨重启,绝不为了聚合去重载转录(内存压力红线)。
 * 保留 180 个自然日(90 天趋势与作息热图的窗口上限),写入时修剪。
 *
 * 存放位置(usageStore.ts):桌面版在 `~/.marionette/usage/`(磁盘,乐观 rev 写),
 * 浏览器预览退 localStorage,Node 脚本退内存。启动时把 localStorage 里的旧账
 * 迁到磁盘,只在迁移成功后删旧键。写失败会记日志并在下一个事件重试
 * (旧版是悄悄关掉记录,丢到上一次成功写为止的全部账)。
 *
 * 记账挂在 turn/complete 处理器里,与事件盖戳共用同一条单窗口不变量
 * (同一轮只被一个窗口处理,否则转录本身就会重复)。**多窗口合并**:
 * 落账前重新读盘、把自己的这笔并进去再写回(读-合并-写);别的窗口保存后
 * 会广播事件,本窗收到后重读并刷新 UI。乐观 rev 撞车时 reload-merge-retry。
 */

import { useSyncExternalStore } from "react";
import { saveTextFile } from "./api";
import type { TurnStats } from "./types";
import type { Tally, TimingSplit, TurnDigest } from "./usageDigest";
import {
  clearLegacyLocalLedger,
  pickBackend,
  readLegacyLocalLedger,
  type LedgerBackend,
} from "./usageStore";
import { appendDebugLog } from "./api";

export type LedgerAgentUsage = { output: number; turns: number };

/** 每模型一行的四桶拆分(牌价折算按桶计价,模型间费率不同)。 */
export type LedgerModelUsage = {
  input: number;
  output: number;
  cached: number;
  cacheCreation: number;
  turns: number;
  /** v2:最近一次落账时的 agent(按 agent 分组的模型表用)。 */
  agent?: string;
  /** v3:DeepSeek 高峰时段(北京时间工作日 9–12/14–18)的桶(只在没有自报费用的轮次上记)。 */
  peak?: { input: number; output: number; cached: number; cacheCreation: number };
  /** v3:agent 自报费用的那部分轮次的桶(费用金额在 repUsd)。 */
  rep?: { input: number; output: number; cached: number; cacheCreation: number; turns: number };
  /** v3:agent 自报费用合计(USD;如 Grok turn_completed.usage.costUsdTicks,仅增量部分)。 */
  repUsd?: number;
};

/** v3(2026-11-09):一天里某个会话的用量(按天记,对话列表不再把整段历史算进短时间窗)。 */
export type SessionDayUsage = {
  input: number;
  output: number;
  cached: number;
  cacheCreation: number;
  turns: number;
  /** models<modelId, turns> 当天该会话每个模型的轮次(按各自牌价算费用)。 */
  models: Record<string, number>;
  last: number;
  /** 当天 agent 自报费用(USD);列表按时间窗取数时费用也随窗走。 */
  repUsd?: number;
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
  /**
   * v2(2026-11-08)按 agent 的完整明细:时间去向、首字/速度直方图、上下文占用分布、
   * 工具/命令/技能计数、改动行数、可见输入估算。旧账没有这个键,读出来就是空表。
   */
  detail?: Record<string, AgentDayDetail>;
  /** 当天有落账的会话 id(去重,上限 SESSION_IDS_CAP)。 */
  sessionIds?: string[];
  /** v3:当天各会话的用量(key = session id;分析页的对话列表按时间窗取这里)。 */
  sessions?: Record<string, SessionDayUsage>;
};

/** 一个 agent 一天的完整明细(Token 用量页 v2 的原料)。 */
export type AgentDayDetail = {
  input: number;
  output: number;
  cached: number;
  cacheCreation: number;
  reasoning: number;
  turns: number;
  /** 有 sentAt 的轮次的墙钟时长之和(send → turn end)。 */
  wallMs: number;
  timing: TimingSplit;
  /** 首字延迟直方图(TTFT_EDGES_MS 分桶)。 */
  ttftHist: number[];
  /** 输出速度直方图(TPS_EDGES 分桶)。 */
  tpsHist: number[];
  /** 轮末上下文占用直方图(10 桶,0–10% … 90–100%)。 */
  ctxHist: number[];
  tools: Record<string, Tally>;
  cmds: Record<string, Tally>;
  skills: Record<string, Tally>;
  /** 工具文本口径的改动行数(v3 起仅作估算展示;账面真数在 gitEdits)。 */
  edits: { ops: number; add: number; del: number };
  est: { user: number; attach: number; toolIo: number };
  /** v3:git 对账出的改动行数(轮末交付卡同一来源,只记被本轮工具碰过的文件)。 */
  gitEdits?: { ops: number; add: number; del: number };
  /** v3:压缩事件数(ACP compaction_update,completed 记 1)。 */
  compactions?: number;
  /** v3:失败轮数 / 中断轮数(区分工具失败:一个轮可以工具全成但轮失败)。 */
  turnFails?: number;
  turnCancels?: number;
  /** v3:文件活动,按文件聚合(读次数 / 编辑次数与行数,cap 见 PATHS_PER_DAY)。 */
  reads?: Record<string, number>;
  edited?: Record<string, { n: number; add: number; del: number }>;
};

/** 最近活跃的会话摘要(对话列表的原料;按 last 修剪到 SESSIONS_CAP)。 */
export type LedgerSession = {
  label: string;
  agentId: string;
  modelId: string | null;
  project: string | null;
  turns: number;
  input: number;
  output: number;
  cached: number;
  cacheCreation: number;
  /** 最近一轮结束时的上下文占用 %(没有实时读数就是 null,冻结值不算)。 */
  ctxPct: number | null;
  /** 最近一轮结束的时间戳(ms)。 */
  last: number;
  /** v3:按天用量的 key 列表(升序;对话列表按时间窗从这里取数)。 */
  dayKeys?: string[];
  /** v3:agent 自报费用合计(USD)。 */
  repUsd?: number;
};

export type UsageLedger = {
  version: 1;
  /** key = 本地日期 YYYY-MM-DD。 */
  days: Record<string, LedgerDay>;
  /** v2:会话摘要,key = session id。旧账没有这个键。 */
  sessions?: Record<string, LedgerSession>;
};

/** 首字延迟分桶上沿(ms);最后一桶开放。 */
export const TTFT_EDGES_MS = [250, 500, 750, 1000, 1500, 2000, 3000, 4000, 6000, 8000, 12000, 16000, 24000, 32000, 60000];
/** 输出速度分桶上沿(tok/s);最后一桶开放。 */
export const TPS_EDGES = [5, 10, 15, 20, 30, 40, 50, 60, 80, 100, 130, 160, 200, 260, 340];
const SESSION_IDS_CAP = 400;
const SESSIONS_CAP = 300;
const TALLY_CAP = 60;
/** 一天里最多记多少个不同文件(超出丢弃最不活跃的,防 runaway 扫盘)。 */
const PATHS_PER_DAY = 240;

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
    detail: normalizeDetailMap(d.detail),
    sessionIds: Array.isArray(d.sessionIds) ? d.sessionIds.filter((v) => typeof v === "string") : [],
    sessions: normalizeSessionDayMap(d.sessions),
  };
}

function numArray(raw: unknown, length: number): number[] {
  const out = new Array(length).fill(0);
  if (Array.isArray(raw)) for (let i = 0; i < length; i++) out[i] = Number(raw[i]) || 0;
  return out;
}

function tallyMap(raw: unknown): Record<string, Tally> {
  const out: Record<string, Tally> = {};
  if (typeof raw !== "object" || raw == null) return out;
  for (const [k, v] of Object.entries(raw as Record<string, Partial<Tally>>)) {
    if (!v || typeof v !== "object") continue;
    out[k] = { n: Number(v.n) || 0, fail: Number(v.fail) || 0, ms: Number(v.ms) || 0 };
  }
  return out;
}

function editedMap(raw: unknown): Record<string, { n: number; add: number; del: number }> {
  const out: Record<string, { n: number; add: number; del: number }> = {};
  if (typeof raw !== "object" || raw == null) return out;
  for (const [k, v] of Object.entries(raw as Record<string, Partial<{ n: number; add: number; del: number }>>)) {
    if (!v || typeof v !== "object") continue;
    out[k] = { n: Number(v.n) || 0, add: Number(v.add) || 0, del: Number(v.del) || 0 };
  }
  return out;
}

/** 有限值或 0(计数/金额聚合用,负数与 NaN 不进账)。 */
function countOf(v: unknown): number {
  return typeof v === "number" && Number.isFinite(v) && v > 0 ? v : 0;
}

export function emptyDetail(): AgentDayDetail {
  return {
    input: 0,
    output: 0,
    cached: 0,
    cacheCreation: 0,
    reasoning: 0,
    turns: 0,
    wallMs: 0,
    timing: { ttft: 0, think: 0, answer: 0, tool: 0, other: 0 },
    ttftHist: new Array(TTFT_EDGES_MS.length + 1).fill(0),
    tpsHist: new Array(TPS_EDGES.length + 1).fill(0),
    ctxHist: new Array(10).fill(0),
    tools: {},
    cmds: {},
    skills: {},
    edits: { ops: 0, add: 0, del: 0 },
    est: { user: 0, attach: 0, toolIo: 0 },
  };
}

function normalizeDetail(raw: unknown): AgentDayDetail {
  const d = (typeof raw === "object" && raw != null ? raw : {}) as Partial<AgentDayDetail>;
  const base = emptyDetail();
  const t = (d.timing ?? {}) as Partial<TimingSplit>;
  const e = (d.edits ?? {}) as Partial<AgentDayDetail["edits"]>;
  const s = (d.est ?? {}) as Partial<AgentDayDetail["est"]>;
  const g = (d.gitEdits ?? {}) as Partial<NonNullable<AgentDayDetail["gitEdits"]>>;
  return {
    input: Number(d.input) || 0,
    output: Number(d.output) || 0,
    cached: Number(d.cached) || 0,
    cacheCreation: Number(d.cacheCreation) || 0,
    reasoning: Number(d.reasoning) || 0,
    turns: Number(d.turns) || 0,
    wallMs: Number(d.wallMs) || 0,
    timing: {
      ttft: Number(t.ttft) || 0,
      think: Number(t.think) || 0,
      answer: Number(t.answer) || 0,
      tool: Number(t.tool) || 0,
      other: Number(t.other) || 0,
    },
    ttftHist: numArray(d.ttftHist, base.ttftHist.length),
    tpsHist: numArray(d.tpsHist, base.tpsHist.length),
    ctxHist: numArray(d.ctxHist, 10),
    tools: tallyMap(d.tools),
    cmds: tallyMap(d.cmds),
    skills: tallyMap(d.skills),
    edits: { ops: Number(e.ops) || 0, add: Number(e.add) || 0, del: Number(e.del) || 0 },
    est: { user: Number(s.user) || 0, attach: Number(s.attach) || 0, toolIo: Number(s.toolIo) || 0 },
    gitEdits: { ops: countOf(g.ops), add: countOf(g.add), del: countOf(g.del) },
    compactions: countOf(d.compactions),
    turnFails: countOf(d.turnFails),
    turnCancels: countOf(d.turnCancels),
    reads: editedCounts(d.reads),
    edited: editedMap(d.edited),
  };
}

function editedCounts(raw: unknown): Record<string, number> {
  const out: Record<string, number> = {};
  if (typeof raw !== "object" || raw == null) return out;
  for (const [k, v] of Object.entries(raw as Record<string, unknown>)) {
    if (typeof v === "number" && Number.isFinite(v) && v > 0) out[k] = v;
  }
  return out;
}

function normalizeDetailMap(raw: unknown): Record<string, AgentDayDetail> {
  const out: Record<string, AgentDayDetail> = {};
  if (typeof raw !== "object" || raw == null) return out;
  for (const [k, v] of Object.entries(raw as Record<string, unknown>)) out[k] = normalizeDetail(v);
  return out;
}

function normalizeSessionDayMap(raw: unknown): Record<string, SessionDayUsage> {
  const out: Record<string, SessionDayUsage> = {};
  if (typeof raw !== "object" || raw == null) return out;
  for (const [k, v] of Object.entries(raw as Record<string, Partial<SessionDayUsage>>)) {
    if (!v || typeof v !== "object") continue;
    out[k] = {
      input: Number(v.input) || 0,
      output: Number(v.output) || 0,
      cached: Number(v.cached) || 0,
      cacheCreation: Number(v.cacheCreation) || 0,
      turns: Number(v.turns) || 0,
      models: editedCounts(v.models),
      last: Number(v.last) || 0,
      repUsd: typeof v.repUsd === "number" && Number.isFinite(v.repUsd) && v.repUsd > 0 ? v.repUsd : undefined,
    };
  }
  return out;
}

function normalizeSessions(raw: unknown): Record<string, LedgerSession> {
  const out: Record<string, LedgerSession> = {};
  if (typeof raw !== "object" || raw == null) return out;
  for (const [id, v] of Object.entries(raw as Record<string, Partial<LedgerSession>>)) {
    if (!v || typeof v !== "object") continue;
    out[id] = {
      label: typeof v.label === "string" ? v.label : id,
      agentId: typeof v.agentId === "string" ? v.agentId : "unknown",
      modelId: typeof v.modelId === "string" ? v.modelId : null,
      project: typeof v.project === "string" ? v.project : null,
      turns: Number(v.turns) || 0,
      input: Number(v.input) || 0,
      output: Number(v.output) || 0,
      cached: Number(v.cached) || 0,
      cacheCreation: Number(v.cacheCreation) || 0,
      ctxPct: typeof v.ctxPct === "number" && Number.isFinite(v.ctxPct) ? v.ctxPct : null,
      last: Number(v.last) || 0,
      dayKeys: Array.isArray(v.dayKeys) ? v.dayKeys.filter((k) => typeof k === "string") : [],
      repUsd: typeof v.repUsd === "number" && Number.isFinite(v.repUsd) && v.repUsd > 0 ? v.repUsd : undefined,
    };
  }
  return out;
}

/** Parse the whole ledger JSON (disk / localStorage / tests); null on garbage. */
export function parseLedgerText(text: string | null | undefined): UsageLedger | null {
  if (!text) return null;
  try {
    const parsed = JSON.parse(text) as UsageLedger;
    if (parsed?.version !== 1 || typeof parsed.days !== "object" || !parsed.days) {
      return null;
    }
    const days: Record<string, LedgerDay> = {};
    for (const [key, value] of Object.entries(parsed.days)) {
      const day = normalizeDay(value);
      if (day) days[key] = day;
    }
    return { version: 1, days, sessions: normalizeSessions(parsed.sessions) };
  } catch {
    return null;
  }
}

/** Index of the bucket a value falls into (edges are upper bounds; the last bucket is open). */
export function histBucket(value: number, edges: readonly number[]): number {
  for (let i = 0; i < edges.length; i++) if (value < edges[i]!) return i;
  return edges.length;
}

function addTallies(into: Record<string, Tally>, from: Record<string, Tally>): Record<string, Tally> {
  const out = Record_add(into, from);
  // A runaway agent inventing tool names must not grow the ledger without bound.
  const keys = Object.keys(out);
  if (keys.length > TALLY_CAP) {
    keys.sort((a, b) => out[b]!.n - out[a]!.n);
    for (const k of keys.slice(TALLY_CAP)) delete out[k];
  }
  return out;
}

function Record_add(into: Record<string, Tally>, from: Record<string, Tally>): Record<string, Tally> {
  const out = { ...into };
  for (const [k, v] of Object.entries(from)) {
    const cur = out[k] ?? { n: 0, fail: 0, ms: 0 };
    out[k] = { n: cur.n + v.n, fail: cur.fail + v.fail, ms: cur.ms + v.ms };
  }
  return out;
}

function addCounts(into: Record<string, number>, from: Record<string, number>): Record<string, number> {
  if (Object.keys(from).length === 0) return into;
  const out = { ...into };
  for (const [k, v] of Object.entries(from)) {
    if (v <= 0) continue;
    if (out[k] != null || Object.keys(out).length < PATHS_PER_DAY) out[k] = (out[k] ?? 0) + v;
  }
  const keys = Object.keys(out);
  if (keys.length > PATHS_PER_DAY) {
    keys.sort((a, b) => out[b]! - out[a]!);
    for (const k of keys.slice(PATHS_PER_DAY)) delete out[k];
  }
  return out;
}

function addEdited(
  into: Record<string, { n: number; add: number; del: number }>,
  from: Record<string, { n: number; add: number; del: number }>,
): Record<string, { n: number; add: number; del: number }> {
  if (Object.keys(from).length === 0) return into;
  const out = { ...into };
  for (const [k, v] of Object.entries(from)) {
    if (v.n <= 0) continue;
    if (out[k] != null || Object.keys(out).length < PATHS_PER_DAY) {
      const cur = out[k] ?? { n: 0, add: 0, del: 0 };
      out[k] = { n: cur.n + v.n, add: cur.add + v.add, del: cur.del + v.del };
    }
  }
  const keys = Object.keys(out);
  if (keys.length > PATHS_PER_DAY) {
    keys.sort((a, b) => out[b]!.n - out[a]!.n);
    for (const k of keys.slice(PATHS_PER_DAY)) delete out[k];
  }
  return out;
}

// ── 磁盘/localStorage 后端(usageStore.ts)上的 store ─────────────────

const backend: LedgerBackend = pickBackend();
let loadedOnce = false;
let loadedValue: UsageLedger = { version: 1, days: {} };
let rev = 0;
let saveTimer: ReturnType<typeof setTimeout> | null = null;
let saveInFlight = false;
let dirtyJson: string | null = null;
const listeners = new Set<() => void>();

function emit() {
  for (const listener of listeners) listener();
}

function logStorageIssue(summary: string, detail: string) {
  void appendDebugLog({ source: "usage", level: "warn", summary, detail }).catch(() => {});
}

/**
 * Load once at module init (async). Until it resolves the page shows an empty
 * ledger — the same one-frame blank every other persisted store already has.
 */
async function initialLoad(): Promise<void> {
  if (loadedOnce || !HAS_WINDOW) return;
  loadedOnce = true;
  try {
    const stored = await backend.load();
    rev = stored.rev;
    let parsed = parseLedgerText(stored.json);
    // Desktop: a ledger still in localStorage predates the disk move. Migrate
    // it (disk wins on conflict — it is the newer backend's truth), and only
    // drop the legacy key after the merged ledger is on disk.
    const legacyText = backend.kind === "disk" ? readLegacyLocalLedger() : null;
    if (legacyText) {
      const legacy = parseLedgerText(legacyText);
      if (legacy && (!parsed || backend.kind !== "disk" || stored.json == null)) {
        parsed = mergeLedgers(legacy, parsed);
      } else if (legacy) {
        parsed = mergeLedgers(parsed!, legacy);
      }
    }
    loadedValue = parsed ?? { version: 1, days: {} };
    if (legacyText && backend.kind === "disk") {
      await flushNow().catch(() => {});
      clearLegacyLocalLedger();
    }
  } catch (error) {
    logStorageIssue(
      "usage ledger load failed",
      error instanceof Error ? error.message : String(error),
    );
    loadedValue = { version: 1, days: {} };
  }
  emit();
}

/** mergeLedgers: union of days (per-day fields summed) and sessions. */
function mergeLedgers(a: UsageLedger, b: UsageLedger | null): UsageLedger {
  if (!b) return a;
  const days = { ...b.days };
  for (const [key, dayA] of Object.entries(a.days)) {
    const dayB = days[key];
    days[key] = dayB ? addDays(dayA, dayB) : dayA;
  }
  return { version: 1, days, sessions: { ...(b.sessions ?? {}), ...(a.sessions ?? {}) } };
}

function addDaySum<K extends keyof LedgerDay>(a: LedgerDay, b: LedgerDay, key: K): void {
  (a[key] as number) = ((a[key] as number) ?? 0) + ((b[key] as number) ?? 0);
}

function addDays(a: LedgerDay, b: LedgerDay): LedgerDay {
  const out: LedgerDay = { ...a };
  for (const key of ["input", "output", "cached", "cacheCreation", "reasoning", "turns", "ms"] as const) {
    addDaySum(out, b, key);
  }
  out.hours = a.hours.map((v, i) => v + (b.hours[i] ?? 0));
  out.agents = { ...a.agents };
  for (const [id, agent] of Object.entries(b.agents)) {
    const cur = out.agents[id] ?? { output: 0, turns: 0 };
    out.agents[id] = { output: cur.output + agent.output, turns: cur.turns + agent.turns };
  }
  out.models = { ...a.models };
  for (const [id, model] of Object.entries(b.models)) {
    const cur = out.models[id] ?? { input: 0, output: 0, cached: 0, cacheCreation: 0, turns: 0 };
    out.models[id] = {
      input: cur.input + model.input,
      output: cur.output + model.output,
      cached: cur.cached + model.cached,
      cacheCreation: cur.cacheCreation + model.cacheCreation,
      turns: cur.turns + model.turns,
      peak: addPeak(cur.peak, model.peak),
      rep: model.rep ?? cur.rep,
      repUsd: (cur.repUsd ?? 0) + (model.repUsd ?? 0),
    };
  }
  if (a.detail || b.detail) {
    out.detail = { ...(a.detail ?? {}) };
    for (const [id, detailB] of Object.entries(b.detail ?? {})) {
      const cur = out.detail[id] ?? emptyDetail();
      out.detail[id] = addDetails(cur, detailB);
    }
  }
  out.sessionIds = [...new Set([...(a.sessionIds ?? []), ...(b.sessionIds ?? [])])].slice(0, SESSION_IDS_CAP);
  if (a.sessions || b.sessions) {
    out.sessions = { ...(a.sessions ?? {}) };
    for (const [id, dayUsage] of Object.entries(b.sessions ?? {})) {
      const cur = out.sessions[id];
      out.sessions[id] = cur
        ? {
            input: cur.input + dayUsage.input,
            output: cur.output + dayUsage.output,
            cached: cur.cached + dayUsage.cached,
            cacheCreation: cur.cacheCreation + dayUsage.cacheCreation,
            turns: cur.turns + dayUsage.turns,
            models: addCounts({ ...cur.models }, dayUsage.models),
            last: Math.max(cur.last, dayUsage.last),
          }
        : dayUsage;
    }
  }
  return out;
}

function addPeak(
  a: { input: number; output: number; cached: number; cacheCreation: number } | undefined,
  b: { input: number; output: number; cached: number; cacheCreation: number } | undefined,
) {
  if (!a) return b;
  if (!b) return a;
  return { input: a.input + b.input, output: a.output + b.output, cached: a.cached + b.cached, cacheCreation: a.cacheCreation + b.cacheCreation };
}

function addDetails(a: AgentDayDetail, b: AgentDayDetail): AgentDayDetail {
  return {
    input: a.input + b.input,
    output: a.output + b.output,
    cached: a.cached + b.cached,
    cacheCreation: a.cacheCreation + b.cacheCreation,
    reasoning: a.reasoning + b.reasoning,
    turns: a.turns + b.turns,
    wallMs: a.wallMs + b.wallMs,
    timing: {
      ttft: a.timing.ttft + b.timing.ttft,
      think: a.timing.think + b.timing.think,
      answer: a.timing.answer + b.timing.answer,
      tool: a.timing.tool + b.timing.tool,
      other: a.timing.other + b.timing.other,
    },
    ttftHist: a.ttftHist.map((v, i) => v + (b.ttftHist[i] ?? 0)),
    tpsHist: a.tpsHist.map((v, i) => v + (b.tpsHist[i] ?? 0)),
    ctxHist: a.ctxHist.map((v, i) => v + (b.ctxHist[i] ?? 0)),
    tools: addTallies(a.tools, b.tools),
    cmds: addTallies(a.cmds, b.cmds),
    skills: addTallies(a.skills, b.skills),
    edits: {
      ops: a.edits.ops + b.edits.ops,
      add: a.edits.add + b.edits.add,
      del: a.edits.del + b.edits.del,
    },
    est: {
      user: a.est.user + b.est.user,
      attach: a.est.attach + b.est.attach,
      toolIo: a.est.toolIo + b.est.toolIo,
    },
    gitEdits: {
      ops: (a.gitEdits?.ops ?? 0) + (b.gitEdits?.ops ?? 0),
      add: (a.gitEdits?.add ?? 0) + (b.gitEdits?.add ?? 0),
      del: (a.gitEdits?.del ?? 0) + (b.gitEdits?.del ?? 0),
    },
    compactions: (a.compactions ?? 0) + (b.compactions ?? 0),
    turnFails: (a.turnFails ?? 0) + (b.turnFails ?? 0),
    turnCancels: (a.turnCancels ?? 0) + (b.turnCancels ?? 0),
    reads: addCounts({ ...(a.reads ?? {}) }, b.reads ?? {}),
    edited: addEdited({ ...(a.edited ?? {}) }, b.edited ?? {}),
  };
}

if (HAS_WINDOW) {
  void initialLoad();
  backend.subscribe(() => {
    // Some window saved; re-read so this one stays current.
    void (async () => {
      try {
        const stored = await backend.load();
        rev = stored.rev;
        const parsed = parseLedgerText(stored.json);
        if (parsed) {
          loadedValue = parsed;
          emit();
        }
      } catch {
        /* keep showing what we have */
      }
    })();
  });
}

function schedulePersist() {
  if (saveTimer != null) return;
  saveTimer = setTimeout(() => {
    saveTimer = null;
    void flushNow();
  }, 450);
}

/** Serialize + save (rev-checked); on a lost race: reload, merge, retry once. */
async function flushNow(): Promise<void> {
  if (saveInFlight || dirtyJson == null) return;
  const body = dirtyJson;
  dirtyJson = null;
  saveInFlight = true;
  try {
    let result = await backend.save(body, rev);
    if (!result.ok) {
      // Lost the rev race: pick up the other window's ledger, merge ours in, retry.
      try {
        const stored = await backend.load();
        rev = stored.rev;
        const theirs = parseLedgerText(stored.json);
        const mine = parseLedgerText(body);
        if (theirs && mine) {
          const merged = mergeLedgers(theirs, mine);
          const retry = await backend.save(JSON.stringify(merged), rev);
          if (!retry.ok) {
            rev = retry.rev;
            dirtyJson = JSON.stringify(merged);
            throw new Error("optimistic-rev retry failed");
          }
          loadedValue = merged;
          rev = retry.rev;
          return;
        }
      } catch (error) {
        dirtyJson = body;
        throw error;
      }
      dirtyJson = body;
      throw new Error("save rejected and merge path unavailable");
    }
    rev = result.rev;
  } catch (error) {
    // Log once per failing streak; retry on the next recorded turn.
    logStorageIssue(
      "usage ledger save failed (will retry on next turn)",
      error instanceof Error ? error.message : String(error),
    );
    schedulePersist();
  } finally {
    saveInFlight = false;
  }
}

export function subscribeUsageLedger(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function getUsageLedger(): UsageLedger {
  return loadedValue;
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
    detail: {},
    sessionIds: [],
    sessions: {},
  };
}

/** 一轮是否真的报了数字(全 null 的 turnStats 不记账)。 */
function hasTokenFigures(stats: TurnStats): boolean {
  return stats.input != null || stats.output != null || stats.total != null;
}

/** 纯函数:把一轮记进台账(不改原对象),返回新台账。 */
export function recordTurnUsageInto(
  target: UsageLedger,
  entry: TurnUsageEntry,
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
  const turnBuckets = {
    input: s.input ?? 0,
    output: s.output ?? 0,
    cached: s.cached ?? 0,
    cacheCreation: s.cacheCreation ?? 0,
  };
  if (modelId) {
    const model = next.models[modelId] ?? {
      input: 0,
      output: 0,
      cached: 0,
      cacheCreation: 0,
      turns: 0,
    };
    next.models[modelId] = {
      input: model.input + turnBuckets.input,
      output: model.output + turnBuckets.output,
      cached: model.cached + turnBuckets.cached,
      cacheCreation: model.cacheCreation + turnBuckets.cacheCreation,
      turns: model.turns + 1,
      agent: agentId,
      // Peak / reported split per model row (see TurnUsageEntry.reportedUsd).
      peak: entry.reportedUsd != null ? model.peak : addPeak(model.peak, entry.peak ?? undefined),
      rep:
        entry.reportedUsd != null
          ? {
              input: (model.rep?.input ?? 0) + turnBuckets.input,
              output: (model.rep?.output ?? 0) + turnBuckets.output,
              cached: (model.rep?.cached ?? 0) + turnBuckets.cached,
              cacheCreation: (model.rep?.cacheCreation ?? 0) + turnBuckets.cacheCreation,
              turns: (model.rep?.turns ?? 0) + 1,
            }
          : model.rep,
      repUsd: (model.repUsd ?? 0) + (entry.reportedUsd ?? 0),
    };
  }
  next.hours[now.getHours()] = (next.hours[now.getHours()] ?? 0) + 1;

  // ── v2 per-agent detail ──
  const detailMap = { ...(day.detail ?? {}) };
  const prev = detailMap[agentId] ?? emptyDetail();
  const dg = entry.digest ?? null;
  const ctx = entry.ctx;
  // Only a reading that changed during THIS turn counts as a context sample —
  // a stale frozen value (agents that report once) must not pile into the chart.
  const ctxPct =
    entry.ctxFresh && ctx && ctx.used != null && ctx.size != null && ctx.size > 0
      ? Math.min(100, Math.max(0, (ctx.used / ctx.size) * 100))
      : null;
  const ttftHist = [...prev.ttftHist];
  if (s.ttftMs != null && s.ttftMs >= 0) ttftHist[histBucket(s.ttftMs, TTFT_EDGES_MS)]! += 1;
  const tpsHist = [...prev.tpsHist];
  if (s.outputTps != null && s.outputTps > 0) tpsHist[histBucket(s.outputTps, TPS_EDGES)]! += 1;
  const ctxHist = [...prev.ctxHist];
  if (ctxPct != null) ctxHist[Math.min(9, Math.floor(ctxPct / 10))]! += 1;
  const wall = entry.sentAt != null ? Math.max(0, entry.endedAt - entry.sentAt) : 0;
  detailMap[agentId] = {
    input: prev.input + (s.input ?? 0),
    output: prev.output + (s.output ?? 0),
    cached: prev.cached + (s.cached ?? 0),
    cacheCreation: prev.cacheCreation + (s.cacheCreation ?? 0),
    reasoning: prev.reasoning + (s.reasoning ?? 0),
    turns: prev.turns + 1,
    wallMs: prev.wallMs + wall,
    timing: dg
      ? {
          ttft: prev.timing.ttft + dg.timing.ttft,
          think: prev.timing.think + dg.timing.think,
          answer: prev.timing.answer + dg.timing.answer,
          tool: prev.timing.tool + dg.timing.tool,
          other: prev.timing.other + dg.timing.other,
        }
      : prev.timing,
    ttftHist,
    tpsHist,
    ctxHist,
    tools: dg ? addTallies(prev.tools, dg.tools) : prev.tools,
    cmds: dg ? addTallies(prev.cmds, dg.cmds) : prev.cmds,
    skills: dg ? addTallies(prev.skills, dg.skills) : prev.skills,
    edits: dg
      ? { ops: prev.edits.ops + dg.edits.ops, add: prev.edits.add + dg.edits.add, del: prev.edits.del + dg.edits.del }
      : prev.edits,
    est: dg
      ? { user: prev.est.user + dg.est.user, attach: prev.est.attach + dg.est.attach, toolIo: prev.est.toolIo + dg.est.toolIo }
      : prev.est,
    gitEdits: entry.gitEdits
      ? {
          ops: (prev.gitEdits?.ops ?? 0) + entry.gitEdits.ops,
          add: (prev.gitEdits?.add ?? 0) + entry.gitEdits.add,
          del: (prev.gitEdits?.del ?? 0) + entry.gitEdits.del,
        }
      : prev.gitEdits,
    compactions: (prev.compactions ?? 0) + (entry.compactions ?? 0),
    turnFails: (prev.turnFails ?? 0) + (entry.outcome === "failed" ? 1 : 0),
    turnCancels: (prev.turnCancels ?? 0) + (entry.outcome === "cancelled" ? 1 : 0),
    reads: dg ? addCounts({ ...(prev.reads ?? {}) }, dg.reads) : prev.reads ?? {},
    edited: dg ? addEdited({ ...(prev.edited ?? {}) }, dg.edited) : prev.edited ?? {},
  };
  next.detail = detailMap;

  const sid = entry.session?.id;
  const ids = day.sessionIds ?? [];
  next.sessionIds = sid && !ids.includes(sid) && ids.length < SESSION_IDS_CAP ? [...ids, sid] : ids;

  // v3: per-session per-day usage (the sessions list shows range-scoped numbers).
  let sessionsMap = target.sessions;
  if (entry.session) {
    if (sid) {
      const cur = day.sessions?.[sid];
      const modelTurns = { ...(cur?.models ?? {}) };
      if (modelId) modelTurns[modelId] = (modelTurns[modelId] ?? 0) + 1;
      const dayUsage: SessionDayUsage = {
        input: (cur?.input ?? 0) + turnBuckets.input,
        output: (cur?.output ?? 0) + turnBuckets.output,
        cached: (cur?.cached ?? 0) + turnBuckets.cached,
        cacheCreation: (cur?.cacheCreation ?? 0) + turnBuckets.cacheCreation,
        turns: (cur?.turns ?? 0) + 1,
        models: modelTurns,
        last: entry.endedAt,
        repUsd: (cur?.repUsd ?? 0) + (entry.reportedUsd ?? 0) || undefined,
      };
      next.sessions = { ...(day.sessions ?? {}), [sid]: dayUsage };
    }
    const prevSummary = target.sessions?.[entry.session.id];
    const summary: LedgerSession = {
      label: entry.session.label || prevSummary?.label || entry.session.id,
      agentId,
      modelId: modelId ?? prevSummary?.modelId ?? null,
      project: entry.session.project ?? prevSummary?.project ?? null,
      turns: (prevSummary?.turns ?? 0) + 1,
      input: (prevSummary?.input ?? 0) + turnBuckets.input,
      output: (prevSummary?.output ?? 0) + turnBuckets.output,
      cached: (prevSummary?.cached ?? 0) + turnBuckets.cached,
      cacheCreation: (prevSummary?.cacheCreation ?? 0) + turnBuckets.cacheCreation,
      ctxPct: ctxPct ?? prevSummary?.ctxPct ?? null,
      last: entry.endedAt,
      dayKeys: [...new Set([...(prevSummary?.dayKeys ?? []), key])].sort().slice(-RETAIN_DAYS),
      repUsd: (prevSummary?.repUsd ?? 0) + (entry.reportedUsd ?? 0) || undefined,
    };
    sessionsMap = trimSessions({ ...(target.sessions ?? {}), [entry.session.id]: summary });
  }

  const days = { ...target.days, [key]: next };
  return { version: 1, days, ...(sessionsMap ? { sessions: sessionsMap } : {}) };
}

/** What App hands the recorder at turn/complete. */
export type TurnUsageEntry = {
  agentId?: string | null;
  modelId?: string | null;
  stats: TurnStats;
  endedAt: number;
  /** When the prompt went over the wire (wall-time anchor); null when unknown. */
  sentAt?: number | null;
  /** usageDigest.digestTurn over this turn's events; null → token figures only. */
  digest?: TurnDigest | null;
  /** Context occupancy at turn end, as the agent last reported it. */
  ctx?: { used: number | null; size: number | null } | null;
  /** True when the context reading was updated DURING this turn (live usage_update / Grok per-call); a frozen seeded value does not count. */
  ctxFresh?: boolean;
  session?: { id: string; label: string; project: string | null } | null;
  /** v3: this turn's stop reason (end_turn | cancelled | error | refusal …). */
  outcome?: "ended" | "cancelled" | "failed";
  /** v3: git-truth edits of this turn (turn delivery card's numbers). */
  gitEdits?: { ops: number; add: number; del: number } | null;
  /** v3: compaction_update completions observed during this turn. */
  compactions?: number;
  /**
   * v3: agent-reported cost for THIS turn in USD (Grok's per-turn costUsdTicks
   * delta), and the DeepSeek-peak-hour subset of this turn's buckets. Reported
   * cost wins over list price for its tokens; peak only applies to turns whose
   * cost was not reported.
   */
  reportedUsd?: number | null;
  peak?: { input: number; output: number; cached: number; cacheCreation: number } | null;
};

/** Keep the SESSIONS_CAP most recently active sessions. */
function trimSessions(map: Record<string, LedgerSession>): Record<string, LedgerSession> {
  const ids = Object.keys(map);
  if (ids.length <= SESSIONS_CAP) return map;
  ids.sort((a, b) => map[b]!.last - map[a]!.last);
  return Object.fromEntries(ids.slice(0, SESSIONS_CAP).map((id) => [id, map[id]!]));
}

/** 修剪超期日(保留最近 RETAIN_DAYS 个自然日,含今天),返回新台账。 */
export function pruneLedger(target: UsageLedger, now: Date = new Date()): UsageLedger {
  const cutoff = new Date(now);
  cutoff.setDate(cutoff.getDate() - (RETAIN_DAYS - 1));
  const cutoffKey = dayKeyOf(cutoff);
  const days = Object.fromEntries(
    Object.entries(target.days).filter(([key]) => key >= cutoffKey),
  );
  if (!target.sessions) return { version: 1, days };
  const cutoffMs = cutoff.setHours(0, 0, 0, 0);
  const sessions = Object.fromEntries(
    Object.entries(target.sessions).filter(([, s]) => s.last >= cutoffMs),
  );
  return { version: 1, days, sessions };
}

/** 落账入口:App 的 turn/complete 处理器调用。 */
export function recordTurnUsage(entry: TurnUsageEntry): void {
  // 读-合并-写:先拾起其它窗口可能刚写的账,再并进这一笔(detached 窗并存)。
  loadedValue = pruneLedger(recordTurnUsageInto(loadedValue, entry));
  dirtyJson = JSON.stringify(loadedValue);
  if (!loadedOnce && HAS_WINDOW) {
    void initialLoad().then(() => {
      loadedValue = pruneLedger(recordTurnUsageInto(loadedValue, entry));
      dirtyJson = JSON.stringify(loadedValue);
      schedulePersist();
      emit();
    });
    return;
  }
  schedulePersist();
  emit();
}

/** 一轮的会话行(usage_store.rs 的 sessions/<id>.jsonl 追加一行;纯数据)。 */
export function turnSessionRow(entry: TurnUsageEntry): string {
  const s = entry.stats;
  const ctx = entry.ctxFresh && entry.ctx ? { used: entry.ctx.used, size: entry.ctx.size } : null;
  const row = {
    t: entry.endedAt,
    model: entry.modelId ?? null,
    agent: entry.agentId ?? null,
    in: s.input ?? null,
    out: s.output ?? null,
    cach: s.cached ?? null,
    cachw: s.cacheCreation ?? null,
    ctx,
    outcome: entry.outcome ?? "ended",
    comp: entry.compactions ?? 0,
    tools: entry.digest?.tools ?? null,
    cmds: entry.digest?.cmds ?? null,
    edits: entry.gitEdits ?? entry.digest?.edits ?? null,
  };
  return JSON.stringify(row);
}

/** 把一行追加进会话用量文件(磁盘后端;后台失败只记日志,不挡记账)。 */
export function appendTurnSessionRow(sessionId: string | null | undefined, entry: TurnUsageEntry): void {
  if (!sessionId) return;
  void backend.appendSession(sessionId, [turnSessionRow(entry)]).catch((error) => {
    logStorageIssue(
      "usage session row append failed",
      error instanceof Error ? error.message : String(error),
    );
  });
}

/**
 * 轮末 git 对账的改动行数回填(recordTurnUsage 之后异步到达,同一轮只调一次)。
 * 记在当天的 agent 明细上;找不到当天的账(极端:跨天竞态/无 session 摘要)就丢弃
 * ——工具文本口径的估算仍在,不比没有差。
 */
export function applyTurnGitEdits(
  sessionId: string,
  endedAt: number,
  git: { ops: number; add: number; del: number },
): void {
  if (git.ops <= 0) return;
  const key = dayKeyOf(new Date(endedAt));
  const agentId = loadedValue.sessions?.[sessionId]?.agentId;
  const detail = agentId ? loadedValue.days[key]?.detail?.[agentId] : undefined;
  if (!agentId || !detail) return;
  loadedValue = {
    ...loadedValue,
    days: {
      ...loadedValue.days,
      [key]: {
        ...loadedValue.days[key]!,
        detail: {
          ...(loadedValue.days[key]!.detail ?? {}),
          [agentId]: {
            ...detail,
            gitEdits: {
              ops: (detail.gitEdits?.ops ?? 0) + git.ops,
              add: (detail.gitEdits?.add ?? 0) + git.add,
              del: (detail.gitEdits?.del ?? 0) + git.del,
            },
          },
        },
      },
    },
  };
  dirtyJson = JSON.stringify(loadedValue);
  schedulePersist();
  emit();
}

/** 读取一个会话的轮次行(详情视图;空数组 = 没有记录)。 */
export async function readSessionRows(sessionId: string): Promise<string[]> {
  try {
    return await backend.readSession(sessionId);
  } catch (error) {
    logStorageIssue(
      "usage session rows read failed",
      error instanceof Error ? error.message : String(error),
    );
    return [];
  }
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

/** 小时×星期折叠(7×24 的 turns 计数)——活动节奏热图的数据源。0 行 = 周日。 */
export function foldHourWeek(
  target: UsageLedger,
  weeks: number,
  endAt: Date = new Date(),
): number[][] {
  return foldHourDays(target, weeks * 7, endAt);
}

/** 与 aggregateDays 同一自然日窗口的小时×星期折叠。0 行 = 周日。 */
export function foldHourDays(
  target: UsageLedger,
  days: number,
  endAt: Date = new Date(),
): number[][] {
  const grid = Array.from({ length: 7 }, () => new Array(24).fill(0));
  const from = new Date(endAt);
  from.setDate(from.getDate() - (days - 1));
  const fromKey = dayKeyOf(from);
  const endKey = dayKeyOf(endAt);
  for (const [key, day] of Object.entries(target.days)) {
    if (key < fromKey || key > endKey) continue;
    const weekday = new Date(`${key}T00:00:00`).getDay();
    for (let h = 0; h < 24; h += 1) grid[weekday]![h]! += day.hours[h] ?? 0;
  }
  return grid;
}

/** 把周日开头的 7×24 转成周一首行(热图从上到下一二三四五六日)。 */
export function mondayFirstHourGrid(grid: number[][]): number[][] {
  return [1, 2, 3, 4, 5, 6, 0].map((row) => grid[row] ?? new Array(24).fill(0));
}

/** 四桶合计。 */
export function totalTokens(agg: {
  input: number;
  output: number;
  cached: number;
  cacheCreation: number;
}): number {
  return agg.input + agg.output + agg.cached + agg.cacheCreation;
}

/**
 * 缓存命中率。分母是进模型的上下文(新鲜输入 + 缓存写 + 缓存读),
 * 故意排除输出——输出从未进过缓存。无上下文时返回 null。
 * 口径与 codeg `cacheHitRatio` 对齐。
 */
export function cacheHitRate(agg: {
  input: number;
  cached: number;
  cacheCreation: number;
}): number | null {
  const denom = agg.input + agg.cached + agg.cacheCreation;
  if (denom <= 0) return null;
  return agg.cached / denom;
}

/** 这段时间真正新算的 token(合计 − 缓存读)。 */
export function freshTokens(agg: {
  input: number;
  output: number;
  cached: number;
  cacheCreation: number;
}): number {
  return Math.max(0, totalTokens(agg) - agg.cached);
}

export type TrendPoint = {
  key: string;
  cache: number;
  fresh: number;
  turns: number;
};

/** 日序列按天或按周(周一首日)收成趋势柱。 */
export function trendSeries(daily: DailyPoint[], bucket: "day" | "week"): TrendPoint[] {
  const toPoint = (key: string, days: DailyPoint[]): TrendPoint => ({
    key,
    cache: days.reduce((sum, day) => sum + day.cached, 0),
    fresh: days.reduce((sum, day) => sum + day.input + day.output + day.cacheCreation, 0),
    turns: days.reduce((sum, day) => sum + day.turns, 0),
  });
  if (bucket === "day") return daily.map((day) => toPoint(day.key, [day]));
  const groups = new Map<string, DailyPoint[]>();
  const order: string[] = [];
  for (const day of daily) {
    const week = mondayOf(day.key);
    if (!groups.has(week)) {
      groups.set(week, []);
      order.push(week);
    }
    groups.get(week)!.push(day);
  }
  return order.map((week) => toPoint(week, groups.get(week)!));
}

function mondayOf(key: string): string {
  const date = new Date(`${key}T00:00:00`);
  date.setDate(date.getDate() - ((date.getDay() + 6) % 7));
  return dayKeyOf(date);
}

/** 助手排名:按输出降序,超过 limit 折叠进「其它」。 */
export function rankedAgents(
  agg: LedgerAggregate,
  limit = 7,
): Array<{ agentId: string; output: number; turns: number }> {
  const rows = Object.entries(agg.agents)
    .map(([agentId, agent]) => ({ agentId, output: agent.output, turns: agent.turns }))
    .filter((row) => row.turns > 0)
    .sort((a, b) => b.output - a.output || b.turns - a.turns);
  const head = rows.slice(0, limit);
  const tail = rows.slice(limit);
  if (tail.length > 0) {
    head.push({
      agentId: "other",
      output: tail.reduce((sum, row) => sum + row.output, 0),
      turns: tail.reduce((sum, row) => sum + row.turns, 0),
    });
  }
  return head;
}

/** 平方根分档(codeg 同口径):一次通宵不会把其余格子全压成最浅一档。0 不进 ramp。 */
export function hourRampStep(value: number, max: number, steps = 7): number | null {
  if (value <= 0 || max <= 0) return null;
  const step = Math.ceil(Math.sqrt(value / max) * steps);
  return Math.min(steps, Math.max(1, step)) - 1;
}

/** 7×24 网格里计数最高的小时(0–23);全 0 返回 null。 */
export function peakHourOf(grid: number[][]): number | null {
  let max = 0;
  let hour: number | null = null;
  for (const row of grid) {
    for (let h = 0; h < row.length; h += 1) {
      const value = row[h] ?? 0;
      if (value > max) {
        max = value;
        hour = h;
      }
    }
  }
  return hour;
}

/** 环比:当前 vs 上一周期,输出 token 的变化百分比(null = 上期为 0,无环比)。 */
export function deltaPercent(current: number, previous: number): number | null {
  if (previous <= 0) return null;
  return (current - previous) / previous;
}

// ── CSV 导出(用量分析页;opcode 式「可导出的用量分析」) ────────────────

const CSV_HEADER = "date,scope,model,turns,input,output,cached,cache_creation,reasoning,ms";

/** CSV 字段转义:含逗号/引号/换行的字段加引号并把引号翻倍。 */
function csvField(value: string | number): string {
  const text = String(value);
  // 正则里的引号一律 \x22——inventory-zh 的注释剥离器不识别正则字面量(见 AGENTS.md)。
  return /[\x22,\n\r]/.test(text) ? `"${text.replace(/\x22/g, '""')}"` : text;
}

/**
 * 台账 → CSV(long format,便于透视表):
 * 每个有记录的自然日先出一行 `scope=total`(日合计,含流式时长 ms),
 * 再按四桶总量降序出 `scope=model` 行(ms 不按模型记账,留空)。
 * 无任何记录返回空串。
 */
export function ledgerToCsv(target: UsageLedger): string {
  const dayKeys = Object.keys(target.days).filter((key) => (target.days[key]?.turns ?? 0) > 0).sort();
  if (dayKeys.length === 0) return "";
  const rows: string[] = [CSV_HEADER];
  for (const key of dayKeys) {
    const day = normalizeDay(target.days[key])!;
    rows.push(
      [
        key,
        "total",
        "",
        day.turns,
        day.input,
        day.output,
        day.cached,
        day.cacheCreation,
        day.reasoning,
        day.ms,
      ].join(","),
    );
    const models = Object.entries(day.models)
      .filter(([, m]) => m.turns > 0)
      .sort(
        (a, b) =>
          b[1].input + b[1].output + b[1].cached + b[1].cacheCreation -
          (a[1].input + a[1].output + a[1].cached + a[1].cacheCreation),
      );
    for (const [modelId, m] of models) {
      rows.push(
        [
          key,
          "model",
          csvField(modelId),
          m.turns,
          m.input,
          m.output,
          m.cached,
          m.cacheCreation,
          // reasoning 只在日合计粒度记账(LedgerModelUsage 无 reasoning 桶),留空。
          "",
          "",
        ].join(","),
      );
    }
  }
  return `${rows.join("\r\n")}\r\n`;
}

/** 导出文件名:`marionette-token-usage-YYYYMMDD.csv`。 */
export function usageCsvFileName(at: Date = new Date()): string {
  const pad = (n: number) => String(n).padStart(2, "0");
  return `marionette-token-usage-${at.getFullYear()}${pad(at.getMonth() + 1)}${pad(at.getDate())}.csv`;
}

/** 高层入口:序列化台账 → 原生「另存为」(非 Tauri 回落浏览器下载)。 */
export async function exportUsageCsv(): Promise<{
  fileName: string;
  csv: string;
  savedPath: string | null;
}> {
  const csv = ledgerToCsv(loadedValue);
  if (!csv) throw new Error("no usage records");
  const fileName = usageCsvFileName();
  const savedPath = await saveTextFile(fileName, csv);
  return { fileName, csv, savedPath };
}
