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
