import type { SessionEvent } from "./types";

const PERSISTABLE = new Set([
  "user_message",
  "assistant_message",
  "thought",
  "tool_call",
  "handoff_prepared",
  "file_change",
  "subtask_started",
  "subtask_result",
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
        // Optional metadata — old transcripts won't have them, new ones will restore correctly
        ...(typeof e.agentId === "string" ? { agentId: e.agentId } : {}),
        ...(typeof e.agentLabel === "string" ? { agentLabel: e.agentLabel } : {}),
        ...(typeof e.modelId === "string" ? { modelId: e.modelId } : {}),
        ...(typeof e.modelLabel === "string" ? { modelLabel: e.modelLabel } : {}),
        ...(typeof e.modeLabel === "string" ? { modeLabel: e.modeLabel } : {}),
        ...(typeof e.effortLabel === "string" ? { effortLabel: e.effortLabel } : {}),
        ...(typeof e.durationMs === "number" ? { durationMs: e.durationMs } : {}),
        ...(type === "user_message" && Array.isArray(e.attachments)
          ? { attachments: e.attachments as import("./imageAttachments").ImageAttachment[] }
          : {}),
        ...(type === "user_message" && e.forceWebSearch === true
          ? { forceWebSearch: true }
          : {}),
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
        toolName: typeof e.toolName === "string" ? e.toolName : undefined,
        // Older transcripts only have `text`; the card falls back to it.
        path: typeof e.path === "string" ? e.path : undefined,
        detail: typeof e.detail === "string" ? e.detail : undefined,
        input: typeof e.input === "string" ? e.input : undefined,
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
      continue;
    }
    if (type === "subtask_started") {
      out.push({
        type: "subtask_started",
        sessionId,
        childSessionId: typeof e.childSessionId === "string" ? e.childSessionId : "",
        agentId: typeof e.agentId === "string" ? e.agentId : "",
        agentLabel: typeof e.agentLabel === "string" ? e.agentLabel : "",
        modelId: typeof e.modelId === "string" ? e.modelId : undefined,
        prompt: typeof e.prompt === "string" ? e.prompt : "",
        createdAt,
      });
      continue;
    }
    if (type === "subtask_result") {
      const st = e.status;
      const status =
        st === "done" || st === "failed" || st === "cancelled" || st === "timeout"
          ? st
          : "failed";
      out.push({
        type: "subtask_result",
        sessionId,
        childSessionId: typeof e.childSessionId === "string" ? e.childSessionId : "",
        agentId: typeof e.agentId === "string" ? e.agentId : "",
        status,
        summary: typeof e.summary === "string" ? e.summary : "",
        durationMs: typeof e.durationMs === "number" ? e.durationMs : undefined,
        error: typeof e.error === "string" ? e.error : undefined,
        createdAt,
      });
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
