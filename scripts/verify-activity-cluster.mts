/**
 * Activity-cluster headline smoke.
 *
 * Run: npx tsx scripts/verify-activity-cluster.mts
 */
import assert from "node:assert/strict";
import type { SessionEvent } from "../src/lib/types.ts";
import {
  clusterActivityRuns,
  activityRunUntilMs,
  activitySpanLayout,
  activitySpanWidthPct,
  activityTimeline,
  formatActivityTally,
  groupConsecutiveByKind,
  LIVE_AXIS_FLOOR_MS,
  liveAxisMs,
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

check("live axis grows left-to-right on the 60s floor, then compresses", () => {
  const start = Date.parse("2026-10-24T12:00:00Z");
  const first = [{ event: { ...thought, createdAt: new Date(start).toISOString() }, index: 0 }];

  const at1s = activityTimeline(first, start + 1_000)!;
  const axis1 = liveAxisMs(at1s.durationMs, LIVE_AXIS_FLOOR_MS);
  assert.equal(axis1, LIVE_AXIS_FLOOR_MS);
  const layout1 = activitySpanLayout(at1s.spans[0]!, axis1);
  assert.equal(layout1.leftPct, 0);
  assert.ok(
    layout1.widthPct < 5,
    `1s thought must be a short left bar on the 60s ruler, got ${layout1.widthPct}%`,
  );

  const at30s = activityTimeline(first, start + 30_000)!;
  const layout30 = activitySpanLayout(at30s.spans[0]!, liveAxisMs(at30s.durationMs, LIVE_AXIS_FLOOR_MS));
  assert.ok(Math.abs(layout30.widthPct - 50) < 0.5, `30s thought should be ~50% of 60s, got ${layout30.widthPct}%`);
  assert.ok(layout30.leftPct + layout30.widthPct < 51, "right edge stays at the playhead, not the track end");

  const at90s = activityTimeline(first, start + 90_000)!;
  assert.equal(liveAxisMs(at90s.durationMs, LIVE_AXIS_FLOOR_MS), 90_000);
  const layout90 = activitySpanLayout(at90s.spans[0]!, liveAxisMs(at90s.durationMs, LIVE_AXIS_FLOOR_MS));
  assert.ok(Math.abs(layout90.widthPct - 100) < 0.5, `past the floor the bar fills the axis, got ${layout90.widthPct}%`);
});

check("a mid-run thought keeps empty track to its right while under the floor", () => {
  // The 10s-floor screenshot: tools for ~5s, then thought running to t=10s.
  // On a 10s ruler that thought glues to the right edge. On 60s it sits near 16%.
  const start = Date.parse("2026-10-24T12:00:00Z");
  const steps = [
    {
      event: tool({
        toolName: "read",
        path: "a.ts",
        createdAt: new Date(start).toISOString(),
        completedAt: new Date(start + 400).toISOString(),
      }),
      index: 0,
    },
    {
      event: tool({
        toolName: "grep",
        createdAt: new Date(start + 500).toISOString(),
        completedAt: new Date(start + 900).toISOString(),
      }),
      index: 1,
    },
    { event: { ...thought, createdAt: new Date(start + 5_000).toISOString() }, index: 2 },
  ];
  const trace = activityTimeline(steps, start + 10_000)!;
  const thoughtSpan = trace.spans.find((span) => span.bucket === "thought")!;
  const layout = activitySpanLayout(thoughtSpan, liveAxisMs(trace.durationMs, LIVE_AXIS_FLOOR_MS));
  const rightEdge = layout.leftPct + layout.widthPct;
  assert.ok(layout.leftPct > 7 && layout.leftPct < 10, `thought starts ~5s/60s, got left ${layout.leftPct}%`);
  assert.ok(
    rightEdge < 20,
    `thought ending at 10s on a 60s ruler must not glue to 100%, got right edge ${rightEdge}%`,
  );
});

check("live tools occupy until the next step so they grow like thought", () => {
  // The 46s screenshot: search/write/command/edit all completed in ~1ms, so
  // honest completedAt made every bar a tick. Occupancy holds each step
  // until the next one (last step grows to now, then to the Reply).
  const start = Date.parse("2026-10-24T12:00:00Z");
  const instant = (toolName: string, at: number, index: number) => ({
    event: tool({
      toolName,
      createdAt: new Date(start + at).toISOString(),
      completedAt: new Date(start + at + 1).toISOString(),
      status: "completed",
    }),
    index,
  });
  const steps = [
    instant("grep", 0, 0),
    instant("write", 10_000, 1),
    instant("write", 20_000, 2),
    instant("bash", 30_000, 3),
    instant("search_replace", 40_000, 4),
  ];
  const now = start + 46_000;
  const live = activityTimeline(steps, now)!;
  assert.deepEqual(live.lanes, ["search", "write", "command", "edit"]);
  assert.equal(live.durationMs, 46_000);
  assert.equal(live.spans[0]!.end - live.spans[0]!.start, 10_000);
  assert.equal(live.spans[4]!.end - live.spans[4]!.start, 6_000);
  assert.equal(live.spans[4]!.open, true);
  const axis = liveAxisMs(live.durationMs, LIVE_AXIS_FLOOR_MS);
  assert.equal(axis, LIVE_AXIS_FLOOR_MS);
  const searchLayout = activitySpanLayout(live.spans[0]!, axis);
  assert.ok(
    searchLayout.widthPct > 15 && searchLayout.widthPct < 18,
    `search holding 10s of a 60s ruler should be ~16.7%, got ${searchLayout.widthPct}%`,
  );
  const editRight =
    activitySpanLayout(live.spans[4]!, axis).leftPct +
    activitySpanLayout(live.spans[4]!, axis).widthPct;
  assert.ok(
    editRight > 75 && editRight < 78,
    `last edit playhead at 46s/60s, got right edge ${editRight}%`,
  );

  const hist = activityTimeline(steps, undefined, now)!;
  assert.equal(hist.spans[0]!.end - hist.spans[0]!.start, 10_000);
  assert.equal(hist.spans[4]!.end - hist.spans[4]!.start, 6_000);
  assert.equal(hist.spans[4]!.open, false);
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

check("historical trailing thought reaches the following reply, not just the last tool", () => {
  // Screenshot: write 0.1s + command 1.1s, then thought for 272s, tally said 11s
  // because the thought had no endedAt and the Reply is outside the cluster.
  const start = Date.parse("2026-10-24T12:00:00Z");
  const thoughtAt = start + 11_000;
  const replyAt = thoughtAt + 272_000;
  const steps = [
    {
      event: tool({
        toolName: "write",
        path: "rs-dump.mjs",
        createdAt: new Date(start).toISOString(),
        completedAt: new Date(start + 100).toISOString(),
      }),
      index: 0,
    },
    {
      event: tool({
        toolName: "bash",
        createdAt: new Date(start + 1_000).toISOString(),
        completedAt: new Date(start + 2_100).toISOString(),
      }),
      index: 1,
    },
    {
      event: { ...thought, createdAt: new Date(thoughtAt).toISOString() },
      index: 2,
    },
  ];
  const reply = {
    type: "assistant_message" as const,
    sessionId: "s",
    text: "done",
    createdAt: new Date(replyAt).toISOString(),
  };
  const untilMs = activityRunUntilMs(steps, [...steps.map((s) => s.event), reply], "s");
  assert.equal(untilMs, replyAt);

  const bare = activityTimeline(steps)!;
  assert.ok(bare.durationMs < 12_000, `without the Reply close the axis stays ~11s, got ${bare.durationMs}`);

  const closed = activityTimeline(steps, undefined, untilMs)!;
  assert.equal(closed.durationMs, 11_000 + 272_000);
  const thoughtSpan = closed.spans.find((span) => span.bucket === "thought")!;
  assert.equal(thoughtSpan.end - thoughtSpan.start, 272_000);
  const summary = summarizeActivity(steps, { untilMs });
  assert.equal(summary.elapsed, "4m 43s");
});

check("occupancy holds an instant write until the next step", () => {
  // Write lands at 0–1 ms; occupancy paints it until the command starts.
  const start = Date.parse("2026-10-24T12:00:00Z");
  const steps = [
    { event: { ...thought, createdAt: new Date(start).toISOString() }, index: 0 },
    {
      event: tool({
        toolName: "write",
        path: "a.ts",
        createdAt: new Date(start + 4000).toISOString(),
        completedAt: new Date(start + 4001).toISOString(),
      }),
      index: 1,
    },
    {
      event: tool({
        toolName: "bash",
        input: '{"command":"ls"}',
        createdAt: new Date(start + 4500).toISOString(),
        completedAt: new Date(start + 5800).toISOString(),
      }),
      index: 2,
    },
  ];
  const untilMs = start + 5800;
  const trace = activityTimeline(steps, undefined, untilMs)!;
  assert.deepEqual(trace.lanes, ["thought", "write", "command"]);
  const writeSpan = trace.spans[1]!;
  assert.equal(writeSpan.end - writeSpan.start, 500);
  const commandSpan = trace.spans[2]!;
  assert.equal(commandSpan.end - commandSpan.start, 1300);
});

check("two tools 1ms apart still clamp to a visible tick", () => {
  const start = Date.parse("2026-10-24T12:00:00Z");
  const steps = [
    {
      event: tool({
        toolName: "write",
        path: "a.ts",
        createdAt: new Date(start).toISOString(),
        completedAt: new Date(start + 1).toISOString(),
      }),
      index: 0,
    },
    {
      event: tool({
        toolName: "bash",
        createdAt: new Date(start + 1).toISOString(),
        completedAt: new Date(start + 1).toISOString(),
      }),
      index: 1,
    },
  ];
  const trace = activityTimeline(steps, undefined, start + 10_000)!;
  const writeSpan = trace.spans[0]!;
  assert.equal(writeSpan.end - writeSpan.start, 1);
  const width = activitySpanWidthPct(writeSpan, trace.durationMs);
  assert.ok(width >= 0.75, `1ms occupancy bar ${width}% must clamp to a visible tick`);
});

check("search_replace is an edit, not a search", () => {
  const edits = [{ event: tool({ toolName: "search_replace", path: "a.ts" }), index: 0 }];
  const groups = groupConsecutiveByKind(edits);
  assert.equal(groups[0].bucket, "edit");
  assert.equal(formatActivityTally(edits), "1 edit");
});

check("the finished chart is a freeze-frame of the last live chart", () => {
  // Screenshot: live occupancy on a 1m axis, then the fold remounted ticks
  // on a 14s axis that stretched to 100%. Same occupancy + untilMs + 60s
  // floor must keep the bars and the empty track to the right.
  const start = Date.parse("2026-10-24T12:00:00Z");
  const instant = (toolName: string, at: number, index: number) => ({
    event: tool({
      toolName,
      createdAt: new Date(start + at).toISOString(),
      completedAt: new Date(start + at + 1).toISOString(),
      status: "completed",
    }),
    index,
  });
  const thoughtAt = 8_000;
  const replyAt = 14_000;
  const steps = [
    instant("write", 0, 0),
    instant("bash", 2_000, 1),
    instant("read", 5_000, 2),
    { event: { ...thought, createdAt: new Date(start + thoughtAt).toISOString() }, index: 3 },
  ];
  const reply = {
    type: "assistant_message" as const,
    sessionId: "s",
    text: "done",
    createdAt: new Date(start + replyAt).toISOString(),
  };
  const untilMs = activityRunUntilMs(steps, [...steps.map((s) => s.event), reply], "s");
  assert.equal(untilMs, start + replyAt);

  const liveEnd = activityTimeline(steps, start + replyAt)!;
  const frozen = activityTimeline(steps, undefined, untilMs)!;
  assert.equal(liveEnd.durationMs, 14_000);
  assert.equal(frozen.durationMs, 14_000);
  assert.deepEqual(
    liveEnd.spans.map((span) => [span.start, span.end]),
    frozen.spans.map((span) => [span.start, span.end]),
  );
  assert.equal(frozen.spans[0]!.end - frozen.spans[0]!.start, 2_000);
  assert.equal(frozen.spans[3]!.end - frozen.spans[3]!.start, 6_000);
  assert.equal(frozen.spans[3]!.open, false);

  const axis = liveAxisMs(frozen.durationMs, LIVE_AXIS_FLOOR_MS);
  assert.equal(axis, LIVE_AXIS_FLOOR_MS);
  const lastLayout = activitySpanLayout(frozen.spans[3]!, axis);
  const rightEdge = lastLayout.leftPct + lastLayout.widthPct;
  assert.ok(
    rightEdge > 22 && rightEdge < 25,
    `14s thought on a 60s ruler must not fill the track, got right edge ${rightEdge}%`,
  );
  const summary = summarizeActivity(steps, { untilMs });
  assert.equal(summary.elapsed, "14s");
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
