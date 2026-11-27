/**
 * P1-11 shelf presentation states: working / settled / snoozed / archived.
 *
 * Run: npx tsx scripts/verify-session-states.mts
 */
import assert from "node:assert/strict";
import {
  DEFAULT_SESSION_QUIET_MS,
  autoSettleSessionIds,
  autoWakeSnoozedSessionIds,
  snoozeShouldWake,
  isSessionHiddenFromShelf,
  partitionSessionsByPresentation,
  resolveSessionPresentation,
  sessionIsWorking,
  sessionPresentationPatch,
  shouldAutoSettle,
  type SessionPresentationSignals,
} from "../src/lib/sessionGroups.ts";
import type { Session } from "../src/lib/types.ts";

let passed = 0;
function check(name: string, fn: () => void) {
  fn();
  passed += 1;
  console.log(`  ok  ${name}`);
}

const base = Date.parse("2026-01-01T12:00:00.000Z");
const iso = (deltaMs: number) => new Date(base + deltaMs).toISOString();

const row = (over: Partial<Session> & { id: string }): Session => ({
  projectId: "p",
  agentId: "opencode",
  label: over.id,
  cwd: "D:\\x",
  status: "waiting",
  processId: null,
  startedAt: iso(-60 * 60_000),
  lastActiveAt: iso(-30 * 60_000),
  transcriptPath: "",
  handoffPath: "",
  viewMode: "clean",
  ...over,
});

const presentation = (state: "settled" | "snoozed" | "archived", extra: Partial<Session["presentation"]> = {}) => ({
  state,
  updatedAt: iso(-1_000),
  ...extra,
});

console.log("working state comes from real signals, not shelf overrides");
check("running / starting / delegated / approval / question are all working", () => {
  assert.equal(sessionIsWorking({ status: "running" }), true);
  assert.equal(sessionIsWorking({ status: "starting" }), true);
  assert.equal(sessionIsWorking({ status: "waiting", delegated: true }), true);
  assert.equal(sessionIsWorking({ status: "waiting", approvalPending: true }), true);
  assert.equal(sessionIsWorking({ status: "waiting", questionPending: true }), true);
  assert.equal(sessionIsWorking({ status: "exited" }), false);
});

check("unread completion is attention, not working", () => {
  const s = row({ id: "s1" });
  const signals: SessionPresentationSignals = { status: "waiting", unreadAttention: true };
  assert.equal(sessionIsWorking(signals), false);
  assert.equal(resolveSessionPresentation(s, signals, {}, base), "settled");
});

check("archived beats a running process: archive is shelf-only", () => {
  const s = row({ id: "s1", status: "running", presentation: presentation("archived") });
  assert.equal(resolveSessionPresentation(s, { status: "running" }, {}, base), "archived");
  assert.equal(isSessionHiddenFromShelf(s, { status: "running" }, {}, base), true);
});

console.log("snooze hides only while asleep and wakes for activity");
check("active snooze is hidden", () => {
  const s = row({
    id: "s1",
    presentation: presentation("snoozed", { snoozedUntil: iso(60 * 60_000) }),
  });
  assert.equal(resolveSessionPresentation(s, { status: "waiting" }, {}, base), "snoozed");
  assert.equal(isSessionHiddenFromShelf(s, { status: "waiting" }, {}, base), true);
});

check("expired snooze surfaces again", () => {
  const s = row({
    id: "s1",
    presentation: presentation("snoozed", { snoozedUntil: iso(-60_000) }),
  });
  assert.equal(resolveSessionPresentation(s, { status: "waiting" }, {}, base), "settled");
  assert.equal(isSessionHiddenFromShelf(s, { status: "waiting" }, {}, base), false);
});

check("snoozed dialog wakes for an attention event", () => {
  const s = row({
    id: "s1",
    presentation: presentation("snoozed", { snoozedUntil: iso(60 * 60_000) }),
  });
  const signals: SessionPresentationSignals = { status: "waiting", unreadAttention: true };
  assert.equal(resolveSessionPresentation(s, signals, {}, base), "settled");
  assert.equal(isSessionHiddenFromShelf(s, signals, {}, base), false);
});

check("snoozed dialog wakes for live work instead of hiding it", () => {
  const s = row({
    id: "s1",
    status: "running",
    presentation: presentation("snoozed", { snoozedUntil: iso(60 * 60_000) }),
  });
  assert.equal(resolveSessionPresentation(s, { status: "running" }, {}, base), "working");
});

check("snoozeShouldWake names time and attention; autoWake returns those ids", () => {
  const active = row({
    id: "active",
    presentation: presentation("snoozed", { snoozedUntil: iso(60 * 60_000) }),
  });
  const expired = row({
    id: "expired",
    presentation: presentation("snoozed", { snoozedUntil: iso(-1_000) }),
  });
  const attention = row({
    id: "attention",
    presentation: presentation("snoozed", { snoozedUntil: iso(60 * 60_000) }),
  });
  assert.equal(snoozeShouldWake(active, { status: "waiting" }, base), false);
  assert.equal(snoozeShouldWake(expired, { status: "waiting" }, base), true);
  assert.equal(
    snoozeShouldWake(attention, { status: "waiting", unreadAttention: true }, base),
    true,
  );
  const ids = autoWakeSnoozedSessionIds(
    [active, expired, attention],
    new Map([["attention", { status: "waiting", unreadAttention: true }]]),
    base,
  );
  assert.deepEqual(ids, ["expired", "attention"]);
});

console.log("auto-settle needs a quiet period with no outstanding work");
check("idle dialog settles only after the quiet period", () => {
  const s = row({ id: "s1", lastActiveAt: iso(-DEFAULT_SESSION_QUIET_MS - 1) });
  assert.equal(shouldAutoSettle(s, { status: "waiting" }, {}, base), true);
  const fresh = row({ id: "s2", lastActiveAt: iso(-1_000) });
  assert.equal(shouldAutoSettle(fresh, { status: "waiting" }, {}, base), false);
});

check("every attention / background / scheduled signal blocks auto-settle", () => {
  const s = row({ id: "s1", lastActiveAt: iso(-60 * 60_000) });
  const blocked: SessionPresentationSignals[] = [
    { status: "waiting", unreadAttention: true },
    { status: "waiting", approvalPending: true },
    { status: "waiting", questionPending: true },
    { status: "waiting", activeBackgroundWork: true },
    { status: "waiting", scheduledSend: true },
    { status: "waiting", delegated: true },
    { status: "running" },
    { status: "starting" },
  ];
  for (const signals of blocked) {
    assert.equal(shouldAutoSettle(s, signals, {}, base), false, JSON.stringify(signals));
  }
});

check("archived and snoozed rows never auto-settle", () => {
  const archived = row({
    id: "a",
    lastActiveAt: iso(-60 * 60_000),
    presentation: presentation("archived"),
  });
  const snoozed = row({
    id: "s",
    lastActiveAt: iso(-60 * 60_000),
    presentation: presentation("snoozed", { snoozedUntil: iso(60 * 60_000) }),
  });
  assert.equal(shouldAutoSettle(archived, { status: "waiting" }, {}, base), false);
  assert.equal(shouldAutoSettle(snoozed, { status: "waiting" }, {}, base), false);
});

check("autoSettleSessionIds skips settled rows and returns only new transitions", () => {
  const old = row({ id: "old", lastActiveAt: iso(-60 * 60_000) });
  const already = row({
    id: "already",
    lastActiveAt: iso(-60 * 60_000),
    presentation: presentation("settled"),
  });
  const attention = row({ id: "attention", lastActiveAt: iso(-60 * 60_000) });
  const ids = autoSettleSessionIds(
    [old, already, attention],
    new Map([["attention", { status: "waiting", unreadAttention: true }]]),
    {},
    base,
  );
  assert.deepEqual(ids, ["old"]);
});

console.log("shelf partition and filters agree");
check("archived and snoozed leave the visible shelf; working includes delegated/attention", () => {
  const sessions = [
    row({ id: "run", status: "running" }),
    row({ id: "delegated", status: "waiting" }),
    row({ id: "approval", status: "waiting" }),
    row({ id: "idle", status: "waiting" }),
    row({
      id: "snoozed",
      presentation: presentation("snoozed", { snoozedUntil: iso(60 * 60_000) }),
    }),
    row({ id: "archived", presentation: presentation("archived") }),
  ];
  const signals = new Map<string, SessionPresentationSignals>([
    ["delegated", { status: "waiting", delegated: true }],
    ["approval", { status: "waiting", approvalPending: true }],
  ]);
  const partition = partitionSessionsByPresentation(sessions, signals, {}, base);
  assert.deepEqual(partition.working.map((s) => s.id), ["run", "delegated", "approval"]);
  assert.deepEqual(partition.visible.map((s) => s.id), ["run", "delegated", "approval", "idle"]);
  assert.deepEqual(partition.snoozed.map((s) => s.id), ["snoozed"]);
  assert.deepEqual(partition.archived.map((s) => s.id), ["archived"]);
});

check("corrupt presentation rows read as no override", () => {
  const s = row({
    id: "s1",
    presentation: { state: "bogus" as never, updatedAt: "x" },
  });
  assert.equal(resolveSessionPresentation(s, { status: "waiting" }, {}, base), "settled");
  assert.equal(isSessionHiddenFromShelf(s, { status: "waiting" }, {}, base), false);
});

console.log("optimistic patches fail closed");
check("working clears; snoozed needs a future wake time", () => {
  assert.equal(sessionPresentationPatch("working", base), null);
  assert.equal(sessionPresentationPatch("snoozed", base, null), null);
  assert.equal(sessionPresentationPatch("snoozed", base, iso(-1_000)), null);
  const patch = sessionPresentationPatch("snoozed", base, iso(60_000));
  assert.equal(patch?.state, "snoozed");
  assert.equal(patch?.snoozedUntil, iso(60_000));
});

check("settled/archived drop a stale snooze wake time", () => {
  const settled = sessionPresentationPatch("settled", base, iso(60_000));
  const archived = sessionPresentationPatch("archived", base, iso(60_000));
  assert.equal(settled?.state, "settled");
  assert.equal(settled?.snoozedUntil, null);
  assert.equal(archived?.state, "archived");
  assert.equal(archived?.snoozedUntil, null);
});

console.log(`\n${passed} checks passed`);
