/**
 * Activity-cluster headline smoke.
 *
 * Run: npx tsx scripts/verify-activity-cluster.mts
 */
import assert from "node:assert/strict";
import type { SessionEvent } from "../src/lib/types.ts";
import {
  clusterActivityRuns,
  activityTimeline,
  formatActivityTally,
  groupConsecutiveByKind,
  summarizeActivity,
  tailActivityFoldKey,
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

check("done headline is a count+type tally", () => {
  const summary = summarizeActivity(items);
  assert.equal(summary.title, "1 write, 3 commands");
  assert.ok(summary.diff);
  assert.equal(summary.diff?.add, 2);
});

check("live preview tracks the current tool", () => {
  const summary = summarizeActivity(items, { liveEvent: items[1].event });
  assert.equal(summary.title, "1 write, 3 commands");
  assert.match(summary.preview, /Ran a command/);
});

check("same-kind run uses a count phrase", () => {
  const reads = [0, 1, 2].map((index) => ({
    event: tool({ toolName: "read", path: `file-${index}.ts` }),
    index,
  }));
  const groups = groupConsecutiveByKind(reads);
  assert.equal(groups.length, 1);
  assert.equal(kindGroupTitle(groups[0]), "Read 3 files");
  assert.equal(summarizeActivity(reads).title, "3 reads");
});

const thought = {
  type: "thought" as const,
  sessionId: "s",
  text: "planning the edit",
  createdAt: "t0",
};

check("parallel spawn_subagent tools fold like any other activity", () => {
  const units = [0, 1, 2, 3, 4, 5, 6].map((index) => ({
    key: `spawn-${index}`,
    event: tool({
      toolName: "spawn_subagent",
      title: `审计模块 ${index}`,
      status: "in_progress",
      toolCallId: `call_${index}`,
    }),
    index,
  }));
  const clustered = clusterActivityRuns(units);
  assert.equal(clustered.length, 1);
  assert.equal(clustered[0].kind, "activity");
  if (clustered[0].kind === "activity") {
    assert.equal(clustered[0].items.length, 7);
  }
});

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
  assert.equal(summary.title, "1 thought, 1 write, 3 commands");
});

check("tally merges the same kind across the fold", () => {
  const mixed = [
    { event: thought, index: 0 },
    { event: tool({ toolName: "read", path: "a.ts" }), index: 1 },
    { event: tool({ toolName: "read", path: "b.ts" }), index: 2 },
    { event: tool({ toolName: "edit", path: "a.ts" }), index: 3 },
    { event: tool({ toolName: "edit", path: "b.ts" }), index: 4 },
    { event: tool({ toolName: "edit", path: "c.ts" }), index: 5 },
  ];
  assert.equal(formatActivityTally(mixed), "1 thought, 2 reads, 3 edits");
  assert.equal(summarizeActivity(mixed).title, "1 thought, 2 reads, 3 edits");
});

check("live thought keeps the tally (status rides on its own step row)", () => {
  const withThought = [{ event: thought, index: 0 }, ...items];
  const summary = summarizeActivity(withThought, { liveEvent: thought });
  assert.equal(summary.title, "1 thought, 1 write, 3 commands");
  assert.equal(summary.preview, "");
});

check("live tool keeps the tally and previews the current step", () => {
  const summary = summarizeActivity(items, { liveEvent: items[1].event });
  assert.equal(summary.title, "1 write, 3 commands");
  assert.match(summary.preview, /Ran a command/);
});

check("a settled run has no live preview", () => {
  const summary = summarizeActivity(items);
  assert.equal(summary.preview, "");
  assert.equal(summary.title, "1 write, 3 commands");
});

check("the first step already has the group's stable key", () => {
  const clustered = clusterActivityRuns([
    { key: "one", event: items[0].event, index: 0 },
  ]);
  assert.equal(clustered.length, 1);
  assert.equal(clustered[0].kind, "activity");
  assert.equal(clustered[0].key, "act-0");
  const growing = clusterActivityRuns([
    { key: "one", event: items[0].event, index: 0 },
    { key: "two", event: items[1].event, index: 1 },
  ]);
  assert.equal(growing[0].key, clustered[0].key);
});

check("the first streaming thought has a growing time bar before any tool arrives", () => {
  const start = Date.parse("2026-10-24T12:00:00Z");
  const first = [{ event: { ...thought, createdAt: new Date(start).toISOString() }, index: 0 }];
  const early = activityTimeline(first, start + 500)!;
  const later = activityTimeline(first, start + 2500)!;
  assert.deepEqual(early.lanes, ["thought"]);
  assert.equal(early.spans[0].open, true);
  assert.equal(early.durationMs, 500);
  assert.equal(later.durationMs, 2500);
  assert.equal(activityTimeline(first)!.spans[0].open, false);
});

check("thought-tool transitions retain both lanes and stop the completed thought", () => {
  const start = Date.parse("2026-10-24T12:00:00Z");
  const steps = [
    { event: { ...thought, createdAt: new Date(start).toISOString() }, index: 0 },
    { event: tool({ toolName: "search", createdAt: new Date(start + 500).toISOString(), status: "in_progress" }), index: 1 },
  ];
  const trace = activityTimeline(steps, start + 1500)!;
  assert.deepEqual(trace.lanes, ["thought", "search"]);
  assert.equal(trace.spans[0].open, false);
  assert.equal(trace.spans[0].end, 500);
  assert.equal(trace.spans[1].open, true);
  assert.equal(trace.spans[1].end, 1500);
});

check("the trailing run is found through delivery cards", () => {
  assert.equal(
    tailActivityFoldKey([
      { kind: "row", key: "you" },
      { kind: "activity", key: "act-2" },
      { kind: "delivery", key: "d-1" },
    ]),
    "act-2",
  );
  assert.equal(
    tailActivityFoldKey([{ kind: "activity", key: "act-1" }, { kind: "row", key: "reply" }]),
    null,
  );
});

console.log(`\n${passed} checks passed`);
