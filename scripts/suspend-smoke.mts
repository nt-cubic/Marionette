/**
 * Suspend-rule smoke test.
 *
 * Run: npx tsx scripts/suspend-smoke.mts
 *
 * Parking a dialog stops its agent process, so the rules have to be exact:
 * only a warm, idle, unblocked dialog that nobody is using may go.
 */
import assert from "node:assert/strict";
import {
  SUSPEND_IDLE_MS,
  lastActiveMs,
  isSuspendable,
  shouldAutoSuspend,
} from "../src/lib/sessionSuspend.ts";

let passed = 0;
function check(name: string, fn: () => void) {
  fn();
  passed += 1;
  console.log(`  ok  ${name}`);
}

const NOW = 1_800_000_000_000;

const warm = (over: Partial<{ id: string; status: string; lastActiveAt: string }> = {}) => ({
  id: "s1",
  status: "waiting",
  lastActiveAt: String(NOW - SUSPEND_IDLE_MS - 1),
  ...over,
});

console.log("when a dialog may be parked");
check("a warm dialog untouched past the window is parked", () => {
  assert.equal(shouldAutoSuspend(warm(), NOW), true);
});
check("a warm dialog still inside the window is left alone", () => {
  assert.equal(shouldAutoSuspend(warm({ lastActiveAt: String(NOW - 60_000) }), NOW), false);
});
check("the dialog this window shows is never parked", () => {
  assert.equal(shouldAutoSuspend(warm(), NOW, { currentSessionId: "s1" }), false);
});
check("a dialog with a question / plan / permission card is never parked", () => {
  assert.equal(shouldAutoSuspend(warm(), NOW, { pendingSessionIds: ["s1"] }), false);
});
check("a dialog with a queued follow-up is never parked", () => {
  assert.equal(shouldAutoSuspend(warm(), NOW, { queuedSessionIds: ["s1"] }), false);
});
check("only warm dialogs are parked", () => {
  for (const status of ["running", "starting", "exited", "error"]) {
    assert.equal(isSuspendable(warm({ status })), false, status);
  }
  assert.equal(isSuspendable(warm()), true);
});

console.log("\nhow idle time is read");
check("epoch-millisecond rows parse", () => {
  assert.equal(lastActiveMs({ lastActiveAt: String(NOW) }), NOW);
});
check("legacy ISO rows parse", () => {
  assert.equal(lastActiveMs({ lastActiveAt: "2026-10-08T10:00:00.000Z" }), Date.parse("2026-10-08T10:00:00.000Z"));
});
check("empty or unreadable rows never trigger a park", () => {
  assert.equal(lastActiveMs({ lastActiveAt: "" }), 0);
  assert.equal(lastActiveMs({ lastActiveAt: "not a date" }), 0);
  assert.equal(shouldAutoSuspend(warm({ lastActiveAt: "" }), NOW), false);
  assert.equal(shouldAutoSuspend(warm({ lastActiveAt: "not a date" }), NOW), false);
});

console.log(`\n${passed} checks passed.`);
