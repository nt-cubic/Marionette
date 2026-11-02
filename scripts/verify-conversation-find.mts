/**
 * verify-conversation-find — 对话内查找(lib/conversationFind.ts)
 *
 * 1. searchableText:主干卡(你/回复/计划/提问/交接/子代理/改动)可搜,
 *    thought/tool_call 返回 null(折叠簇内,v1 不搜)。
 * 2. findConversationMatches:大小写不敏感子串、查询 trim、空查询/无命中
 *    返回空数组、命中下标升序、附件字段(重命名旧路径/裁决行/选中项)参与匹配。
 *
 * 跑法:npx tsx scripts/verify-conversation-find.mts
 */

import { findConversationMatches, searchableText } from "../src/lib/conversationFind";
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

const AT = "2026-11-01T09:00:00Z";

const events: SessionEvent[] = [
  { type: "user_message", sessionId: "s", text: "帮我修复 UTF-8 截断的崩溃", createdAt: AT },
  { type: "thought", sessionId: "s", text: "先看 terminal_runtime 的 drain 逻辑", createdAt: AT },
  { type: "tool_call", sessionId: "s", text: "读取 terminal_runtime.rs", toolName: "read", createdAt: AT },
  {
    type: "assistant_message",
    sessionId: "s",
    text: "问题出在 `drain(..overflow)` 落在多字节字符中间。修复方案是回退到字符边界。",
    createdAt: AT,
  },
  { type: "file_change", sessionId: "s", path: "src-tauri/src/terminal_runtime.rs", oldPath: "src-tauri/src/terminal.rs", changeType: "modified", createdAt: AT },
  {
    type: "ask_prompt",
    sessionId: "s",
    requestId: "r1",
    questions: [{ question: "要加回归脚本吗?", options: [{ label: "加 verify 脚本" }, { label: "先不加" }] }],
    status: "answered",
    answers: [{ question: "要加回归脚本吗?", selected: ["加 verify 脚本"] }],
    createdAt: AT,
  },
];

// ── 1. searchableText 类型范围 ─────────────────────────────────────────
check("你/回复可搜", searchableText(events[0]!) != null && searchableText(events[3]!) != null);
check("thought/tool 不搜(折叠簇内)", searchableText(events[1]!) === null && searchableText(events[2]!) === null);
check("改动卡含重命名旧路径", searchableText(events[4]!)!.includes("terminal.rs"));
check("提问卡含问题与选项与选中项", searchableText(events[5]!)!.includes("回归脚本"));

// ── 2. findConversationMatches ─────────────────────────────────────────
check("空查询返回空", findConversationMatches(events, "  ").length === 0);
check("大小写不敏感且跳过不可搜类型", JSON.stringify(findConversationMatches(events, "DRAIN")) === "[3]", JSON.stringify(findConversationMatches(events, "DRAIN")));
const hits = findConversationMatches(events, "修复");
check("命中下标升序且跨卡", JSON.stringify(hits) === "[0,3]", JSON.stringify(hits));
check("子串即命中(与浏览器查找同语义)", findConversationMatches(events, "utf-8").includes(0));
check("无命中返回空", findConversationMatches(events, "不存在的词").length === 0);
check("旧路径可搜", findConversationMatches(events, "src/terminal.rs").includes(4));
check("选项文本可搜", findConversationMatches(events, "verify 脚本").includes(5));
check("空白查询只按 trim 后判断", findConversationMatches(events, "   修复   ").includes(0));

console.log(failures === 0 ? "\n全部通过" : `\n${failures} 项失败`);
process.exit(failures === 0 ? 0 : 1);
