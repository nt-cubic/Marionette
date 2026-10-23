/**
 * Turn-delivery headline + restore classification smoke.
 *
 * Run: npx tsx scripts/verify-turn-delivery.mts
 */
import assert from "node:assert/strict";
import type { SessionEvent } from "../src/lib/types.ts";
import {
  buildTurnDeliveries,
  formatDeliveryHeadline,
  normalizeWorkspacePath,
  selectTurnWorkspaceChanges,
  type TurnDelivery,
} from "../src/lib/turnDelivery.ts";

let passed = 0;
function check(name: string, fn: () => void) {
  fn();
  passed += 1;
  console.log(`  ok  ${name}`);
}

function user(text: string): SessionEvent {
  return {
    type: "user_message",
    sessionId: "s",
    text,
    createdAt: "t0",
    messageId: "m1",
  };
}

function tool(
  partial: Partial<Extract<SessionEvent, { type: "tool_call" }>> & {
    toolName?: string;
    path?: string;
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

function fileChange(
  path: string,
  changeType: "added" | "modified" | "deleted" | "untracked",
  stats?: { add: number; del: number },
): SessionEvent {
  return {
    type: "file_change",
    sessionId: "s",
    path,
    changeType,
    createdAt: "t",
    ...(stats ? { add: stats.add, del: stats.del } : {}),
  };
}

const sample: TurnDelivery = {
  files: [
    { path: "a.ts", relPath: "a.ts", changeType: "modified", add: 10, del: 2, touched: 1 },
    { path: "b.ts", relPath: "b.ts", changeType: "added", add: 4, del: 0, touched: 1 },
  ],
  totalDiff: { add: 14, del: 2 },
  commandCount: 1,
  tally: "1 write, 1 command",
};

console.log("turn delivery");
check("headline prefers file count", () => {
  assert.equal(formatDeliveryHeadline(sample), "2 个文件已更改");
});

check("command-only headline", () => {
  assert.equal(
    formatDeliveryHeadline({ files: [], commandCount: 3 }),
    "跑了 3 条命令",
  );
});

check("empty turn copy", () => {
  assert.equal(formatDeliveryHeadline({ files: [], commandCount: 0 }), "本轮无改动");
});

check("buildTurnDeliveries unions git truth + tool edits", () => {
  const events: SessionEvent[] = [
    user("do it"),
    tool({ toolName: "write", path: "D:/proj/a.ts", detail: "+\n+\n+\n" }),
    tool({ toolName: "bash", input: '{"command":"ls"}' }),
    fileChange("D:/proj/a.ts", "modified"),
    fileChange("D:/proj/b.ts", "added"),
    { type: "assistant_message", sessionId: "s", text: "done", createdAt: "t9", durationMs: 12 },
  ];
  const units = buildTurnDeliveries(events, "s", "D:/proj", false);
  assert.equal(units.length, 1);
  assert.equal(formatDeliveryHeadline(units[0].delivery), "2 个文件已更改");
  assert.equal(units[0].delivery.totalDiff?.add, 3);
  assert.equal(units[0].delivery.commandCount, 1);
  const paths = units[0].delivery.files.map((f) => f.relPath).sort();
  assert.deepEqual(paths, ["a.ts", "b.ts"]);
});

check("git numstat wins over tool-reported stats", () => {
  const events: SessionEvent[] = [
    user("refactor"),
    tool({ toolName: "edit", path: "D:/proj/a.ts", detail: "+\n+\n" }),
    fileChange("D:/proj/a.ts", "modified", { add: 7, del: 3 }),
    fileChange("D:/proj/gen.ts", "untracked", { add: 40, del: 0 }),
    { type: "assistant_message", sessionId: "s", text: "done", createdAt: "t9", durationMs: 5 },
  ];
  const units = buildTurnDeliveries(events, "s", "D:/proj", false);
  const byPath = new Map(units[0].delivery.files.map((f) => [f.relPath, f]));
  assert.equal(byPath.get("a.ts")?.add, 7);
  assert.equal(byPath.get("a.ts")?.del, 3);
  // `gen.ts` only exists because a shell command wrote it: no tool stats, real
  // git counts instead of a blank row.
  assert.equal(byPath.get("gen.ts")?.add, 40);
  assert.equal(units[0].delivery.totalDiff?.add, 47);
});

check("one row per file when git and the tool disagree on the path form", () => {
  const events: SessionEvent[] = [
    user("edit it"),
    // Windows agents hand over an absolute path; git reports it project-relative.
    tool({ toolName: "edit", path: "D:\\proj\\src\\a.ts", detail: "+\n+\n" }),
    tool({ toolName: "read", path: "D:\\proj\\src\\a.ts" }),
    fileChange("src/a.ts", "modified", { add: 12, del: 4 }),
    { type: "assistant_message", sessionId: "s", text: "done", createdAt: "t9", durationMs: 8 },
  ];
  const units = buildTurnDeliveries(events, "s", "D:/proj", false);
  assert.equal(units[0].delivery.files.length, 1, "the file must not be listed twice");
  const row = units[0].delivery.files[0];
  assert.equal(row.relPath, "src/a.ts");
  assert.equal(row.path, "src/a.ts");
  assert.equal(row.changeType, "modified");
  assert.equal(row.touched, 1, "only the edit counts, not the read");
  assert.equal(row.add, 12);
  assert.equal(units[0].delivery.totalDiff?.add, 12);
});

console.log("turn workspace attribution");
check("normalizeWorkspacePath makes tool paths comparable to git paths", () => {
  assert.equal(normalizeWorkspacePath("D:\\proj\\src\\a.ts", "D:/proj"), "src/a.ts");
  assert.equal(normalizeWorkspacePath("d:/PROJ/src/a.ts", "D:/proj/"), "src/a.ts");
  assert.equal(normalizeWorkspacePath("src/a.ts", "D:/proj"), "src/a.ts");
  assert.equal(normalizeWorkspacePath("./src/a.ts", "D:/proj"), "src/a.ts");
});

check("a file already dirty at turn start needs this turn's own edit", () => {
  const changes = [
    { path: "src/old.ts", changeType: "modified" as const, add: 1, del: 1 },
    { path: "src/mine.ts", changeType: "modified" as const, add: 2, del: 0 },
    { path: "src/fresh.ts", changeType: "untracked" as const, add: 9, del: 0 },
  ];
  const kept = selectTurnWorkspaceChanges(changes, {
    // Both old.ts and mine.ts were dirty before the turn; only mine.ts was
    // edited by this turn's tools.
    dirtyAtStart: ["src/old.ts", "src/mine.ts"],
    touched: ["D:\\proj\\src\\mine.ts"],
    root: "D:/proj",
  });
  assert.deepEqual(
    kept.map((change) => change.path),
    ["src/mine.ts", "src/fresh.ts"],
    "another conversation's dirty file must not land in this card",
  );
});

check("without HEAD the split is unknowable, so nothing is dropped", () => {
  const changes = [
    { path: "src/old.ts", changeType: "modified" as const, add: 1, del: 1 },
  ];
  assert.equal(
    selectTurnWorkspaceChanges(changes, { dirtyAtStart: null, touched: [] }).length,
    1,
  );
});

console.log(`\n${passed} checks passed`);