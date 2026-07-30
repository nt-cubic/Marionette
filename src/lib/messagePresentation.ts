import type { SessionEvent } from "./types";

export type IndexedSessionEvent = {
  event: SessionEvent;
  /** Index in the raw transcript. Kept stable for React keys and anchors. */
  index: number;
};

export type ReplyMetadata = {
  agentId?: string;
  agentLabel?: string;
  modelId?: string;
  modelLabel?: string;
  modeLabel?: string;
  effortLabel?: string;
  durationMs?: number;
  startedAt: string;
};

export type ReplyPart =
  | { type: "text"; event: IndexedSessionEvent; text: string }
  | { type: "thought"; event: IndexedSessionEvent; text: string }
  | { type: "tool"; event: IndexedSessionEvent; toolCallId?: string }
  | { type: "delegation"; event: IndexedSessionEvent; childSessionId: string }
  | { type: "file_change"; event: IndexedSessionEvent; path: string };

export type ReplyGroupItem = {
  kind: "reply_group";
  key: string;
  events: IndexedSessionEvent[];
  parts: ReplyPart[];
  metadata: ReplyMetadata;
};

export type StandaloneTranscriptItem = {
  kind: "event";
  key: string;
  event: SessionEvent;
  index: number;
};

export type MessagePresentationItem = ReplyGroupItem | StandaloneTranscriptItem;

type AssistantEvent = Extract<SessionEvent, { type: "assistant_message" }>;

/**
 * Events that belong to an agent turn. They stay raw in the transcript, but
 * the presentation layer can place them inside one ReplyGroup so a tool round
 * does not manufacture another Reply header.
 */
function isTurnPart(event: SessionEvent): boolean {
  return (
    event.type === "assistant_message" ||
    event.type === "thought" ||
    event.type === "tool_call" ||
    event.type === "file_change" ||
    event.type === "subtask_started" ||
    event.type === "subtask_result"
  );
}

function isReplyBoundary(event: SessionEvent): boolean {
  return event.type === "user_message" || event.type === "handoff_prepared";
}

function eventIdentity(event: SessionEvent, index: number): string {
  if (event.type === "assistant_message" || event.type === "thought") {
    return event.messageId || event.createdAt || `index-${index}`;
  }
  if (event.type === "tool_call") {
    return event.toolCallId || event.createdAt || `index-${index}`;
  }
  return event.createdAt || `index-${index}`;
}

function eventSnapshot(event: SessionEvent): string {
  switch (event.type) {
    case "user_message":
      return `${event.type}|${event.sessionId}|${event.messageId ?? ""}|${event.createdAt}|${event.text}`;
    case "assistant_message":
      return `${event.type}|${event.sessionId}|${event.messageId ?? ""}|${event.createdAt}|${event.text}|${event.durationMs ?? ""}`;
    case "thought":
      return `${event.type}|${event.sessionId}|${event.messageId ?? ""}|${event.createdAt}|${event.text}`;
    case "tool_call":
      return `${event.type}|${event.sessionId}|${event.toolCallId ?? ""}|${event.createdAt}|${event.status ?? ""}|${event.text}|${event.detail ?? ""}`;
    case "file_change":
      return `${event.type}|${event.sessionId}|${event.createdAt}|${event.path}|${event.changeType}|${event.revision ?? ""}`;
    case "subtask_started":
      return `${event.type}|${event.sessionId}|${event.childSessionId}|${event.createdAt}|${event.prompt}`;
    case "subtask_result":
      return `${event.type}|${event.sessionId}|${event.childSessionId}|${event.createdAt}|${event.status}|${event.summary}`;
    case "handoff_prepared":
      return `${event.type}|${event.sessionId}|${event.createdAt}|${event.handoffPath}`;
    default:
      return "";
  }
}

/** Drop only exact adjacent snapshots; never collapse legitimate updates. */
function dedupeExactAdjacentEvents(events: SessionEvent[]): IndexedSessionEvent[] {
  const out: IndexedSessionEvent[] = [];
  for (let index = 0; index < events.length; index += 1) {
    const event = events[index];
    const previous = out[out.length - 1];
    if (
      previous &&
      (previous.event === event ||
        (eventIdentity(previous.event, previous.index) === eventIdentity(event, index) &&
          eventSnapshot(previous.event) === eventSnapshot(event)))
    ) {
      continue;
    }
    out.push({ event, index });
  }
  return out;
}

function lastValue<T>(events: AssistantEvent[], key: keyof AssistantEvent): T | undefined {
  for (let i = events.length - 1; i >= 0; i -= 1) {
    const value = events[i][key];
    if (value !== undefined && value !== null && value !== "") return value as T;
  }
  return undefined;
}

function buildReplyMetadata(
  events: IndexedSessionEvent[],
  fallbackStartedAt: string,
): ReplyMetadata {
  const assistants = events
    .map(({ event }) => event)
    .filter((event): event is AssistantEvent => event.type === "assistant_message");
  const first = assistants[0];
  const durationMs = assistants.reduce((sum, event) => sum + (event.durationMs ?? 0), 0);

  return {
    agentId: lastValue<string>(assistants, "agentId"),
    agentLabel: lastValue<string>(assistants, "agentLabel"),
    modelId: lastValue<string>(assistants, "modelId"),
    modelLabel: lastValue<string>(assistants, "modelLabel"),
    modeLabel: lastValue<string>(assistants, "modeLabel"),
    effortLabel: lastValue<string>(assistants, "effortLabel"),
    ...(durationMs > 0 ? { durationMs } : {}),
    startedAt: first?.createdAt ?? fallbackStartedAt,
  };
}

function buildReplyParts(events: IndexedSessionEvent[]): ReplyPart[] {
  return events.flatMap((item): ReplyPart[] => {
    const { event } = item;
    switch (event.type) {
      case "assistant_message":
        return [{ type: "text", event: item, text: event.text }];
      case "thought":
        return [{ type: "thought", event: item, text: event.text }];
      case "tool_call":
        return [{ type: "tool", event: item, toolCallId: event.toolCallId }];
      case "subtask_started":
      case "subtask_result":
        return [{ type: "delegation", event: item, childSessionId: event.childSessionId }];
      case "file_change":
        return [{ type: "file_change", event: item, path: event.path }];
      default:
        return [];
    }
  });
}

/**
 * Convert the flat event log into render items without mutating or rewriting
 * the persisted events. A user message/handoff ends a run; tool, thought,
 * delegation and assistant events between those boundaries stay in order
 * inside one ReplyGroup.
 */
export function buildMessagePresentation(
  events: SessionEvent[],
  sessionId?: string,
): MessagePresentationItem[] {
  const output: MessagePresentationItem[] = [];
  let buffer: IndexedSessionEvent[] = [];

  const flush = () => {
    if (buffer.length === 0) return;

    const hasReply = buffer.some(
      ({ event }) => event.type === "assistant_message" || event.type === "thought",
    );
    if (!hasReply) {
      for (const item of buffer) {
        output.push({
          kind: "event",
          key: `event-${eventIdentity(item.event, item.index)}-${item.index}`,
          event: item.event,
          index: item.index,
        });
      }
      buffer = [];
      return;
    }

    const first = buffer[0];
    output.push({
      kind: "reply_group",
      key: `reply-${eventIdentity(first.event, first.index)}-${first.index}`,
      events: buffer,
      parts: buildReplyParts(buffer),
      metadata: buildReplyMetadata(buffer, first.event.createdAt),
    });
    buffer = [];
  };

  for (const { event, index } of dedupeExactAdjacentEvents(events)) {
    if (sessionId && event.sessionId !== sessionId) {
      flush();
      output.push({
        kind: "event",
        key: `event-${eventIdentity(event, index)}-${index}`,
        event,
        index,
      });
      continue;
    }

    if (isReplyBoundary(event)) {
      flush();
      output.push({
        kind: "event",
        key: `event-${eventIdentity(event, index)}-${index}`,
        event,
        index,
      });
      continue;
    }

    if (isTurnPart(event)) {
      buffer.push({ event, index });
      continue;
    }

    flush();
    output.push({
      kind: "event",
      key: `event-${eventIdentity(event, index)}-${index}`,
      event,
      index,
    });
  }

  flush();
  return output;
}
