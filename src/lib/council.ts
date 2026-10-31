/**
 * Council Mode — 前端数据层(api 封装 + 类型 + 事件订阅 + 展示逻辑)。
 *
 * 类型与 Rust `council/types.rs` 的 camelCase serde 输出一一对应。
 * 纯展示函数(memberColor / phaseLabel / groupTimeline)与 IO 分离,
 * `scripts/verify-council.mts` 离线回归。
 */
import { invoke } from "@tauri-apps/api/core";
import { isTauriRuntime } from "./api";
import { t, type DictKey } from "./i18n";

// ---------------------------------------------------------------------------
// 类型(镜像 src-tauri/src/council/types.rs)
// ---------------------------------------------------------------------------

export type CouncilMessageKind =
  | "game_action"
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
  /** M5 能力分层:lead=主导+背书他人;contributor=产出需 lead 背书。缺省 lead(旧 run 兼容)。 */
  tier?: MemberTier;
}

export type MemberTier = "lead" | "contributor";

/** M5 组队:模型画像(三信号源混合,附证据)。 */
export interface ModelProfile {
  model: string;
  score: number;
  evidence: string[];
}

export interface ProbeOutcome {
  score: number;
  notes: string;
  at: number;
}

export interface FormationMember {
  model: string;
  role: string;
  tier: MemberTier;
  isChair: boolean;
  assignment?: string;
  rationale?: string;
}

export interface FormationPlan {
  members: FormationMember[];
  rounds: number;
  notes?: string;
}

export interface AutoFormationResult {
  profiles: ModelProfile[];
  plan: FormationPlan;
}

export type CouncilMode = "council_only" | "council_and_execution" | "autonomous" | "sgs_game";

export interface CouncilBudget {
  maxRequests?: number | null;
  maxTokens?: number | null;
  wallClockSecs?: number | null;
}

export interface ExecutorSpec {
  agentId: string;
  cwd: string;
  projectId?: string | null;
  stepTimeoutSecs?: number | null;
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
  executor?: ExecutorSpec | null;
}

export type TaskStatus =
  | "pending"
  | "running"
  | "completed"
  | { failed: string }
  | { skipped: string }
  | "staged"
  | "merged";

export interface TaskProgress {
  stepId: string;
  title: string;
  status: TaskStatus;
  sessionId?: string | null;
  worktree?: string | null;
  branch?: string | null;
  changes: string[];
  error?: string | null;
  attempts: number;
  notes: string;
}

export function taskStatusLabel(status: TaskStatus | undefined): string {
  if (!status || status === "pending") return t("council.task.pending");
  if (status === "running") return t("council.task.running");
  if (status === "staged") return t("council.task.staged");
  if (status === "merged") return t("council.task.merged");
  if (status === "completed") return t("council.task.completed");
  if (typeof status === "object" && "failed" in status) return t("council.task.failed", { reason: status.failed });
  return t("council.task.skipped");
}

export function isTaskTerminal(status: TaskStatus | undefined): boolean {
  return status === "merged" || status === "completed" || (typeof status === "object" && status !== null && ("failed" in status || "skipped" in status));
}

export type RunPhase =
  | "created"
  | "playing"
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
  tasks: TaskProgress[];
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

export interface SgsPlayerState {
  memberId: string;
  role: "lord" | "loyalist" | "rebel" | "spy";
  revealed: boolean;
  hp: number;
  maxHp: number;
  hand: number;
  alive: boolean;
}

/** 身份局游戏快照(RunDetail.game;体力/手牌数/身份亮暗)。 */
export interface SgsGameState {
  players: SgsPlayerState[];
  round: number;
  turn: number;
  finished?: { winner: "lord_loyalist" | "rebel" | "spy"; summary: string } | null;
}

export interface RunDetail {
  spec: CouncilSpec;
  state: CouncilState;
  messages: CouncilMessage[];
  transitions: TransitionRecord[];
  plan?: PlanDoc | null;
  game?: SgsGameState | null;
  live: boolean;
}

/** 游戏身份的展示文案(已亮明才该被调用)。 */
export function sgsRoleLabel(role: SgsPlayerState["role"]): string {
  return t(`council.sgs.role.${role}` as Parameters<typeof t>[0]);
}

/** agent 下拉用的轻量形状(来自 api.listAgents 的 AgentConfig)。 */
export interface AgentLite {
  id: string;
  label: string;
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

/** M5:候选模型能力画像(静态启发+历史统计;探针结果若已跑过则并入)。 */
export async function councilModelProfiles(models: string[]): Promise<ModelProfile[]> {
  if (!isTauriRuntime()) return [];
  try {
    return await invoke<ModelProfile[]>("council_model_profiles", { models });
  } catch {
    return [];
  }
}

/** M5:本地能力探针(每模型 1 次请求;显式动作,结果持久化)。 */
export async function councilProbeModels(models: string[]): Promise<ProbeOutcome[] | null> {
  if (!isTauriRuntime() || models.length === 0) return null;
  try {
    return await invoke<ProbeOutcome[]>("council_probe_models", { models });
  } catch {
    return null;
  }
}

/** M5:AI 自动组队——返回建议的团队配置(用户审阅后才应用)。 */
export async function councilAutoFormation(
  task: string,
  models: string[],
): Promise<AutoFormationResult | null> {
  if (!isTauriRuntime()) return null;
  try {
    return await invoke<AutoFormationResult>("council_auto_formation", { task, models });
  } catch {
    return null;
  }
}

/** 后端全局事件名(payload:{runId,kind,data})。 */
export const COUNCIL_EVENT = "council-event";

export interface CouncilEvent {
  runId: string;
  kind: "message" | "phase" | "member" | "plan-ready" | "finished" | "warn" | "error" | "created" | "task";
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
  if (memberId === "system" || memberId === "chair") return "var(--text-muted)";
  const idx = members.findIndex((m) => m.id === memberId);
  if (idx < 0) return "var(--text-muted)";
  return MEMBER_COLORS[idx % MEMBER_COLORS.length] ?? "var(--text-muted)";
}

export function memberLabel(members: MemberSpec[], memberId: string): string {
  if (memberId === "system") return t("council.msg.system");
  const member = members.find((m) => m.id === memberId);
  return member ? member.displayName : memberId;
}

export function memberModelLabel(members: MemberSpec[], memberId: string): string {
  const member = members.find((m) => m.id === memberId);
  return member ? member.model : "";
}

export function phaseLabel(phase: RunPhase): string {
  return t(`council.phase.${phase}` as DictKey);
}

export function isTerminalPhase(phase: RunPhase): boolean {
  return phase === "completed" || phase === "cancelled" || phase === "failed" || phase === "budget_exhausted";
}

/** 圆桌阶段轨道(简报→…→计划,执行型再接执行/验证/完成)。 */
const RAIL_DELIB: readonly RunPhase[] = [
  "analyzing",
  "proposing",
  "reviewing",
  "revising",
  "synthesizing",
  "plan_ready",
];
const RAIL_EXEC: readonly RunPhase[] = ["executing", "verifying", "completed"];

export function councilRail(mode: CouncilMode): RunPhase[] {
  return mode === "council_only" ? [...RAIL_DELIB, "completed"] : [...RAIL_DELIB, ...RAIL_EXEC];
}

export type RailStepStatus = "done" | "current" | "pending" | "error";

/** 把运行态(含暂停/待批/失败)投影到轨道上的一步。 */
export function councilRailFocus(phase: RunPhase, pausedFrom?: RunPhase | null): RunPhase {
  if (phase === "paused") return pausedFrom ?? "analyzing";
  if (phase === "awaiting_approval") return "plan_ready";
  if (phase === "created") return "analyzing";
  return phase;
}

export function councilRailStatus(
  phase: RunPhase,
  pausedFrom: RunPhase | null | undefined,
  step: RunPhase,
  rail: readonly RunPhase[],
): RailStepStatus {
  if (phase === "completed") return "done";
  const error = phase === "failed" || phase === "cancelled" || phase === "budget_exhausted";
  const focus = councilRailFocus(error ? (pausedFrom ?? phase) : phase, pausedFrom);
  const ei = rail.indexOf(rail.includes(focus) ? focus : rail[0] ?? "analyzing");
  const si = rail.indexOf(step);
  if (si < 0 || ei < 0) return "pending";
  if (si < ei) return "done";
  if (si === ei) return error ? "error" : "current";
  return "pending";
}

export function railLabel(phase: RunPhase): string {
  return t(`council.rail.${phase}` as DictKey);
}

export function phaseBadgeClass(phase: RunPhase): string {
  if (phase === "completed") return "council-badge council-badge--done";
  if (phase === "failed" || phase === "cancelled" || phase === "budget_exhausted") {
    return "council-badge council-badge--err";
  }
  if (phase === "plan_ready" || phase === "awaiting_approval") return "council-badge council-badge--plan";
  if (phase === "paused") return "council-badge council-badge--paused";
  return "council-badge council-badge--live";
}

export function kindLabel(kind: CouncilMessageKind): string {
  return t(`council.kind.${kind}` as DictKey);
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
      const roundLabel = msg.kind === "briefing" ? "" : t("council.timeline.roundPrefix", { n: msg.round });
      group = { key, round: msg.round, kind: msg.kind, title: `${roundLabel}${kindLabel(msg.kind)}`, messages: [] };
      groups.push(group);
    }
    group.messages.push(msg);
  }
  return groups;
}

/** M5 分层徽标文案。 */
export function tierLabel(tier: MemberTier | undefined): string {
  return tier === "contributor" ? t("council.tier.contributor") : t("council.tier.lead");
}

/** 成员运行状态点(state.memberStatus → 展示态)。 */
export function memberStatusText(status: MemberRuntimeStatus | undefined): string {
  if (!status || status === "idle") return t("council.member.idle");
  if (status === "working") return t("council.member.working");
  if (status === "done") return t("council.member.done");
  return t("council.member.failed", { reason: status.failed });
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

