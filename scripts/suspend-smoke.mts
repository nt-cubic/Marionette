/**
 * Suspend-rule smoke test.
 *
 * Run: npx tsx scripts/suspend-smoke.mts
 *
 * Parking a dialog stops its agent process, so the rules have to be exact.
 * Two different rules live here: the hand-driven park button stops any dialog
 * that still holds a process (including one mid-turn), while the unattended idle
 * timer only touches warm, idle, unblocked dialogs nobody is using.
 */
import assert from "node:assert/strict";
import {
  SUSPEND_IDLE_MS,
  lastActiveMs,
  isSuspendable,
  canPark,
  suspendControl,
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
check("only warm idle dialogs are parked, even though a working one can be", () => {
  for (const status of ["running", "starting", "exited", "error"]) {
    assert.equal(isSuspendable(warm({ status })), false, status);
  }
  assert.equal(isSuspendable(warm()), true);
  // The hand-driven park is wider than the idle timer: anything still holding a
  // process can be stopped. Only an already parked one cannot.
  for (const status of ["starting", "running", "waiting", "error"]) {
    assert.equal(canPark(warm({ status })), true, status);
  }
  assert.equal(canPark(warm({ status: "exited" })), false);
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

console.log("\nwhat the row's park control says");
check("a warm dialog offers to park", () => {
  const control = suspendControl(warm(), "Jingzhe");
  assert.equal(control.shown, true);
  assert.match(control.title, /^挂起 Jingzhe — /);
  assert.equal(control.ariaLabel, "Suspend Jingzhe");
});
check("a failed turn still holds a process, so it can be parked", () => {
  assert.equal(canPark(warm({ status: "error" })), true);
  assert.equal(suspendControl(warm({ status: "error" })).shown, true);
});
check("a working dialog is stoppable, and the title says it cuts the turn", () => {
  for (const status of ["starting", "running"]) {
    assert.equal(canPark(warm({ status })), true, status);
    const control = suspendControl(warm({ status }), "Jingzhe");
    assert.equal(control.shown, true, status);
    assert.match(control.title, /^挂起 Jingzhe — /, status);
  }
  assert.match(suspendControl(warm({ status: "running" }), "Jingzhe").title, /中断/);
});
check("an already parked dialog draws no control at all", () => {
  for (const status of ["exited", "", "weird"]) {
    assert.equal(canPark(warm({ status })), false, status);
    assert.equal(suspendControl(warm({ status }), "Jingzhe").shown, false, status);
  }
});
check("every drawn control is live — no dead affordance is painted", () => {
  for (const status of ["starting", "running", "waiting", "error"]) {
    const control = suspendControl(warm({ status }), "s1");
    assert.equal(control.shown, true, status);
    assert.ok(control.title.length > 0, status);
    assert.equal(control.ariaLabel, "Suspend s1", status);
  }
});

console.log(`\n${passed} checks passed.`);