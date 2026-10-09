/**
 * Activity-cluster headline smoke.
 *
 * Run: npx tsx scripts/verify-activity-cluster.mts
 */
import assert from "node:assert/strict";
import type { SessionEvent } from "../src/lib/types.ts";
import {
  clusterActivityRuns,
  groupConsecutiveByKind,
  summarizeActivity,
  kindGroupTitle,
} from "../src/lib/turnActivity.ts";

let passed = 0;
function check(name: string, fn: () => void) {
  fn();
  passed += 1;
  console.log(`  ok  ${name}`);
}

function tool(
  partial: Partial<Extract<SessionEvent, { type: "tool_call" }>> & {
    toolName?: string;
    path?: string;
    input?: string;
    detail?: string;
  },
): Extract<SessionEvent, { type: "tool_call" }> {
  return {
    type: "tool_call",
    sessionId: "s",
    text: "",
    createdAt: "t",
    status: "completed",
    ...partial,
  };
}

const items = [
  { event: tool({ toolName: "write", path: "HudGammaBlend.cs", detail: "+\n+\n" }), index: 0 },
  { event: tool({ toolName: "bash", input: '{"command":"grep defaultMaterial"}' }), index: 1 },
  { event: tool({ toolName: "bash", input: '{"command":"ls"}' }), index: 2 },
  { event: tool({ toolName: "bash", input: '{"command":"cat x"}' }), index: 3 },
];

console.log("activity cluster headlines");
check("groups write then 3 commands", () => {
  const groups = groupConsecutiveByKind(items);
  assert.equal(groups.length, 2);
  assert.equal(groups[0].bucket, "write");
  assert.equal(groups[1].bucket, "command");
  assert.equal(groups[1].items.length, 3);
  assert.equal(kindGroupTitle(groups[0]), "Wrote HudGammaBlend.cs");
  assert.equal(kindGroupTitle(groups[1]), "Ran 3 commands");
});

check("mixed headline joins like Claude/Codex", () => {
  const summary = summarizeActivity(items);
  assert.equal(summary.title, "Wrote HudGammaBlend.cs, ran 3 commands");
  assert.ok(summary.diff);
  assert.equal(summary.diff?.add, 2);
});

check("live headline tracks the current tool", () => {
  const summary = summarizeActivity(items, { liveEvent: items[1].event });
  assert.match(summary.title, /Ran a command/);
});

check("same-kind run uses a count phrase", () => {
  const reads = [0, 1, 2].map((index) => ({
    event: tool({ toolName: "read", path: `file-${index}.ts` }),
    index,
  }));
  const groups = groupConsecutiveByKind(reads);
  assert.equal(groups.length, 1);
  assert.equal(kindGroupTitle(groups[0]), "Read 3 files");
  assert.equal(summarizeActivity(reads).title, "Read 3 files");
});

const thought = {
  type: "thought" as const,
  sessionId: "s",
  text: "planning the edit",
  createdAt: "t0",
};

check("thought sits in the same cluster as following tools", () => {
  const units = [
    { key: "thought", event: thought, index: 0 },
    ...items.map((item, i) => ({
      key: `tool-${i}`,
      event: item.event,
      index: item.index + 1,
    })),
    {
      key: "reply",
      event: {
        type: "assistant_message" as const,
        sessionId: "s",
        text: "done",
        createdAt: "t9",
      },
      index: 99,
    },
  ];
  const clustered = clusterActivityRuns(units);
  assert.equal(clustered.length, 2);
  assert.equal(clustered[0].kind, "activity");
  if (clustered[0].kind === "activity") {
    assert.equal(clustered[0].items.length, 5);
    assert.equal(clustered[0].items[0].event.type, "thought");
    assert.equal(clustered[0].key, "act-0");
  }
  assert.equal(clustered[1].kind, "row");
});

check("mixed headline includes thought", () => {
  const withThought = [{ event: thought, index: 0 }, ...items.map((item, i) => ({ ...item, index: i + 1 }))];
  const groups = groupConsecutiveByKind(withThought);
  assert.equal(groups[0].bucket, "thought");
  const summary = summarizeActivity(withThought);
  assert.equal(summary.title, "Thought, wrote HudGammaBlend.cs, ran 3 commands");
});

check("live thought headline is Thinking…", () => {
  const withThought = [{ event: thought, index: 0 }, ...items];
  const summary = summarizeActivity(withThought, { liveEvent: thought });
  assert.equal(summary.title, "Thinking…");
});

check("a single tool stays its own row", () => {
  const clustered = clusterActivityRuns([
    { key: "one", event: items[0].event, index: 0 },
  ]);
  assert.equal(clustered.length, 1);
  assert.equal(clustered[0].kind, "row");
});

console.log(`\n${passed} checks passed`);
