/**
 * Plan / Ask live in the conversation stream (not a docked popup),
 * persist through transcript JSONL, and hide the composer while pending.
 *
 * Run: npx tsx scripts/verify-interaction-stream.mts
 */
import assert from "node:assert/strict";
import {
  askPromptEvent,
  expirePendingPrompts,
  patchPromptEvent,
  planDecisionLabel,
  planPromptEvent,
  sessionHidesComposer,
  upsertPromptEvent,
} from "../src/lib/interactionEvents.ts";
import { buildMessagePresentation } from "../src/lib/messagePresentation.ts";
import {
  parseTranscriptEvents,
  persistableEventsForSession,
} from "../src/lib/transcript.ts";
import type { SessionEvent } from "../src/lib/types.ts";

let passed = 0;
function check(name: string, fn: () => void) {
  fn();
  passed += 1;
  console.log(`  ok  ${name}`);
}

const user = (text: string): SessionEvent => ({
  type: "user_message",
  sessionId: "s1",
  text,
  createdAt: "2026-10-18T10:00:00.000Z",
});

const reply = (text: string): SessionEvent => ({
  type: "assistant_message",
  sessionId: "s1",
  text,
  createdAt: "2026-10-18T10:00:05.000Z",
});

console.log("composer hides only for the session that has a live Plan/Ask");
check("matching ask hides composer", () => {
  assert.equal(
    sessionHidesComposer("s1", { sessionId: "s1" }, null),
    true,
  );
});
check("matching plan hides composer", () => {
  assert.equal(
    sessionHidesComposer("s1", null, { sessionId: "s1" }),
    true,
  );
});
check("other session keeps composer", () => {
  assert.equal(
    sessionHidesComposer("s2", { sessionId: "s1" }, { sessionId: "s1" }),
    false,
  );
});

console.log("\nprompt events upsert, patch, expire");
const plan = planPromptEvent({
  sessionId: "s1",
  requestId: "p1",
  source: "permission",
  title: "Approve Plan",
  planMarkdown: "# Steps\n\n- one",
  options: [{ optionId: "allow", name: "Yes", kind: "allow_once" }],
});
const ask = askPromptEvent({
  sessionId: "s1",
  requestId: "q1",
  questions: [{ question: "颜色?", options: [{ label: "红" }] }],
});

check("upsert appends then replaces the same requestId", () => {
  const once = upsertPromptEvent([user("hi")], plan);
  const twice = upsertPromptEvent(once, { ...plan, planMarkdown: "# Steps\n\n- two" });
  assert.equal(once.filter((e) => e.type === "plan_prompt").length, 1);
  assert.equal(twice.filter((e) => e.type === "plan_prompt").length, 1);
  const stored = twice.find((e) => e.type === "plan_prompt");
  assert.equal(stored?.type === "plan_prompt" && stored.planMarkdown.includes("- two"), true);
});

check("patch records the decision without duplicating", () => {
  const patched = patchPromptEvent([plan], "p1", {
    status: "answered",
    decision: "allow",
    decisionLabel: planDecisionLabel("allow", plan.options),
  });
  assert.equal(patched.length, 1);
  const row = patched[0];
  assert.equal(row.type === "plan_prompt" && row.status, "answered");
  assert.equal(row.type === "plan_prompt" && row.decisionLabel, "Yes");
});

check("expire turns leftover pending cards into timeout", () => {
  const expired = expirePendingPrompts([plan, ask, reply("ok")], "s1");
  const prompts = expired.filter(
    (e): e is Extract<SessionEvent, { type: "plan_prompt" | "ask_prompt" }> =>
      e.type === "plan_prompt" || e.type === "ask_prompt",
  );
  assert.equal(prompts.length, 2);
  assert.equal(prompts.every((e) => e.status === "timeout"), true);
});

console.log("\npresentation treats Plan/Ask as standalone cards");
check("plan sits between reply groups, not inside Reply", () => {
  const items = buildMessagePresentation([
    user("写计划"),
    reply("这是草稿"),
    plan,
    user("继续"),
    reply("好"),
  ]);
  const kinds = items.map((item) =>
    item.kind === "event" ? item.event.type : item.kind,
  );
  assert.deepEqual(kinds, [
    "user_message",
    "reply_group",
    "plan_prompt",
    "user_message",
    "reply_group",
  ]);
});

console.log("\ntranscript round-trip keeps Plan/Ask");
check("persist + parse restores markdown, options, answers", () => {
  const answeredAsk: SessionEvent = {
    ...ask,
    status: "answered",
    answers: [{ question: "颜色?", selected: ["红"] }],
  };
  const persisted = persistableEventsForSession([plan, answeredAsk], "s1");
  const parsed = parseTranscriptEvents(persisted);
  assert.equal(parsed.length, 2);
  const p = parsed.find((e) => e.type === "plan_prompt");
  const a = parsed.find((e) => e.type === "ask_prompt");
  assert.equal(p?.type === "plan_prompt" && p.planMarkdown.includes("# Steps"), true);
  assert.equal(p?.type === "plan_prompt" && p.options?.[0]?.optionId, "allow");
  assert.equal(a?.type === "ask_prompt" && a.answers?.[0]?.selected[0], "红");
});

console.log(`\n${passed} checks passed`);
