/**
 * AI 派活的任务事件 → 父对话卡片（纯函数；App.tsx 的监听只负责把结果
 * 落进会话 rail，所以这里的规则可以直接进回归脚本）。
 *
 * 只有两种事件会变成卡片：
 * - `task.started` → 运行卡（「AI 派活」芯片 + 实时流 + 停止按钮）；
 * - 终态（completed / failed / cancelled / interrupted）→ 结果卡。
 *
 * `task.progress` **不是卡片**。额度午休排程、重试下发时后端都会发一条
 * progress——把它画成结果卡，「午休中」的任务会立刻显示成已失败，而且
 * 真正完成时的结果卡会被去重规则吃掉（2026-11-23 修）。
 */
import type { ChiefTask, DelegateTaskEventPayload } from "./api";
import type { SessionEvent } from "./types";

const TERMINAL_KINDS: ReadonlySet<string> = new Set([
  "task.completed",
  "task.failed",
  "task.cancelled",
  "task.interrupted",
]);

export type DelegateCardEvent = Extract<
  SessionEvent,
  { type: "subtask_started" | "subtask_result" }
>;

function cardsFor(rail: readonly SessionEvent[], taskId: string): DelegateCardEvent[] {
  return rail.filter(
    (event): event is DelegateCardEvent =>
      (event.type === "subtask_started" || event.type === "subtask_result") &&
      event.initiatedBy === "agent" &&
      event.taskId === taskId,
  );
}

/**
 * 该为这条事件追加哪张卡片；`null` = 不追加。
 *
 * @param rail 父会话当前的事件列（未加载的会话传 null/空——冷会话不落卡，
 *   唤醒文本补投时结果会自然进转录）
 */
export function delegateCardFor(
  payload: DelegateTaskEventPayload,
  rail: readonly SessionEvent[] | null | undefined,
  now: string = new Date().toISOString(),
): DelegateCardEvent | null {
  const parent = payload.sessionId;
  const task: ChiefTask | undefined = payload.task;
  if (!parent || !task?.taskId || !task.workerSessionId) return null;
  if (!rail || rail.length === 0) return null;
  const existing = cardsFor(rail, task.taskId);

  if (payload.kind === "task.started") {
    if (existing.some((event) => event.type === "subtask_started")) return null;
    return {
      type: "subtask_started",
      sessionId: parent,
      childSessionId: task.workerSessionId,
      agentId: task.agentId,
      agentLabel: task.agentId,
      modelId: task.model ?? undefined,
      displayModel: task.displayModel ?? task.bridgeModel ?? task.model ?? undefined,
      prompt: task.title,
      createdAt: now,
      initiatedBy: "agent",
      taskId: task.taskId,
    };
  }

  if (!TERMINAL_KINDS.has(payload.kind)) return null;
  // 结果卡只补在运行卡之后（重启后回放的历史任务不凭空造卡），且只补一次。
  if (!existing.some((event) => event.type === "subtask_started")) return null;
  if (existing.some((event) => event.type === "subtask_result")) return null;

  const status =
    task.status === "completed" ? "done" : task.status === "cancelled" ? "cancelled" : "failed";
  return {
    type: "subtask_result",
    sessionId: parent,
    childSessionId: task.workerSessionId,
    agentId: task.agentId,
    status,
    // 判词（宿主的一句人话 + worker 的最后输出）放在 summary 里，展开即见。
    // `error` 留空：卡片的状态位因此显示本地化的「失败 / 已取消」，
    // 而不是后端的内部错误码。
    summary: task.lastSummary ?? "",
    displayModel: task.displayModel ?? task.bridgeModel ?? task.model ?? undefined,
    durationMs:
      task.updatedAt > task.createdAt ? task.updatedAt - task.createdAt : undefined,
    createdAt: now,
    initiatedBy: "agent",
    taskId: task.taskId,
    // 有存档点才出现「回滚此任务」。
    checkpointBase: task.checkpointBase ?? undefined,
  };
}
