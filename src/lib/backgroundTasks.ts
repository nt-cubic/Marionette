import { t } from "./i18n";

/**
 * 后台任务面板的数据聚合层（纯函数，无 React 依赖）。
 *
 * 来源与行类型：
 * - @-delegate 子代理（`delegateMetaRef` 未完成 + `delegateQueueRef` 排队）→ "delegate"
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
  | "subagent"
  | "terminal"
  | "queued"
  | "scheduled"
  | "attention";

export type BackgroundTaskState = "running" | "queued" | "attention" | "done" | "failed" | "stopped";

export type BackgroundTaskRow = {
  /** Stable React key：delegate/terminal 用自己的 id，queued/attention 用 sessionId。 */
  key: string;
  kind: BackgroundTaskKind;
  state: BackgroundTaskState;
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
    rows.push({
      key: `terminal:${term.terminalId}`,
      kind: "terminal",
      state: term.running ? "running" : exit?.exitCode === 0 ? "done" : "failed",
      sessionId: term.sessionId,
      title: clip(term.command, 80) || term.terminalId,
      detail: term.running
        ? undefined
        : exit?.signal
          ? t("tasks.signal", { signal: exit.signal })
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
