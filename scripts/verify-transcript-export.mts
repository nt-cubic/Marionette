/**
 * Transcript Markdown export smoke.
 *
 * Run: npx tsx scripts/verify-transcript-export.mts
 */
import assert from "node:assert/strict";
import { applyLocale } from "../src/lib/i18n.ts";
import {
  exportFileName,
  transcriptToMarkdown,
} from "../src/lib/exportTranscript.ts";
import type { SessionEvent } from "../src/lib/types.ts";

let passed = 0;
function check(name: string, fn: () => void) {
  fn();
  passed += 1;
  console.log(`  ok  ${name}`);
}

const at = new Date(2026, 9, 31, 16, 30, 0);

function user(partial: Partial<Extract<SessionEvent, { type: "user_message" }>>): Extract<SessionEvent, { type: "user_message" }> {
  return {
    type: "user_message",
    sessionId: "s",
    text: "hello",
    createdAt: "2026-10-31T08:30:00.000Z",
    ...partial,
  };
}

function assistant(partial: Partial<Extract<SessionEvent, { type: "assistant_message" }>>): Extract<SessionEvent, { type: "assistant_message" }> {
  return {
    type: "assistant_message",
    sessionId: "s",
    text: "world",
    createdAt: "2026-10-31T08:30:05.000Z",
    ...partial,
  };
}

function thought(text: string): Extract<SessionEvent, { type: "thought" }> {
  return {
    type: "thought",
    sessionId: "s",
    text,
    createdAt: "2026-10-31T08:30:02.000Z",
  };
}

function tool(partial: Partial<Extract<SessionEvent, { type: "tool_call" }>>): Extract<SessionEvent, { type: "tool_call" }> {
  return {
    type: "tool_call",
    sessionId: "s",
    text: "ok",
    createdAt: "2026-10-31T08:30:03.000Z",
    toolName: "read",
    status: "completed",
    ...partial,
  };
}

function fileChange(
  partial: Partial<Extract<SessionEvent, { type: "file_change" }>> & { path: string },
): Extract<SessionEvent, { type: "file_change" }> {
  return {
    type: "file_change",
    sessionId: "s",
    changeType: "modified",
    createdAt: "2026-10-31T08:30:06.000Z",
    add: 3,
    del: 1,
    ...partial,
  };
}

await applyLocale("zh-CN");

console.log("transcript export");

check("file name strips Windows-illegal chars and stamps", () => {
  const name = exportFileName('修复导出:*?<>|"/\\', at);
  assert.equal(name, "修复导出 20261031-1630.md");
});

check("empty label falls back to session.md stamp", () => {
  assert.equal(exportFileName("   ", at), "session 20261031-1630.md");
});

check("long labels cap at 60 chars before the stamp", () => {
  const name = exportFileName("a".repeat(80), at);
  assert.equal(name, `${"a".repeat(60)} 20261031-1630.md`);
});

const events: SessionEvent[] = [
  user({ text: "请改 src/a.ts", agentLabel: "Grok", modelLabel: "grok-4" }),
  thought("先读文件"),
  thought(""),
  tool({ toolName: "read", title: "Read a.ts", text: "contents of a.ts" }),
  assistant({ text: "改好了", agentLabel: "Grok", durationMs: 4200 }),
  fileChange({ path: "src/a.ts", add: 4, del: 2 }),
  fileChange({ path: "src/b.ts", changeType: "added", add: 10, del: 0 }),
  user({ text: "第二轮" }),
  assistant({ text: "收到" }),
];

const md = transcriptToMarkdown(events, "修复导出");

check("title and counts land in the header", () => {
  assert.match(md, /^# 修复导出\n/);
  assert.match(md, /9 条事件/);
  assert.match(md, /4 条发言/);
});

check("user and assistant stay on the main rail", () => {
  assert.match(md, /## 你 · /);
  assert.match(md, /请改 src\/a\.ts/);
  assert.match(md, /\*Grok · grok-4\*/);
  assert.match(md, /## Grok · /);
  assert.match(md, /改好了/);
  assert.match(md, /\*4 秒\*/);
  assert.match(md, /## 助手 · /);
});

check("thought and tool go into details folds", () => {
  assert.match(md, /<details>\n<summary>思考<\/summary>\n\n先读文件/);
  assert.match(md, /<details>\n<summary>read \(completed\)<\/summary>\n\ncontents of a\.ts/);
});

check("empty thought does not emit an empty details block", () => {
  const folds = md.match(/<summary>思考<\/summary>/g) ?? [];
  assert.equal(folds.length, 1);
});

check("consecutive file_change rows merge into one table", () => {
  const tables = md.match(/\*\*本轮改动\*\*/g) ?? [];
  assert.equal(tables.length, 1);
  assert.match(md, /\| src\/a\.ts \| 修改 \| 4 \| 2 \|/);
  assert.match(md, /\| src\/b\.ts \| 新增 \| 10 \| 0 \|/);
});

check("later user messages start a new turn with a rule", () => {
  assert.match(md, /\n---\n\n## 你 · /);
  assert.match(md, /第二轮/);
});

console.log(`\n${passed} checks passed`);
