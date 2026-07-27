import type { SessionEvent } from "./types";

export type AcpTextPart = {
  role: "assistant" | "thought" | "tool" | "other";
  text: string;
  /** True when this looks like a streaming delta (append). False ≈ full snapshot (replace). */
  isDelta: boolean;
  sessionUpdate: string;
  messageId?: string;
  toolCallId?: string;
  toolStatus?: string;
  toolTitle?: string;
  /** First `locations[]` entry — what file the tool is working on. */
  toolPath?: string;
  /** Rendered `content[]` / `rawOutput` — what the tool actually produced. */
  toolDetail?: string;
  /** Clipped `rawInput` — only useful until real output arrives. */
  toolInput?: string;
};

type UpdateObj = Record<string, unknown>;

function asRecord(value: unknown): UpdateObj | null {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as UpdateObj)
    : null;
}

/** Pull the ACP `update` object from a session/update notification payload. */
export function getSessionUpdate(data: unknown): UpdateObj | null {
  const root = asRecord(data);
  if (!root) return null;
  const params = asRecord(root.params) ?? root;
  const update = asRecord(params.update) ?? params;
  return update;
}

export function getSessionUpdateKind(update: UpdateObj): string {
  return String(update.sessionUpdate ?? update.type ?? update.kind ?? "");
}

function contentBlockType(content: unknown): string {
  const c = asRecord(content);
  if (!c) return "";
  return typeof c.type === "string" ? c.type.toLowerCase() : "";
}

/** Per-card ceiling for tool output — enough to follow along, not a file dump. */
const MAX_TOOL_DETAIL = 4000;

function clipToolDetail(text: string): string {
  const trimmed = text.trimEnd();
  if (trimmed.length <= MAX_TOOL_DETAIL) return trimmed;
  const extra = trimmed.length - MAX_TOOL_DETAIL;
  return `${trimmed.slice(0, MAX_TOOL_DETAIL)}\n… (+${extra} more characters)`;
}

/** `locations[0].path` — the file a tool is reading/editing (follow-along cue). */
function firstToolLocation(update: UpdateObj): string | undefined {
  const locations = update.locations ?? update.location;
  const list = Array.isArray(locations) ? locations : locations != null ? [locations] : [];
  for (const entry of list) {
    if (typeof entry === "string" && entry.trim()) return entry.trim();
    const record = asRecord(entry);
    const path = record && typeof record.path === "string" ? record.path.trim() : "";
    if (path) return path;
  }
  return undefined;
}

/** Windows vs POSIX separators and escaping differ per agent — compare loosely. */
function samePath(a: string, b: string): boolean {
  const normalize = (value: string) =>
    value.replace(/\\\\/g, "\\").replace(/\\/g, "/").trim().toLowerCase();
  return normalize(a) === normalize(b);
}

/** True when rawInput carries nothing beyond the location already on screen. */
function inputIsOnlyPath(rawInput: unknown, path: string | undefined): boolean {
  if (!path) return false;
  const record = asRecord(rawInput);
  if (!record) return typeof rawInput === "string" && samePath(rawInput, path);
  const values = Object.values(record);
  return (
    values.length === 1 && typeof values[0] === "string" && samePath(values[0], path)
  );
}

/**
 * Render ACP `ToolCallContent[]` — text blocks, diffs and terminals.
 *
 * ACP says these collections are *overwritten* by each update, so this is
 * always a full snapshot of what the tool has produced so far.
 */
function renderToolContent(content: unknown): string {
  if (!Array.isArray(content) || content.length === 0) return "";
  const parts: string[] = [];

  for (const block of content) {
    const record = asRecord(block);
    if (!record) {
      const plain = extractTextContent(block);
      if (plain) parts.push(plain);
      continue;
    }
    const blockType = typeof record.type === "string" ? record.type.toLowerCase() : "";

    if (blockType === "diff") {
      const path = typeof record.path === "string" ? record.path : "";
      const oldText = typeof record.oldText === "string" ? record.oldText : "";
      const newText = typeof record.newText === "string" ? record.newText : "";
      const removed = oldText ? oldText.split("\n").length : 0;
      const added = newText ? newText.split("\n").length : 0;
      parts.push(`[diff] ${path || "(file)"} · +${added} / -${removed} lines`);
      continue;
    }
    if (blockType === "terminal") {
      const id =
        (typeof record.terminalId === "string" && record.terminalId) ||
        (typeof record.terminal_id === "string" && record.terminal_id) ||
        "";
      parts.push(id ? `[terminal ${id}]` : "[terminal]");
      continue;
    }

    // `{ type: "content", content: <ContentBlock> }` or a bare content block.
    const inner = record.content ?? record;
    const text = extractTextContent(inner);
    if (text) {
      parts.push(text);
      continue;
    }
    const innerType = contentBlockType(inner);
    if (innerType === "image") {
      parts.push("[image]");
    } else if (innerType === "audio") {
      parts.push("[audio]");
    } else if (innerType.startsWith("resource")) {
      const innerRecord = asRecord(inner);
      const uri =
        innerRecord && typeof innerRecord.uri === "string" ? innerRecord.uri : "";
      parts.push(uri ? `[resource ${uri}]` : "[resource]");
    }
  }

  const body = parts.filter(Boolean).join("\n\n").trim();
  return body ? clipToolDetail(body) : "";
}

/** Fallback when a tool reports results only as `rawOutput`. */
function renderToolRawOutput(rawOutput: unknown): string {
  if (rawOutput == null) return "";
  const text = extractTextContent(rawOutput);
  if (text.trim()) return clipToolDetail(text);
  if (typeof rawOutput === "object") {
    try {
      const json = JSON.stringify(rawOutput, null, 2);
      return json && json !== "{}" ? clipToolDetail(json) : "";
    } catch {
      return "";
    }
  }
  return clipToolDetail(String(rawOutput));
}

/**
 * Compose the tool card body.
 *
 * Line 1 stays `title · status` — App's stuck-tool cleanup rewrites exactly that
 * line and keeps the rest.
 */
export function renderToolText(fields: {
  title?: string;
  status?: string;
  path?: string;
  input?: string;
  detail?: string;
}): string {
  const title = fields.title || "tool";
  const status = fields.status || "pending";
  const lines = [`${title} · ${status}`];
  if (fields.path) lines.push(fields.path);
  // Input is a placeholder for "what was asked" — real output replaces it.
  if (!fields.detail && fields.input) lines.push(fields.input);
  if (fields.detail) lines.push("", fields.detail);
  return lines.join("\n");
}

/** Extract human-readable progress from ACP session/update payloads. */
export function extractAcpUpdateText(data: unknown): AcpTextPart | null {
  const update = getSessionUpdate(data);
  if (!update) return null;

  const sessionUpdate = getSessionUpdateKind(update);
  const messageId =
    typeof update.messageId === "string"
      ? update.messageId
      : typeof update.message_id === "string"
        ? update.message_id
        : undefined;

  // ── tool_call / tool_call_update ──────────────────────────────────────────
  if (sessionUpdate === "tool_call" || sessionUpdate === "tool_call_update" || sessionUpdate.includes("tool_call")) {
    const toolCallId = typeof update.toolCallId === "string"
      ? update.toolCallId
      : typeof update.tool_call_id === "string"
        ? update.tool_call_id
        : undefined;
    // Only report what this update actually carried: ACP updates are partial,
    // and a fabricated fallback here would rename a finished tool back to
    // "tool" (or flip "completed" to "updated") on a bare status ping.
    const title =
      (typeof update.title === "string" && update.title) ||
      (typeof update.name === "string" && update.name) ||
      (typeof update.kind === "string" && update.kind !== "other" && update.kind) ||
      undefined;
    const status =
      (typeof update.status === "string" && update.status) ||
      (sessionUpdate === "tool_call" ? "pending" : undefined);
    const rawInput = update.rawInput ?? update.raw_input ?? update.input;
    const inputPreview =
      rawInput == null
        ? ""
        : typeof rawInput === "string"
          ? rawInput
          : Object.keys(rawInput as object).length === 0
            ? ""
            : JSON.stringify(rawInput);
    const clippedInput = inputPreview.length > 200 ? `${inputPreview.slice(0, 200)}…` : inputPreview;
    // What the tool is actually doing / produced — agents stream this and the
    // card used to throw it away, which is why long tools looked frozen.
    const toolPath = firstToolLocation(update);
    const toolDetail =
      renderToolContent(update.content) || renderToolRawOutput(update.rawOutput ?? update.raw_output);
    // `{"filePath":"…"}` under a line that already shows that path is noise.
    const toolInput = inputIsOnlyPath(rawInput, toolPath) ? "" : clippedInput;

    return {
      role: "tool",
      text: renderToolText({ title, status, path: toolPath, input: toolInput, detail: toolDetail }),
      isDelta: false,
      sessionUpdate,
      messageId,
      toolCallId,
      toolStatus: status,
      toolTitle: title,
      toolPath,
      toolDetail,
      toolInput,
    };
  }

  // Prefer a single text source (avoid double-reading content + message).
  let textFromContent = "";
  let isDelta = sessionUpdate.includes("chunk");
  let blockType = "";

  if (update.delta != null) {
    textFromContent = extractTextContent(update.delta);
    blockType = contentBlockType(update.delta);
    isDelta = true;
  } else if (update.content != null) {
    textFromContent = extractTextContent(update.content);
    blockType = contentBlockType(update.content);
    isDelta = sessionUpdate.includes("chunk");
  } else if (typeof update.text === "string") {
    textFromContent = update.text;
  } else if (update.thought != null) {
    textFromContent = extractTextContent(update.thought);
    blockType = "thought";
    isDelta = sessionUpdate.includes("chunk");
  } else if (update.message != null) {
    textFromContent = extractTextContent(update.message);
    isDelta = sessionUpdate.includes("chunk");
  }

  if (!textFromContent) return null;

  // Protocol-level thinking
  const isThoughtKind =
    sessionUpdate.includes("thought") ||
    sessionUpdate.includes("reasoning") ||
    sessionUpdate === "agent_thought_chunk" ||
    sessionUpdate === "thought_message_chunk" ||
    blockType === "thought" ||
    blockType === "thinking" ||
    blockType === "reasoning" ||
    blockType === "redacted_thinking";

  // Soft-detect thought ONLY for explicit § markers (OpenCode CoT), never for
  // bare punctuation — "。" / "，" must stay on the assistant stream.
  const looksLikeThoughtMarker =
    /^§/.test(textFromContent.trim()) || /^\s*§+\d*§*/.test(textFromContent);

  if (isThoughtKind || looksLikeThoughtMarker) {
    return {
      role: "thought",
      text: textFromContent,
      isDelta: isDelta || looksLikeThoughtMarker || isThoughtNoiseToken(textFromContent),
      sessionUpdate,
      messageId,
    };
  }

  if (sessionUpdate.includes("user_message")) return null;

  if (
    sessionUpdate.includes("agent_message") ||
    sessionUpdate.includes("assistant") ||
    sessionUpdate.includes("message_chunk") ||
    sessionUpdate === "message" ||
    sessionUpdate.includes("chunk")
  ) {
    // Always prefer append for short chunks (punctuation, tokens) so we never
    // replace a long reply with a single "。" snapshot mis-flagged as non-delta.
    const forceDelta = isDelta || textFromContent.length <= 24;
    return {
      role: "assistant",
      text: textFromContent,
      isDelta: forceDelta,
      sessionUpdate,
      messageId,
    };
  }

  return { role: "other", text: textFromContent, isDelta, sessionUpdate, messageId };
}

/**
 * Tokens that are noise *inside a thought stream* (not real prose punctuation).
 * IMPORTANT: 。，、！？ etc. are NOT noise — they belong in Reply when streamed
 * as agent_message_chunk.
 */
export function isThoughtNoiseToken(text: string): boolean {
  const t = text.trim();
  if (!t) return true;
  // OpenCode / model section markers between reasoning steps
  if (/^§+\d*§*$/u.test(t)) return true;
  if (/^§+$/u.test(t)) return true;
  // Lone tiny numeric tokens sometimes emitted mid-CoT
  if (/^\d{1,3}$/.test(t)) return true;
  return false;
}

/** @deprecated use isThoughtNoiseToken — kept for call sites */
export function isThoughtFragment(text: string): boolean {
  return isThoughtNoiseToken(text);
}

/**
 * Merge streaming text without doubling.
 * Never invent spaces or drop punctuation — trust agent bytes.
 */
export function mergeStreamText(previous: string, incoming: string, isDelta: boolean): string {
  if (!incoming) return previous;
  if (!previous) return incoming;
  if (incoming === previous) return previous;

  // Cumulative snapshot
  if (incoming.startsWith(previous)) return incoming;
  // Stale shorter snapshot
  if (previous.startsWith(incoming) && incoming.length < previous.length) return previous;
  // Exact tail already applied
  if (previous.endsWith(incoming)) return previous;

  // Overlap (shared suffix/prefix) — only when substantial to avoid eating "。"
  const maxOverlap = Math.min(previous.length, incoming.length, 64);
  // Require overlap of at least 2 code units so a single "。" match does not
  // strip a legitimate new period delta (previous ends with 。, incoming is 。foo).
  for (let n = maxOverlap; n >= 2; n -= 1) {
    if (previous.endsWith(incoming.slice(0, n))) {
      return previous + incoming.slice(n);
    }
  }

  if (isDelta) return previous + incoming;

  // Non-delta tiny piece: append rather than replace a long message
  if (incoming.length <= 24 || incoming.length < previous.length * 0.5) {
    return previous + incoming;
  }

  return incoming;
}

function extractTextContent(content: unknown): string {
  if (typeof content === "string") return content;
  if (!content || typeof content !== "object") return "";

  if (Array.isArray(content)) {
    return content.map(extractTextContent).filter(Boolean).join("");
  }

  const c = content as UpdateObj;

  if (typeof c.text === "string") return c.text;
  if (typeof c.thought === "string") return c.thought;

  if (Array.isArray(c.content)) {
    return c.content.map(extractTextContent).filter(Boolean).join("");
  }
  if (c.content != null && typeof c.content === "object") {
    return extractTextContent(c.content);
  }

  return "";
}

export function userMessageEvent(
  sessionId: string,
  text: string,
  meta?: {
    agentId?: string;
    agentLabel?: string;
    modelId?: string;
    modelLabel?: string;
    modeLabel?: string;
    effortLabel?: string;
  },
): SessionEvent {
  return {
    type: "user_message",
    sessionId,
    text,
    messageId: `um-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`,
    createdAt: new Date().toISOString(),
    ...meta,
  };
}

/** DOM / outline id for a user message card. */
export function userMessageAnchorId(event: {
  messageId?: string;
  createdAt: string;
  text: string;
}): string {
  if (event.messageId) return `user-msg-${event.messageId}`;
  // Legacy transcript rows without messageId.
  const slug = `${event.createdAt}-${event.text.slice(0, 24)}`.replace(/[^\w-]+/g, "_");
  return `user-msg-${slug}`;
}

export function assistantMessageEvent(
  sessionId: string,
  text: string,
  messageId?: string,
  meta?: {
    agentId?: string;
    agentLabel?: string;
    modelId?: string;
    modelLabel?: string;
    modeLabel?: string;
    effortLabel?: string;
  },
): SessionEvent {
  return {
    type: "assistant_message",
    sessionId,
    text,
    messageId,
    createdAt: new Date().toISOString(),
    ...meta,
  };
}

/** Scan events backward for the last user_message with metadata. */
export function metaFromLastUser(events: SessionEvent[], sessionId: string):
  | { agentId?: string; agentLabel?: string; modelId?: string; modelLabel?: string; modeLabel?: string; effortLabel?: string }
  | undefined {
  for (let i = events.length - 1; i >= 0; i--) {
    const e = events[i];
    if (e.sessionId === sessionId && e.type === "user_message") {
      if (e.agentId || e.agentLabel || e.modelId || e.modelLabel || e.modeLabel || e.effortLabel) {
        return {
          agentId: e.agentId,
          agentLabel: e.agentLabel,
          modelId: e.modelId,
          modelLabel: e.modelLabel,
          modeLabel: e.modeLabel,
          effortLabel: e.effortLabel,
        };
      }
      return undefined;
    }
  }
  return undefined;
}

export function thoughtEvent(sessionId: string, text: string, messageId?: string): SessionEvent {
  return {
    type: "thought",
    sessionId,
    text,
    messageId,
    createdAt: new Date().toISOString(),
  };
}

export function toolCallEvent(
  sessionId: string,
  text: string,
  toolCallId?: string,
  status?: string,
  title?: string,
  extra?: { path?: string; detail?: string; input?: string },
): SessionEvent {
  return {
    type: "tool_call",
    sessionId,
    text,
    toolCallId,
    status,
    title,
    // First title is the tool name; later updates replace it with a summary.
    toolName: title,
    path: extra?.path,
    detail: extra?.detail,
    input: extra?.input,
    createdAt: new Date().toISOString(),
  };
}

function sameStreamTarget(
  last: SessionEvent,
  sessionId: string,
  type: "assistant_message" | "thought",
  messageId?: string,
  /** Thoughts often churn message ids mid-stream (OpenCode) — prefer merge. */
  ignoreMessageId = false,
): boolean {
  if (last.sessionId !== sessionId || last.type !== type) return false;
  if (!ignoreMessageId && messageId && "messageId" in last) {
    // New ACP message id → new bubble (do not merge across messages)
    if (last.messageId && last.messageId !== messageId) return false;
  }
  return true;
}

/** Index of the latest thought in this session after the last user message. */
function findLastThoughtInTurn(events: SessionEvent[], sessionId: string): number {
  let lastUser = -1;
  for (let i = 0; i < events.length; i += 1) {
    if (events[i].sessionId === sessionId && events[i].type === "user_message") lastUser = i;
  }
  for (let i = events.length - 1; i > lastUser; i -= 1) {
    const e = events[i];
    if (e.sessionId === sessionId && e.type === "thought") return i;
  }
  return -1;
}

function patchThoughtAt(
  events: SessionEvent[],
  index: number,
  text: string,
  messageId?: string,
): SessionEvent[] {
  const prev = events[index];
  if (prev.type !== "thought") return events;
  if (prev.text === text && (!messageId || prev.messageId === messageId)) return events;
  const next = [...events];
  next[index] = {
    ...prev,
    text,
    messageId: messageId ?? prev.messageId,
  };
  return next;
}

/** Apply a streamed ACP part onto the live transcript list. */
export function applyAcpPartToEvents(
  current: SessionEvent[],
  sessionId: string,
  part: AcpTextPart,
): SessionEvent[] {
  if (part.role === "assistant") {
    const last = current[current.length - 1];
    if (last && sameStreamTarget(last, sessionId, "assistant_message", part.messageId) && last.type === "assistant_message") {
      const nextText = mergeStreamText(last.text, part.text, part.isDelta);
      if (nextText === last.text && (!part.messageId || last.messageId === part.messageId)) return current;
      const next = [...current];
      next[next.length - 1] = {
        ...last,
        text: nextText,
        messageId: part.messageId ?? last.messageId,
      };
      return next;
    }
    const meta = metaFromLastUser(current, sessionId);
    return [...current, assistantMessageEvent(sessionId, part.text, part.messageId, meta)];
  }

  if (part.role === "thought") {
    const trimmed = part.text.trim();
    if (!trimmed) return current;

    const noise = isThoughtNoiseToken(part.text);
    const asDelta = part.isDelta || noise || part.text.length <= 32;
    const last = current[current.length - 1];

    // Safety: if we are mid-Reply and a mis-tagged short piece arrives as thought
    // (should be rare after extract fix), keep it on the assistant bubble.
    if (
      last &&
      last.type === "assistant_message" &&
      last.sessionId === sessionId &&
      (noise || part.text.length <= 8 || /[。，、．！？；：,.!?;:…—\-]/.test(part.text))
    ) {
      const nextText = mergeStreamText(last.text, part.text, true);
      if (nextText === last.text) return current;
      const next = [...current];
      next[next.length - 1] = { ...last, text: nextText };
      return next;
    }

    // 1) Continuous thought stream — ignore messageId churn.
    if (last && sameStreamTarget(last, sessionId, "thought", part.messageId, true) && last.type === "thought") {
      const nextText = mergeStreamText(last.text, part.text, asDelta);
      return patchThoughtAt(current, current.length - 1, nextText, part.messageId ?? last.messageId);
    }

    // 2) § / digit noise — glue to last thought or drop (never open a "§" card).
    if (noise) {
      const thoughtIdx = findLastThoughtInTurn(current, sessionId);
      if (thoughtIdx >= 0) {
        const prev = current[thoughtIdx];
        if (prev.type === "thought") {
          const nextText = mergeStreamText(prev.text, part.text, true);
          return patchThoughtAt(current, thoughtIdx, nextText, part.messageId ?? prev.messageId);
        }
      }
      return current;
    }

    // 3) Substantial new thought (e.g. after a tool) — new card is correct.
    return [...current, thoughtEvent(sessionId, part.text, part.messageId)];
  }

  if (part.role === "tool") {
    // Working notes before a tool are usually CoT — fold them as Thinking.
    // Never touch a sealed Reply (finished turn / previous agent in this dialog).
    let base = current;
    const last = current[current.length - 1];
    if (
      last &&
      last.type === "assistant_message" &&
      last.sessionId === sessionId &&
      !isSealedAssistantReply(last)
    ) {
      base = [
        ...current.slice(0, -1),
        {
          type: "thought" as const,
          sessionId: last.sessionId,
          text: last.text,
          messageId: last.messageId,
          createdAt: last.createdAt,
        },
      ];
    }

    if (part.toolCallId) {
      const idx = base.findIndex(
        (e) => e.type === "tool_call" && e.sessionId === sessionId && e.toolCallId === part.toolCallId,
      );
      if (idx >= 0) {
        const prev = base[idx];
        if (prev.type !== "tool_call") return base;
        // Updates carry only the fields that changed: a bare status update must
        // not wipe the output the tool already produced.
        const merged = {
          title: part.toolTitle ?? prev.title,
          status: part.toolStatus ?? prev.status,
          path: part.toolPath ?? prev.path,
          detail: part.toolDetail || prev.detail,
          input: part.toolInput || prev.input,
        };
        const next = [...base];
        next[idx] = {
          ...prev,
          ...merged,
          // Never overwritten: `title` becomes a summary, the name must not.
          toolName: prev.toolName ?? part.toolTitle,
          text: renderToolText(merged),
        };
        return next;
      }
    }
    return [
      ...base,
      toolCallEvent(
        sessionId,
        part.text,
        part.toolCallId,
        part.toolStatus ?? "pending",
        part.toolTitle ?? "tool",
        { path: part.toolPath, detail: part.toolDetail, input: part.toolInput },
      ),
    ];
  }

  return current;
}

/**
 * Merge adjacent fragment-like thoughts inside a turn (cleanup pass).
 * Does not cross tools or user messages — those stay as separate blocks.
 */
export function coalesceAdjacentThoughts(
  events: SessionEvent[],
  sessionId: string,
): SessionEvent[] {
  if (events.length < 2) return events;
  const out: SessionEvent[] = [];

  for (const e of events) {
    const prev = out[out.length - 1];
    if (
      e.sessionId === sessionId &&
      e.type === "thought" &&
      prev &&
      prev.sessionId === sessionId &&
      prev.type === "thought"
    ) {
      // Adjacent thoughts with no tool between → usually broken stream; merge.
      // Keep a blank line when both sides look like real prose.
      const a = prev.text;
      const b = e.text;
      const join =
        isThoughtFragment(b) || isThoughtFragment(a)
          ? mergeStreamText(a, b, true)
          : a.endsWith("\n") || b.startsWith("\n")
            ? a + b
            : `${a}\n${b}`;
      out[out.length - 1] = {
        ...prev,
        text: join,
        messageId: e.messageId ?? prev.messageId,
      };
      continue;
    }
    out.push(e);
  }
  return out;
}

/**
 * A finished Reply must never be demoted to Thinking.
 *
 * `durationMs` is stamped when the turn completes. We also seal on agent switch
 * so the previous harness's last answer survives the next agent's tool/CoT
 * stream (same dialog, shared transcript).
 */
export function isSealedAssistantReply(event: SessionEvent): boolean {
  return event.type === "assistant_message" && event.durationMs != null;
}

/**
 * Mark open assistant bubbles as finished so later streams cannot fold them
 * into Thinking. Used when the dialog changes agent (or otherwise ends a turn
 * without an rpc/response).
 */
export function sealOpenAssistantReplies(
  events: SessionEvent[],
  sessionId: string,
  endedAtMs: number = Date.now(),
): SessionEvent[] {
  let changed = false;
  const next = events.map((e) => {
    if (e.sessionId !== sessionId || e.type !== "assistant_message") return e;
    if (e.durationMs != null) return e;
    // System notifications (no agentId) are not CoT either — leave type, but
    // stamp duration so collapse/tool paths treat them as sealed.
    const started = Date.parse(e.createdAt);
    const durationMs =
      Number.isFinite(started) && started > 0
        ? Math.max(0, endedAtMs - started)
        : 0;
    changed = true;
    return { ...e, durationMs };
  });
  return changed ? next : events;
}

/**
 * OpenCode (and some models) stream chain-of-thought as agent_message_chunk.
 * After a turn ends, collapse intermediate assistant bubbles after the last
 * user message into type "thought" so the UI can fold them by default.
 *
 * Sealed Replies (finished turn / previous agent) are never demoted — otherwise
 * switching agent in the same dialog turns the last answer into Thinking as
 * soon as the new harness streams tools or a second assistant bubble.
 */
export function collapseIntermediateAssistantAsThought(
  events: SessionEvent[],
  sessionId: string,
): SessionEvent[] {
  let lastUser = -1;
  for (let i = 0; i < events.length; i += 1) {
    if (events[i].sessionId === sessionId && events[i].type === "user_message") lastUser = i;
  }
  if (lastUser < 0) return coalesceAdjacentThoughts(events, sessionId);

  // Only unsealed assistants in this turn are candidates for CoT folding.
  const demotableIdx: number[] = [];
  for (let i = lastUser + 1; i < events.length; i += 1) {
    const e = events[i];
    if (e.sessionId !== sessionId) continue;
    if (e.type !== "assistant_message") continue;
    if (isSealedAssistantReply(e)) continue;
    demotableIdx.push(i);
  }
  if (demotableIdx.length <= 1) {
    return coalesceAdjacentThoughts(events, sessionId);
  }

  const keep = demotableIdx[demotableIdx.length - 1];
  const demote = new Set(demotableIdx.filter((i) => i !== keep));
  const mapped = events.map((e, i) => {
    if (!demote.has(i) || e.type !== "assistant_message") return e;
    return {
      type: "thought" as const,
      sessionId: e.sessionId,
      text: e.text,
      messageId: e.messageId,
      createdAt: e.createdAt,
    };
  });
  return coalesceAdjacentThoughts(mapped, sessionId);
}
