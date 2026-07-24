import type { SessionEvent } from "./types";

const PERSISTABLE = new Set([
  "user_message",
  "assistant_message",
  "thought",
  "tool_call",
  "handoff_prepared",
  "file_change",
]);

export function isPersistableEvent(event: SessionEvent): boolean {
  return PERSISTABLE.has(event.type);
}

export function persistableEventsForSession(
  events: SessionEvent[],
  sessionId: string,
): SessionEvent[] {
  return events.filter((e) => e.sessionId === sessionId && isPersistableEvent(e));
}

/** Best-effort revive of JSONL rows written by write_transcript. */
export function parseTranscriptEvents(raw: unknown[]): SessionEvent[] {
  const out: SessionEvent[] = [];
  for (const row of raw) {
    if (!row || typeof row !== "object") continue;
    const e = row as Record<string, unknown>;
    const type = typeof e.type === "string" ? e.type : "";
    const sessionId = typeof e.sessionId === "string" ? e.sessionId : "";
    const createdAt = typeof e.createdAt === "string" ? e.createdAt : new Date().toISOString();
    if (!sessionId || !PERSISTABLE.has(type)) continue;

    if (type === "user_message" || type === "assistant_message" || type === "thought") {
      const text = typeof e.text === "string" ? e.text : "";
      const messageId = typeof e.messageId === "string" ? e.messageId : undefined;
      out.push({
        type,
        sessionId,
        text,
        createdAt,
        ...(messageId ? { messageId } : {}),
      } as SessionEvent);
      continue;
    }
    if (type === "tool_call") {
      out.push({
        type: "tool_call",
        sessionId,
        text: typeof e.text === "string" ? e.text : "",
        createdAt,
        toolCallId: typeof e.toolCallId === "string" ? e.toolCallId : undefined,
        status: typeof e.status === "string" ? e.status : undefined,
        title: typeof e.title === "string" ? e.title : undefined,
      });
      continue;
    }
    if (type === "handoff_prepared") {
      out.push({
        type: "handoff_prepared",
        sessionId,
        targetAgentId: typeof e.targetAgentId === "string" ? e.targetAgentId : "",
        handoffPath: typeof e.handoffPath === "string" ? e.handoffPath : "",
        prompt: typeof e.prompt === "string" ? e.prompt : "",
        createdAt,
      });
      continue;
    }
    if (type === "file_change") {
      const changeType = e.changeType;
      if (
        changeType === "added" ||
        changeType === "modified" ||
        changeType === "deleted"
      ) {
        out.push({
          type: "file_change",
          sessionId,
          path: typeof e.path === "string" ? e.path : "",
          changeType,
          createdAt,
        });
      }
    }
  }
  return out;
}

export function titleFromUserText(text: string): string {
  const oneLine = text.replace(/\s+/g, " ").trim();
  if (!oneLine) return "New session";
  if (oneLine.length <= 48) return oneLine;
  return `${oneLine.slice(0, 46)}…`;
}

export function shouldAutoRenameLabel(label: string): boolean {
  const t = label.trim();
  return (
    t === "" ||
    t === "New session" ||
    t === "New conversation" ||
    /^session-\d+$/i.test(t)
  );
}
