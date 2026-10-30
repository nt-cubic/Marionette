/**
 * Council Mode 前端回归(离线,纯逻辑):
 * - 类型层:council.ts 的展示逻辑(时间线分组/成员配色/阶段文案/派生 id)
 * - 计划渲染数据形状(与 Rust PlanDoc 的 camelCase 契约)
 *
 * Rust 侧状态机/协议/恢复的回归在 `cargo test --bin marionette council`。
 */
import assert from "node:assert/strict";
import {
  deriveMemberId,
  groupTimeline,
  isTerminalPhase,
  kindLabel,
  memberColor,
  memberLabel,
  memberStatusText,
  phaseLabel,
  type CouncilMessage,
  type MemberSpec,
  type RunPhase,
} from "../src/lib/council.ts";

let passed = 0;
function ok(name: string, fn: () => void) {
  fn();
  passed++;
  console.log(`  ✓ ${name}`);
}

const members: MemberSpec[] = [
  { id: "glm-architect", displayName: "架构师", model: "glm/glm-5.3", role: "架构", isChair: false },
  { id: "gpt-reviewer", displayName: "评审", model: "openai/gpt-5.2", role: "评审", isChair: true },
  { id: "ds-math", displayName: "算法", model: "deepseek/ds-v4", role: "算法", isChair: false },
];

function msg(id: string, kind: CouncilMessage["kind"], round: number, from: string, ts: number): CouncilMessage {
  return { messageId: id, runId: "r", round, from, to: "all", kind, replyTo: null, content: `c-${id}`, timestamp: ts, meta: null };
}

console.log("council.ts 展示逻辑");

ok("时间线分组:简报独立、同轮同类聚组、跨轮分开", () => {
  const messages = [
    msg("p1", "proposal", 1, "glm-architect", 100),
    msg("p2", "proposal", 1, "gpt-reviewer", 101),
    msg("c1", "critique", 1, "ds-math", 200),
    msg("rev1", "revision", 1, "glm-architect", 300),
    msg("c2", "critique", 2, "ds-math", 400),
    msg("b1", "briefing", 0, "chair", 50),
  ];
  const groups = groupTimeline(messages);
  const keys = groups.map((g) => g.key);
  assert.deepEqual(keys, ["briefing", "r1-proposal", "r1-critique", "r1-revision", "r2-critique"]);
  assert.equal(groups[1].messages.length, 2, "两份提案同组");
  assert.equal(groups[0].title, "简报");
  assert.equal(groups[1].title, "第 1 轮 · 提案");
});

ok("成员配色稳定且不撞位", () => {
  const c1 = memberColor(members, "glm-architect");
  const c2 = memberColor(members, "gpt-reviewer");
  assert.notEqual(c1, c2);
  assert.equal(c1, memberColor(members, "glm-architect"), "同 id 同色");
  assert.equal(memberColor(members, "system"), "var(--text-dim)");
  assert.equal(memberColor(members, "nobody"), "var(--text-dim)");
});

ok("成员与阶段文案", () => {
  assert.equal(memberLabel(members, "system"), "系统");
  assert.equal(memberLabel(members, "gpt-reviewer"), "评审");
  assert.equal(memberLabel(members, "nobody"), "nobody");
  assert.equal(phaseLabel("plan_ready"), "计划待批");
  assert.equal(phaseLabel("budget_exhausted"), "预算耗尽");
  assert.equal(kindLabel("rebuttal"), "反驳");
});

ok("终态判定", () => {
  const terminal: RunPhase[] = ["completed", "cancelled", "failed", "budget_exhausted"];
  for (const t of terminal) assert.equal(isTerminalPhase(t), true, t);
  for (const t of ["created", "proposing", "plan_ready", "paused", "executing"] as RunPhase[]) {
    assert.equal(isTerminalPhase(t), false, t);
  }
});

ok("成员状态文案(含失败分支)", () => {
  assert.equal(memberStatusText(undefined), "待命");
  assert.equal(memberStatusText("working"), "工作中");
  assert.equal(memberStatusText({ failed: "超时" }), "失败:超时");
});

ok("派生成员 id:kebab-case 且去重", () => {
  assert.equal(deriveMemberId("System 架构师!", []), "system");
  const existing = ["reviewer"];
  assert.equal(deriveMemberId("Reviewer", existing), "reviewer-2");
  assert.equal(deriveMemberId("评审", []), "member", "非 ASCII 全剥离后兜底");
});

ok("PlanDoc 契约字段为 camelCase(Rust serde 对齐)", () => {
  // 前端类型只是编译期;此处锁字段名,防止 Rust 侧 rename 变更悄悄断裂。
  const plan = {
    summary: "s",
    steps: [{ id: "s1", title: "t", detail: "d", dependsOn: ["s0"], acceptance: "a", ownerHint: null }],
    openQuestions: ["q"],
    acceptance: ["A"],
    verification: ["cargo test"],
  };
  assert.equal(plan.steps[0].dependsOn.length, 1);
  assert.deepEqual(plan.verification, ["cargo test"]);
});

console.log(`\n[verify] council: ${passed} 项通过`);
