/**
 * Focused fixture loader/self-test for P0-8.
 *
 * This is NOT the P0-10 `verify-subagent-observability.mts`. It proves the
 * committed sanitized fixture is structurally intact and can drive the same
 * parse -> replay -> event-rail chain the app uses:
 *
 * - 100+ tool_call starts survive, with every update linked to a prior start.
 * - user / thought / assistant / tool ordering is present.
 * - `extractAcpUpdateText` + `applyAcpPartToEvents` turn 145 tool starts into
 *   145 tool cards (not one collapsed card) and keep the same order.
 * - the fixture carries no known original paths, ids, repo names or URLs.
 *
 * Run: npx tsx scripts/verify-subagent-fixture.mts
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
import {
  parseSubagentFixture,
  validateSubagentFixture,
  type SubagentFixtureEvent,
} from "./fixtures/subagentFixture.ts";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const FIXTURE_PATH = path.join(HERE, "fixtures", "subagent-worker.events.jsonl");

const FORBIDDEN = [
  "C:\\Users",
  "D:\\Project",
  "Marionette",
  "github.com",
  "https://",
  "http://",
  "Bearer ",
  "Authorization",
  "api_key",
  "openai",
  "deepseek",
  "anthropic",
  "opencode",
  "zhipu",
  "sk-",
  "session-179577",
  "ses_de345",
  "call_df48",
  "prt_21cb",
  "msg_21cd",
];

const MARKER_PATTERNS = [
  /^\[redacted #\d+\]$/,
  /^\/redacted\/path-\d{4}$/,
  /^redacted-url-\d+$/,
  /^call_fixture_\d{4}$/,
  /^msg_fixture_\d{4}$/,
];
const SAFE_FIXED = new Set([
  "session-fixture-worker",
  "ses_fixture_worker",
  "[redacted user prompt]",
]);
const SAFE_SEMANTIC = /^[A-Za-z0-9_./:-]{1,40}$/;
const SEMANTIC_BLOCKLIST =
  /marionette|deepseek|openai|anthropic|opencode|zhipu|grok|gemini|claude|codex|github|http|bearer|api[_-]?key|gpt|llama|mistral|qwen|kimi|glm|minimax|moonshot|bytedance|nvidia|cohere|perplexity|xai|x-ai/i;

function assertNoOriginalStrings(text: string): void {
  const found = FORBIDDEN.filter((token) => text.includes(token));
  assert.deepEqual(found, [], `fixture contains forbidden original strings: ${found.join(", ")}`);
}

function assertOnlySafeStrings(events: readonly SubagentFixtureEvent[]): void {
  const visit = (value: unknown, trail: string): void => {
    if (typeof value === "string") {
      if (!value || /^\s+$/.test(value)) return;
      if (SAFE_FIXED.has(value)) return;
      if (MARKER_PATTERNS.some((pattern) => pattern.test(value))) return;
      assert.ok(
        value.length <= 40 && SAFE_SEMANTIC.test(value) && !SEMANTIC_BLOCKLIST.test(value),
        `unsafe string at ${trail}: length ${value.length}`,
      );
      return;
    }
    if (Array.isArray(value)) {
      value.forEach((item, index) => visit(item, `${trail}[${index}]`));
      return;
    }
    if (value && typeof value === "object") {
      for (const [key, child] of Object.entries(value)) {
        visit(child, trail ? `${trail}.${key}` : key);
      }
    }
  };
  events.forEach((event, index) => visit(event, `event[${index}]`));
}

function replayThroughAcp(events: readonly SubagentFixtureEvent[]): {
  rail: SessionEvent[];
  toolStarts: string[];
  userEchoCards: number;
} {
  const sessionId = "session-fixture-worker";
  const afterCursor = teeEventsAfterCursor(events, 0);
  assert.equal(afterCursor.length, events.length, "cursor filter dropped fixture events");
  const uncovered = uncoveredTeeEvents(afterCursor, 0);
  assert.equal(uncovered.length, events.length, "uncovered span dropped fixture events");

  let rail: SessionEvent[] = [];
  const toolStarts: string[] = [];
  let userEcho = "";
  let userEchoCards = 0;

  for (const event of uncovered) {
    const method = typeof event.method === "string" ? event.method : "";
    const update = getSessionUpdate(event.data);
    const kind = update ? getSessionUpdateKind(update) : "";

    if (method === "session/update" && kind === "user_message_chunk") {
      const extracted = extractAcpUserText(event.data);
      if (extracted && extracted.text) {
        userEcho = mergeUserEcho(userEcho, {
          text: extracted.text,
          isDelta: extracted.isDelta,
        });
      }
      continue;
    }

    if (userEcho && shouldFlushUserEcho(method, kind)) {
      rail = [...rail, userMessageEvent(sessionId, userEcho)];
      userEcho = "";
      userEchoCards += 1;
    }

    const extracted = extractAcpUpdateText(event.data);
    if (
      !extracted ||
      (extracted.role !== "assistant" &&
        extracted.role !== "thought" &&
        extracted.role !== "tool" &&
        extracted.role !== "system")
    ) {
      continue;
    }
    if (extracted.role === "tool" && extracted.sessionUpdate === "tool_call" && extracted.toolCallId) {
      toolStarts.push(extracted.toolCallId);
    }
    rail = applyAcpPartToEvents(rail, sessionId, extracted);
  }

  if (userEcho) {
    rail = [...rail, userMessageEvent(sessionId, userEcho)];
    userEchoCards += 1;
  }

  return { rail, toolStarts, userEchoCards };
}

function main(): void {
  const text = readFileSync(FIXTURE_PATH, "utf8");
  const events = parseSubagentFixture(text);
  const validation = validateSubagentFixture(events);
  assert.equal(validation.ok, true, validation.errors.join("\n"));
  const summary = validation.summary;

  // Fixture counts survive the loader.
  assert.equal(summary.toolCallStarts, 145, "tool_call starts");
  assert.equal(summary.distinctToolCallIds, 145, "distinct tool call ids");
  assert.equal(summary.toolCallUpdates, 673, "tool_call_update count");
  assert.equal(summary.userChunks, 1, "user seed");
  assert.equal(summary.thoughtChunks, 1300, "thought chunks");
  assert.equal(summary.assistantChunks, 24, "assistant chunks");
  assert.equal(summary.turnCompletions, 1, "turn completion");
  assert.equal(summary.lines, 2144, "fixture lines");
  assert.equal(summary.firstSeq, 1, "first seq");
  assert.equal(summary.lastSeq, 2144, "last seq");

  // No original content survives.
  assertNoOriginalStrings(text);
  assertOnlySafeStrings(events);

  // Ordering: the user seed is first; every tool update comes after its start.
  assert.equal(events[0]?.seq, 1, "user seed is not first");
  const firstUpdate = events[0] ? getSessionUpdate(events[0].data) : null;
  assert.equal(
    firstUpdate ? getSessionUpdateKind(firstUpdate) : "",
    "user_message_chunk",
    "first fixture event is not the user seed",
  );
  assert.equal(
    shouldFlushUserEcho("session/update", "agent_thought_chunk"),
    true,
    "user echo would not flush before agent activity",
  );
  const firstToolStart = events.findIndex((event) => {
    const update = getSessionUpdate(event.data);
    return update ? getSessionUpdateKind(update) === "tool_call" : false;
  });
  assert.ok(firstToolStart >= 0, "no tool_call start found");
  assert.ok(firstToolStart > 1, "user seed must precede agent activity");

  // The same chain the app uses must not collapse 145 starts into one card.
  const { rail, toolStarts, userEchoCards } = replayThroughAcp(events);
  const toolCards = rail.filter((event) => event.type === "tool_call");
  assert.equal(toolStarts.length, 145, "parse dropped tool starts");
  assert.equal(toolCards.length, 145, "event rail collapsed tool cards");
  assert.ok(userEchoCards <= 1, "user echo produced more than one card");

  const railToolIds = toolCards
    .map((card) => (card.type === "tool_call" ? card.toolCallId : ""))
    .filter(Boolean);
  assert.deepEqual(railToolIds, toolStarts, "tool card order changed during replay");

  const assistantCards = rail.filter((event) => event.type === "assistant_message");
  const thoughtCards = rail.filter((event) => event.type === "thought");
  assert.ok(assistantCards.length >= 1, "no assistant card survived replay");
  assert.ok(thoughtCards.length >= 1, "no thought card survived replay");

  const lastToolIndex = rail.reduce(
    (last, event, index) => (event.type === "tool_call" ? index : last),
    -1,
  );
  const lastAssistantIndex = rail.reduce(
    (last, event, index) => (event.type === "assistant_message" ? index : last),
    -1,
  );
  assert.ok(
    lastAssistantIndex > lastToolIndex,
    "final assistant turn text must come after the last tool card",
  );

  const pathCards = toolCards.filter(
    (card) => card.type === "tool_call" && typeof card.path === "string" && card.path.length > 0,
  );
  assert.ok(pathCards.length >= 100, `only ${pathCards.length} tool cards carry a path`);

  const titledCards = toolCards.filter(
    (card) => card.type === "tool_call" && typeof card.title === "string" && card.title.length > 0,
  );
  assert.equal(titledCards.length, 145, "a tool card lost its title");
  const completedCards = toolCards.filter(
    (card) => card.type === "tool_call" && card.status === "completed",
  );
  assert.ok(completedCards.length >= 100, `only ${completedCards.length} tool cards completed`);

  console.log("subagent fixture self-test: OK");
  console.log(
    `  lines=${summary.lines} tool_call=${summary.toolCallStarts} ` +
      `tool_call_update=${summary.toolCallUpdates} thoughts=${summary.thoughtChunks} ` +
      `assistant=${summary.assistantChunks} user=${summary.userChunks}`,
  );
  console.log(
    `  replay: cards=${rail.length} toolCards=${toolCards.length} ` +
      `assistantCards=${assistantCards.length} thoughtCards=${thoughtCards.length} ` +
      `userEchoCards=${userEchoCards} paths=${pathCards.length} completed=${completedCards.length}`,
  );
}

main();
