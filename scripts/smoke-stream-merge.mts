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

console.log("smoke-stream-merge: ok");
