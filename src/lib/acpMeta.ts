/**
 * Codex / Claude session meta extraction (Codeg-aligned, pure data).
 *
 * - Codex: `session_info_update` → `_meta.codex.goal` / `.error` (retry)
 * - Claude: chunk `_meta.claudeCode.parentToolUseId` → subagent stream
 * - Tool content: `agent_transcript` / nested subagent text
 */

export type CodexGoalMarker = {
  toolName: "create_goal" | "update_goal";
  toolCallId: string;
  title: string;
  objective: string;
  status: string;
  detailJson: string;
};

export type CodexRetryInfo = {
  message: string;
  httpStatus?: number | null;
};

/** Module-level goal occurrence counter (per app session is fine). */
let goalOccurrence = 0;

function asRecord(v: unknown): Record<string, unknown> | null {
  return v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : null;
}

function getMeta(update: Record<string, unknown>): Record<string, unknown> | null {
  return asRecord(update._meta) ?? asRecord(update.meta);
}

function getCodexMeta(update: Record<string, unknown>): Record<string, unknown> | null {
  const meta = getMeta(update);
  return meta ? asRecord(meta.codex) : null;
}

/**
 * Parse Codex goal from session_info_update (or any update carrying _meta.codex.goal).
 * Call once per update; returns null when absent / unusable.
 */
export function parseCodexGoalUpdate(update: Record<string, unknown>): CodexGoalMarker | null {
  const codex = getCodexMeta(update);
  if (!codex) return null;
  const goal = codex.goal;
  if (goal == null) {
    // Null goal = clear. Without remembered objective we skip (no blank card).
    return null;
  }
  const g = asRecord(goal);
  if (!g) return null;
  const objective =
    typeof g.objective === "string" ? g.objective.trim() : "";
  if (!objective) return null;
  let status =
    typeof g.status === "string" && g.status.trim()
      ? g.status.trim().toLowerCase()
      : "active";
  if (!status) status = "active";
  const toolName = status === "active" ? "create_goal" : "update_goal";
  goalOccurrence += 1;
  return {
    toolName,
    toolCallId: `codex-goal-${goalOccurrence}`,
    title: `Goal updated (${status}): ${objective}`,
    objective,
    status,
    detailJson: JSON.stringify({ goal: { ...g, objective, status } }),
  };
}

/** Codex willRetry error under `_meta.codex.error` — turn stays alive. */
export function parseCodexRetryUpdate(update: Record<string, unknown>): CodexRetryInfo | null {
  const codex = getCodexMeta(update);
  if (!codex) return null;
  const err = asRecord(codex.error);
  if (!err) return null;
  if (err.willRetry === false) return null;
  const message = typeof err.message === "string" ? err.message.trim() : "";
  if (!message) return null;
  let httpStatus: number | null = null;
  const info = err.codexErrorInfo;
  if (info && typeof info === "object") {
    for (const v of Object.values(info as Record<string, unknown>)) {
      const inner = asRecord(v);
      if (inner && typeof inner.httpStatusCode === "number") {
        httpStatus = inner.httpStatusCode;
        break;
      }
    }
  }
  return { message, httpStatus };
}

/**
 * Claude Code ≥0.63: subagent chunks carry parent tool id when client
 * advertised `_meta.subagent-transcript: true`.
 */
export function claudeParentToolUseId(update: Record<string, unknown>): string | null {
  const meta = getMeta(update);
  if (!meta) return null;
  const claude = asRecord(meta.claudeCode) ?? asRecord(meta["claude-code"]);
  if (!claude) return null;
  const id =
    (typeof claude.parentToolUseId === "string" && claude.parentToolUseId) ||
    (typeof claude.parent_tool_use_id === "string" && claude.parent_tool_use_id) ||
    "";
  return id.trim() || null;
}

/** CodeBuddy marks subagent chunks — we do not suppress, but can parent if id known. */
export function codebuddyIsSubagent(update: Record<string, unknown>): boolean {
  const meta = getMeta(update);
  if (!meta) return false;
  if (meta["codebuddy.ai/isSubagent"] === true) return true;
  const cb = asRecord(meta["codebuddy.ai"]);
  return cb?.isSubagent === true;
}

/**
 * Pull nested agent transcript text from tool content blocks (if any).
 * Codeg: `ContentBlock.tool_result.agent_transcript`.
 */
export function extractAgentTranscript(update: Record<string, unknown>): string {
  const chunks: string[] = [];
  const visit = (node: unknown) => {
    if (!node) return;
    if (typeof node === "string") {
      if (node.trim()) chunks.push(node);
      return;
    }
    if (Array.isArray(node)) {
      for (const item of node) visit(item);
      return;
    }
    const o = asRecord(node);
    if (!o) return;
    if (typeof o.agent_transcript === "string" && o.agent_transcript.trim()) {
      chunks.push(o.agent_transcript);
    }
    if (typeof o.agentTranscript === "string" && o.agentTranscript.trim()) {
      chunks.push(o.agentTranscript);
    }
    if (typeof o.text === "string" && o.type === "agent_transcript") {
      chunks.push(o.text);
    }
    for (const v of Object.values(o)) {
      if (v && typeof v === "object") visit(v);
    }
  };
  visit(update.content);
  visit(update.rawOutput ?? update.raw_output);
  return chunks.join("\n").trim();
}
