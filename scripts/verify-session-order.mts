/**
 * Session shelf order: pins and unread-complete rows stay in the collapsed preview.
 *
 * Run: npx tsx scripts/verify-session-order.mts
 */
import assert from "node:assert/strict";
import { collapsedShelfSessions, sortSessionsNewestFirst } from "../src/lib/sessionOrder.ts";
import type { Session } from "../src/lib/types.ts";

let passed = 0;
function check(name: string, fn: () => void) {
  fn();
  passed += 1;
  console.log(`  ok  ${name}`);
}

const row = (over: Partial<Session> & { id: string }): Session => ({
  projectId: "p",
  agentId: "opencode",
  label: over.id,
  cwd: "D:\\x",
  status: "waiting",
  processId: null,
  startedAt: "1",
  lastActiveAt: String(1_700_000_000_000),
  transcriptPath: "",
  handoffPath: "",
  viewMode: "clean",
  ...over,
});

console.log("collapsed shelf keeps pins and unread-complete rows");
check("preview of 5 keeps the newest unpinned when nothing is sticky", () => {
  const sorted = sortSessionsNewestFirst(
    [6, 5, 4, 3, 2, 1].map((n) =>
      row({ id: `s${n}`, lastActiveAt: String(1_700_000_000_000 + n) }),
    ),
  );
  const visible = collapsedShelfSessions(sorted, 5);
  assert.deepEqual(visible.map((s) => s.id), ["s6", "s5", "s4", "s3", "s2"]);
});

check("a pinned row behind the preview still shows", () => {
  const sorted = sortSessionsNewestFirst([
    row({ id: "old-pin", lastActiveAt: String(1_700_000_000_001), pinnedAt: String(1_700_000_000_010) }),
    ...[6, 5, 4, 3, 2].map((n) =>
      row({ id: `s${n}`, lastActiveAt: String(1_700_000_000_000 + n) }),
    ),
  ]);
  const visible = collapsedShelfSessions(sorted, 5);
  assert.equal(visible[0].id, "old-pin");
  assert.equal(visible.length, 5);
  assert.ok(visible.some((s) => s.id === "old-pin"));
});

check("an unread-complete row behind the preview still shows", () => {
  const sorted = sortSessionsNewestFirst(
    [6, 5, 4, 3, 2, 1].map((n) =>
      row({ id: `s${n}`, lastActiveAt: String(1_700_000_000_000 + n) }),
    ),
  );
  const visible = collapsedShelfSessions(sorted, 5, new Set(["s1"]));
  assert.ok(visible.some((s) => s.id === "s1"));
  assert.equal(visible.length, 5);
  assert.equal(visible[0].id, "s6");
  assert.equal(visible[visible.length - 1].id, "s1");
});

console.log(`\n${passed} checks passed`);
