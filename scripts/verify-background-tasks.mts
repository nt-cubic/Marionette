/**
 * 后台任务聚合器回归：buildBackgroundTaskRows 的排序、状态、key 稳定性。
 * npx tsx scripts/verify-background-tasks.mts
 */
import {
  buildBackgroundTaskRows,
  summarizeBackgroundTasks,
  type DelegateMetaSnapshot,
  type DelegateQueueSnapshot,
  type TerminalRowRaw,
} from "../src/lib/backgroundTasks.ts";

let failed = 0;
function check(name: string, cond: boolean, detail?: string) {
  if (cond) {
    console.log(`ok   ${name}`);
  } else {
    failed += 1;
    console.error(`FAIL ${name}${detail ? ` — ${detail}` : ""}`);
  }
}

const meta = new Map<string, DelegateMetaSnapshot>();
meta.set("child-1", {
  parentId: "parent-1",
  agentId: "grok",
  agentLabel: "Grok",
  modelId: "grok-4",
  prompt: "帮我调研一下 codeg 的实现",
  startedAt: 1000,
  finished: false,
});
meta.set("child-done", {
  parentId: "parent-1",
  agentId: "grok",
  agentLabel: "Grok",
  prompt: "完成的任务",
  startedAt: 900,
  finished: true,
});

const queue = new Map<string, DelegateQueueSnapshot[]>();
queue.set("parent-1", [
  { agentId: "dsh", agentLabel: "DeepSeek", prompt: "排队的委托", projectId: "p1" },
]);

const terminals: TerminalRowRaw[] = [
  {
    terminalId: "term_2",
    sessionId: "parent-2",
    command: "npm run build",
    running: false,
    startedAtMs: 500,
    exitStatus: { exitCode: 0 },
  },
  {
    terminalId: "term_3",
    sessionId: "parent-2",
    command: "cargo watch",
    running: true,
    startedAtMs: 600,
  },
  {
    terminalId: "term_4",
    sessionId: "parent-2",
    command: "failed job",
    running: false,
    startedAtMs: 400,
    exitStatus: { exitCode: 1 },
  },
];

const queuedSends = new Map<string, number>([["parent-3", 2]]);
const queuedSendLabels = new Map<string, string>([["parent-3", "会话丙"]]);
const sessionLabels = new Map<string, string>([
  ["parent-1", "会话甲"],
  ["parent-2", "会话乙"],
  ["parent-3", "会话丙"],
]);

const rows = buildBackgroundTaskRows({
  delegateMeta: meta,
  delegateQueue: queue,
  terminals,
  queuedSends,
  queuedSendLabels,
  currentSessionId: "parent-1",
  permissionSessionId: "parent-2",
  permissionTitle: "允许运行 rm -rf",
  askSessionId: null,
  planSessionId: "parent-3",
  sessionLabels,
});

// attention first
check(
  "attention rows sort first",
  rows[0]!.state === "attention" && rows[1]!.state === "attention",
  JSON.stringify(rows.map((r) => r.state)),
);
check("attention rows keyed per session+title", rows[0]!.key !== rows[1]!.key);
check("attention badge in summary", summarizeBackgroundTasks(rows).includes("2 需要你"));

// running block: delegate child + live terminal
const runningRows = rows.filter((r) => r.state === "running");
check(
  "running rows: delegate + live terminal",
  runningRows.length === 2 &&
    runningRows.some((r) => r.kind === "delegate") &&
    runningRows.some((r) => r.kind === "terminal" && r.title === "cargo watch"),
);

const withSub = buildBackgroundTaskRows({
  delegateMeta: new Map(),
  delegateQueue: new Map(),
  terminals: [],
  liveSubagents: [
    {
      sessionId: "parent-1",
      key: "spawn-1",
      title: "审计模块 A",
      detail: "subagent_id: abc",
      startedAtMs: 1200,
    },
  ],
  queuedSends: new Map(),
  queuedSendLabels: new Map(),
  scheduledSends: [
    { sessionId: "parent-1", id: "sched-1", atMs: 9999, preview: "明早再问" },
  ],
  currentSessionId: "parent-1",
  sessionLabels: new Map([["parent-1", "会话甲"]]),
});
check(
  "native subagent is a running row",
  withSub.some((r) => r.kind === "subagent" && r.state === "running" && r.title === "审计模块 A"),
);
check(
  "scheduled send is a queued row",
  withSub.some((r) => r.kind === "scheduled" && r.state === "queued" && r.detail === "明早再问"),
);

// finished delegate child never listed
check("finished delegate excluded", !rows.some((r) => r.key === "delegate:child-done"));

// queued delegate row
const dq = rows.find((r) => r.kind === "delegate_queued");
check("queued delegate row present", Boolean(dq) && dq!.state === "queued");

// queued send row
const qs = rows.find((r) => r.kind === "queued");
check(
  "queued send row with count",
  Boolean(qs) && qs!.detail!.includes("2 条") && qs!.title === "会话丙",
);

// terminal settled states
const t2 = rows.find((r) => r.key === "terminal:term_2");
const t4 = rows.find((r) => r.key === "terminal:term_4");
check("exit 0 terminal is done", t2!.state === "done" && t2!.detail === "退出码 0");
check("exit 1 terminal is failed", t4!.state === "failed");

// settled rows sink below running/queued
const firstFailedIdx = rows.findIndex((r) => r.state === "done" || r.state === "failed");
check(
  "settled rows sink to bottom",
  firstFailedIdx >= 0 &&
    rows.slice(firstFailedIdx).every((r) => r.state === "done" || r.state === "failed"),
);

// key stability across rebuilds
const rows2 = buildBackgroundTaskRows({
  delegateMeta: meta,
  delegateQueue: queue,
  terminals,
  queuedSends,
  queuedSendLabels,
  currentSessionId: "parent-1",
  permissionSessionId: "parent-2",
  permissionTitle: "允许运行 rm -rf",
  askSessionId: null,
  planSessionId: "parent-3",
  sessionLabels,
});
check(
  "keys stable across rebuilds",
  rows.map((r) => r.key).join("|") === rows2.map((r) => r.key).join("|"),
);

// empty input → empty list
const empty = buildBackgroundTaskRows({
  delegateMeta: new Map(),
  delegateQueue: new Map(),
  terminals: [],
  queuedSends: new Map(),
  queuedSendLabels: new Map(),
  currentSessionId: null,
  sessionLabels: new Map(),
});
check("empty input renders nothing", empty.length === 0);

if (failed > 0) {
  console.error(`\n${failed} check(s) failed`);
  process.exit(1);
}
console.log("\nall background-task checks passed");
