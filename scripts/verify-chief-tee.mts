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

console.log("verify-chief-tee: all assertions passed");
process.exit(0);
