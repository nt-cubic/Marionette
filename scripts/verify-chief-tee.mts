/**
 * chief tee 回放辅助的回归（scripts/verify-chief-tee.mts）。
 *
 * 覆盖：游标过滤 / maxSeq / 来源判定 / user echo 合并与 flush 时机。
 * 运行：npx tsx scripts/verify-chief-tee.mts
 */
import assert from "node:assert/strict";

import {
  teeEventsAfterCursor,
  maxSeqOf,
  isChiefOrigin,
  mergeUserEcho,
  shouldFlushUserEcho,
  uncoveredTeeEvents,
  transcriptMaxCreatedAtMs,
} from "../src/lib/chiefTee.ts";

function teeEvent(seq: number, kind: string, method: string | null, data: unknown) {
  return { seq, ts: 1700000000000 + seq, sessionId: "session_w", kind: "status", method, data };
}

// 1. 游标过滤：严格大于，坏 seq 丢弃
{
  const events = [
    teeEvent(1, "status", "session/update", {}),
    teeEvent(2, "status", "session/update", {}),
    teeEvent(3, "status", "session/update", {}),
  ];
  const after = teeEventsAfterCursor(events, 1);
  assert.deepEqual(after.map((e) => e.seq), [2, 3], "严格大于游标");
  assert.deepEqual(teeEventsAfterCursor(events, 0).map((e) => e.seq), [1, 2, 3]);
  const bad = [{ seq: Number.NaN }, { seq: undefined } as never, ...events];
  assert.equal(teeEventsAfterCursor(bad, 0).length, 3, "坏 seq 不进回放");
}

// 2. maxSeq：空批回 floor
{
  assert.equal(maxSeqOf([], 7), 7);
  const events = [teeEvent(4, "status", "session/update", {}), teeEvent(9, "status", "session/update", {})];
  assert.equal(maxSeqOf(events, 1), 9);
}

// 3. 来源判定
{
  assert.equal(isChiefOrigin("chief"), true);
  assert.equal(isChiefOrigin("chief-worker"), true);
  assert.equal(isChiefOrigin("memo"), false);
  assert.equal(isChiefOrigin("user"), false);
  assert.equal(isChiefOrigin(null), false);
  assert.equal(isChiefOrigin(undefined), false);
}

// 4. user echo 合并：增量追加、累计替换、退化保护
{
  assert.equal(mergeUserEcho("", { text: "帮我修", isDelta: true }), "帮我修");
  assert.equal(mergeUserEcho("帮我修", { text: "构建", isDelta: true }), "帮我修构建");
  assert.equal(mergeUserEcho("帮我修", { text: "帮我修构建", isDelta: false }), "帮我修构建");
  assert.equal(mergeUserEcho("很长的上文", { text: "x", isDelta: false }), "很长的上文", "退化保护");
  assert.equal(mergeUserEcho("", { text: "", isDelta: true }), "");
}

// 5. flush 时机：agent 活动立即 flush；继续收 user 不 flush
{
  assert.equal(shouldFlushUserEcho("session/update", "agent_message_chunk"), true);
  assert.equal(shouldFlushUserEcho("session/update", "tool_call"), true);
  assert.equal(shouldFlushUserEcho("session/update", "user_message_chunk"), false);
  assert.equal(shouldFlushUserEcho("session/update", ""), false, "空 kind 不判 flush");
  assert.equal(shouldFlushUserEcho("turn/complete", undefined), true);
  assert.equal(shouldFlushUserEcho("process/ended", undefined), true);
  assert.equal(shouldFlushUserEcho(null, undefined), false);
}

// 6. 未覆盖跨度过滤：ts 晚于转录最新 createdAt 的才回填；缺 ts 保守保留
{
  const transcriptMax = transcriptMaxCreatedAtMs([
    { createdAt: "2026-11-15T10:00:00.000Z" },
    { createdAt: "2026-11-15T10:00:05.000Z" },
  ]);
  assert.equal(transcriptMax, Date.parse("2026-11-15T10:00:05.000Z"));
  const events = [
    { seq: 1, ts: Date.parse("2026-11-15T10:00:04.000Z") }, // 转录已覆盖
    { seq: 2, ts: Date.parse("2026-11-15T10:00:05.000Z") }, // 恰等于最新 → 视为覆盖
    { seq: 3, ts: Date.parse("2026-11-15T10:00:09.000Z") }, // 未覆盖 → 回填
    { seq: 4 }, // 缺 ts → 保守保留
  ];
  const uncovered = uncoveredTeeEvents(events, transcriptMax);
  assert.deepEqual(
    uncovered.map((e) => e.seq),
    [3, 4],
  );
  // 全覆盖 → 空批次（不重放 UI 自己写过的时间段）
  assert.deepEqual(uncoveredTeeEvents(events.slice(0, 2), transcriptMax), []);
}

console.log("verify-chief-tee: all assertions passed");
process.exit(0);
