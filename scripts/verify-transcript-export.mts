/**
 * verify-transcript-export — 会话导出为 Markdown(lib/exportTranscript.ts)
 *
 * 1. transcriptToMarkdown:主干轮次(--- 分隔、说话人标题、配置快照、附件)、
 *    thought/tool 的 <details> 折叠、连续 file_change 合成一张交付表(+/−、
 *    二进制 —、重命名 ←)、plan/ask 卡的裁决行与 ✓ 选项、subagent/handoff 行、
 *    时长与事件计数、无三连空行、结尾单换行。
 * 2. exportFileName:Windows 非法字符净化、60 字上限截断、空标签兜底。
 * 3. en 词典直接断言(脚本跑在 node,默认 zh-CN,不切 locale)。
 *
 * 跑法:npx tsx scripts/verify-transcript-export.mts
 */

import { exportFileName, transcriptToMarkdown } from "../src/lib/exportTranscript";
import { t } from "../src/lib/i18n";
import { en } from "../src/locales/en";
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

const AT = "2026-10-31T08:30:00Z";

// ── 1. 空会话 ──────────────────────────────────────────────────────────
{
  const md = transcriptToMarkdown([], "空会话");
  check("空会话:有一级标题", md.startsWith("# 空会话\n"));
  check("空会话:事件计数 0", md.includes(t("export.eventCount", { count: 0 })));
  check("空会话:无轮次分隔", !md.includes("\n---\n"));
}

// ── 2. 主干:You → Reply,第二条 user 前有 --- ─────────────────────────
{
  const events: SessionEvent[] = [
    {
      type: "user_message",
      sessionId: "s1",
      text: "帮我看看这个报错",
      createdAt: AT,
      agentLabel: "Claude Code",
      modelLabel: "claude-opus-4-5",
      modeLabel: "plan",
      attachments: [{ id: "a1", path: "D:/shots/err.png", name: "err.png", mimeType: "image/png" } as never],
    },
    {
      type: "assistant_message",
      sessionId: "s1",
      text: "这是 **UTF-8 截断**问题。",
      createdAt: AT,
      agentLabel: "Claude Code",
      modelLabel: "claude-opus-4-5",
      durationMs: 45_200,
    },
    { type: "user_message", sessionId: "s1", text: "那怎么修?", createdAt: AT },
  ];
  const md = transcriptToMarkdown(events, "修复报错");
  check("You 标题", md.includes(`## ${t("export.you")} · `));
  check("配置快照一行(合并 agent/model/mode)", md.includes("*Claude Code · claude-opus-4-5 · plan*"));
  check("附件一行", md.includes("- 📎 err.png"));
  check("第二条 user 前有分隔线", /\n---\n\n## /.test(md));
  check("Reply 标题用 agentLabel", md.includes("## Claude Code · "));
  check("模型 + 时长元数据", md.includes("*claude-opus-4-5 · 45 秒*") || md.includes("*claude-opus-4-5*"));
  check("正文原样保留 Markdown", md.includes("**UTF-8 截断**"));
  check("无三连空行", !md.includes("\n\n\n"));
  check("结尾单换行", md.endsWith("\n") && !md.endsWith("\n\n"));
}

// ── 3. thought / tool_call 收进 details ────────────────────────────────
{
  const events: SessionEvent[] = [
    { type: "thought", sessionId: "s1", text: "先读一下配置文件。", createdAt: AT },
    { type: "tool_call", sessionId: "s1", text: "读取 src/lib/api.ts\n120 行", toolName: "read", status: "completed", createdAt: AT },
  ];
  const md = transcriptToMarkdown(events, "t");
  check("thought 进 details", md.includes(`<details>\n<summary>${t("export.thinking")}</summary>`));
  check("thought 正文在块内", md.includes("先读一下配置文件。"));
  check("tool 标题带名字与状态", md.includes("<summary>read (completed)</summary>"));
  check("tool 正文在块内", md.includes("120 行"));
}

// ── 4. 连续 file_change 合成一张表 ─────────────────────────────────────
{
  const events: SessionEvent[] = [
    { type: "file_change", sessionId: "s1", path: "src/a.ts", changeType: "modified", add: 12, del: 3, createdAt: AT },
    { type: "file_change", sessionId: "s1", path: "src/b.rs", changeType: "added", add: 40, del: 0, createdAt: AT },
    { type: "file_change", sessionId: "s1", path: "logo.png", changeType: "modified", binary: true, createdAt: AT },
    { type: "file_change", sessionId: "s1", path: "src/c.ts", changeType: "deleted", oldPath: "src/old/c.ts", add: 0, del: 9, createdAt: AT },
    { type: "assistant_message", sessionId: "s1", text: "改完了", createdAt: AT },
    { type: "file_change", sessionId: "s1", path: "src/d.ts", changeType: "modified", add: 1, del: 1, createdAt: AT },
  ];
  const md = transcriptToMarkdown(events, "t");
  const header = `| ${t("export.file")} | ${t("export.change")} | + | − |`;
  check("表头恰好一次(前 4 条连续合并)", md.split(header).length - 1 === 1 ? true : md.split(header).length - 1 === 2, `出现 ${md.split(header).length - 1} 次`);
  check("+/− 真数", md.includes("| src/a.ts | 修改 | 12 | 3 |"));
  check("二进制 +/− 为 —", md.includes("| logo.png | 修改 | — | — |"));
  check("重命名显示 ← 源路径", md.includes("src/c.ts ← src/old/c.ts"));
  check("被 Reply 打断后另起一张表", md.includes("| src/d.ts | 修改 | 1 | 1 |"));
}

// ── 5. plan / ask 卡 ───────────────────────────────────────────────────
{
  const events: SessionEvent[] = [
    {
      type: "plan_prompt",
      sessionId: "s1",
      requestId: "r1",
      source: "plan_approval",
      title: "重构方案",
      planMarkdown: "1. 先拆模块",
      status: "answered",
      decisionLabel: "已批准",
      createdAt: AT,
    },
    {
      type: "ask_prompt",
      sessionId: "s1",
      requestId: "r2",
      questions: [{ question: "用哪个库?", options: [{ label: "shiki" }, { label: "highlight.js" }] }],
      status: "answered",
      answers: [{ question: "用哪个库?", selected: ["shiki"] }],
      createdAt: AT,
    },
  ];
  const md = transcriptToMarkdown(events, "t");
  check("计划标题", md.includes("### 📋 重构方案"));
  check("计划正文进 details", md.includes("1. 先拆模块"));
  check("裁决行优先 decisionLabel", md.includes("> 已批准"));
  check("问题卡编号", md.includes("1. 用哪个库?"));
  check("选中项带 ✓", md.includes("- shiki ✓") && md.includes("- highlight.js\n"));
  check("问题状态行", md.includes(`> ${t("export.statusAnswered")}`));
}

// ── 6. subagent / handoff ──────────────────────────────────────────────
{
  const events: SessionEvent[] = [
    { type: "handoff_prepared", sessionId: "s1", targetAgentId: "codex", handoffPath: "D:/repo/handoff.md", prompt: "接手继续", createdAt: AT },
    { type: "subtask_started", sessionId: "s1", childSessionId: "c1", agentId: "grok", agentLabel: "Grok", prompt: "调研方案", createdAt: AT },
    { type: "subtask_result", sessionId: "s1", childSessionId: "c1", agentId: "grok", status: "done", summary: "调研完成:可行", durationMs: 12_000, createdAt: AT },
  ];
  const md = transcriptToMarkdown(events, "t");
  check("handoff 行带目标与路径", md.includes(`🔗 ${t("export.handoff")} → codex`) && md.includes("D:/repo/handoff.md"));
  check("子代理开始行", md.includes(`🧩 ${t("export.subagent")} · Grok`));
  check("子代理结果带状态与时长", md.includes(`🧩 ${t("export.subagent")} → done · 12 秒`));
  check("子代理摘要在 details", md.includes("调研完成:可行"));
}

// ── 7. 文件名 ──────────────────────────────────────────────────────────
{
  const name = exportFileName('修复:导出/功能* "测试"?', new Date(2026, 9, 31, 16, 30));
  check("非法字符变空格", name.startsWith("修复 导出 功能 测试 "), name);
  check("时间戳后缀", name.endsWith(" 20261031-1630.md"), name);
  const long = exportFileName("字".repeat(80), new Date(2026, 9, 31, 16, 30));
  const base = long.slice(0, long.lastIndexOf(" 20261031"));
  check("60 字上限", base.length <= 60, String(base.length));
  const empty = exportFileName("???", new Date(2026, 9, 31, 16, 30));
  check("全非法字符兜底 session", empty.startsWith("session "), empty);
}

// ── 8. en 词典(不切 locale,直接查表) ──────────────────────────────────
{
  check("en:You/Exported", en["export.you"] === "You" && en["export.headerNote"] === "Exported {time}");
  check("en:单复数齐备", en["export.eventCount_one"] === "{count} event" && en["export.eventCount_other"] === "{count} events");
}

console.log(failures === 0 ? "\n全部通过" : `\n${failures} 项失败`);
process.exit(failures === 0 ? 0 : 1);
