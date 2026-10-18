/**
 * ACP render budget: paintSet, cheap-path classification, shadow flush.
 *
 * Run: npx tsx scripts/verify-acp-render-budget.mts
 */
import assert from "node:assert/strict";
import {
  applyAcpPartToEvents,
  coalesceAdjacentThoughts,
  type AcpTextPart,
  type SessionEvent,
} from "../src/lib/acpTranscript.ts";
import {
  chunkKindFromRole,
  flushShadowsIntoPaintSet,
  paintSet,
  shouldPublishChunk,
  unfinishedDelegateChildrenFromMeta,
} from "../src/lib/acpRenderBudget.ts";

let passed = 0;
function check(name: string, fn: () => void) {
  fn();
  passed += 1;
  console.log(`  ok  ${name}`);
}

const thought = (text: string): AcpTextPart => ({
  role: "thought",
  text,
  isDelta: true,
  sessionUpdate: "agent_thought_chunk",
});
const message = (text: string): AcpTextPart => ({
  role: "assistant",
  text,
  isDelta: true,
  sessionUpdate: "agent_message_chunk",
});
const tool = (id: string): AcpTextPart => ({
  role: "tool",
  text: "",
  isDelta: false,
  sessionUpdate: "tool_call",
  toolCallId: id,
  toolName: "read",
  toolStatus: "pending",
});

console.log("acp render budget");

check("paintSet is just the visible session when there are no children", () => {
  const set = paintSet({
    visibleSessionIds: ["focus"],
    unfinishedDelegateChildren: [],
  });
  assert.deepEqual([...set].sort(), ["focus"]);
});

check("paintSet includes unfinished @-delegate children of a visible parent", () => {
  const set = paintSet({
    visibleSessionIds: ["parent"],
    unfinishedDelegateChildren: [
      { childId: "c1", parentId: "parent" },
      { childId: "c2", parentId: "parent" },
      { childId: "other", parentId: "hidden" },
    ],
  });
  assert.ok(set.has("parent"));
  assert.ok(set.has("c1"));
  assert.ok(set.has("c2"));
  assert.equal(set.has("other"), false);
});

check("hidden tabs are not in paintSet", () => {
  const set = paintSet({
    visibleSessionIds: ["focus"],
    unfinishedDelegateChildren: [],
  });
  assert.equal(set.has("hidden-tab"), false);
});

check("shouldPublishChunk: focus is full; hidden thought is shadow-only; hidden tool is now", () => {
  const paint = new Set(["focus"]);
  assert.equal(shouldPublishChunk("focus", "thought", paint), "now");
  assert.equal(shouldPublishChunk("focus", "message", paint), "now");
  assert.equal(shouldPublishChunk("hidden-tab", "thought", paint), "shadow-only");
  assert.equal(shouldPublishChunk("hidden-tab", "message", paint), "shadow-only");
  assert.equal(shouldPublishChunk("hidden-tab", "meta", paint), "now");
});

check("child of a painted parent publishes now", () => {
  const paint = paintSet({
    visibleSessionIds: ["parent"],
    unfinishedDelegateChildren: [{ childId: "c1", parentId: "parent" }],
  });
  assert.equal(shouldPublishChunk("c1", "thought", paint), "now");
  assert.equal(shouldPublishChunk("c1", "message", paint), "now");
});

check("chunkKindFromRole maps thought/assistant/tool", () => {
  assert.equal(chunkKindFromRole("thought"), "thought");
  assert.equal(chunkKindFromRole("assistant"), "message");
  assert.equal(chunkKindFromRole("tool"), "meta");
  assert.equal(chunkKindFromRole("system"), "meta");
});

check("activate parent flushes parent + two unfinished children", () => {
  const shadow = new Map<string, SessionEvent[]>();
  const published = new Map<string, SessionEvent[]>();
  shadow.set("parent", [{ type: "thought", sessionId: "parent", text: "p", createdAt: "t" }]);
  shadow.set("c1", [{ type: "thought", sessionId: "c1", text: "1", createdAt: "t" }]);
  shadow.set("c2", [{ type: "thought", sessionId: "c2", text: "2", createdAt: "t" }]);
  const next = paintSet({
    visibleSessionIds: ["parent"],
    unfinishedDelegateChildren: [
      { childId: "c1", parentId: "parent" },
      { childId: "c2", parentId: "parent" },
    ],
  });
  const flushed = flushShadowsIntoPaintSet(next, shadow, published);
  assert.deepEqual(flushed.sort(), ["c1", "c2", "parent"]);
  assert.equal(published.get("parent"), shadow.get("parent"));
  assert.equal(published.get("c1"), shadow.get("c1"));
  assert.equal(published.get("c2"), shadow.get("c2"));
});

check("same-batch thought then permission: flush puts thought on published", () => {
  const shadow = new Map<string, SessionEvent[]>();
  const published = new Map<string, SessionEvent[]>();
  const sid = "hidden";
  let paint = paintSet({ visibleSessionIds: ["focus"], unfinishedDelegateChildren: [] });
  assert.equal(shouldPublishChunk(sid, "thought", paint), "shadow-only");
  const nextThought = applyAcpPartToEvents([], sid, thought("plan…"));
  shadow.set(sid, nextThought);
  // permission steals focus → session enters paintSet mid-batch
  paint = paintSet({ visibleSessionIds: [sid], unfinishedDelegateChildren: [] });
  const flushed = flushShadowsIntoPaintSet(paint, shadow, published);
  assert.deepEqual(flushed, [sid]);
  assert.equal(published.get(sid), nextThought);
  assert.equal(published.get(sid)?.[0]?.type, "thought");
});

check("same apply wrapper keeps a tool between two thoughts (cheap path must not concat)", () => {
  const sid = "s";
  const parts: AcpTextPart[] = [
    thought("aaa"),
    tool("t1"),
    thought("bbb"),
    message("done"),
  ];
  let rail: SessionEvent[] = [];
  for (const part of parts) {
    rail = applyAcpPartToEvents(rail, sid, part);
    if (part.role === "thought" || part.role === "assistant") {
      rail = coalesceAdjacentThoughts(rail, sid);
    }
  }
  assert.deepEqual(
    rail.map((e) => e.type),
    ["thought", "tool_call", "thought", "assistant_message"],
  );
  assert.equal(rail.filter((e) => e.type === "thought" && e.text === "aaabbb").length, 0);
});

check("tool is never deferred even when the session is hidden", () => {
  const paint = new Set(["focus"]);
  assert.equal(shouldPublishChunk("hidden", chunkKindFromRole("tool"), paint), "now");
});

check("unfinishedDelegateChildrenFromMeta skips finished children", () => {
  const meta = new Map<string, { parentId: string; finished: boolean }>([
    ["c1", { parentId: "p", finished: false }],
    ["c2", { parentId: "p", finished: true }],
  ]);
  const kids = unfinishedDelegateChildrenFromMeta(meta);
  assert.deepEqual(kids, [{ childId: "c1", parentId: "p" }]);
});

console.log(`\n${passed} checks passed`);
