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
    const title =
      (typeof update.title === "string" && update.title) ||
      (typeof update.name === "string" && update.name) ||
      (typeof update.kind === "string" && update.kind !== "other" && update.kind) ||
      "tool";
    const status =
      (typeof update.status === "string" && update.status) ||
      (sessionUpdate === "tool_call" ? "pending" : "updated");
    const rawInput = update.rawInput ?? update.raw_input ?? update.input;
    const inputPreview =
      rawInput == null
        ? ""
        : typeof rawInput === "string"
          ? rawInput
          : Object.keys(rawInput as object).length === 0
            ? ""
            : JSON.stringify(rawInput);
    const clipped = inputPreview.length > 200 ? `${inputPreview.slice(0, 200)}…` : inputPreview;
    const text = clipped ? `${title} · ${status}\n${clipped}` : `${title} · ${status}`;
    return {
      role: "tool",
      text,
      isDelta: false,
      sessionUpdate,
      messageId,
      toolCallId,
      toolStatus: status,
      toolTitle: title,
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

export function userMessageEvent(sessionId: string, text: string): SessionEvent {
  return {
    type: "user_message",
    sessionId,
    text,
    messageId: `um-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`,
    createdAt: new Date().toISOString(),
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

export function assistantMessageEvent(sessionId: string, text: string, messageId?: string): SessionEvent {
  return {
    type: "assistant_message",
    sessionId,
    text,
    messageId,
    createdAt: new Date().toISOString(),
  };
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
): SessionEvent {
  return {
    type: "tool_call",
    sessionId,
    text,
    toolCallId,
    status,
    title,
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
    return [...current, assistantMessageEvent(sessionId, part.text, part.messageId)];
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
    // Working notes before a tool are usually CoT — fold them as Thinking immediately.
    let base = current;
    const last = current[current.length - 1];
    if (last && last.type === "assistant_message" && last.sessionId === sessionId) {
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
        const next = [...base];
        next[idx] = {
          ...prev,
          text: part.text || prev.text,
          status: part.toolStatus ?? prev.status,
          title: part.toolTitle ?? prev.title,
        };
        return next;
      }
    }
    return [
      ...base,
      toolCallEvent(sessionId, part.text, part.toolCallId, part.toolStatus, part.toolTitle),
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
 * OpenCode (and some models) stream chain-of-thought as agent_message_chunk.
 * After a turn ends, collapse every assistant bubble after the last user message
 * except the final one into type "thought" so the UI can fold them by default.
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

  const assistantIdx: number[] = [];
  for (let i = lastUser + 1; i < events.length; i += 1) {
    const e = events[i];
    if (e.sessionId !== sessionId) continue;
    if (e.type === "assistant_message") assistantIdx.push(i);
  }
  if (assistantIdx.length <= 1) {
    return coalesceAdjacentThoughts(events, sessionId);
  }

  const keep = assistantIdx[assistantIdx.length - 1];
  const mapped = events.map((e, i) => {
    if (i === keep || e.type !== "assistant_message" || e.sessionId !== sessionId) return e;
    if (!assistantIdx.includes(i)) return e;
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
