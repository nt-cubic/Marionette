/**
 * Live nested-agent cards that still own the parent process.
 *
 * Grok's `spawn_subagent` completes immediately with "Subagent started in
 * background" while the child keeps running inside the same ACP process.
 * OpenCode/Claude Task cards stay `in_progress`. Either shape means the
 * dialog is still working — auto-park must leave it alone, and the
 * background-task panel should show a running row.
 */
import type { SessionEvent } from "./types";
import { isSubagentTool, isToolInProgress } from "./activityHealth";
import { parseBackgroundSubagentStart } from "./acpTranscript";
import { toolMainText } from "./turnActivity";

export type LiveSubagent = {
  sessionId: string;
  key: string;
  title: string;
  detail?: string;
  startedAtMs: number;
};

export function sessionProcessAlive(status: string | null | undefined): boolean {
  return (
    status === "starting" ||
    status === "running" ||
    status === "waiting" ||
    status === "error"
  );
}

function backgroundStub(event: Extract<SessionEvent, { type: "tool_call" }>): boolean {
  return Boolean(
    parseBackgroundSubagentStart(event.detail) ||
      parseBackgroundSubagentStart(event.text) ||
      parseBackgroundSubagentStart(event.input),
  );
}

/** True while this card still represents a nested agent that has not finished. */
export function isLiveSubagentEvent(
  event: SessionEvent,
  opts?: { processAlive?: boolean },
): boolean {
  if (event.type !== "tool_call") return false;
  if (opts?.processAlive === false) return false;
  if (!isSubagentTool(event.toolName, event.title)) return false;
  if (isToolInProgress(event.status)) return true;
  return backgroundStub(event);
}

export function listLiveSubagents(
  events: readonly SessionEvent[],
  sessionId: string,
  opts?: { processAlive?: boolean },
): LiveSubagent[] {
  if (opts?.processAlive === false) return [];
  const rows: LiveSubagent[] = [];
  for (const event of events) {
    if (event.sessionId !== sessionId) continue;
    if (event.type !== "tool_call") continue;
    if (!isLiveSubagentEvent(event, opts)) continue;
    const startedAtMs = Date.parse(event.createdAt);
    const stub = parseBackgroundSubagentStart(event.detail);
    rows.push({
      sessionId,
      key: event.toolCallId || event.createdAt,
      title: (event.title && event.title.trim()) || toolMainText(event) || stub?.description || "子代理",
      detail: stub?.description || stub?.id || event.input?.split("\n")[0],
      startedAtMs: Number.isFinite(startedAtMs) ? startedAtMs : Date.now(),
    });
  }
  return rows;
}

export function sessionHasLiveSubagents(
  events: readonly SessionEvent[],
  sessionId: string,
  opts?: { processAlive?: boolean },
): boolean {
  for (const event of events) {
    if (event.sessionId !== sessionId) continue;
    if (isLiveSubagentEvent(event, opts)) return true;
  }
  return false;
}
