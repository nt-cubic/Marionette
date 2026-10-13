/**
 * Stream-merge smoke tests:
 * - Grok Reply: one token per agent_message_chunk with rotating messageId
 * - Codex Thought: whitespace/punctuation tokens must not be dropped
 * - Multi-session interleave: concurrent windows must not split Reply/Thought
 * Run: npx tsx scripts/smoke-stream-merge.mts
 */
import assert from "node:assert/strict";
import {
  applyAcpPartToEvents,
  coalesceAdjacentAssistantFragments,
  coalesceAdjacentThoughts,
  extractAcpUpdateText,
  userMessageEvent,
  type SessionEvent,
} from "../src/lib/acpTranscript.ts";
import { isWorkspaceDiffPath, looksLikeUnifiedDiff } from "../src/lib/toolBody.ts";
import { extractDiffStats } from "../src/lib/turnActivity.ts";

let events: SessionEvent[] = [
  userMessageEvent("s1", "hi", {
    agentId: "grok-build",
    agentLabel: "Grok Build",
    modelLabel: "Grok 4.5",
    modeLabel: "Build",
  }),
];

const chunks = ["两个", "问题", "都修了", "。", "Always", " ", "approve"];
for (let i = 0; i < chunks.length; i++) {
  events = applyAcpPartToEvents(events, "s1", {
    role: "assistant",
    text: chunks[i],
    isDelta: true,
    sessionUpdate: "agent_message_chunk",
    messageId: `msg-${i}`,
  });
}

const replies = events.filter((e) => e.type === "assistant_message");
assert.equal(replies.length, 1, `expected 1 Reply, got ${replies.length}`);
assert.equal(replies[0].text, chunks.join(""));
// Header meta only on the single card
assert.equal(replies[0].agentLabel, "Grok Build");

// Already-split fragments (mid-turn mess) still glue
let broken: SessionEvent[] = [
  userMessageEvent("s1", "x"),
  {
    type: "assistant_message",
    sessionId: "s1",
    text: "假",
    createdAt: new Date().toISOString(),
  },
  {
    type: "assistant_message",
    sessionId: "s1",
    text: "开",
    createdAt: new Date().toISOString(),
  },
  {
    type: "assistant_message",
    sessionId: "s1",
    text: "着",
    createdAt: new Date().toISOString(),
  },
];
broken = coalesceAdjacentAssistantFragments(broken, "s1");
const r2 = broken.filter((e) => e.type === "assistant_message");
assert.equal(r2.length, 1);
assert.equal(r2[0].text, "假开着");

// Sealed reply must NOT absorb the next turn's stream
let sealed: SessionEvent[] = [
  userMessageEvent("s1", "a"),
  {
    type: "assistant_message",
    sessionId: "s1",
    text: "done",
    durationMs: 100,
    createdAt: new Date().toISOString(),
  },
];
sealed = applyAcpPartToEvents(sealed, "s1", {
  role: "assistant",
  text: "next",
  isDelta: true,
  sessionUpdate: "agent_message_chunk",
  messageId: "other",
});
assert.equal(sealed.filter((e) => e.type === "assistant_message").length, 2);

// Codex-style thought stream: spaces + punctuation as their own chunks
const thoughtTokens = [
  "Comparing",
  " ",
  "duplicate",
  " ",
  "engine",
  " ",
  "files",
  " ",
  "and",
  " ",
  "verifying",
  " ",
  "candidates",
  ".",
  " ",
  "Verifying",
  " ",
  "candidate",
  " ",
  "file",
  " ",
  "existence",
  " ",
  "and",
  " ",
  "hashing",
  " ",
  "duplicates",
  ".",
];
let thoughtEvents: SessionEvent[] = [userMessageEvent("s2", "check dups")];
for (let i = 0; i < thoughtTokens.length; i++) {
  const part = extractAcpUpdateText({
    params: {
      update: {
        sessionUpdate: "agent_thought_chunk",
        messageId: `th-${i}`,
        content: { type: "text", text: thoughtTokens[i] },
      },
    },
  });
  assert.ok(part, `thought token ${i} (${JSON.stringify(thoughtTokens[i])}) must extract`);
  thoughtEvents = applyAcpPartToEvents(thoughtEvents, "s2", part);
}
const thoughts = thoughtEvents.filter((e) => e.type === "thought");
assert.equal(thoughts.length, 1, `expected 1 Thought card, got ${thoughts.length}`);
assert.equal(
  thoughts[0].text,
  thoughtTokens.join(""),
  "thought stream must keep spaces and periods",
);

// Bare space extract must not be null (regression guard for trim-drop)
const spacePart = extractAcpUpdateText({
  params: {
    update: {
      sessionUpdate: "agent_thought_chunk",
      messageId: "sp",
      content: { type: "text", text: " " },
    },
  },
});
assert.ok(spacePart);
assert.equal(spacePart!.text, " ");
assert.equal(spacePart!.role, "thought");

// ── Multi-session interleave (multi-window concurrent streams) ────────────
// Shared liveEvents rail: session A token, session B token, A token, B token…
// Old bug: only absolute last event was mergeable → one Reply card per token.
const aChunks = ["砍到了", "**", " —— ", "A2_32k", " 块", " 1"];
const bChunks = ["hello", " ", "world", "!"];
let multi: SessionEvent[] = [
  userMessageEvent("sess-A", "go", {
    agentId: "opencode",
    agentLabel: "OpenCode",
    modelLabel: "DeepSeek V4 Flash",
    modeLabel: "plan",
  }),
  userMessageEvent("sess-B", "hi", {
    agentId: "opencode",
    agentLabel: "OpenCode",
    modelLabel: "DeepSeek V4 Flash",
    modeLabel: "plan",
  }),
];
const maxLen = Math.max(aChunks.length, bChunks.length);
for (let i = 0; i < maxLen; i++) {
  if (i < aChunks.length) {
    multi = applyAcpPartToEvents(multi, "sess-A", {
      role: "assistant",
      text: aChunks[i],
      isDelta: true,
      sessionUpdate: "agent_message_chunk",
      messageId: `a-${i}`,
    });
  }
  if (i < bChunks.length) {
    multi = applyAcpPartToEvents(multi, "sess-B", {
      role: "assistant",
      text: bChunks[i],
      isDelta: true,
      sessionUpdate: "agent_message_chunk",
      messageId: `b-${i}`,
    });
  }
}
const aReplies = multi.filter(
  (e) => e.type === "assistant_message" && e.sessionId === "sess-A",
);
const bReplies = multi.filter(
  (e) => e.type === "assistant_message" && e.sessionId === "sess-B",
);
assert.equal(
  aReplies.length,
  1,
  `multi-session A: expected 1 Reply, got ${aReplies.length} (fragments: ${aReplies.map((r) => JSON.stringify(r.text)).join("|")})`,
);
assert.equal(
  bReplies.length,
  1,
  `multi-session B: expected 1 Reply, got ${bReplies.length}`,
);
assert.equal(aReplies[0].text, aChunks.join(""));
assert.equal(bReplies[0].text, bChunks.join(""));
assert.equal(aReplies[0].agentLabel, "OpenCode");

// Coalesce cleanup: already-split fragments with foreign session between them
let interleavedBroken: SessionEvent[] = [
  userMessageEvent("sess-A", "x"),
  {
    type: "assistant_message",
    sessionId: "sess-A",
    text: "窄",
    createdAt: new Date().toISOString(),
  },
  {
    type: "assistant_message",
    sessionId: "sess-B",
    text: "noise",
    createdAt: new Date().toISOString(),
  },
  {
    type: "assistant_message",
    sessionId: "sess-A",
    text: "窗口",
    createdAt: new Date().toISOString(),
  },
  {
    type: "thought",
    sessionId: "sess-B",
    text: "other",
    createdAt: new Date().toISOString(),
  },
  {
    type: "assistant_message",
    sessionId: "sess-A",
    text: "guard",
    createdAt: new Date().toISOString(),
  },
];
interleavedBroken = coalesceAdjacentAssistantFragments(interleavedBroken, "sess-A");
const aGlued = interleavedBroken.filter(
  (e) => e.type === "assistant_message" && e.sessionId === "sess-A",
);
assert.equal(aGlued.length, 1, `coalesce across foreign sessions: expected 1, got ${aGlued.length}`);
assert.equal(aGlued[0].text, "窄窗口guard");
// Foreign session's Reply must remain
assert.equal(
  interleavedBroken.filter((e) => e.sessionId === "sess-B" && e.type === "assistant_message")
    .length,
  1,
);

// Thought interleave (avoid repeated 1-char tails — mergeStreamText treats
// previous.endsWith(incoming) as already-applied and would drop a real "想"+"想")
let thoughtMulti: SessionEvent[] = [userMessageEvent("tA", "a"), userMessageEvent("tB", "b")];
const tA = ["先", "想", "一下"];
const tB = ["also", " ", "thinking"];
for (let i = 0; i < 3; i++) {
  thoughtMulti = applyAcpPartToEvents(thoughtMulti, "tA", {
    role: "thought",
    text: tA[i],
    isDelta: true,
    sessionUpdate: "agent_thought_chunk",
    messageId: `ta-${i}`,
  });
  thoughtMulti = applyAcpPartToEvents(thoughtMulti, "tB", {
    role: "thought",
    text: tB[i],
    isDelta: true,
    sessionUpdate: "agent_thought_chunk",
    messageId: `tb-${i}`,
  });
}
assert.equal(
  thoughtMulti.filter((e) => e.type === "thought" && e.sessionId === "tA").length,
  1,
);
assert.equal(
  thoughtMulti.filter((e) => e.type === "thought" && e.sessionId === "tB").length,
  1,
);
assert.equal(
  thoughtMulti.find((e) => e.type === "thought" && e.sessionId === "tA")!.text,
  "先想一下",
);
assert.equal(
  thoughtMulti.find((e) => e.type === "thought" && e.sessionId === "tB")!.text,
  "also thinking",
);
// Coalesce thoughts across foreign interleave
let thoughtBroken: SessionEvent[] = [
  { type: "thought", sessionId: "tA", text: "甲", createdAt: new Date().toISOString() },
  { type: "thought", sessionId: "tB", text: "x", createdAt: new Date().toISOString() },
  { type: "thought", sessionId: "tA", text: "乙", createdAt: new Date().toISOString() },
];
thoughtBroken = coalesceAdjacentThoughts(thoughtBroken, "tA");
assert.equal(thoughtBroken.filter((e) => e.sessionId === "tA" && e.type === "thought").length, 1);
assert.equal(
  thoughtBroken.find((e) => e.sessionId === "tA" && e.type === "thought")!.text,
  "甲\n乙",
);

// Same-session tool between Replies must still block coalesce
let toolBetween: SessionEvent[] = [
  {
    type: "assistant_message",
    sessionId: "s1",
    text: "before",
    createdAt: new Date().toISOString(),
  },
  {
    type: "tool_call",
    sessionId: "s1",
    text: "read",
    toolCallId: "t1",
    createdAt: new Date().toISOString(),
  },
  {
    type: "assistant_message",
    sessionId: "s1",
    text: "after",
    createdAt: new Date().toISOString(),
  },
];
toolBetween = coalesceAdjacentAssistantFragments(toolBetween, "s1");
assert.equal(
  toolBetween.filter((e) => e.type === "assistant_message").length,
  2,
  "tool between same-session Replies must keep two cards",
);

console.log("notices stay off the transcript");
{
  const noticePart = extractAcpUpdateText({
    params: {
      update: {
        sessionUpdate: "notice",
        severity: "warning",
        title: "MCP server unavailable",
        description: "Continuing without it.",
      },
    },
  });
  assert.equal(noticePart, null, "protocol notice must not become a chat card");

  const retryPart = extractAcpUpdateText({
    params: {
      update: {
        sessionUpdate: "session_info_update",
        _meta: { codex: { error: { willRetry: true, message: "rate limited" } } },
      },
    },
  });
  assert.equal(retryPart, null, "Codex retry must not become a Reply");

  const compactPart = extractAcpUpdateText({
    params: {
      update: {
        sessionUpdate: "compaction_update",
        compaction: { trigger: "auto", summary: "kept the plan" },
      },
    },
  });
  assert.equal(compactPart?.role, "system");
  assert.match(compactPart!.text, /kept the plan/);
}

console.log("OpenCode v2 child-session streams stay off the parent Reply");
{
  const childMeta = {
    "opencode/child-session": {
      id: "ses_child_explore",
      parentID: "ses_parent",
      depth: 1,
      title: "explore Notion (@explore subagent)",
    },
  };

  const childPart = extractAcpUpdateText({
    params: {
      update: {
        sessionUpdate: "agent_message_chunk",
        messageId: "msg_child",
        content: { type: "text", text: "正在看 Notion" },
        _meta: childMeta,
      },
    },
  });
  assert.equal(childPart?.role, "tool", "child text must nest in a tool card");
  assert.equal(childPart?.toolCallId, "ses_child_explore");
  assert.equal(childPart?.toolDetailAppend, true);
  assert.equal(childPart?.toolName, "task");

  const childThought = extractAcpUpdateText({
    params: {
      update: {
        sessionUpdate: "agent_thought_chunk",
        content: { type: "text", text: "先搜目录" },
        _meta: childMeta,
      },
    },
  });
  assert.equal(childThought?.role, "tool", "child thought must not open a Thought card");

  const parentPart = extractAcpUpdateText({
    params: {
      update: {
        sessionUpdate: "agent_message_chunk",
        content: { type: "text", text: "我来拆任务" },
      },
    },
  });
  assert.equal(parentPart?.role, "assistant");

  let mixed: SessionEvent[] = [userMessageEvent("s1", "go")];
  mixed = applyAcpPartToEvents(mixed, "s1", parentPart!);
  for (const token of ["正在", "看", " Notion"]) {
    mixed = applyAcpPartToEvents(
      mixed,
      "s1",
      extractAcpUpdateText({
        params: {
          update: {
            sessionUpdate: "agent_message_chunk",
            content: { type: "text", text: token },
            _meta: childMeta,
          },
        },
      })!,
    );
  }
  const replies = mixed.filter((e) => e.type === "assistant_message");
  assert.equal(replies.length, 1, "parent Reply must stay a single card");
  assert.equal(replies[0].text, "我来拆任务");
  const tools = mixed.filter((e) => e.type === "tool_call");
  assert.equal(tools.length, 1);
  assert.equal(tools[0].detail, "正在看 Notion");
  assert.doesNotMatch(tools[0].detail ?? "", /\n正在\n/);

  // One in-progress Task absorbs the child stream (Codeg-style nesting).
  let nested: SessionEvent[] = [
    userMessageEvent("s1", "go"),
    {
      type: "tool_call",
      sessionId: "s1",
      text: "task",
      toolCallId: "task_1",
      toolName: "task",
      title: "Task (explore)",
      status: "in_progress",
      createdAt: new Date().toISOString(),
    },
  ];
  nested = applyAcpPartToEvents(nested, "s1", childPart!);
  const taskCards = nested.filter((e) => e.type === "tool_call");
  assert.equal(taskCards.length, 1, "child stream should land on the open Task");
  assert.equal(taskCards[0].toolCallId, "task_1");
  assert.match(taskCards[0].detail ?? "", /正在看 Notion/);
  const childDone = extractAcpUpdateText({
    method: "opencode/session/child_update",
    params: {
      rootSessionId: "ses_parent",
      childSessionId: "ses_child_explore",
      parentSessionId: "ses_parent",
      depth: 1,
      title: "explore Notion (@explore subagent)",
      type: "status",
      status: "completed",
    },
  });
  assert.equal(childDone?.toolStatus, "completed");
  nested = applyAcpPartToEvents(nested, "s1", childDone!);
  assert.equal(nested.filter((e) => e.type === "tool_call").length, 1);
  assert.equal(
    nested.find((e) => e.type === "tool_call")?.status,
    "in_progress",
    "child completed must not settle the parent Task",
  );

  // Parallel Tasks: each child keeps its own card.
  let parallel: SessionEvent[] = [
    userMessageEvent("s1", "go"),
    {
      type: "tool_call",
      sessionId: "s1",
      text: "task a",
      toolCallId: "task_a",
      toolName: "task",
      title: "Task (explore)",
      status: "in_progress",
      createdAt: new Date().toISOString(),
    },
    {
      type: "tool_call",
      sessionId: "s1",
      text: "task b",
      toolCallId: "task_b",
      toolName: "task",
      title: "Task (general)",
      status: "in_progress",
      createdAt: new Date().toISOString(),
    },
  ];
  const otherChild = extractAcpUpdateText({
    params: {
      update: {
        sessionUpdate: "agent_message_chunk",
        content: { type: "text", text: "噩梦关卡" },
        _meta: {
          "opencode/child-session": {
            id: "ses_child_boss",
            parentID: "ses_parent",
            depth: 1,
            title: "噩梦 (@general subagent)",
          },
        },
      },
    },
  })!;
  parallel = applyAcpPartToEvents(parallel, "s1", childPart!);
  parallel = applyAcpPartToEvents(parallel, "s1", otherChild);
  const parallelTools = parallel.filter((e) => e.type === "tool_call");
  assert.equal(parallelTools.length, 4, "two Tasks + two child cards");
  assert.equal(
    parallelTools.find((e) => e.type === "tool_call" && e.toolCallId === "ses_child_explore")?.detail,
    "正在看 Notion",
  );
  assert.equal(
    parallelTools.find((e) => e.type === "tool_call" && e.toolCallId === "ses_child_boss")?.detail,
    "噩梦关卡",
  );

  const claudePart = extractAcpUpdateText({
    params: {
      update: {
        sessionUpdate: "agent_message_chunk",
        content: { type: "text", text: "nested agent text" },
        _meta: { claudeCode: { parentToolUseId: "toolu_agent" } },
      },
    },
  });
  assert.equal(claudePart?.role, "tool");
  assert.equal(claudePart?.toolCallId, "toolu_agent");
  assert.equal(claudePart?.toolName, "agent");

  const childUpdateStatus = extractAcpUpdateText({
    method: "opencode/session/child_update",
    params: {
      rootSessionId: "ses_parent",
      childSessionId: "ses_child_explore",
      parentSessionId: "ses_parent",
      depth: 1,
      title: "explore Notion (@explore subagent)",
      type: "status",
      status: "running",
    },
  });
  assert.equal(childUpdateStatus?.role, "tool");
  assert.equal(childUpdateStatus?.toolCallId, "ses_child_explore");
  assert.equal(childUpdateStatus?.toolStatus, "in_progress");

  const childUpdateChunk = extractAcpUpdateText({
    method: "opencode/session/child_update",
    params: {
      rootSessionId: "ses_parent",
      childSessionId: "ses_child_explore",
      parentSessionId: "ses_parent",
      depth: 1,
      type: "update",
      update: {
        sessionUpdate: "agent_message_chunk",
        content: { type: "text", text: "via child_update" },
        _meta: childMeta,
      },
    },
  });
  assert.equal(childUpdateChunk?.role, "tool");
  assert.equal(childUpdateChunk?.toolCallId, "ses_child_explore");
  assert.equal(childUpdateChunk?.toolDetail, "via child_update");
}

// OpenCode write: file body in rawInput.content, completed is a success note.
{
  const writePath = "C:\\Users\\c_huang\\AppData\\Local\\Temp\\opencode\\hud_probe\\BuildAnim.cs";
  const fileBody = "using UnityEngine;\nclass BuildAnim {}\n";
  let writeEvents: SessionEvent[] = [userMessageEvent("s1", "write the file")];

  const pending = extractAcpUpdateText({
    params: {
      update: {
        sessionUpdate: "tool_call",
        toolCallId: "call_write_1",
        name: "write",
        title: "write",
        kind: "edit",
        status: "pending",
        locations: [],
        rawInput: {},
      },
    },
  });
  assert.equal(pending?.role, "tool");
  writeEvents = applyAcpPartToEvents(writeEvents, "s1", pending!);

  const inProgress = extractAcpUpdateText({
    params: {
      update: {
        sessionUpdate: "tool_call_update",
        toolCallId: "call_write_1",
        status: "in_progress",
        kind: "edit",
        title: "write",
        locations: [{ path: writePath }],
        rawInput: { content: fileBody },
      },
    },
  });
  assert.equal(inProgress?.role, "tool");
  assert.equal(inProgress?.toolInput, "", "rawInput JSON must not be the card body");
  assert.ok(inProgress?.toolDetail && looksLikeUnifiedDiff(inProgress.toolDetail), "write synthesizes a unified diff");
  assert.ok(inProgress.toolDetail.includes("+using UnityEngine;"));
  assert.ok(inProgress.toolDetail.includes("+class BuildAnim {}"));
  const liveStats = extractDiffStats(inProgress.toolDetail);
  assert.equal(liveStats?.add, 2);
  assert.equal(liveStats?.del, 0);
  writeEvents = applyAcpPartToEvents(writeEvents, "s1", inProgress!);

  const completed = extractAcpUpdateText({
    params: {
      update: {
        sessionUpdate: "tool_call_update",
        toolCallId: "call_write_1",
        status: "completed",
        locations: [{ path: writePath }],
        content: [
          {
            type: "content",
            content: { type: "text", text: `Created file successfully: ${writePath.replace(/\\/g, "/")}` },
          },
        ],
        rawOutput: { metadata: { truncated: false } },
      },
    },
  });
  writeEvents = applyAcpPartToEvents(writeEvents, "s1", completed!);
  const writeCard = writeEvents.find((e) => e.type === "tool_call" && e.toolCallId === "call_write_1");
  assert.ok(writeCard && writeCard.type === "tool_call");
  assert.ok(writeCard.detail && looksLikeUnifiedDiff(writeCard.detail), "completed must keep the synthesized diff");
  assert.equal(writeCard.detail.includes("Created file successfully"), false, "success note must not replace the diff");
  const doneStats = extractDiffStats(writeCard.detail);
  assert.equal(doneStats?.add, 2);
  assert.equal(doneStats?.del, 0);

  // Real ACP type:diff wins over rawInput.content.
  const acpDiff = extractAcpUpdateText({
    params: {
      update: {
        sessionUpdate: "tool_call_update",
        toolCallId: "call_edit_1",
        name: "edit",
        kind: "edit",
        status: "in_progress",
        locations: [{ path: "src/a.ts" }],
        rawInput: { content: "SHOULD_NOT_APPEAR" },
        content: [{ type: "diff", path: "src/a.ts", oldText: "a\n", newText: "b\n" }],
      },
    },
  });
  assert.ok(acpDiff?.toolDetail && looksLikeUnifiedDiff(acpDiff.toolDetail));
  assert.equal(acpDiff.toolDetail.includes("SHOULD_NOT_APPEAR"), false);
  assert.ok(acpDiff.toolDetail.includes("-a"));
  assert.ok(acpDiff.toolDetail.includes("+b"));

  assert.equal(isWorkspaceDiffPath("src/a.ts", "D:\\Myself\\Marionette"), true);
  assert.equal(isWorkspaceDiffPath("D:\\Myself\\Marionette\\src\\a.ts", "D:\\Myself\\Marionette"), true);
  assert.equal(
    isWorkspaceDiffPath("C:\\Users\\c_huang\\AppData\\Local\\Temp\\opencode\\hud_probe\\BuildAnim.cs", "D:\\Myself\\Marionette"),
    false,
  );
  assert.equal(isWorkspaceDiffPath("C:\\Users\\c_huang\\AppData\\Local\\Temp\\x.cs", null), false);
}

console.log("smoke-stream-merge: ok");
