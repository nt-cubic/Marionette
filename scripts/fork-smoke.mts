/**
 * Fork smoke test.
 *
 * Run: npx tsx scripts/fork-smoke.mts
 *
 * A fork copies the dialog up to one reply. Getting the cut wrong either
 * duplicates the wrong turn or drops context, so the anchor matching and the
 * dropped rows are pinned here.
 */
import assert from "node:assert/strict";
import { forkCopyEvents, forkCutIndex } from "../src/lib/sessionFork.ts";
import type { SessionEvent } from "../src/lib/types.ts";

let passed = 0;
function check(name: string, fn: () => void) {
  fn();
  passed += 1;
  console.log(`  ok  ${name}`);
}

const user = (text: string, messageId: string, sessionId = "s1"): SessionEvent => ({
  type: "user_message",
  sessionId,
  text,
  messageId,
  createdAt: "2026-10-08T10:00:00.000Z",
});

const reply = (text: string, messageId: string | undefined, sessionId = "s1"): SessionEvent => ({
  type: "assistant_message",
  sessionId,
  text,
  messageId,
  createdAt: "2026-10-08T10:00:05.000Z",
});

const CONVERSATION: SessionEvent[] = [
  user("第一问", "u1"),
  reply("第一答", "a1"),
  user("第二问", "u2"),
  reply("第二答", "a2"),
  user("第三问", "u3"),
  reply("第三答", "a3"),
];

console.log("cutting at the clicked reply");
check("a message id picks its own reply", () => {
  assert.equal(forkCutIndex(CONVERSATION, { text: "第一答", messageId: "a2" }), 3);
});
check("with no id, the last reply that reads the same wins", () => {
  const repeated = [...CONVERSATION, reply("第一答", undefined)];
  assert.equal(forkCutIndex(repeated, { text: "第一答" }), repeated.length - 1);
});
check("an anchor that is gone cuts nothing", () => {
  assert.equal(forkCutIndex(CONVERSATION, { text: "没说过的话", messageId: "nope" }), -1);
});
check("a stale id does not fall back to text", () => {
  assert.equal(forkCutIndex(CONVERSATION, { text: "第一答", messageId: "gone" }), -1);
});

console.log("\nwhat the copy keeps");
check("everything up to the reply travels, re-keyed", () => {
  const copied = forkCopyEvents(CONVERSATION, { text: "第二答", messageId: "a2" }, "fork");
  assert.ok(copied);
  assert.deepEqual(
    copied.map((event) => (event.type === "user_message" || event.type === "assistant_message" ? event.text : event.type)),
    ["第一问", "第一答", "第二问", "第二答"],
  );
  assert.equal(copied.every((event) => event.sessionId === "fork"), true);
});
check("the original keeps its own event objects", () => {
  forkCopyEvents(CONVERSATION, { text: "第一答", messageId: "a1" }, "fork");
  assert.equal(CONVERSATION[1].sessionId, "s1");
  assert.equal(CONVERSATION[1].type === "assistant_message" && CONVERSATION[1].text, "第一答");
});
check("subtask cards and handoffs stay behind", () => {
  const withChildren: SessionEvent[] = [
    user("第一问", "u1"),
    reply("第一答", "a1"),
    {
      type: "subtask_started",
      sessionId: "s1",
      childSessionId: "c1",
      agentId: "claude-code",
      agentLabel: "Claude",
      prompt: "子任务",
      createdAt: "2026-10-08T10:00:06.000Z",
    },
    {
      type: "subtask_result",
      sessionId: "s1",
      childSessionId: "c1",
      agentId: "claude-code",
      status: "done",
      summary: "做完了",
      createdAt: "2026-10-08T10:00:07.000Z",
    },
    {
      type: "handoff_prepared",
      sessionId: "s1",
      targetAgentId: "codex",
      handoffPath: "D:\\x\\handoff.md",
      prompt: "接手",
      createdAt: "2026-10-08T10:00:08.000Z",
    },
  ];
  const copied = forkCopyEvents(withChildren, { text: "第一答", messageId: "a1" }, "fork");
  assert.ok(copied);
  assert.deepEqual(
    copied.map((event) => event.type),
    ["user_message", "assistant_message"],
  );
});
check("an anchor that is gone produces no dialog", () => {
  assert.equal(forkCopyEvents(CONVERSATION, { text: "x", messageId: "gone" }, "fork"), null);
});

console.log(`\n${passed} checks passed.`);
