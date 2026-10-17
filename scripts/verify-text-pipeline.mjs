/**
 * Presentation/text pipeline smoke checks.
 * Run: npm run verify:text
 *
 * Imports only markdownText.ts (self-contained) so Node's ESM resolver does
 * not need the whole app import graph.
 */
import assert from "node:assert/strict";
import { unified } from "unified";
import remarkGfm from "remark-gfm";
import remarkParse from "remark-parse";
import {
  cleanAssistantText,
  isRuntimeMetadataOnly,
  normalizeMarkdownFences,
  normalizeMarkdownTables,
  prepareMarkdownForRender,
  stripClaudeUsagePanel,
  stripSectionMarkers,
  stripRuntimeMetadata,
} from "../src/lib/markdownText.ts";

assert.equal(
  stripSectionMarkers("§11\n§13§ 替换完成 ✅"),
  "替换完成 ✅",
);
assert.equal(stripSectionMarkers("普通文本 §11 不应被改写"), "普通文本 §11 不应被改写");

const runtimeFooter = [
  "已完成：",
  "",
  "Model: gpt-5.6-luna[max]",
  "Directory: \\?\\D:\\Myself\\Marionette",
  "Approval: never",
  "Sandbox: danger-full-access",
  "Account: ChatGPT plus (user@example.com)",
  "Session: 019fb223-0f8f-7b51-a84e-bced9756d2a3",
  "",
  "Token usage: 4.8M total",
  "Context window: data not available yet",
  "codex Weekly limit: 86% left",
  "codex Credits: 0",
].join("\n");
const markdownRuntimeFooter = [
  "已完成：",
  "",
  "**Model:** gpt-5.6-luna[max]",
  "**Directory:** \\?\\D:\\Myself\\Marionette",
  "**Approval:** never",
  "**Sandbox:** danger-full-access",
  "**Account:** ChatGPT plus (user@example.com)",
  "**Session:** 019fb223-0f8f-7b51-a84e-bced9756d2a3",
  "",
  "**Token usage:** 4.8M total",
  "**Context window:** data not available yet",
  "**codex Weekly limit:** 86% left",
  "**codex Credits:** 0",
].join("\n");
const statusOnlyMarkdown = markdownRuntimeFooter.slice(
  markdownRuntimeFooter.indexOf("**Model:**"),
);
const statusOnlyPlain = runtimeFooter.slice(runtimeFooter.indexOf("Model:"));

assert.equal(stripRuntimeMetadata(runtimeFooter), "已完成：");
assert.equal(stripRuntimeMetadata(markdownRuntimeFooter), "已完成：");
assert.equal(stripRuntimeMetadata("Model: gpt-5\nDirectory: C:\\repo"), "");
assert.equal(
  cleanAssistantText("§11\n§13§ 已完成\n\n" + statusOnlyMarkdown),
  "已完成",
);
assert.equal(isRuntimeMetadataOnly(statusOnlyMarkdown), true);
assert.equal(isRuntimeMetadataOnly(statusOnlyPlain), true);
assert.equal(isRuntimeMetadataOnly(runtimeFooter), false);
assert.equal(isRuntimeMetadataOnly(markdownRuntimeFooter), false);
assert.equal(
  isRuntimeMetadataOnly("Model: this is ordinary prose\n下一行仍是正文"),
  false,
);
assert.equal(
  stripRuntimeMetadata("Model: this is ordinary prose\n下一行仍是正文"),
  "Model: this is ordinary prose\n下一行仍是正文",
);

// Claude Code `/usage` — same policy as Codex `/status`: parse for Usage panel,
// hide from the chat rail so every refresh does not open a Reply card.
const claudeUsageOnly = [
  "You are currently using your subscription to power your Claude Code usage",
  "",
  "Current session: 64% used · resets Aug 8, 4:09am (Asia/Tokyo)",
  "Current week (all models): 17% used · resets Aug 13, 6:59pm (Asia/Tokyo)",
  "",
  "What's contributing to your limits usage?",
  "Approximate, based on local sessions on this machine — does not include other devices or claude.ai. Behaviors are independent characteristics, not a breakdown.",
  "",
  "Last 24h · 279 requests · 6 sessions",
  "65% of your usage was at >150k context",
  "",
  "Last 7d · 331 requests · 7 sessions",
  "58% of your usage was at >150k context",
].join("\n");
const claudeUsageFooter = ["已修好权限逻辑。", "", ...claudeUsageOnly.split("\n")].join("\n");
assert.equal(isRuntimeMetadataOnly(claudeUsageOnly), true);
assert.equal(stripRuntimeMetadata(claudeUsageOnly), "");
assert.equal(cleanAssistantText(claudeUsageOnly), "");
assert.equal(stripRuntimeMetadata(claudeUsageFooter), "已修好权限逻辑。");
assert.equal(isRuntimeMetadataOnly(claudeUsageFooter), false);
// Ordinary prose that merely mentions a % must stay visible.
assert.equal(
  isRuntimeMetadataOnly("约 65% of your usage was at peak last week, so we should trim context."),
  false,
);
assert.equal(
  stripRuntimeMetadata("约 65% of your usage was at peak last week, so we should trim context."),
  "约 65% of your usage was at peak last week, so we should trim context.",
);

// Claude Code v2.1 usage footer — TUI reprints the cost/token table plus its
// status bar at the end of every turn, now prefixed by `Reply` + a
// locale-formatted clock (real transcript, 2026-10-17).
const claudePanelOnly = [
  "Reply",
  "下午10:24",
  "Usage",
  "This session",
  "Cost\tAPI time\tActive",
  "$0.12\t4s\t9s",
  "Breakdown\tTokens",
  "Input\t29.4K",
  "Output\t64",
  "Cache read\t128",
  "Cache write\t0",
  "▣",
  "Bypass permissions",
  "Claude Code",
  "grok/grok-4.5",
  "Max",
].join("\n");
assert.equal(stripClaudeUsagePanel(claudePanelOnly), "");
assert.equal(cleanAssistantText(claudePanelOnly), "");
assert.equal(isRuntimeMetadataOnly(claudePanelOnly), true);
assert.equal(cleanAssistantText("搞定了。\n\n" + claudePanelOnly), "搞定了。");
// v2.0 shape (no Reply/clock prefix) still strips.
const claudePanelV2 = claudePanelOnly.slice(claudePanelOnly.indexOf("Usage"));
assert.equal(stripClaudeUsagePanel(claudePanelV2), "");
// A Reply/clock pair without the panel is not a footer — keep it.
assert.equal(stripClaudeUsagePanel("Reply\n下午10:24"), "Reply\n下午10:24");
// Label and clock collapsed onto one line also belong to the panel.
const claudePanelOneLine = claudePanelOnly.replace("Reply\n下午10:24", "Reply · 下午10:24");
assert.equal(cleanAssistantText(claudePanelOneLine), "");
// Prose clock lines must not be eaten by a stray panel anchor above them.
assert.equal(
  cleanAssistantText("Usage 说明\n会议在 10:24 开始。"),
  "Usage 说明\n会议在 10:24 开始。",
);

// Claude Code ≥2.1 also emits the panel as Markdown in its own message
// (real transcript, 2026-10-17 22:53): headings, hr, GFM tables.
const claudeMdPanelOnly = [
  "## Usage",
  "",
  "---",
  "",
  "### This session",
  "",
  "| Cost | API time | Active |",
  "|:--|:--|:--|",
  "| $0.12 | 12s | 16s |",
  "",
  "| Breakdown | Tokens |",
  "|:--|--:|",
  "| Input | 29.4K |",
  "| Output | 235 |",
  "| Cache read | 576 |",
  "| Cache write | 0 |",
].join("\n");
assert.equal(stripClaudeUsagePanel(claudeMdPanelOnly), "");
assert.equal(cleanAssistantText(claudeMdPanelOnly), "");
assert.equal(isRuntimeMetadataOnly(claudeMdPanelOnly), true);
assert.equal(cleanAssistantText("正常回复正文。\n\n" + claudeMdPanelOnly), "正常回复正文。");
// A real `## Usage` docs heading with prose below must survive.
assert.equal(
  cleanAssistantText("## Usage\n\nCall the API like this:\n\n```bash\ncurl example.com\n```"),
  "## Usage\n\nCall the API like this:\n\n```bash\ncurl example.com\n```",
);
// Plain-text form preceded by its own status bar leaves no orphan glyph.
const claudePanelWithStatusBar = [
  "正文完了。",
  "",
  "▣",
  "Bypass permissions",
  "Claude Code",
  "glm-4.5-air",
  "Max",
  ...claudePanelOnly.split("\n").slice(2),
].join("\n");
assert.equal(cleanAssistantText(claudePanelWithStatusBar), "正文完了。");

// Mimic transcript policy: user text never cleaned; assistant cleaned; empty assistant dropped.
const userText = "保留我的 You 卡片\n\n" + statusOnlyMarkdown;
const assistantWithFooter = "正常回复\n\n" + statusOnlyMarkdown;
const userKept = userText; // never run cleanAssistantText on user
const assistantClean = cleanAssistantText(assistantWithFooter);
const statusOnlyClean = cleanAssistantText(statusOnlyMarkdown);
assert.equal(userKept, userText);
assert.equal(assistantClean, "正常回复");
assert.equal(statusOnlyClean, "");
assert.ok(userKept.includes("**Model:**"), "user paste of status must remain");

const malformedTable = [
  "| 项目 | 第一次 | 第二次 | 现在 |",
  "|------|--------|----------------|",
  "| 大小 | 1,325,568 | 1,291,264 | 1,254,912 |",
].join("\n");
const repairedTable = normalizeMarkdownTables(malformedTable);
const processor = unified().use(remarkParse).use(remarkGfm);
const tree = await processor.run(processor.parse(repairedTable));

assert.equal(tree.children[0]?.type, "table");
assert.equal(tree.children[0]?.children[0]?.children.length, 4);
assert.equal(
  normalizeMarkdownTables("```\n| A | B |\n|---|\n```"),
  "```\n| A | B |\n|---|\n```",
);

// Glued fences (real model output) must open/close correctly.
assert.equal(
  normalizeMarkdownFences('校验要求：```json\n{ "type": "api" }\n```\n修复完成。'),
  '校验要求：\n```json\n{ "type": "api" }\n```\n修复完成。',
);
assert.equal(
  normalizeMarkdownFences("你看到的：```\nReply · Build\n```\n每张卡。"),
  "你看到的：\n```\nReply · Build\n```\n每张卡。",
);
// Closer stuck to last code line
assert.equal(
  normalizeMarkdownFences("```\ncode here```\nafter"),
  "```\ncode here\n```\nafter",
);
// Prose about backticks must not be split into a fence
assert.equal(
  normalizeMarkdownFences("请用 ``` 包裹代码，不要用缩进。"),
  "请用 ``` 包裹代码，不要用缩进。",
);
// Chinese section breaks must not rewrite inside fences
assert.equal(
  prepareMarkdownForRender("能力。一、技术\n\n```\nfoo。一、bar\n```"),
  "能力。\n\n一、技术\n\n```\nfoo。一、bar\n```",
);

const gluedTree = await processor.run(
  processor.parse(prepareMarkdownForRender('校验要求：```json\n{ "a": 1 }\n```\n修复完成。')),
);
assert.equal(gluedTree.children[0]?.type, "paragraph");
assert.equal(gluedTree.children[1]?.type, "code");
assert.equal(gluedTree.children[1]?.lang, "json");
assert.equal(gluedTree.children[2]?.type, "paragraph");

console.log("text pipeline checks passed");
