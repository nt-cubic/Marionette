/**
 * Codex / Claude / OpenCode session meta extraction (Codeg-aligned, pure data).
 *
 * - Codex: `session_info_update` → `_meta.codex.goal` / `.error` (retry)
 * - Claude: chunk `_meta.claudeCode.parentToolUseId` → subagent stream
 * - OpenCode v2: `_meta["opencode/child-session"]` or `opencode/session/child_update`
 * - Tool content: `agent_transcript` / nested subagent text
 */

/** OpenCode v2 child events when the client advertised child-session-updates. */
export const OPENCODE_CHILD_UPDATE_METHOD = "opencode/session/child_update";
/** Client capability OpenCode v2 checks before using the custom child method. */
export const OPENCODE_CHILD_SESSION_UPDATES = "opencode/child-session-updates";
/** Grok Build custom session channel (subagent_spawned / subagent_finished / turn_completed). */
export const GROK_SESSION_UPDATE_METHOD = "_x.ai/session/update";

export function isAcpTranscriptMethod(method: string | null | undefined): boolean {
  return (
    method === "session/update" ||
    method === OPENCODE_CHILD_UPDATE_METHOD ||
    method === GROK_SESSION_UPDATE_METHOD
  );
}

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

export type OpenCodeChildSession = {
  id: string;
  parentID: string;
  depth: number;
  title?: string;
};

/**
 * OpenCode v2 Task/subagent projection (`packages/cli/src/acp/child.ts`).
 *
 * Without `opencode/child-session-updates`, child tokens land on the parent
 * `session/update` with `_meta["opencode/child-session"]`. With the capability,
 * the same fields sit on `opencode/session/child_update` params.
 */
export function opencodeChildSession(update: Record<string, unknown>): OpenCodeChildSession | null {
  const meta = getMeta(update);
  const nested =
    (meta && (asRecord(meta["opencode/child-session"]) ?? asRecord(meta.opencodeChildSession))) ||
    null;
  const id =
    (nested && typeof nested.id === "string" && nested.id.trim()) ||
    (typeof update.childSessionId === "string" && update.childSessionId.trim()) ||
    (typeof update.child_session_id === "string" && update.child_session_id.trim()) ||
    "";
  if (!id) return null;
  const parentID =
    (nested && typeof nested.parentID === "string" && nested.parentID) ||
    (nested && typeof nested.parentId === "string" && nested.parentId) ||
    (typeof update.parentSessionId === "string" && update.parentSessionId) ||
    (typeof update.parent_session_id === "string" && update.parent_session_id) ||
    "";
  const depthRaw = nested?.depth ?? update.depth;
  const depth = typeof depthRaw === "number" && Number.isFinite(depthRaw) ? depthRaw : 1;
  const titleRaw =
    (nested && typeof nested.title === "string" && nested.title) ||
    (typeof update.title === "string" && update.title) ||
    "";
  const title = titleRaw.trim() || undefined;
  return { id, parentID, depth, title };
}

/**
 * Id to nest this chunk under a tool card: Claude's parent Agent tool, or the
 * OpenCode child session (mapped onto the parent Task card at apply time).
 */
export function nestedSubagentParentId(update: Record<string, unknown>): string | null {
  return claudeParentToolUseId(update) ?? opencodeChildSession(update)?.id ?? null;
}

export type GrokSubagentUpdate = {
  kind: "spawned" | "finished";
  subagentId: string;
  childSessionId: string;
  description: string;
  subagentType?: string;
  model?: string;
  status?: string;
  output?: string;
  durationMs?: number;
  toolCalls?: number;
};

/**
 * Grok Build `_x.ai/session/update` with sessionUpdate `subagent_spawned` /
 * `subagent_finished`. Child tokens stay in the child session; the parent
 * only gets these two envelopes plus a later wait-tool MultiResult.
 */
export function parseGrokSubagentUpdate(
  update: Record<string, unknown>,
): GrokSubagentUpdate | null {
  const kindRaw = String(update.sessionUpdate ?? update.type ?? "").trim().toLowerCase();
  const kind: GrokSubagentUpdate["kind"] | null =
    kindRaw === "subagent_spawned" || kindRaw === "subagent-spawned"
      ? "spawned"
      : kindRaw === "subagent_finished" || kindRaw === "subagent-finished"
        ? "finished"
        : null;
  if (!kind) return null;
  const subagentId =
    (typeof update.subagent_id === "string" && update.subagent_id.trim()) ||
    (typeof update.subagentId === "string" && update.subagentId.trim()) ||
    "";
  const childSessionId =
    (typeof update.child_session_id === "string" && update.child_session_id.trim()) ||
    (typeof update.childSessionId === "string" && update.childSessionId.trim()) ||
    subagentId;
  const id = subagentId || childSessionId;
  if (!id) return null;
  const description =
    (typeof update.description === "string" && update.description.trim()) ||
    (typeof update.title === "string" && update.title.trim()) ||
    "";
  const subagentType =
    (typeof update.subagent_type === "string" && update.subagent_type.trim()) ||
    (typeof update.subagentType === "string" && update.subagentType.trim()) ||
    undefined;
  const model = typeof update.model === "string" && update.model.trim() ? update.model.trim() : undefined;
  const status = typeof update.status === "string" && update.status.trim() ? update.status.trim() : undefined;
  const output =
    (typeof update.output === "string" && update.output) ||
    (typeof update.result === "string" && update.result) ||
    undefined;
  const durationRaw = update.duration_ms ?? update.durationMs;
  const durationMs =
    typeof durationRaw === "number" && Number.isFinite(durationRaw) ? durationRaw : undefined;
  const toolCallsRaw = update.tool_calls ?? update.toolCalls;
  const toolCalls =
    typeof toolCallsRaw === "number" && Number.isFinite(toolCallsRaw) ? toolCallsRaw : undefined;
  return {
    kind,
    subagentId: id,
    childSessionId: childSessionId || id,
    description,
    subagentType,
    model,
    status,
    output,
    durationMs,
    toolCalls,
  };
}

/** `opencode/session/child_update` status → ACP tool status. */
export function opencodeChildToolStatus(status: string): string | null {
  switch (status.trim().toLowerCase()) {
    case "created":
    case "running":
      return "in_progress";
    case "completed":
      return "completed";
    case "failed":
      return "failed";
    case "interrupted":
      return "cancelled";
    default:
      return null;
  }
}

export type CompactionMarker = {
  trigger: "manual" | "auto" | "unknown";
  summary: string;
};

/** Claude / DeepSeek / Codex context compaction on session_info_update or a dedicated update. */
export function parseCompactionUpdate(update: Record<string, unknown>): CompactionMarker | null {
  const kind = String(update.sessionUpdate ?? update.type ?? "").toLowerCase();
  const meta = getMeta(update);
  const compaction =
    (meta && (asRecord(meta.compaction) || asRecord(meta.contextCompaction))) ||
    asRecord(update.compaction) ||
    asRecord(update.contextCompaction);
  const looksLike =
    kind.includes("compaction") ||
    kind.includes("compact") ||
    compaction != null;
  if (!looksLike) return null;
  const triggerRaw = compaction
    ? String(compaction.trigger ?? compaction.source ?? compaction.reason ?? "")
    : "";
  const trigger: CompactionMarker["trigger"] = /manual|user|command|slash/i.test(triggerRaw)
    ? "manual"
    : /auto|threshold|limit|window/i.test(triggerRaw)
      ? "auto"
      : kind.includes("manual")
        ? "manual"
        : "unknown";
  const summary =
    (compaction && typeof compaction.summary === "string" && compaction.summary.trim()) ||
    (typeof update.message === "string" && update.message.trim()) ||
    (typeof update.text === "string" && update.text.trim()) ||
    (trigger === "manual" ? "手动压缩了上下文。" : trigger === "auto" ? "自动压缩了上下文。" : "上下文已压缩。");
  return { trigger, summary };
}

export type SessionNotice = {
  severity: "info" | "warning" | "error";
  title: string;
  description: string;
};

function noticeSeverity(raw: unknown): SessionNotice["severity"] {
  const s = String(raw ?? "").toLowerCase();
  if (s === "error" || s === "danger" || s === "fatal") return "error";
  if (s === "warning" || s === "warn") return "warning";
  return "info";
}

/**
 * ACP `sessionUpdate: "notice"` (v1 clientCapabilities.session.notices).
 * Advisory only — must not become conversation history.
 */
export function parseSessionNoticeUpdate(update: Record<string, unknown>): SessionNotice | null {
  const kind = String(update.sessionUpdate ?? update.type ?? "").toLowerCase();
  if (
    kind !== "notice" &&
    kind !== "session_notice" &&
    !kind.endsWith("/notice")
  ) {
    return null;
  }
  const nested = asRecord(update.notice);
  const src = nested ?? update;
  const title =
    (typeof src.title === "string" && src.title.trim()) ||
    (typeof src.message === "string" && src.message.trim()) ||
    "";
  if (!title) return null;
  const description =
    (typeof src.description === "string" && src.description.trim()) ||
    (typeof src.detail === "string" && src.detail.trim()) ||
    (typeof src.body === "string" && src.body.trim()) ||
    "";
  return {
    severity: noticeSeverity(src.severity ?? src.level),
    title,
    description,
  };
}

/**
 * Live advisory that belongs above the composer, not in the chat:
 * protocol notices, Codex retry, Claude AIR sessionFailure.
 */
export function parseLiveNotice(update: Record<string, unknown>): SessionNotice | null {
  const notice = parseSessionNoticeUpdate(update);
  if (notice) return notice;
  const retry = parseCodexRetryUpdate(update);
  if (retry) {
    return {
      severity: "warning",
      title: retry.httpStatus != null ? `正在重试（HTTP ${retry.httpStatus}）` : "正在重试…",
      description: retry.message,
    };
  }
  const failure = parseSessionFailureUpdate(update);
  if (failure) {
    const extra = failure.details ? ` ${failure.details}` : "";
    return {
      severity: "error",
      title: failure.title,
      description: `${failure.category}${extra}`.trim(),
    };
  }
  return null;
}

export type SessionFailureMarker = {
  title: string;
  details: string;
  category: string;
  actions: string[];
};

/** Claude AIR `sessionFailure` (client advertised jetbrains.air.sessionFailure). */
export function parseSessionFailureUpdate(update: Record<string, unknown>): SessionFailureMarker | null {
  const meta = getMeta(update);
  if (!meta) return null;
  const jetbrains = asRecord(meta.jetbrains) ?? asRecord(meta["jetbrains.air"]);
  const air = jetbrains ? asRecord(jetbrains.air) ?? jetbrains : asRecord(meta["jetbrains.air"]);
  const failure = air ? asRecord(air.sessionFailure) : asRecord(meta.sessionFailure);
  if (!failure) return null;
  const title =
    (typeof failure.title === "string" && failure.title.trim()) ||
    (typeof failure.category === "string" && failure.category.trim()) ||
    "Session failure";
  const details =
    (typeof failure.details === "string" && failure.details.trim()) ||
    (typeof failure.detail === "string" && failure.detail.trim()) ||
    "";
  const category = typeof failure.category === "string" ? failure.category : "unknown";
  const actions = Array.isArray(failure.actions)
    ? failure.actions.filter((a): a is string => typeof a === "string")
    : [];
  return { title, details, category, actions };
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
