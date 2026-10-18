import type { SessionEvent } from "./types";

export type PromptEvent = Extract<SessionEvent, { type: "plan_prompt" | "ask_prompt" }>;
export type PromptStatus = PromptEvent["status"];

export function isPromptEvent(event: SessionEvent): event is PromptEvent {
  return event.type === "plan_prompt" || event.type === "ask_prompt";
}

export function sessionHidesComposer(
  sessionId: string,
  liveAsk: { sessionId: string } | null | undefined,
  livePlan: { sessionId: string } | null | undefined,
): boolean {
  return liveAsk?.sessionId === sessionId || livePlan?.sessionId === sessionId;
}

export function planPromptEvent(input: {
  sessionId: string;
  requestId: string;
  source: "plan_approval" | "permission";
  title?: string;
  planMarkdown: string;
  options?: Array<{ optionId: string; name: string; kind: string }> | null;
  createdAt?: string;
}): Extract<SessionEvent, { type: "plan_prompt" }> {
  const options = (input.options ?? [])
    .filter((o) => o.optionId)
    .map((o) => ({
      optionId: o.optionId,
      name: o.name || o.optionId,
      kind: o.kind || "",
    }));
  return {
    type: "plan_prompt",
    sessionId: input.sessionId,
    requestId: input.requestId,
    source: input.source,
    ...(input.title ? { title: input.title } : {}),
    planMarkdown: input.planMarkdown,
    ...(options.length > 0 ? { options } : {}),
    status: "pending",
    createdAt: input.createdAt ?? new Date().toISOString(),
  };
}

export function askPromptEvent(
  prompt: {
    sessionId: string;
    requestId: string;
    mode?: string | null;
    questions: Extract<SessionEvent, { type: "ask_prompt" }>["questions"];
  },
  createdAt?: string,
): Extract<SessionEvent, { type: "ask_prompt" }> {
  return {
    type: "ask_prompt",
    sessionId: prompt.sessionId,
    requestId: prompt.requestId,
    ...(prompt.mode ? { mode: prompt.mode } : {}),
    questions: prompt.questions,
    status: "pending",
    createdAt: createdAt ?? new Date().toISOString(),
  };
}

export function upsertPromptEvent(
  events: SessionEvent[],
  event: PromptEvent,
): SessionEvent[] {
  const idx = events.findIndex(
    (row) => isPromptEvent(row) && row.requestId === event.requestId,
  );
  if (idx < 0) return [...events, event];
  const next = events.slice();
  const prev = events[idx];
  next[idx] = { ...prev, ...event, createdAt: prev.createdAt } as PromptEvent;
  return next;
}

export function patchPromptEvent(
  events: SessionEvent[],
  requestId: string,
  patch: Partial<PromptEvent> & { status: PromptStatus },
): SessionEvent[] {
  const idx = events.findIndex(
    (row) => isPromptEvent(row) && row.requestId === requestId,
  );
  if (idx < 0) return events;
  const next = events.slice();
  next[idx] = {
    ...events[idx],
    ...patch,
    answeredAt: patch.answeredAt ?? new Date().toISOString(),
  } as SessionEvent;
  return next;
}

/** Mark every still-open Plan/Ask on this session as timed out (process died / remote). */
export function expirePendingPrompts(
  events: SessionEvent[],
  sessionId: string,
  status: Exclude<PromptStatus, "pending"> = "timeout",
): SessionEvent[] {
  let changed = false;
  const answeredAt = new Date().toISOString();
  const next = events.map((row) => {
    if (!isPromptEvent(row) || row.sessionId !== sessionId || row.status !== "pending") {
      return row;
    }
    changed = true;
    const label = status === "timeout" ? "已超时" : "已在其他窗口回复";
    if (row.type === "plan_prompt") {
      return {
        ...row,
        status,
        decisionLabel: row.decisionLabel ?? label,
        answeredAt,
      };
    }
    return { ...row, status, answeredAt };
  });
  return changed ? next : events;
}

export function planDecisionLabel(
  decision: string | undefined,
  options?: Array<{ optionId: string; name: string }> | null,
): string {
  if (!decision) return "已回复";
  const named = options?.find((o) => o.optionId === decision);
  if (named?.name) return named.name;
  switch (decision) {
    case "approve":
      return "已批准并执行";
    case "request_changes":
      return "已要求修改";
    case "abandon":
      return "已放弃计划";
    default:
      return decision;
  }
}




