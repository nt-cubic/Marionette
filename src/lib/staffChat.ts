import type { Session, SessionEvent } from "./types";

export type StaffChatIdentity = {
  name: string;
  title: string;
};

export type StaffDialogueEvent = Extract<
  SessionEvent,
  { type: "user_message" | "assistant_message" }
>;

export type StaffChatEvent = Extract<
  SessionEvent,
  {
    type:
      | "user_message"
      | "assistant_message"
      | "notice"
      | "plan_prompt"
      | "ask_prompt";
  }
>;

/** Staff sessions are people-facing; worker sessions remain agent workbenches. */
export function isStaffChatSession(
  session: Pick<Session, "origin"> | null | undefined,
): boolean {
  return session?.origin === "chief";
}

export function isStaffDialogueEvent(event: SessionEvent): event is StaffDialogueEvent {
  return event.type === "user_message" || event.type === "assistant_message";
}

export function isStaffChatEvent(event: SessionEvent): event is StaffChatEvent {
  return (
    isStaffDialogueEvent(event) ||
    (event.type === "notice" && event.severity !== "info") ||
    event.type === "plan_prompt" ||
    event.type === "ask_prompt"
  );
}

/**
 * Keep the people-facing chat at message level. The raw transcript still holds
 * thoughts, tools and delivery receipts for audit and worker drill-down.
 */
export function selectStaffChatEvents(
  events: readonly SessionEvent[],
  sessionId: string,
): StaffChatEvent[] {
  return events.filter(
    (event): event is StaffChatEvent =>
      event.sessionId === sessionId && isStaffChatEvent(event),
  );
}

/** Typing is honest only while the employee is composing a direct reply. */
export function staffChatAwaitingReply(
  events: readonly StaffChatEvent[],
  running: boolean,
): boolean {
  if (!running) return false;
  for (let index = events.length - 1; index >= 0; index -= 1) {
    const event = events[index]!;
    if (event.type === "ask_prompt" || event.type === "plan_prompt") return false;
    if (!isStaffDialogueEvent(event)) continue;
    return event.type === "user_message";
  }
  return true;
}

/** Legacy Chief sessions only have a label; staff sessions also have a profile. */
export function staffChatIdentity(
  label: string,
  fallbackName: string,
): StaffChatIdentity {
  const [rawName, ...titleParts] = label.split(" \u00b7 ");
  const name = rawName?.trim() || fallbackName.trim() || "Assistant";
  const title = titleParts.join(" \u00b7 ").trim();
  return { name, title };
}

export function staffMonogram(name: string): string {
  return Array.from(name.trim())[0]?.toLocaleUpperCase() || "?";
}
