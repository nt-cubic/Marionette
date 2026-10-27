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
check("ZCode [background] completed card stays live while the process is up", () => {
  const event = tool({
    toolName: "agent",
    title: "[background] 扫一遍 src/",
    status: "completed",
    detail: "working in the background agentId: ag_9",
  });
  assert.equal(isLiveSubagentEvent(event), true);
  assert.equal(isLiveSubagentEvent(event, { processAlive: false }), false);
});
check("Codex collab spawnAgent in_progress is live; wait is not", () => {
  const input = JSON.stringify({
    prompt: "write tests",
    senderThreadId: "p",
    receiverThreadIds: ["c"],
    agentsStates: { c: { status: "running" } },
  });
  assert.equal(
    isLiveSubagentEvent(
      tool({ toolName: "collab_agent", title: "spawnAgent", status: "in_progress", input }),
    ),
    true,
  );
  assert.equal(
    isLiveSubagentEvent(tool({ toolName: "wait", title: "wait", status: "in_progress", input })),
    false,
  );
});
check("Hermes delegate_task in_progress is live", () => {
  assert.equal(
    isLiveSubagentEvent(tool({ toolName: "delegate_task", title: "Write the tests", status: "in_progress" })),
    true,
  );
});
check("create_goal is never a live nested agent", () => {
  assert.equal(
    isLiveSubagentEvent(
      tool({
        toolName: "create_goal",
        title: "Goal updated (active): ship it",
        status: "in_progress",
      }),
    ),
    false,
  );
});

console.log(`\n${passed} checks passed.`);
