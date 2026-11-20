import assert from "node:assert/strict";
import {
  isStaffChatEvent,
  isStaffChatSession,
  isStaffDialogueEvent,
  selectStaffChatEvents,
  staffChatAwaitingReply,
  staffChatIdentity,
  staffMonogram,
} from "../src/lib/staffChat.ts";
import type { SessionEvent } from "../src/lib/types.ts";

const base = {
  sessionId: "staff-session",
  createdAt: "2026-11-19T00:00:00.000Z",
};

const events = [
  { ...base, type: "user_message", text: "Inspect the project" },
  { ...base, type: "thought", text: "internal" },
  { ...base, type: "tool_call", text: "read file" },
  {
    ...base,
    type: "notice",
    severity: "info",
    title: "Status",
    description: "Ready",
    text: "Ready",
  },
  { ...base, type: "assistant_message", text: "Done." },
  { ...base, sessionId: "other", type: "assistant_message", text: "Other" },
] satisfies SessionEvent[];

assert.equal(isStaffChatSession({ origin: "chief" }), true);
assert.equal(isStaffChatSession({ origin: "chief-worker" }), false);
assert.equal(isStaffChatSession({ origin: "user" }), false);
assert.equal(isStaffChatSession(null), false);

assert.equal(isStaffDialogueEvent(events[0]!), true);
assert.equal(isStaffDialogueEvent(events[1]!), false);
assert.equal(isStaffChatEvent(events[0]!), true);
assert.equal(isStaffChatEvent(events[2]!), false);
assert.equal(isStaffChatEvent(events[3]!), false);
assert.equal(
  isStaffChatEvent({ ...events[3]!, severity: "warning" }),
  true,
);

const projected = selectStaffChatEvents(events, "staff-session");
assert.deepEqual(
  projected.map((event) => event.type),
  ["user_message", "assistant_message"],
);
assert.equal(staffChatAwaitingReply(projected, true), false);
assert.equal(staffChatAwaitingReply([projected[0]!], true), true);
assert.equal(staffChatAwaitingReply([projected[0]!], false), false);
assert.equal(staffChatAwaitingReply([], true), true);
assert.equal(
  staffChatAwaitingReply(
    [
      { ...base, type: "user_message", text: "Pick one" },
      {
        ...base,
        type: "ask_prompt",
        requestId: "ask-1",
        questions: [],
        status: "pending",
      },
    ],
    true,
  ),
  false,
);

assert.deepEqual(staffChatIdentity("Mari \u00b7 Chief", "Claude"), {
  name: "Mari",
  title: "Chief",
});
assert.deepEqual(staffChatIdentity("", "Claude"), {
  name: "Claude",
  title: "",
});
assert.equal(staffMonogram("Mari"), "M");
assert.equal(staffMonogram("\u963f\u73cd"), "\u963f");

console.log("verify-staff-chat: 20 assertions passed");
