/**
 * Live nested-agent cards that still own the parent process.
 *
 * Grok's `spawn_subagent` completes immediately with "Subagent started in
 * background" while the child keeps running inside the same ACP process.
 * OpenCode/Claude Task cards stay `in_progress`. Either shape means the
 * dialog is still working — auto-park must leave it alone, and the
 * background-task panel should show a running row.
 */
import { t } from "./i18n";
import type { SessionEvent } from "./types";
import { isSubagentTool, isToolInProgress } from "./activityHealth";
import { parseBackgroundSubagentStart } from "./acpTranscript";
import {
  extractNestedAgentId,
  isSubagentWaitTool,
  parseCodexCollabSnapshot,
} from "./subagentAdapters";
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
      parseBackgroundSubagentStart(event.input) ||
      parseBackgroundSubagentStart(event.title),
  );
}

function clip(text: string, max: number): string {
  const raw = text.replace(/\s+/g, " ").trim();
  return raw.length > max ? `${raw.slice(0, max)}…` : raw;
}

type CodexChildMeta = {
  title?: string;
  prompt?: string;
  startedAtMs: number;
  detail?: string;
};

/**
 * Codex's spawnAgent call completes as soon as the child thread is registered;
 * the child keeps running while a separate collab `wait` tool is in progress.
 * Use that wait's receiverThreadIds as the live child roster and recover labels
 * from the still-present spawn cards.
 */
function codexCollabLiveRows(
  events: readonly SessionEvent[],
  sessionId: string,
): LiveSubagent[] {
  const meta = new Map<string, CodexChildMeta>();
  for (const event of events) {
    if (event.type !== "tool_call" || event.sessionId !== sessionId) continue;
    const collab = parseCodexCollabSnapshot(event.input);
    if (!collab) continue;
    const startedAtMs = Date.parse(event.createdAt);
    for (const threadId of collab.receiverThreadIds) {
      const state = collab.agentsStates[threadId];
      const prior = meta.get(threadId);
      const prompt = collab.prompt || prior?.prompt;
      meta.set(threadId, {
        title: prompt ? clip(prompt.split(/\r?\n/, 1)[0] || prompt, 72) : prior?.title,
        prompt,
        startedAtMs: prior?.startedAtMs ?? (Number.isFinite(startedAtMs) ? startedAtMs : Date.now()),
        detail: state?.message || prior?.detail,
      });
    }
  }

  const active = new Map<string, number>();
  for (const event of events) {
    if (event.type !== "tool_call" || event.sessionId !== sessionId) continue;
    if (!isToolInProgress(event.status)) continue;
    if (!isSubagentWaitTool(event.toolName, event.title)) continue;
    const collab = parseCodexCollabSnapshot(event.input);
    if (!collab || collab.receiverThreadIds.length === 0) continue;
    const startedAtMs = Date.parse(event.createdAt);
    for (const threadId of collab.receiverThreadIds) {
      if (!active.has(threadId)) {
        active.set(threadId, Number.isFinite(startedAtMs) ? startedAtMs : Date.now());
      }
    }
  }

  return [...active.entries()].map(([threadId, fallbackStartedAt]) => {
    const child = meta.get(threadId);
    return {
      sessionId,
      key: `codex:${threadId}`,
      title: child?.title || t("subagents.fallbackTitle"),
      detail: child?.detail || clip(child?.prompt || `thread: ${threadId}`, 180),
      startedAtMs: child?.startedAtMs ?? fallbackStartedAt,
    };
  });
}

/** True while this card still represents a nested agent that has not finished. */
export function isLiveSubagentEvent(
  event: SessionEvent,
  opts?: { processAlive?: boolean },
): boolean {
  if (event.type !== "tool_call") return false;
  if (opts?.processAlive === false) return false;
  if (!isSubagentTool(event.toolName, event.title, event.input)) return false;
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
  const seen = new Set<string>();
  rows.push(...codexCollabLiveRows(events, sessionId));
  for (const row of rows) seen.add(row.key);
  for (const event of events) {
    if (event.sessionId !== sessionId) continue;
    if (event.type !== "tool_call") continue;
    if (!isLiveSubagentEvent(event, opts)) continue;
    const stub = parseBackgroundSubagentStart(event.detail);
    const agentId =
      stub?.id ||
      extractNestedAgentId(event.input) ||
      extractNestedAgentId(event.detail) ||
      event.toolCallId ||
      event.createdAt;
    if (seen.has(agentId) || seen.has(`codex:${agentId}`)) continue;
    seen.add(agentId);
    const startedAtMs = Date.parse(event.createdAt);
    rows.push({
      sessionId,
      key: event.toolCallId || event.createdAt,
      title: (event.title && event.title.trim()) || toolMainText(event) || stub?.description || t("subagents.fallbackTitle"),
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
  if (opts?.processAlive === false) return false;
  if (codexCollabLiveRows(events, sessionId).length > 0) return true;
  for (const event of events) {
    if (event.sessionId !== sessionId) continue;
    if (isLiveSubagentEvent(event, opts)) return true;
  }
  return false;
}
