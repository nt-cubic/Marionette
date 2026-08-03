/**
 * Grok streams one token per agent_message_chunk with a rotating messageId.
 * Without open-stream merge, each token becomes its own Reply card.
 * Run: npx tsx scripts/smoke-stream-merge.mts
 */
import assert from "node:assert/strict";
import {
  applyAcpPartToEvents,
  coalesceAdjacentAssistantFragments,
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

console.log("smoke-stream-merge: ok");
