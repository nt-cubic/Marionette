/**
 * Live nested-agent detection: in_progress Task cards and Grok's
 * "started in background" stub both count as still running.
 *
 * Run: npx tsx scripts/verify-live-subagents.mts
 */
import assert from "node:assert/strict";
import type { SessionEvent } from "../src/lib/types.ts";
import {
  isLiveSubagentEvent,
  listLiveSubagents,
  sessionHasLiveSubagents,
  sessionProcessAlive,
} from "../src/lib/liveSubagents.ts";

let passed = 0;
function check(name: string, fn: () => void) {
  fn();
  passed += 1;
  console.log(`  ok  ${name}`);
}

function tool(
  partial: Partial<Extract<SessionEvent, { type: "tool_call" }>>,
): Extract<SessionEvent, { type: "tool_call" }> {
  return {
    type: "tool_call",
    sessionId: "s",
    text: "",
    createdAt: "2026-10-28T00:00:00.000Z",
    status: "completed",
    ...partial,
  };
}

console.log("live nested agents");
check("OpenCode task in_progress is live", () => {
  const event = tool({ toolName: "task", title: "Task (general)", status: "in_progress" });
  assert.equal(isLiveSubagentEvent(event), true);
});
check("Grok background stub is live even when the tool card completed", () => {
  const event = tool({
    toolName: "spawn_subagent",
    title: "调研 codeg",
    status: "completed",
    detail: "Subagent started in background\nsubagent_id: abc123\ndescription: 调研 codeg",
  });
  assert.equal(isLiveSubagentEvent(event), true);
  assert.equal(isLiveSubagentEvent(event, { processAlive: false }), false);
});
check("finished spawn is not live", () => {
  const event = tool({
    toolName: "spawn_subagent",
    title: "调研 codeg",
    status: "completed",
    detail: "research notes\nsubagent_id: abc123",
  });
  assert.equal(isLiveSubagentEvent(event), false);
});
check("read tools are never nested agents", () => {
  assert.equal(isLiveSubagentEvent(tool({ toolName: "read", status: "in_progress" })), false);
});
check("listLiveSubagents returns titles for the background panel", () => {
  const events: SessionEvent[] = [
    tool({
      toolName: "spawn_subagent",
      title: "审计模块 A",
      status: "in_progress",
      toolCallId: "c1",
    }),
    tool({ toolName: "read", path: "a.ts", status: "in_progress" }),
  ];
  const rows = listLiveSubagents(events, "s");
  assert.equal(rows.length, 1);
  assert.equal(rows[0]!.title, "审计模块 A");
  assert.equal(sessionHasLiveSubagents(events, "s"), true);
});
check("parked / exited dialogs are not process-alive", () => {
  assert.equal(sessionProcessAlive("waiting"), true);
  assert.equal(sessionProcessAlive("running"), true);
  assert.equal(sessionProcessAlive("exited"), false);
});

console.log(`\n${passed} checks passed.`);
