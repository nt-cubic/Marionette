/**
 * P0-10: end-to-end observability regression for delegated workers.
 *
 * This uses the committed real-shape fixture plus the same ACP normalization
 * primitives as the application. It specifically guards against the original
 * failure mode: dozens or hundreds of tool calls collapsing into one thought
 * card, or the parent card reporting a step count unrelated to the worker rail.
 */
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  applyAcpPartToEvents,
  extractAcpUpdateText,
  extractAcpUserText,
  getSessionUpdate,
  getSessionUpdateKind,
  userMessageEvent,
  type SessionEvent,
} from "../src/lib/acpTranscript.ts";
import {
  mergeUserEcho,
  shouldFlushUserEcho,
  teeEventsAfterCursor,
  uncoveredTeeEvents,
} from "../src/lib/chiefTee.ts";
import { observeSubagent } from "../src/lib/subagentObservation.ts";
import { projectWorkerFrames } from "../src/lib/workerStreamProjection.ts";
import { planColdStartReplay } from "../src/lib/coldStartReplay.ts";
import { projectDelegatedTask } from "../src/lib/backgroundTasks.ts";
import {
  parseSubagentFixture,
  validateSubagentFixture,
  type SubagentFixtureEvent,
} from "./fixtures/subagentFixture.ts";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const FIXTURE_PATH = path.join(HERE, "fixtures", "subagent-worker.events.jsonl");
const SESSION_ID = "session-fixture-worker";

function replayWorker(events: readonly SubagentFixtureEvent[]): {
  rail: SessionEvent[];
  parsedToolStarts: string[];
  parsedUpdates: number;
} {
  const afterCursor = teeEventsAfterCursor(events, 0);
  assert.equal(afterCursor.length, events.length, "cursor dropped fixture events");
  const uncovered = uncoveredTeeEvents(afterCursor, 0);
  assert.equal(uncovered.length, events.length, "uncovered-span filter dropped fixture events");

  let rail: SessionEvent[] = [];
  let userEcho = "";
  const parsedToolStarts: string[] = [];
  let parsedUpdates = 0;

  for (const event of uncovered) {
    const method = typeof event.method === "string" ? event.method : "";
    const update = getSessionUpdate(event.data);
    const kind = update ? getSessionUpdateKind(update) : "";

    if (method === "session/update" && kind === "user_message_chunk") {
      const extracted = extractAcpUserText(event.data);
      if (extracted?.text) {
        userEcho = mergeUserEcho(userEcho, {
          text: extracted.text,
          isDelta: extracted.isDelta,
        });
      }
      continue;
    }

    if (userEcho && shouldFlushUserEcho(method, kind)) {
      rail = [...rail, userMessageEvent(SESSION_ID, userEcho)];
      userEcho = "";
    }

    const extracted = extractAcpUpdateText(event.data);
    if (!extracted) continue;
    if (extracted.role === "tool" && extracted.sessionUpdate === "tool_call" && extracted.toolCallId) {
      parsedToolStarts.push(extracted.toolCallId);
    }
    if (extracted.sessionUpdate === "tool_call_update") parsedUpdates += 1;
    rail = applyAcpPartToEvents(rail, SESSION_ID, extracted);
  }

  if (userEcho) rail = [...rail, userMessageEvent(SESSION_ID, userEcho)];
  return { rail, parsedToolStarts, parsedUpdates };
}

function main(): void {
  const events = parseSubagentFixture(readFileSync(FIXTURE_PATH, "utf8"));
  const validation = validateSubagentFixture(events);
  assert.equal(validation.ok, true, validation.errors.join("\n"));
  assert.ok(events.length > 2_000, `fixture unexpectedly small: ${events.length}`);

  const { rail, parsedToolStarts, parsedUpdates } = replayWorker(events);
  const toolCards = rail.filter((event) => event.type === "tool_call");
  const toolIds = toolCards
    .map((event) => (event.type === "tool_call" ? event.toolCallId ?? "" : ""))
    .filter(Boolean);
  const uniqueToolIds = new Set(toolIds);

  assert.equal(parsedToolStarts.length, 145, "ACP parser lost tool_call starts");
  assert.equal(parsedUpdates, 673, "ACP parser lost tool_call_update events");
  assert.equal(toolCards.length, parsedToolStarts.length, "tool starts collapsed in the event rail");
  assert.notEqual(toolCards.length, 1, "regression: 145 tool calls collapsed into one card");
  assert.equal(uniqueToolIds.size, parsedToolStarts.length, "tool cards are duplicated");
  assert.deepEqual(toolIds, parsedToolStarts, "tool card order differs from parser order");

  const thoughtCards = rail.filter((event) => event.type === "thought");
  const assistantCards = rail.filter((event) => event.type === "assistant_message");
  const userCards = rail.filter((event) => event.type === "user_message");
  assert.ok(thoughtCards.length > 0, "worker thought body is not observable");
  assert.ok(assistantCards.length > 0, "worker assistant body is not observable");
  assert.equal(userCards.length, 1, "worker user echo must appear exactly once");

  const observation = observeSubagent({
    events: rail,
    sessionId: SESSION_ID,
    lifecycle: { status: "completed" },
  });
  assert.equal(observation.state, "completed", "completed lifecycle was not preserved");
  assert.equal(observation.stepCount, toolCards.length, "observation model lost worker steps");
  assert.ok(observation.durationMs != null, "completed observation has no duration");
  assert.ok(observation.finalAssistantText, "completed observation has no final assistant text");

  const currentAction = toolCards.at(-1);
  assert.ok(currentAction?.type === "tool_call", "no current tool action derived");
  assert.ok(currentAction.title, "current action has no tool title");
  assert.ok(currentAction.status, "current action has no status");
  assert.equal(observation.currentAction?.toolCallId, currentAction.toolCallId, "current action source diverged");
  assert.equal(observation.currentAction?.title, currentAction.title, "current action title diverged");

  // The parent card and worker rail must count the same source events.
  const parentStepCount = observation.card.stepCount;
  assert.equal(parentStepCount, parsedToolStarts.length, "parent step count diverges from worker parse");
  assert.equal(parentStepCount, uniqueToolIds.size, "parent step count diverges from worker cards");

  const projected = projectWorkerFrames(events, { sessionId: SESSION_ID });
  const projectedTools = projected.rail.filter((event) => event.type === "tool_call");
  assert.equal(projected.report.toolStarts, 145, "worker projector lost tool starts");
  assert.equal(projected.report.toolUpdates, 673, "worker projector lost tool updates");
  assert.equal(projected.report.deferredUserEcho, 1, "worker projector did not defer user echo");
  assert.equal(projectedTools.length, 145, "worker projector collapsed tool cards");
  assert.equal(projected.report.distinctToolCalls, 145, "worker projector lost distinct tool ids");
  assert.deepEqual(
    projectedTools.map((event) => (event.type === "tool_call" ? event.toolCallId : null)),
    parsedToolStarts,
    "worker projector order diverged",
  );

  const firstPlan = planColdStartReplay({
    sessionId: SESSION_ID,
    transcriptEvents: [],
    teeEvents: events,
    storedCursor: 0,
  });
  assert.equal(firstPlan.counts.replayedUserEchoes, 1, "cold start lost the You card");
  assert.equal(firstPlan.counts.replayedStreamParts, 2142, "cold start lost stream parts");
  assert.equal(firstPlan.nextCursor, 2144, "cold start did not cover the full ledger");
  assert.equal(firstPlan.emptyReason, null, "cold start unexpectedly returned an empty plan");

  const secondPlan = planColdStartReplay({
    sessionId: SESSION_ID,
    transcriptEvents: rail,
    teeEvents: events,
    storedCursor: firstPlan.nextCursor,
  });
  assert.equal(secondPlan.actions.length, 0, "repeated cold start replayed already consumed rows");
  assert.equal(secondPlan.emptyReason, "nothing_after_cursor", "repeated open cursor reason");

  const fixtureUserTs = events[0]?.ts;
  const transcriptRail = rail.map((event) =>
    event.type === "user_message" && typeof fixtureUserTs === "number"
      ? { ...event, createdAt: new Date(fixtureUserTs).toISOString() }
      : event,
  );
  const duplicatePlan = planColdStartReplay({
    sessionId: SESSION_ID,
    transcriptEvents: transcriptRail,
    teeEvents: events,
    storedCursor: 0,
    transcriptMaxMs: 0,
  });
  assert.equal(duplicatePlan.counts.duplicateYouCards, 1, "existing You card was not deduped");
  assert.equal(
    duplicatePlan.counts.duplicateToolCalls,
    145,
    "existing worker tool cards were not deduped",
  );

  const lifecycleCases = [
    ["proposed", "starting"],
    ["running", "running"],
    ["waiting_user", "waiting_user"],
    ["completed", "completed"],
    ["failed", "failed"],
    ["cancelled", "cancelled"],
    ["interrupted", "cancelled"],
  ] as const;
  for (const [status, expected] of lifecycleCases) {
    assert.equal(
      projectDelegatedTask({
        status,
        lastEventAt: Date.now(),
        now: Date.now(),
      }).state,
      expected,
      `lifecycle ${status} should project to ${expected}`,
    );
  }

  const firstTs = events.find((event) => typeof event.ts === "number")?.ts ?? 0;
  const lastTs = [...events].reverse().find((event) => typeof event.ts === "number")?.ts ?? 0;
  assert.ok(lastTs > firstTs, "fixture has no usable duration span");

  console.log("verify-subagent-observability: OK");
  console.log(
    `  events=${events.length} toolStarts=${parsedToolStarts.length} toolUpdates=${parsedUpdates} ` +
      `toolCards=${toolCards.length} parentSteps=${parentStepCount} thoughts=${thoughtCards.length} ` +
      `assistant=${assistantCards.length} durationMs=${lastTs - firstTs}`,
  );
  console.log(`  currentAction=${currentAction.title ?? ""} status=${currentAction.status ?? ""}`);
}

main();
