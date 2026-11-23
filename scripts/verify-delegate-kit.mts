/**
 * AI 自主派活（第一期「拆门」）前端回归：
 * - backgroundTasks 的 agent-delegate 行：状态映射、终态 10 分钟退场、
 *   点击目标 worker 优先；
 * - i18n 片段对键集一致、占位符用 {agent}/{title}（PR #6 的 {n} 教训：
 *   t() 的替换表只认 params 里的键）；
 * - chiefTee 的后端驱动家族判定包含 agent-worker（挂起/压制豁免 + tee 回填）。
 * npx tsx scripts/verify-delegate-kit.mts
 */
import assert from "node:assert/strict";
import {
  buildBackgroundTaskRows,
  type AgentDelegateSnapshot,
} from "../src/lib/backgroundTasks.ts";
import { isChiefOrigin } from "../src/lib/chiefTee.ts";
import { projectAttention, STALL_MS } from "../src/lib/fleetAttention.ts";
import { delegateCardFor } from "../src/lib/delegateCards.ts";
import { zh as zhDelegate } from "../src/locales/parts/fe-delegate.zh";
import { en as enDelegate } from "../src/locales/parts/fe-delegate.en";
import { applyLocale, t } from "../src/lib/i18n.ts";

await applyLocale("zh-CN");

let failed = 0;
function check(name: string, cond: boolean, detail?: string) {
  if (cond) {
    console.log(`ok   ${name}`);
  } else {
    failed += 1;
    console.error(`FAIL ${name}${detail ? ` — ${detail}` : ""}`);
  }
}

// ── 1) agent-delegate 行聚合 ────────────────────────────────────────────────

function snapshot(overrides: Partial<AgentDelegateSnapshot> = {}): AgentDelegateSnapshot {
  return {
    taskId: "task_1",
    parentSessionId: "parent-1",
    workerSessionId: "worker-1",
    agentId: "codex",
    title: "调研 orca 的编排模型",
    status: "running",
    waitingReason: null,
    retryAt: null,
    startedAtMs: Date.now() - 60_000,
    updatedAtMs: Date.now() - 10_000,
    summary: null,
    ...overrides,
  };
}

function rowsFor(tasks: AgentDelegateSnapshot[]) {
  return buildBackgroundTaskRows({
    delegateMeta: new Map(),
    delegateQueue: new Map(),
    terminals: [],
    queuedSends: new Map(),
    queuedSendLabels: new Map(),
    agentDelegates: tasks,
    currentSessionId: "parent-1",
    sessionLabels: new Map(),
  }).filter((row) => row.kind === "agent-delegate");
}

{
  const rows = rowsFor([
    snapshot({ taskId: "t-run", status: "running" }),
    snapshot({ taskId: "t-wait", status: "waiting_user" }),
    snapshot({ taskId: "t-start", status: "starting" }),
    snapshot({ taskId: "t-done", status: "completed" }),
    snapshot({ taskId: "t-fail", status: "failed" }),
    snapshot({ taskId: "t-old", status: "completed", updatedAtMs: Date.now() - 11 * 60_000 }),
    snapshot({ taskId: "t-noworker", status: "running", workerSessionId: null }),
  ]);
  const byId = new Map(rows.map((row) => [row.key.slice("agent-delegate:".length), row]));
  check("running → running 行", byId.get("t-run")?.state === "running");
  check("waiting_user → attention 行（需要你）", byId.get("t-wait")?.state === "attention");
  check("starting → queued 行", byId.get("t-start")?.state === "queued");
  check("completed → done 行", byId.get("t-done")?.state === "done");
  check("failed → failed 行", byId.get("t-fail")?.state === "failed");
  check("终态超 10 分钟退场", !byId.has("t-old"), "11 分钟前的终态任务不应再列出");
  check("点击目标 worker 优先", byId.get("t-run")?.sessionId === "worker-1");
  check(
    "无 worker 时点击回父会话",
    byId.get("t-noworker")?.sessionId === "parent-1",
  );
  check("fromCurrent 按父会话判定", byId.get("t-run")?.fromCurrent === true);
  check(
    "标题用 agentDelegateTitle 模板",
    t("tasks.agentDelegateTitle", { agent: "codex", title: "x" }).includes("codex"),
  );
}

// ── 2) i18n 片段对 ─────────────────────────────────────────────────────────

{
  const zhKeys = Object.keys(zhDelegate).sort();
  const enKeys = Object.keys(enDelegate).sort();
  check("zh/en 键集一致", JSON.stringify(zhKeys) === JSON.stringify(enKeys));
  for (const key of ["shelf.agentWorkerOrigin", "session.subtaskAgentChip", "session.subtaskAgentRunning"]) {
    check(`关键键存在：${key}`, key in zhDelegate);
  }
  check(
    "agentDelegateTitle 占位符是 {agent}/{title}（不是 {n}）",
    zhDelegate["tasks.agentDelegateTitle"].includes("{agent}")
      && zhDelegate["tasks.agentDelegateTitle"].includes("{title}")
      && !zhDelegate["tasks.agentDelegateTitle"].includes("{n}"),
  );
  check(
    "en 侧占位符同为 {agent}/{title}",
    enDelegate["tasks.agentDelegateTitle"].includes("{agent}")
      && enDelegate["tasks.agentDelegateTitle"].includes("{title}"),
  );
}

// ── 3) attention 统一投影（第六期「安静」） ─────────────────────────────

{
  const now = Date.now();
  check(
    "权限/计划等待 → approval，需要人",
    projectAttention({ status: "waiting_user", waitingReason: "permission", now }).category === "approval"
      && projectAttention({ status: "waiting_user", waitingReason: "permission", now }).requiresAction,
  );
  check(
    "提问等待 → input，需要人",
    projectAttention({ status: "waiting_user", waitingReason: "question", now }).category === "input",
  );
  check("失败 → failure，需要人", projectAttention({ status: "failed", now }).requiresAction);
  check("中断 → interruption，需要人", projectAttention({ status: "interrupted", now }).requiresAction);
  check(
    "额度午休 → waiting_retry，不需要人",
    projectAttention({ status: "running", waitingReason: "auto_retry", retryAt: now + 300_000, now })
      .requiresAction === false,
  );
  check(
    "20 分钟无动静 → stalled，需要人",
    projectAttention({ status: "running", lastEventAt: now - STALL_MS - 1, now }).category === "stalled",
  );
  check(
    "午休任务不做卡死误判（waiting_retry 优先）",
    projectAttention({
      status: "running",
      waitingReason: "auto_retry",
      retryAt: now + 300_000,
      lastEventAt: now - STALL_MS - 1,
      now,
    }).category === "waiting_retry",
  );
  check("完成 → settled，安静", projectAttention({ status: "completed", now }).category === "settled");
}

// 额度重试行：waiting_reason=auto_retry 的任务显示排队中 + 重试分钟数
{
  const rows = rowsFor([
    snapshot({
      taskId: "t-retry",
      status: "running",
      waitingReason: "auto_retry",
      retryAt: Date.now() + 4 * 60_000,
      summary: "额度/限流受限……",
    }),
  ]);
  check("额度重试中 → queued 行", rows[0]?.state === "queued");
  check("重试行文案带分钟数", (rows[0]?.detail ?? "").includes("4"));
}

// ── 4) 后端驱动家族判定 ────────────────────────────────────────────────────

{
  check("agent-worker 属于后端驱动家族（挂起/压制豁免）", isChiefOrigin("agent-worker"));
  check("chief-worker 仍在家族内", isChiefOrigin("chief-worker"));
  check("chief 会话本体仍在家族内", isChiefOrigin("chief"));
  check("普通 user 会话不在家族内", !isChiefOrigin("user"));
  check("delegate（@ 委托子会话）不在家族内", !isChiefOrigin("delegate"));
}

// ── 5) 父对话卡片规则（lib/delegateCards.ts） ─────────────────────────────
//
// 2026-11-23 实锤：额度午休排程时后端发一条 task.progress，旧逻辑把它画成
// 「失败」结果卡，任务真正完成时的结果卡又被去重吃掉——午休中的任务永远
// 显示成已失败。

{
  const task = (overrides: Record<string, unknown> = {}) =>
    ({
      taskId: "task-card",
      requestId: "req-card",
      staffId: null,
      chiefSessionId: "parent-card",
      title: "调研 orca 的编排模型",
      goal: "goal",
      projectId: "project-x",
      workerSessionId: "worker-card",
      agentId: "grok-build",
      status: "running",
      createdAt: 1_000,
      updatedAt: 2_000,
      ...overrides,
    }) as never;
  const event = (kind: string, t: unknown) => ({ sessionId: "parent-card", kind, task: t });
  const started = event("task.started", task());
  const baseRail = [{ type: "user_message", sessionId: "parent-card", text: "去调研", createdAt: "2026-01-01T00:00:00.000Z" }] as never[];

  const startCard = delegateCardFor(started, baseRail, "2026-01-01T00:00:01.000Z");
  check("task.started → 运行卡（带 agent 标识与 taskId）",
    startCard?.type === "subtask_started" && startCard.initiatedBy === "agent" && startCard.taskId === "task-card");
  check("运行卡只落一次（事件重放/多窗不重复）",
    delegateCardFor(started, [...baseRail, startCard] as never[]) === null);

  const withStart = [...baseRail, startCard] as never[];
  const napping = event("task.progress", task({ status: "running", waitingReason: "auto_retry" }));
  check("task.progress 不生成卡片（额度午休不能被画成失败）", delegateCardFor(napping, withStart) === null);
  check("未知 kind 不生成卡片", delegateCardFor(event("task.weird", task()), withStart) === null);

  const doneCard = delegateCardFor(event("task.completed", task({ status: "completed" })), withStart);
  check("completed → done 结果卡", doneCard?.type === "subtask_result" && doneCard.status === "done");
  const failedCard = delegateCardFor(
    event("task.failed", task({ status: "failed", error: "worker_gone", lastSummary: "worker 进程退出。" })),
    withStart,
  );
  check("failed → failed 结果卡；error 留空（状态位显示本地化的「失败」而不是内部错误码）",
    failedCard?.type === "subtask_result" && failedCard.status === "failed" && failedCard.error === undefined
      && failedCard.summary === "worker 进程退出。");
  const cancelledCard = delegateCardFor(event("task.cancelled", task({ status: "cancelled" })), withStart);
  check("cancelled → cancelled 结果卡（用户停止后卡片显示已取消）",
    cancelledCard?.type === "subtask_result" && cancelledCard.status === "cancelled" && cancelledCard.error === undefined);
  check("结果卡只补一次",
    delegateCardFor(event("task.completed", task({ status: "completed" })), [...withStart, doneCard] as never[]) === null);
  check("没有运行卡就不凭空补结果卡（重启后回放的历史任务）", delegateCardFor(event("task.completed", task()), baseRail) === null);
  check("冷会话不落卡", delegateCardFor(started, null) === null && delegateCardFor(started, []) === null);
  check("有存档点才带 checkpointBase（「回滚此任务」的依据）",
    delegateCardFor(event("task.completed", task({ status: "completed", checkpointBase: "abc123" })), withStart)?.checkpointBase === "abc123"
      && doneCard?.checkpointBase === undefined);
}

if (failed > 0) {
  console.error(`\nverify-delegate-kit: ${failed} 项失败`);
  process.exit(1);
}
console.log("\nverify-delegate-kit: 全部通过");
