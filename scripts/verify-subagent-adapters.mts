/**
 * Per-agent nested-agent adapters (Codeg-aligned).
 *
 * ACP has no subagent type. Each harness smuggles spawn differently, and some
 * never put "subagent" on the wire. We still have to show an independent card.
 *
 * Run: npx tsx scripts/verify-subagent-adapters.mts
 */
import assert from "node:assert/strict";
import {
  applyAcpPartToEvents,
  extractAcpUpdateText,
  userMessageEvent,
  type SessionEvent,
} from "../src/lib/acpTranscript.ts";
import {
  isSubagentTool,
  isSubagentWaitTool,
  parseCodeBuddyTeamMembers,
  parseCodexSubagentActivity,
  recoverSubagentToolName,
} from "../src/lib/subagentAdapters.ts";
import { clusterActivityRuns, isSubagentEvent } from "../src/lib/turnActivity.ts";
import { isLiveSubagentEvent } from "../src/lib/liveSubagents.ts";

let passed = 0;
function check(name: string, fn: () => void) {
  fn();
  passed += 1;
  console.log(`  ok  ${name}`);
}

function toolUpdate(update: Record<string, unknown>) {
  return extractAcpUpdateText({
    params: { update: { sessionUpdate: "tool_call", toolCallId: "c1", ...update } },
  });
}

function toolEvent(
  partial: Partial<Extract<SessionEvent, { type: "tool_call" }>>,
): Extract<SessionEvent, { type: "tool_call" }> {
  return {
    type: "tool_call",
    sessionId: "s",
    text: "",
    createdAt: "2026-10-28T00:00:00.000Z",
    status: "in_progress",
    ...partial,
  };
}

const collabInput = JSON.stringify({
  prompt: "write tests",
  senderThreadId: "parent-thread",
  receiverThreadIds: ["child-thread"],
  agentsStates: { "child-thread": { status: "running", message: null } },
});

console.log("subagent adapters");

check("ZCode Agent title is the task, identity lives in _meta.claudeCode.toolName", () => {
  const part = toolUpdate({
    title: "调研 README 并列出风险",
    kind: "other",
    status: "in_progress",
    _meta: { claudeCode: { toolName: "Agent" } },
    rawInput: { prompt: "调研 README", run_in_background: true },
  });
  assert.equal(part?.toolName, "agent");
  assert.equal(part?.toolTitle, "调研 README 并列出风险");
  assert.equal(isSubagentTool(part?.toolName, part?.toolTitle, part?.toolInput), true);
});

check("ZCode [background] title is a live nested agent even after the spawn tool completes", () => {
  const part = toolUpdate({
    title: "[background] 扫一遍 src/",
    kind: "other",
    status: "completed",
    _meta: { subagent: { id: "ag_1" } },
    content: [{ type: "content", content: { type: "text", text: "working in the background agentId: ag_1" } }],
  });
  assert.equal(part?.toolName, "agent");
  assert.equal(part?.toolStatus, "in_progress");
  let events: SessionEvent[] = [userMessageEvent("s", "go")];
  events = applyAcpPartToEvents(events, "s", part!);
  const card = events.find((e) => e.type === "tool_call");
  assert.ok(card && card.type === "tool_call");
  assert.equal(isLiveSubagentEvent(card), true);
});

check("Codex collab spawnAgent rawInput is a nested agent, wait/closeAgent are not", () => {
  const spawn = toolUpdate({
    title: "spawnAgent",
    kind: "other",
    status: "in_progress",
    rawInput: JSON.parse(collabInput),
  });
  assert.equal(spawn?.toolName, "collab_agent");
  assert.equal(isSubagentTool(spawn?.toolName, spawn?.toolTitle, spawn?.toolInput), true);

  const wait = toolUpdate({
    title: "wait",
    kind: "other",
    status: "in_progress",
    rawInput: JSON.parse(collabInput),
  });
  assert.equal(isSubagentWaitTool(wait?.toolName, wait?.toolTitle), true);
  assert.equal(isSubagentTool(wait?.toolName, wait?.toolTitle, wait?.toolInput), false);

  const close = toolUpdate({
    title: "closeAgent",
    rawInput: JSON.parse(collabInput),
  });
  assert.equal(isSubagentTool(close?.toolName, close?.toolTitle, close?.toolInput), false);

  const send = toolUpdate({
    title: "sendInput",
    rawInput: JSON.parse(collabInput),
  });
  assert.equal(isSubagentTool(send?.toolName, send?.toolTitle, send?.toolInput), false);
});

check("Codex native team-of-agents synthesizes a card from _meta.codex.subagent with no spawn tool", () => {
  const part = extractAcpUpdateText({
    params: {
      update: {
        sessionUpdate: "session_info_update",
        _meta: {
          codex: {
            subagent: {
              threadId: "019f07aa-f57b-4000-8000-000000000001",
              agentPath: "reviewer",
              activityKind: "running",
            },
          },
        },
      },
    },
  });
  assert.equal(part?.toolName, "agent");
  assert.equal(part?.toolTitle, "reviewer");
  assert.equal(part?.toolStatus, "in_progress");
  assert.equal(part?.toolCallId, "019f07aa-f57b-4000-8000-000000000001");
});

check("parseCodexSubagentActivity ignores a goal-only session_info", () => {
  assert.equal(
    parseCodexSubagentActivity({
      sessionUpdate: "session_info_update",
      _meta: { codex: { goal: { objective: "ship it", status: "active" } } },
    }),
    null,
  );
});

check("Hermes delegate_task is a nested agent", () => {
  assert.equal(recoverSubagentToolName({ name: "delegate_task" }), "delegate_task");
  assert.equal(isSubagentTool("delegate_task", "Write the tests"), true);
  const part = toolUpdate({
    name: "delegate_task",
    title: "Write the tests",
    status: "in_progress",
  });
  assert.equal(part?.toolName, "delegate_task");
});

check("OpenCode Task (general) and Grok spawn_subagent still match", () => {
  assert.equal(isSubagentTool("task", "Task (general)"), true);
  assert.equal(isSubagentTool("spawn_subagent", "调研 codeg"), true);
  assert.equal(recoverSubagentToolName({ name: "spawn_subagent" }), "spawn_subagent");
});

check("create_goal is not a nested agent", () => {
  assert.equal(isSubagentTool("create_goal", "Goal updated (active): ship it"), false);
  assert.equal(
    recoverSubagentToolName({
      name: "create_goal",
      title: "Goal updated (active): ship it",
      input: JSON.stringify({ goal: { objective: "ship it", status: "active" } }),
    }),
    null,
  );
});

check("a description title with run_in_background input is still a spawn", () => {
  const input = JSON.stringify({ prompt: "扫一遍 src/", run_in_background: true });
  assert.equal(isSubagentTool("调研 README", "调研 README", input), true);
  assert.equal(recoverSubagentToolName({ title: "调研 README", input }), "agent");
});

check("CodeBuddy teamUpdate members become an agent card", () => {
  const rows = parseCodeBuddyTeamMembers({
    _meta: {
      "codebuddy.ai/teamUpdate": {
        type: "team_updated",
        members: [{ name: "explorer", taskId: "t1", status: "running", description: "scan repo" }],
      },
    },
  });
  assert.equal(rows.length, 1);
  assert.equal(rows[0]!.title, "explorer");
  const part = extractAcpUpdateText({
    params: {
      update: {
        sessionUpdate: "session_info_update",
        _meta: {
          "codebuddy.ai/teamUpdate": {
            members: [{ name: "explorer", taskId: "t1", status: "running", description: "scan repo" }],
          },
        },
      },
    },
  });
  assert.equal(part?.toolName, "agent");
  assert.equal(part?.toolCallId, "t1");
});

check("ZCode description-title Agent stays out of the activity tally", () => {
  const units = [
    {
      key: "read",
      event: toolEvent({ toolName: "read", path: "a.ts", status: "completed" }),
      index: 0,
    },
    {
      key: "agent",
      event: toolEvent({
        toolName: "agent",
        title: "调研 README 并列出风险",
        status: "in_progress",
      }),
      index: 1,
    },
  ];
  assert.equal(isSubagentEvent(units[1]!.event), true);
  const clustered = clusterActivityRuns(units);
  assert.equal(clustered.length, 2);
  assert.equal(clustered[0]!.kind, "activity");
  assert.equal(clustered[1]!.kind, "row");
});

check("unrecovered ZCode card is still a nested agent once input is kept", () => {
  const event = toolEvent({
    toolName: "调研 README 并列出风险",
    title: "调研 README 并列出风险",
    input: JSON.stringify({ prompt: "调研 README", run_in_background: true }),
  });
  assert.equal(isSubagentEvent(event), true);
  assert.equal(isLiveSubagentEvent(event), true);
});

console.log(`\n${passed} checks passed.`);
