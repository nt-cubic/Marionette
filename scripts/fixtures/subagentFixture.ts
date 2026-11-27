/**
 * Pure helpers for the committed subagent replay fixture.
 *
 * No Node imports here on purpose: the fixture is plain JSONL and these
 * helpers can be imported by any TypeScript consumer (browser or node) that
 * has already read the text. The Node-only file loading lives in the
 * capture/verify scripts.
 *
 * Fixture shape (one JSON object per line):
 *   { sessionId, kind, method, data, seq, ts }
 * where `data.params.update.sessionUpdate` carries the ACP update kind for
 * `session/update`, and `method === "turn/complete"` is the turn boundary.
 *
 * See `scripts/fixtures/README.md` for provenance and redaction rules.
 */

export type SubagentFixtureEvent = {
  sessionId?: string;
  kind?: string;
  method?: string | null;
  data?: unknown;
  seq: number;
  ts?: number;
};

export type SubagentFixtureSummary = {
  lines: number;
  kinds: Record<string, number>;
  toolCallStarts: number;
  toolCallUpdates: number;
  distinctToolCallIds: number;
  toolUpdatesMissingStart: number;
  userChunks: number;
  thoughtChunks: number;
  assistantChunks: number;
  turnCompletions: number;
  firstSeq: number | null;
  lastSeq: number | null;
  seqStrictlyIncreasing: boolean;
  timestampsNonDecreasing: boolean;
};

export type FixtureValidation = {
  ok: boolean;
  errors: string[];
  summary: SubagentFixtureSummary;
};

function asRecord(value: unknown): Record<string, unknown> | null {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

/** The ACP `update` object for a session/update event, else null. */
export function fixtureUpdate(event: SubagentFixtureEvent): Record<string, unknown> | null {
  const data = asRecord(event.data);
  if (!data) return null;
  const params = asRecord(data.params);
  if (!params) return null;
  return asRecord(params.update);
}

/** Event kind used for counting: sessionUpdate, or `event:<method>`. */
export function fixtureEventKind(event: SubagentFixtureEvent): string {
  const update = fixtureUpdate(event);
  const kind = update?.sessionUpdate;
  if (typeof kind === "string" && kind) return kind;
  if (typeof event.method === "string" && event.method) return `event:${event.method}`;
  return "unknown";
}

/** Tool call id carried by a tool_call / tool_call_update event. */
export function fixtureToolCallId(event: SubagentFixtureEvent): string {
  const update = fixtureUpdate(event);
  const id = update?.toolCallId ?? update?.tool_call_id;
  return typeof id === "string" ? id : "";
}

/** Message id carried by a chunk event. */
export function fixtureMessageId(event: SubagentFixtureEvent): string {
  const update = fixtureUpdate(event);
  const id = update?.messageId ?? update?.message_id;
  return typeof id === "string" ? id : "";
}

/**
 * Parse a JSONL fixture. Throws with the line number on any malformed line so
 * a corrupt fixture can never silently look like an empty one.
 */
export function parseSubagentFixture(text: string): SubagentFixtureEvent[] {
  const events: SubagentFixtureEvent[] = [];
  const lines = text.split(/\r?\n/);
  for (let index = 0; index < lines.length; index += 1) {
    const raw = lines[index] ?? "";
    if (!raw.trim()) continue;
    let parsed: unknown;
    try {
      parsed = JSON.parse(raw);
    } catch {
      throw new Error(`subagent fixture line ${index + 1}: invalid JSON`);
    }
    const record = asRecord(parsed);
    if (!record) throw new Error(`subagent fixture line ${index + 1}: not a JSON object`);
    const seq = record.seq;
    if (typeof seq !== "number" || !Number.isFinite(seq)) {
      throw new Error(`subagent fixture line ${index + 1}: missing numeric seq`);
    }
    events.push(record as SubagentFixtureEvent);
  }
  return events;
}

export function summarizeSubagentFixture(
  events: readonly SubagentFixtureEvent[],
): SubagentFixtureSummary {
  const kinds: Record<string, number> = {};
  const starts: string[] = [];
  const updateIds = new Set<string>();
  let toolCallUpdates = 0;
  let userChunks = 0;
  let thoughtChunks = 0;
  let assistantChunks = 0;
  let turnCompletions = 0;
  let seqStrictlyIncreasing = true;
  let timestampsNonDecreasing = true;
  let previousSeq: number | null = null;
  let previousTs: number | null = null;

  for (const event of events) {
    const kind = fixtureEventKind(event);
    kinds[kind] = (kinds[kind] ?? 0) + 1;
    if (kind === "tool_call") starts.push(fixtureToolCallId(event));
    if (kind === "tool_call_update") {
      toolCallUpdates += 1;
      updateIds.add(fixtureToolCallId(event));
    }
    if (kind === "user_message_chunk") userChunks += 1;
    if (kind === "agent_thought_chunk") thoughtChunks += 1;
    if (kind === "agent_message_chunk") assistantChunks += 1;
    if (event.method === "turn/complete") turnCompletions += 1;

    if (previousSeq !== null && event.seq <= previousSeq) seqStrictlyIncreasing = false;
    previousSeq = event.seq;

    if (typeof event.ts === "number" && Number.isFinite(event.ts)) {
      if (previousTs !== null && event.ts < previousTs) timestampsNonDecreasing = false;
      previousTs = event.ts;
    }
  }

  const startSet = new Set(starts.filter(Boolean));
  let toolUpdatesMissingStart = 0;
  for (const id of updateIds) {
    if (!startSet.has(id)) toolUpdatesMissingStart += 1;
  }

  return {
    lines: events.length,
    kinds,
    toolCallStarts: starts.length,
    toolCallUpdates,
    distinctToolCallIds: startSet.size,
    toolUpdatesMissingStart,
    userChunks,
    thoughtChunks,
    assistantChunks,
    turnCompletions,
    firstSeq: events.length ? (events[0]?.seq ?? null) : null,
    lastSeq: events.length ? (events[events.length - 1]?.seq ?? null) : null,
    seqStrictlyIncreasing,
    timestampsNonDecreasing,
  };
}

/**
 * Structural validation for the committed fixture. This is deliberately about
 * survival of shape and ordering, not rendering: 100+ tool starts, every
 * tool update linked to a start that came before it, both stream kinds, a
 * turn boundary, and a user-echo seed before agent activity.
 */
export function validateSubagentFixture(
  events: readonly SubagentFixtureEvent[],
): FixtureValidation {
  const errors: string[] = [];
  const summary = summarizeSubagentFixture(events);

  if (summary.lines === 0) errors.push("fixture is empty");
  if (!summary.seqStrictlyIncreasing) errors.push("seq is not strictly increasing");
  if (!summary.timestampsNonDecreasing) errors.push("ts goes backwards");
  if (summary.toolCallStarts < 100) {
    errors.push(`tool_call starts ${summary.toolCallStarts} < 100`);
  }
  if (summary.toolCallStarts !== summary.distinctToolCallIds) {
    errors.push("duplicate tool_call ids in fixture");
  }
  if (summary.toolUpdatesMissingStart > 0) {
    errors.push(`${summary.toolUpdatesMissingStart} tool_call_update ids have no tool_call start`);
  }
  if (summary.userChunks < 1) errors.push("missing user_message_chunk seed");
  if (summary.thoughtChunks < 1) errors.push("missing agent_thought_chunk");
  if (summary.assistantChunks < 1) errors.push("missing agent_message_chunk");
  if (summary.toolCallUpdates < 1) errors.push("missing tool_call_update");
  if (summary.turnCompletions < 1) errors.push("missing turn/complete");

  const firstUser = events.findIndex((event) => fixtureEventKind(event) === "user_message_chunk");
  const firstNonUser = events.findIndex((event) => fixtureEventKind(event) !== "user_message_chunk");
  if (firstUser < 0) errors.push("no user_message_chunk seed");
  else if (firstNonUser >= 0 && firstUser > firstNonUser) {
    errors.push("user_message_chunk seed is not before agent activity");
  }

  const startIndex = new Map<string, number>();
  events.forEach((event, index) => {
    if (fixtureEventKind(event) === "tool_call") {
      startIndex.set(fixtureToolCallId(event), index);
    }
  });
  events.forEach((event, index) => {
    if (fixtureEventKind(event) !== "tool_call_update") return;
    const id = fixtureToolCallId(event);
    const start = startIndex.get(id);
    if (start === undefined) return; // already reported by toolUpdatesMissingStart
    if (index <= start) errors.push(`tool_call_update ${id} appears before its tool_call`);
  });

  return { ok: errors.length === 0, errors, summary };
}
