import assert from "node:assert/strict";
import {
  groupMemosByDay,
  memosFromEvents,
  parseTags,
  unwrapCaptureText,
} from "../src/lib/memos.ts";
import type { SessionEvent } from "../src/lib/types.ts";

const wrapped = `新收藏进来了，请整理进知识库：

来源：输入 · 2026-10-23 12:00:00

买菜 #errand

记得带酱油

要求：
1. 判断主题`;

const parsed = unwrapCaptureText(wrapped);
assert.equal(parsed.text, "买菜 #errand\n\n记得带酱油");
assert.equal(parsed.source, "输入 · 2026-10-23 12:00:00");
assert.deepEqual(parseTags(parsed.text), ["errand"]);

const events: SessionEvent[] = [
  {
    type: "user_message",
    sessionId: "s1",
    text: wrapped,
    createdAt: "2026-10-23T04:00:00.000Z",
    messageId: "m1",
  },
  {
    type: "assistant_message",
    sessionId: "s1",
    text: "已归档",
    createdAt: "2026-10-23T04:00:01.000Z",
  },
  {
    type: "user_message",
    sessionId: "s1",
    text: "plain note",
    createdAt: "2026-10-23T05:00:00.000Z",
    messageId: "m2",
  },
];

const memos = memosFromEvents(events);
assert.equal(memos.length, 2);
assert.equal(memos[0].text, "plain note");
assert.equal(memos[1].text, "买菜 #errand\n\n记得带酱油");

const groups = groupMemosByDay(memos, Date.parse("2026-10-23T12:00:00.000Z"));
assert.ok(groups.length >= 1);
assert.equal(groups[0].items.length, 2);

console.log("verify-memos: ok");
