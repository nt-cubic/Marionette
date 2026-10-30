/**
 * Council Mode — 前端数据层(api 封装 + 类型 + 事件订阅 + 展示逻辑)。
 *
 * 类型与 Rust `council/types.rs` 的 camelCase serde 输出一一对应。
 * 纯展示函数(memberColor / phaseLabel / groupTimeline)与 IO 分离,
 * `scripts/verify-council.mts` 离线回归。
 */
import { invoke } from "@tauri-apps/api/core";
import { isTauriRuntime } from "./api";

// ---------------------------------------------------------------------------
// 类型(镜像 src-tauri/src/council/types.rs)
// ---------------------------------------------------------------------------

export type CouncilMessageKind =
  | "briefing"
  | "proposal"
  | "critique"
  | "rebuttal"
  | "revision"
  | "question"
  | "answer"
  | "synthesis"
  | "plan"
  | "task"
  | "status"
  | "evidence";

export interface CouncilMessage {
  messageId: string;
  runId: string;
  round: number;
  from: string;
  to: string;
  kind: CouncilMessageKind;
  replyTo?: string | null;
  content: string;
  timestamp: number;
  meta?: Record<string, unknown> | null;
}

export interface MemberSpec {
  id: string;
  displayName: string;
  model: string;
  role: string;
  isChair: boolean;
}

export type CouncilMode = "council_only" | "council_and_execution" | "autonomous";

export interface CouncilBudget {
  maxRequests?: number | null;
  maxTokens?: number | null;
  wallClockSecs?: number | null;
}

export interface CouncilSpec {
  runId: string;
  title: string;
  task: string;
  mode: CouncilMode;
  members: MemberSpec[];
  rounds: number;
  maxConcurrent: number;
  budget: CouncilBudget;
  createdAt: number;
  projectId?: string | null;
  cwd?: string | null;
}

export type RunPhase =
  | "created"
  | "analyzing"
  | "proposing"
  | "reviewing"
  | "revising"
  | "synthesizing"
  | "plan_ready"
  | "awaiting_approval"
  | "executing"
  | "verifying"
  | "completed"
  | "paused"
  | "cancelled"
  | "failed"
  | "budget_exhausted";

export type MemberRuntimeStatus =
  | "idle"
  | "working"
  | "done"
  | { failed: string };

export interface BudgetUsage {
  requests: number;
  tokensEst: number;
  startedAt: number;
}

export interface CouncilState {
  runId: string;
  phase: RunPhase;
  round: number;
  pausedFrom?: RunPhase | null;
  memberStatus: Record<string, MemberRuntimeStatus>;
  budgetUsage: BudgetUsage;
  updatedAt: number;
  error?: string | null;
}

export interface PlanStep {
  id: string;
  title: string;
  detail: string;
  dependsOn: string[];
  acceptance: string;
  ownerHint?: string | null;
}

export interface PlanDoc {
  summary: string;
  steps: PlanStep[];
  openQuestions: string[];
  acceptance: string[];
  verification: string[];
}

export interface TransitionRecord {
  at: number;
  from: RunPhase;
  to: RunPhase;
  reason?: string | null;
}

export interface RunSummary {
  runId: string;
  title: string;
  phase: RunPhase;
  mode: CouncilMode;
  memberCount: number;
  rounds: number;
  messageCount: number;
  createdAt: number;
  updatedAt: number;
}

export interface RunDetail {
  spec: CouncilSpec;
  state: CouncilState;
  messages: CouncilMessage[];
  transitions: TransitionRecord[];
  plan?: PlanDoc | null;
  live: boolean;
}

export interface CouncilModelInfo {
  id: string;
  contextWindow: number;
  maxOutput: number;
  images: boolean;
}

export interface CouncilModelsResult {
  ready: boolean;
  models: CouncilModelInfo[];
}

// ---------------------------------------------------------------------------
// api 封装(isTauriRuntime 模式,浏览器 mock 返回空)
// ---------------------------------------------------------------------------

export async function councilListRuns(): Promise<RunSummary[]> {
  if (!isTauriRuntime()) return [];
  try {
    return await invoke<RunSummary[]>("council_list_runs");
  } catch {
    return [];
  }
}

export async function councilGetRun(runId: string): Promise<RunDetail | null> {
  if (!isTauriRuntime()) return null;
  try {
    return await invoke<RunDetail>("council_get_run", { runId });
  } catch {
    return null;
  }
}

export async function councilCreate(spec: Omit<CouncilSpec, "runId" | "createdAt">): Promise<RunDetail | null> {
  if (!isTauriRuntime()) return null;
  const payload = { ...spec, runId: "", createdAt: 0 };
  return invoke<RunDetail>("council_create", { input: payload });
}

export async function councilCancelRun(runId: string): Promise<void> {
  if (!isTauriRuntime()) return;
  await invoke("council_cancel_run", { runId });
}

export async function councilPauseRun(runId: string): Promise<void> {
  if (!isTauriRuntime()) return;
  await invoke("council_pause_run", { runId });
}

export async function councilResumeRun(runId: string): Promise<void> {
  if (!isTauriRuntime()) return;
  await invoke("council_resume_run", { runId });
}

export async function councilApprovePlan(runId: string, accept: boolean): Promise<void> {
  if (!isTauriRuntime()) return;
  await invoke("council_approve_plan", { runId, accept });
}

export async function councilDeleteRun(runId: string): Promise<void> {
  if (!isTauriRuntime()) return;
  await invoke("council_delete_run", { runId });
}

export async function councilListModels(): Promise<CouncilModelsResult> {
  if (!isTauriRuntime()) return { ready: false, models: [] };
  try {
    return await invoke<CouncilModelsResult>("council_list_models");
  } catch {
    return { ready: false, models: [] };
  }
}

/** 后端全局事件名(payload:{runId,kind,data})。 */
export const COUNCIL_EVENT = "council-event";

export interface CouncilEvent {
  runId: string;
  kind: "message" | "phase" | "member" | "plan-ready" | "finished" | "warn" | "error" | "created";
  data: Record<string, unknown>;
}

// ---------------------------------------------------------------------------
// 展示逻辑(纯函数,离线可测)
// ---------------------------------------------------------------------------

const MEMBER_COLORS = [
  "var(--council-m1)",
  "var(--council-m2)",
  "var(--council-m3)",
  "var(--council-m4)",
  "var(--council-m5)",
  "var(--council-m6)",
  "var(--council-m7)",
  "var(--council-m8)",
];

/** 成员稳定配色:spec 成员序为准;未知 id(如 system)灰。 */
export function memberColor(members: MemberSpec[], memberId: string): string {
  if (memberId === "system" || memberId === "chair") return "var(--text-dim)";
  const idx = members.findIndex((m) => m.id === memberId);
  if (idx < 0) return "var(--text-dim)";
  return MEMBER_COLORS[idx % MEMBER_COLORS.length] ?? "var(--text-dim)";
}

export function memberLabel(members: MemberSpec[], memberId: string): string {
  if (memberId === "system") return "系统";
  const member = members.find((m) => m.id === memberId);
  return member ? member.displayName : memberId;
}

export function memberModelLabel(members: MemberSpec[], memberId: string): string {
  const member = members.find((m) => m.id === memberId);
  return member ? member.model : "";
}

const PHASE_LABELS: Record<RunPhase, string> = {
  created: "已创建",
  analyzing: "任务分析",
  proposing: "独立提案",
  reviewing: "交叉评审",
  revising: "修订回应",
  synthesizing: "主席综合",
  plan_ready: "计划待批",
  awaiting_approval: "等待确认",
  executing: "执行中",
  verifying: "验证中",
  completed: "已完成",
  paused: "已暂停",
  cancelled: "已取消",
  failed: "失败",
  budget_exhausted: "预算耗尽",
};

export function phaseLabel(phase: RunPhase): string {
  return PHASE_LABELS[phase] ?? phase;
}

export function isTerminalPhase(phase: RunPhase): boolean {
  return phase === "completed" || phase === "cancelled" || phase === "failed" || phase === "budget_exhausted";
}

const KIND_LABELS: Record<CouncilMessageKind, string> = {
  briefing: "简报",
  proposal: "提案",
  critique: "质询",
  rebuttal: "反驳",
  revision: "修订",
  question: "提问",
  answer: "回答",
  synthesis: "综合",
  plan: "计划",
  task: "任务",
  status: "状态",
  evidence: "证据",
};

export function kindLabel(kind: CouncilMessageKind): string {
  return KIND_LABELS[kind] ?? kind;
}

/** 讨论时间线分组:briefing 独立一组;之后按 (round, kind) 分组,
 *  组内按时间排序——同轮提案聚在一起、质询聚在一起,阅读动线对齐
 *  Phase A→B→C。 */
export interface TimelineGroup {
  key: string;
  round: number;
  kind: CouncilMessageKind;
  title: string;
  messages: CouncilMessage[];
}

export function groupTimeline(messages: CouncilMessage[]): TimelineGroup[] {
  const sorted = [...messages].sort((a, b) => a.timestamp - b.timestamp);
  const groups: TimelineGroup[] = [];
  for (const msg of sorted) {
    const key = msg.kind === "briefing" ? "briefing" : `r${msg.round}-${msg.kind}`;
    let group = groups.find((g) => g.key === key);
    if (!group) {
      const roundLabel = msg.kind === "briefing" ? "" : `第 ${msg.round} 轮 · `;
      group = { key, round: msg.round, kind: msg.kind, title: `${roundLabel}${kindLabel(msg.kind)}`, messages: [] };
      groups.push(group);
    }
    group.messages.push(msg);
  }
  return groups;
}

/** 成员运行状态点(state.memberStatus → 展示态)。 */
export function memberStatusText(status: MemberRuntimeStatus | undefined): string {
  if (!status) return "待命";
  if (status === "idle") return "待命";
  if (status === "working") return "工作中";
  if (status === "done") return "已完成";
  return `失败:${status.failed}`;
}

/** 从成员配置生成默认成员 id(displayName 派生,保证 kebab-case 唯一)。 */
export function deriveMemberId(displayName: string, existing: string[]): string {
  const base =
    displayName
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-+|-+$/g, "") || "member";
  let candidate = base;
  let n = 2;
  while (existing.includes(candidate)) {
    candidate = `${base}-${n}`;
    n += 1;
  }
  return candidate;
}

