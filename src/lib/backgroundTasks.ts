import { t, type DictKey } from "./i18n";
import { isExitAnswer, readCommandExit, signalOfExitCode } from "./exitMeaning";
import { projectAttention, type Attention } from "./fleetAttention";

/**
 * 后台任务面板的数据聚合层（纯函数，无 React 依赖）。
 *
 * 来源与行类型：
 * - @-delegate 子代理（`delegateMetaRef` 未完成 + `delegateQueueRef` 排队）→ "delegate"
 * - AI 自主派活（Rust 任务账本 staffId=null 的任务，useStaffController 轮询）→ "agent-delegate"
 * - ACP 原生嵌套子代理（Grok spawn_subagent / Claude Task / OpenCode task）→ "subagent"
 * - ACP `terminal/*` 宿主里的后台命令（Rust `list_acp_terminals`）→ "terminal"
 * - 排队发送（`pendingSendsRef`，等回合空位）→ "queued"
 * - 定时发送 → "scheduled"
 * - 待审批 / 待回答 / 计划审批（permission / ask / plan）→ "attention"（需要你）
 *
 * 会话的 running/starting 状态本身不进列表：侧栏与贴边窄条已经覆盖，
 * 这里只收「不是当前对话也能在跑的东西」。
 */

export type BackgroundTaskKind =
  | "delegate"
  | "delegate_queued"
  | "agent-delegate"
  | "subagent"
  | "terminal"
  | "queued"
  | "scheduled"
  | "attention";

/** Six user-facing lifecycle states for a delegated worker task (P0-6). */
export type DelegatedTaskState =
  | "starting"
  | "running"
  | "waiting_user"
  | "completed"
  | "failed"
  | "cancelled";

export type BackgroundTaskState = "running" | "queued" | "attention" | "done" | "failed" | "stopped";

export type BackgroundTaskRow = {
  /** Stable React key: delegate/terminal use their own id; queued/attention use sessionId. */
  key: string;
  kind: BackgroundTaskKind;
  state: BackgroundTaskState;
  /** Six-state delegated-worker lifecycle; only agent-delegate rows carry it (row.state stays attention-priority). */
  lifecycle?: DelegatedTaskState;
  /** 点击行时打开的目标会话。 */
  sessionId: string;
  /** 主标题：子代理标签 / 命令行 / 会话名。 */
  title: string;
  /** 次行：prompt 摘要 / 退出码 / 排队说明。 */
  detail?: string;
  startedAtMs: number;
  /** 是否由当前对话发起（决定点击行为是切标签还是切会话）。 */
  fromCurrent?: boolean;
};

export type DelegateMetaSnapshot = {
  parentId: string;
  agentId: string;
  agentLabel: string;
  modelId?: string;
  prompt: string;
  startedAt: number;
  finished: boolean;
};

export type DelegateQueueSnapshot = {
  agentId: string;
  agentLabel: string;
  modelId?: string;
  prompt: string;
  projectId: string;
};

export type TerminalRowRaw = {
  terminalId: string;
  sessionId: string;
  command: string;
  running: boolean;
  startedAtMs: number;
  exitStatus?: { exitCode?: number; signal?: string };
  /** PTY 输出尾部（Rust 截好的 ~2KB）：读退出码的含义用（无匹配 vs 报错）。 */
  outputTail?: string;
};

/** 面板排序：需要你 > 跑动中 > 排队 > 已结束；同级按开始时间新→旧。 */
const STATE_ORDER: Record<BackgroundTaskState, number> = {
  attention: 0,
  running: 1,
  queued: 2,
  done: 3,
  failed: 3,
  stopped: 3,
};

export function backgroundTaskAttentionCount(rows: BackgroundTaskRow[]): number {
  return rows.filter((row) => row.state === "attention").length;
}

export function backgroundTaskRunningCount(rows: BackgroundTaskRow[]): number {
  return rows.filter((row) => row.state === "running").length;
}

/** 一行摘要给分区头：「2 运行中 · 1 需要你」。 */
export function summarizeBackgroundTasks(rows: BackgroundTaskRow[]): string {
  const attention = backgroundTaskAttentionCount(rows);
  const running = backgroundTaskRunningCount(rows);
  const queued = rows.filter((row) => row.state === "queued").length;
  const parts: string[] = [];
  if (running > 0) parts.push(t("tasks.runningCount", { count: running }));
  if (queued > 0) parts.push(t("tasks.queuedCount", { count: queued }));
  if (attention > 0) parts.push(t("tasks.attentionCount", { count: attention }));
  return parts.join(" · ");
}

function clip(text: string, max: number): string {
  const raw = (text ?? "").replace(/\s+/g, " ").trim();
  return raw.length > max ? `${raw.slice(0, max)}…` : raw;
}

export type LiveSubagentSnapshot = {
  sessionId: string;
  key: string;
  title: string;
  detail?: string;
  startedAtMs: number;
};

export type ScheduledSendSnapshot = {
  sessionId: string;
  id: string;
  atMs: number;
  preview: string;
};

/** AI 自主派活的任务快照（chiefTasksListAll 里 staffId 为空的行）。 */
export type AgentDelegateSnapshot = {
  taskId: string;
  parentSessionId: string;
  workerSessionId: string | null;
  agentId: string;
  title: string;
  status:
    | "proposed"
    | "starting"
    | "running"
    | "waiting_user"
    | "completed"
    | "failed"
    | "cancelled"
    | "interrupted";
  waitingReason?: string | null;
  retryAt?: number | null;
  startedAtMs: number;
  updatedAtMs: number;
  summary?: string | null;
};

/** Raw task-ledger status (includes retired proposed / interrupted; normalized at the boundary). */
export type DelegatedTaskStatus = AgentDelegateSnapshot["status"];

export type DelegatedTaskProjectionInput = {
  status: DelegatedTaskStatus;
  waitingReason?: string | null;
  retryAt?: number | null;
  lastEventAt?: number | null;
  now?: number;
};

export type DelegatedTaskProjection = {
  /** The only lifecycle field components may render. */
  state: DelegatedTaskState;
  /** Boundary-normalized status: proposed -> starting, interrupted -> cancelled. */
  status: Exclude<DelegatedTaskStatus, "proposed" | "interrupted">;
  /** Same attention projection as the background panel (approval / input / retry / ...). */
  attention: Attention;
  /** completed / failed / cancelled: no further transitions. */
  terminal: boolean;
  /** Quota-limited auto-retry (lifecycle stays starting, never running). */
  waitingRetry: boolean;
};

/**
 * The single lifecycle projection for delegated tasks (P0-6).
 *
 * Input is the raw task-ledger status plus waitingReason/retryAt; output is always one
 * of six user-visible states. Retired names fold at the boundary: proposed -> starting,
 * interrupted -> cancelled.
 *
 * Reduction goes through projectAttention (same truth as the background panel), then to
 * lifecycle: approval/input -> waiting_user (waiting_user is never shown as running),
 * failure -> failed, interruption -> cancelled, waiting_retry -> starting (waiting for the
 * next attempt), settled -> completed/cancelled, stalled/running follow the status.
 * Attention priority decides whether a human is needed; it does not rewrite lifecycle.
 */
export function projectDelegatedTask(input: DelegatedTaskProjectionInput): DelegatedTaskProjection {
  const status: Exclude<DelegatedTaskStatus, "proposed" | "interrupted"> =
    input.status === "proposed"
      ? "starting"
      : input.status === "interrupted"
        ? "cancelled"
        : input.status;
  const attention = projectAttention({
    status: input.status,
    waitingReason: input.waitingReason,
    retryAt: input.retryAt,
    lastEventAt: input.lastEventAt,
    now: input.now,
  });
  let state: DelegatedTaskState;
  switch (attention.category) {
    case "approval":
    case "input":
      state = "waiting_user";
      break;
    case "failure":
      state = "failed";
      break;
    case "interruption":
      state = "cancelled";
      break;
    case "waiting_retry":
      // Quota pause: the task is alive but not running this round; wait for the next attempt.
      state = "starting";
      break;
    case "settled":
      state = status === "completed" ? "completed" : "cancelled";
      break;
    case "stalled":
    case "running":
    default:
      state = status === "starting" ? "starting" : "running";
      break;
  }
  return {
    state,
    status,
    attention,
    terminal: state === "completed" || state === "failed" || state === "cancelled",
    waitingRetry: attention.category === "waiting_retry",
  };
}

/** i18n keys for the six lifecycle states; components must not invent status words. */
export const DELEGATED_TASK_STATE_LABEL: Record<DelegatedTaskState, DictKey> = {
  starting: "tasks.stateStarting",
  running: "tasks.stateRunning",
  waiting_user: "tasks.stateWaitingUser",
  completed: "tasks.stateCompleted",
  failed: "tasks.stateFailed",
  cancelled: "tasks.stateCancelled",
};

export type BuildBackgroundTasksInput = {
  delegateMeta: Map<string, DelegateMetaSnapshot>;
  delegateQueue: Map<string, DelegateQueueSnapshot[]>;
  terminals: TerminalRowRaw[];
  /** ACP-native nested agents still running inside a parent process. */
  liveSubagents?: LiveSubagentSnapshot[];
  /** sessionId → 排队发送条数（0 条或缺失 = 无）。 */
  queuedSends: Map<string, number>;
  queuedSendLabels: Map<string, string>;
  scheduledSends?: ScheduledSendSnapshot[];
  /** AI 自主派活的任务（staffId=null；终态只保留最近 10 分钟的行）。 */
  agentDelegates?: AgentDelegateSnapshot[];
  currentSessionId: string | null;
  permissionSessionId?: string | null;
  permissionTitle?: string | null;
  askSessionId?: string | null;
  planSessionId?: string | null;
  /** sessionId → 会话名（attention 行显示用）。 */
  sessionLabels: Map<string, string>;
};

export function buildBackgroundTaskRows(input: BuildBackgroundTasksInput): BackgroundTaskRow[] {
  const rows: BackgroundTaskRow[] = [];
  const now = Date.now();

  // 1) @-delegate 子代理：跑动的 + 排队的。
  for (const [childId, meta] of input.delegateMeta) {
    if (meta.finished) continue;
    rows.push({
      key: `delegate:${childId}`,
      kind: "delegate",
      state: "running",
      sessionId: meta.parentId,
      title: `${meta.agentLabel || t("tasks.subagent")}${meta.modelId ? ` · ${meta.modelId}` : ""}`,
      detail: clip(meta.prompt, 64),
      startedAtMs: meta.startedAt || now,
      fromCurrent: input.currentSessionId === meta.parentId,
    });
  }
  for (const child of input.liveSubagents ?? []) {
    rows.push({
      key: `subagent:${child.sessionId}:${child.key}`,
      kind: "subagent",
      state: "running",
      sessionId: child.sessionId,
      title: child.title || t("tasks.subagent"),
      detail: child.detail,
      startedAtMs: child.startedAtMs || now,
      fromCurrent: input.currentSessionId === child.sessionId,
    });
  }

  // 1b) AI 自主派活（Rust 后端驱动的任务账本行）。非终态全列；终态只留
  // 最近 10 分钟（「干完了」的行给一眼回看，不让面板变成历史清单）。
  // 状态归约走 fleetAttention 统一投影（第六期）：「要不要人」是同一份
  // 真相，桌面通知 / Telegram 以后接同一个函数，不再各拼各的。
  // The delegated-worker UI renders projectDelegatedTask's six-state lifecycle.
  for (const task of input.agentDelegates ?? []) {
    const terminal = ["completed", "failed", "cancelled", "interrupted"].includes(task.status);
    if (terminal && Date.now() - task.updatedAtMs > 10 * 60_000) continue;
    const projection = projectDelegatedTask({
      status: task.status,
      waitingReason: task.waitingReason,
      retryAt: task.retryAt,
      lastEventAt: task.updatedAtMs,
      now,
    });
    const attention = projection.attention;
    const state: BackgroundTaskState =
      attention.category === "approval" || attention.category === "input" || attention.category === "stalled"
        ? "attention"
        : attention.category === "failure"
          ? "failed"
          : attention.category === "interruption"
            ? "stopped"
            : attention.category === "waiting_retry"
              ? "queued"
              : task.status === "completed"
                ? "done"
                : terminal
                  ? "stopped"
                  : task.status === "proposed" || task.status === "starting"
                    ? "queued"
                    : "running";
    const retryInMin =
      attention.category === "waiting_retry" && task.retryAt
        ? Math.max(0, Math.round((task.retryAt - now) / 60_000))
        : null;
    rows.push({
      key: `agent-delegate:${task.taskId}`,
      kind: "agent-delegate",
      state,
      lifecycle: projection.state,
      // 点击优先开 worker 会话（看它干活）；还没建 worker 时回父会话。
      sessionId: task.workerSessionId || task.parentSessionId,
      title: t("tasks.agentDelegateTitle", { agent: task.agentId, title: clip(task.title, 48) }),
      detail:
        attention.category === "waiting_retry"
          ? t("tasks.agentDelegateRetry", { count: retryInMin ?? 0 })
          : attention.requiresAction
            ? t("tasks.agentDelegateAttention")
            : task.summary
              ? clip(task.summary, 64)
              : undefined,
      startedAtMs: task.startedAtMs || now,
      fromCurrent: input.currentSessionId === task.parentSessionId,
    });
  }

  for (const [parentId, jobs] of input.delegateQueue) {
    for (let i = 0; i < jobs.length; i++) {
      const job = jobs[i]!;
      rows.push({
        key: `delegate-queued:${parentId}:${i}:${job.prompt.slice(0, 24)}`,
        kind: "delegate_queued",
        state: "queued",
        sessionId: parentId,
        title: t("tasks.queuedSubagent", { agent: job.agentLabel || t("tasks.subagent") }),
        detail: clip(job.prompt, 64),
        startedAtMs: now,
        fromCurrent: input.currentSessionId === parentId,
      });
    }
  }

  // 2) ACP terminal/* 后台命令。结束的行保留（退出码一目了然），排序沉底。
  for (const term of input.terminals) {
    const exit = term.exitStatus;
    // 退出码人话：grep exit 1 = 无匹配、test exit 1 = 不成立……（读自 codeg
    // exit-meaning 的保守解析；答案类退出按完成着色，不再标红）。
    const meaning =
      !term.running && exit
        ? readCommandExit(
            term.command,
            { code: exit.exitCode ?? null, signal: exit.signal ?? null },
            term.outputTail ?? "",
          )
        : null;
    const codeSignal = exit?.signal ?? signalOfExitCode(exit?.exitCode ?? null);
    rows.push({
      key: `terminal:${term.terminalId}`,
      kind: "terminal",
      state: term.running
        ? "running"
        : isExitAnswer(meaning) || exit?.exitCode === 0
          ? "done"
          : "failed",
      sessionId: term.sessionId,
      title: clip(term.command, 80) || term.terminalId,
      detail: term.running
        ? undefined
        : codeSignal
          ? t("tasks.signal", { signal: codeSignal })
          : meaning
            ? t("tasks.exitMeaning.withCode", {
                meaning: t(`tasks.exitMeaning.${meaning}`),
                code: exit?.exitCode ?? "?",
              })
            : t("tasks.exitCode", { code: exit?.exitCode ?? "?" }),
      startedAtMs: term.startedAtMs || now,
    });
  }

  // 3) 排队发送：等当前回合结束才发出的插话。定时发送另列。
  for (const [sessionId, count] of input.queuedSends) {
    if (count <= 0) continue;
    rows.push({
      key: `queued:${sessionId}`,
      kind: "queued",
      state: "queued",
      sessionId,
      title: input.queuedSendLabels.get(sessionId) || t("tasks.queuedSend"),
      detail: t("tasks.queuedMessages", { count }),
      startedAtMs: now,
    });
  }

  for (const item of input.scheduledSends ?? []) {
    rows.push({
      key: `scheduled:${item.sessionId}:${item.id}`,
      kind: "scheduled",
      state: "queued",
      sessionId: item.sessionId,
      title: input.sessionLabels.get(item.sessionId) || t("tasks.scheduledSend"),
      detail: item.preview,
      startedAtMs: item.atMs,
      fromCurrent: input.currentSessionId === item.sessionId,
    });
  }

  // 4) 需要你：permission / ask / plan 卡在哪场对话，哪场对话就有一行。
  const attention: Array<{ sessionId: string | null | undefined; title: string; detail: string }> = [
    {
      sessionId: input.permissionSessionId,
      title: t("tasks.permissionApprovalTitle"),
      detail: clip(input.permissionTitle ?? "", 64),
    },
    { sessionId: input.askSessionId, title: t("tasks.askTitle"), detail: "" },
    { sessionId: input.planSessionId, title: t("tasks.planApprovalTitle"), detail: "" },
  ];
  for (const item of attention) {
    if (!item.sessionId) continue;
    rows.push({
      key: `attention:${item.sessionId}:${item.title}`,
      kind: "attention",
      state: "attention",
      sessionId: item.sessionId,
      title: item.title,
      detail: item.detail || input.sessionLabels.get(item.sessionId) || "",
      startedAtMs: now,
    });
  }

  rows.sort((a, b) => {
    const byState = STATE_ORDER[a.state] - STATE_ORDER[b.state];
    if (byState !== 0) return byState;
    return b.startedAtMs - a.startedAtMs;
  });
  return rows;
}
