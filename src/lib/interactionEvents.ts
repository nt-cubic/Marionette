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

/**
 * Is there a still-open prompt card for this request on the transcript rail?
 *
 * The composer is the only other input surface, and an Ask/Plan card is only
 * rendered where its prompt event exists. Hiding the composer for a prompt that
 * never reached the rail leaves the user with no way to answer at all, so the
 * hide is gated on this.
 */
export function promptCardOnRail(
  events: SessionEvent[],
  requestId: string | null | undefined,
): boolean {
  if (!requestId) return false;
  return events.some(
    (row) => isPromptEvent(row) && row.requestId === requestId && row.status === "pending",
  );
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

function lastPromptIndex(events: SessionEvent[], requestId: string): number {
  for (let i = events.length - 1; i >= 0; i -= 1) {
    const row = events[i];
    if (isPromptEvent(row) && row.requestId === requestId) return i;
  }
  return -1;
}

export function upsertPromptEvent(
  events: SessionEvent[],
  event: PromptEvent,
): SessionEvent[] {
  const idx = lastPromptIndex(events, event.requestId);
  if (idx < 0) return [...events, event];
  const prev = events[idx];
  if (!isPromptEvent(prev)) return [...events, event];
  // The agent reused a request id for a *new* prompt (a restarted process
  // restarts its counter). Overwriting the settled row would move the live
  // card back into history while the composer stays hidden for it, leaving no
  // surface to answer on. Keep the settled row and append the live one — and
  // close any older row still marked pending so only one card looks live.
  if (event.status === "pending" && prev.status !== "pending") {
    const retired = events.map((row) =>
      isPromptEvent(row) && row.requestId === event.requestId && row.status === "pending"
        ? ({ ...row, status: "timeout" } as PromptEvent)
        : row,
    );
    return [...retired, event];
  }
  const next = events.slice();
  next[idx] = { ...prev, ...event, createdAt: prev.createdAt } as PromptEvent;
  return next;
}

export function patchPromptEvent(
  events: SessionEvent[],
  requestId: string,
  patch: Partial<PromptEvent> & { status: PromptStatus },
): SessionEvent[] {
  // Prefer the newest still-open row so a reused id settles the card the user
  // is actually looking at, never an older answered one.
  let idx = -1;
  for (let i = events.length - 1; i >= 0; i -= 1) {
    const row = events[i];
    if (!isPromptEvent(row) || row.requestId !== requestId) continue;
    if (row.status === "pending") {
      idx = i;
      break;
    }
    if (idx < 0) idx = i;
  }
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




