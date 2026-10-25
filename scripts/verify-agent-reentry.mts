/**
 * verify-agent-reentry — 再次进入对话（post-turn background continuation）
 *
 * lib/agentReentry.ts 的判定与收尾：
 * 1. shouldArmAgentReentry：waiting + 优雅期已过才武装；running / 无 turn 边界 /
 *    刚结束 2.5s 内的迟到回声都不武装。
 * 2. sealAgentSegmentForIdle：只封本会话最后一张卡（Reply → durationMs，
 *    Thought → endedAt），已封卡 / 其他会话的尾巴不动 —— 与 turn/complete 的
 *    封口行为一致（中段卡保持 open）。
 * 3. 用 2026-10-24 zcode 真实形状回放：sealed 总结卡之后 80s 出现的续跑
 *    Reply 卡在安静收尾时获得自己的 duration，不被粘进前一张卡。
 *
 * 跑法：npx tsx scripts/verify-agent-reentry.mts
 */

import {
  REENTRY_GRACE_MS,
  sealAgentSegmentForIdle,
  shouldArmAgentReentry,
} from "../src/lib/agentReentry";
import {
  collapseIntermediateAssistantAsThought,
} from "../src/lib/acpTranscript";
import type { SessionEvent } from "../src/lib/types";

let failures = 0;
function check(name: string, ok: boolean, detail = "") {
  if (ok) {
    console.log(`  ok  ${name}`);
  } else {
    failures += 1;
    console.error(`FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
  }
}

// ── 1. 判定 ────────────────────────────────────────────────────────────
const now = 1_800_000_000_000;
check(
  "waiting + turn 结束已久 → 武装",
  shouldArmAgentReentry("waiting", now - 60_000, now),
);
check(
  "running（本回合仍在）→ 不武装",
  !shouldArmAgentReentry("running", now - 60_000, now),
);
check(
  "无 turn 边界记录（冷启动/恢复的会话）→ 不武装",
  !shouldArmAgentReentry("waiting", null, now) &&
    !shouldArmAgentReentry("waiting", undefined, now),
);
check(
  `turn/complete 后 ${REENTRY_GRACE_MS}ms 内的迟到回声 → 不武装`,
  !shouldArmAgentReentry("waiting", now - (REENTRY_GRACE_MS - 100), now),
);
check(
  "恰好过了优雅期 → 武装",
  shouldArmAgentReentry("waiting", now - REENTRY_GRACE_MS, now),
);

// ── 2. 安静收尾封口 ────────────────────────────────────────────────────
const sid = "session-a";
const other = "session-b";
const base: SessionEvent[] = [
  { type: "user_message", sessionId: sid, text: "查一下", createdAt: "2026-10-24T16:16:11.000Z" },
  {
    type: "assistant_message",
    sessionId: sid,
    text: "总结：查完了。",
    createdAt: "2026-10-24T16:17:26.000Z",
    durationMs: 169_923,
    endedAt: "2026-10-24T16:20:16.000Z",
  },
  {
    type: "tool_call",
    sessionId: sid,
    text: "bg scan · completed",
    toolCallId: "bg_1",
    status: "completed",
    title: "bg scan",
    createdAt: "2026-10-24T16:21:10.000Z",
  },
  {
    type: "assistant_message",
    sessionId: sid,
    text: "贴纸扫描跑完了：120/120 全部完成。",
    messageId: "m-cont",
    createdAt: "2026-10-24T16:21:16.000Z",
  },
  // 多窗口共享 rail：别的会话事件排在本会话续跑卡之后。
  { type: "user_message", sessionId: other, text: "别的窗口", createdAt: "2026-10-24T16:21:20.000Z" },
];

const endMs = Date.parse("2026-10-24T16:27:16.000Z");
const sealed = sealAgentSegmentForIdle(base, sid, endMs);
const cont = sealed[3]!;
check("续跑卡获得自己的 duration", cont.type === "assistant_message" && cont.durationMs === 6 * 60_000);
check(
  "续跑卡不吞进已封总结卡",
  sealed[1]!.type === "assistant_message" &&
    sealed[1]!.text === "总结：查完了。" &&
    sealed[1]!.durationMs === 169_923,
);
check(
  "其他会话的尾巴不被封",
  sealed[4]!.type === "user_message" && sealed[4]!.sessionId === other,
);
check("重复收尾是幂等的", sealAgentSegmentForIdle(sealed, sid, endMs + 1000) === sealed);

const thoughtTail: SessionEvent[] = [
  { type: "user_message", sessionId: sid, text: "q", createdAt: "2026-10-24T16:00:00.000Z" },
  { type: "thought", sessionId: sid, text: "想想", createdAt: "2026-10-24T16:21:16.000Z" },
];
const thoughtSealed = sealAgentSegmentForIdle(thoughtTail, sid, endMs);
check(
  "Thought 收尾只补 endedAt",
  thoughtSealed[1]!.type === "thought" && thoughtSealed[1]!.endedAt === "2026-10-24T16:27:16.000Z",
);

// ── 3. zcode 真实形状回放（含 collapse 管线）───────────────────────────
const collapsed = collapseIntermediateAssistantAsThought(sealed, sid);
check(
  "收尾管线不合并跨工具的两张卡",
  collapsed.filter((e) => e.type === "assistant_message").length === 2,
);

if (failures > 0) {
  console.error(`\nverify-agent-reentry: ${failures} failure(s)`);
  process.exit(1);
}
console.log("\nverify-agent-reentry: all checks passed");
