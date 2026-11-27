/**
 * Explicit bounded session reference (P1-4, pure snapshot builder).
 *
 * Only user/assistant messages and successful tool outcomes are eligible.
 * Thoughts, notices, file changes, subtasks, prompts, credentials and raw tool
 * bodies are excluded. Event count and output bytes both have hard caps.
 */
import { ansiToPlainText } from "./ansi";
import { cleanAssistantText } from "./markdownText";
import { isInjectedPromptText } from "./transcript";
import type { Session, SessionEvent, SessionStatus } from "./types";

export const SESSION_REFERENCE_VERSION = 1;

// ─── Limits ─────────────────────────────────────────────────────────────────

export type SessionReferenceLimits = {
  /** 最多保留多少轮 user/assistant 对话。 */
  maxTurns: number;
  /** 每轮最多保留多少条助手回复（re-entry 可能连发多条）。 */
  maxRepliesPerTurn: number;
  /** 尾部扫描的事件条数（所有会话的原始事件都计数）。 */
  maxScanEvents: number;
  /** æå¤ä¿çå¤å°æ¡æåå·¥å·ç»æã */
  maxToolOutcomes: number;
  /** 渲染块总字节上限（UTF-8）。 */
  maxBytes: number;
  /** 最初请求的单条字节上限。 */
  maxFirstRequestBytes: number;
  /** 对话消息的单条字节上限。 */
  maxMessageBytes: number;
  /** å·¥å·ç»ææè¦çåæ¡å­èä¸éã */
  maxToolOutcomeBytes: number;
};

const LIMIT_DEFAULTS: SessionReferenceLimits = {
  maxTurns: 6,
  maxRepliesPerTurn: 2,
  maxScanEvents: 1500,
  maxToolOutcomes: 8,
  maxBytes: 12 * 1024,
  maxFirstRequestBytes: 2 * 1024,
  maxMessageBytes: 2 * 1024,
  maxToolOutcomeBytes: 400,
};

/** 上限的天花板：调用方调参不能越过这些值。 */
const LIMIT_CEILINGS: SessionReferenceLimits = {
  maxTurns: 12,
  maxRepliesPerTurn: 4,
  maxScanEvents: 5000,
  maxToolOutcomes: 20,
  maxBytes: 32 * 1024,
  maxFirstRequestBytes: 4 * 1024,
  maxMessageBytes: 8 * 1024,
  maxToolOutcomeBytes: 2 * 1024,
};

export const SESSION_REFERENCE_LIMITS = {
  defaults: LIMIT_DEFAULTS,
  ceilings: LIMIT_CEILINGS,
} as const;

/**
 * æè°ç¨æ¹çé¨åéç½®å¤¹è¿ [ä¸é, ç¡¬ä¸é]ãéæ³å¼ï¼NaN / éæ°å­ï¼åè½é»è®¤å¼ã
 * 字节类下限是 32–1024，保证渲染仍能装ä¸åæ°æ®å¤´ï¼è®¡æ°ç±»å¯ä»¥æ¯ 0ï¼æ¾å¼å³é­ï¼ã
 */
export function resolveSessionReferenceLimits(
  partial?: Partial<SessionReferenceLimits> | null,
): SessionReferenceLimits {
  const pick = (key: keyof SessionReferenceLimits, min: number): number => {
    const raw = partial?.[key];
    if (typeof raw !== "number" || !Number.isFinite(raw)) return LIMIT_DEFAULTS[key];
    return Math.min(LIMIT_CEILINGS[key], Math.max(min, Math.floor(raw)));
  };
  return {
    maxTurns: pick("maxTurns", 1),
    maxRepliesPerTurn: pick("maxRepliesPerTurn", 0),
    maxScanEvents: pick("maxScanEvents", 1),
    maxToolOutcomes: pick("maxToolOutcomes", 0),
    maxBytes: pick("maxBytes", 1024),
    maxFirstRequestBytes: pick("maxFirstRequestBytes", 64),
    maxMessageBytes: pick("maxMessageBytes", 64),
    maxToolOutcomeBytes: pick("maxToolOutcomeBytes", 32),
  };
}

// ─── Snapshot types ─────────────────────────────────────────────────────────

export type SessionReferenceMetadata = {
  sessionId: string;
  label: string;
  projectId: string;
  agentId: string;
  status: SessionStatus;
  origin: string | null;
  startedAt: string;
  lastActiveAt: string;
  exitedAt: string | null;
  preferredModel: string | null;
  preferredMode: string | null;
};

export type SessionReferenceMessage = {
  role: "user" | "assistant";
  text: string;
  createdAt: string;
  /** 截断后实际字节数（UTF-8）。 */
  bytes: number;
  clipped: boolean;
};

export type SessionReferenceTurn = {
  /** tail 窗口从一轮中间开始时为 null（提问在窗口之外）。 */
  user: SessionReferenceMessage | null;
  replies: SessionReferenceMessage[];
};

export type SessionReferenceToolOutcome = {
  toolName: string;
  title: string | null;
  path: string | null;
  status: string;
  /** 单行摘要：输出里第一行非 diff 噪音的正文；没有就用 title。 */
  summary: string;
  createdAt: string;
  completedAt: string | null;
};

export type SessionReferenceTruncationReason =
  | "max_scan_events"
  | "max_turns"
  | "max_replies_per_turn"
  | "max_tool_outcomes"
  | "max_bytes"
  | "message_clipped"
  | "first_request_clipped"
  | "first_request_missing"
  | "source_head_truncated"
  | "no_events"
  | "credentials_redacted";

export type SessionReferenceTruncation = {
  /** 有内容真的丢了（或疑似凭据被隐去）；`first_request_missing` / `no_events` 不算。 */
  truncated: boolean;
  reasons: SessionReferenceTruncationReason[];
  /** 实际生效的上限（已夹到硬上限）。 */
  limits: SessionReferenceLimits;
  /** 调用方交给 builder 的事件总数。 */
  eventsProvided: number;
  /** 尾部窗口实际查看的事件数。 */
  eventsScanned: number;
  /** 为找最初请求向前扫描的事件数。 */
  firstRequestScanned: number;
  /** æ«æçªå£éå¶å®ä¼è¯çäºä»¶æ°ã */
  unrelatedEvents: number;
  /** 扫描窗口里被类型白名单排除的事件数。 */
  excludedByType: number;
  /** æ¾æåè¯·æ±æ¶è·³è¿çæ³¨å¥åå¯¼æ¡æ°ã */
  preamblesSkipped: number;
  /** 扫描窗口里通过白名单的事件数。 */
  eligibleEvents: number;
  /** 窗口里拼出的完整轮数（含被上限丢掉的）。 */
  turnsInWindow: number;
  turnsKept: number;
  /** 每轮回复上限丢掉的条数。 */
  repliesOmitted: number;
  /** çªå£éç¶ææç¡®ä¸ºæåçå·¥å·æ°ï¼å«è¢«ä¸éä¸¢æçï¼ã */
  toolOutcomesInWindow: number;
  toolOutcomesKept: number;
  /** 被单条字节上限截断的对话消息条数。 */
  messageClips: number;
  /** 疑似凭据隐去次数。 */
  redactions: number;
  /** 最终 `text` 的字节数。 */
  bytes: number;
  /** 调用方声明 events 只有尾部（更早历史未加载）。 */
  sourceHeadTruncated: boolean;
  /** 因 maxScanEvents 没有扫到数组头部。 */
  scanCutOffEvents: boolean;
};

export type SessionReferenceSnapshot = {
  kind: "marionette.session-reference";
  version: typeof SESSION_REFERENCE_VERSION;
  sessionId: string;
  metadata: SessionReferenceMetadata;
  firstUserRequest: SessionReferenceMessage | null;
  /** false = 事件窗口没有覆盖会话开头，无法证明这是真正的第一条请求。 */
  firstUserRequestExact: boolean;
  /** 最初请求已经在 `turns` 里（渲染时不再重复一段）。 */
  firstUserRequestInTurns: boolean;
  turns: SessionReferenceTurn[];
  toolOutcomes: SessionReferenceToolOutcome[];
  truncation: SessionReferenceTruncation;
  /** å¯ç´æ¥æå¥ Composer çå¼ç¨åï¼UTF-8 å­èæ°ä¸ä¼è¶过 `limits.maxBytes`。 */
  text: string;
};

export type BuildSessionReferenceInput = {
  session: Session;
  events: readonly SessionEvent[];
  limits?: Partial<SessionReferenceLimits> | null;
  /** 调用方知道 `events` 从会话中段开始（例如 App 只加载了尾部窗口）。 */
  sourceHeadTruncated?: boolean;
};

// ─── Byte helpers ───────────────────────────────────────────────────────────

/** UTF-8 字节数（孤立代理项按替换字符 3 字节计，与 TextEncoder 一致）。 */
export function utf8ByteLength(text: string): number {
  let bytes = 0;
  for (const char of text) {
    const code = char.codePointAt(0) ?? 0;
    if (code <= 0x7f) bytes += 1;
    else if (code <= 0x7ff) bytes += 2;
    else if (code <= 0xffff) bytes += 3;
    else bytes += 4;
  }
  return bytes;
}

const ELLIPSIS = "...";
const ELLIPSIS_BYTES = 3;

/**
 * 按字节截断，永远落在码点边界上；超限时尽量保留正文并补一个省略号。
 * 文本本身装得下就原样返回（不会为了省略号提前吃掉两个字节）。
 */
function clipToBytes(
  text: string,
  maxBytes: number,
): { text: string; bytes: number; clipped: boolean } {
  if (!text) return { text: "", bytes: 0, clipped: false };
  if (maxBytes <= 0) return { text: "", bytes: 0, clipped: true };

  const chars: string[] = [];
  let used = 0;
  for (const char of text) {
    const size = utf8ByteLength(char);
    if (used + size > maxBytes) {
      while (chars.length > 0 && used + ELLIPSIS_BYTES > maxBytes) {
        const dropped = chars.pop();
        if (dropped === undefined) break;
        used -= utf8ByteLength(dropped);
      }
      const suffix = maxBytes >= ELLIPSIS_BYTES ? ELLIPSIS : "";
      return {
        text: `${chars.join("")}${suffix}`,
        bytes: used + utf8ByteLength(suffix),
        clipped: true,
      };
    }
    chars.push(char);
    used += size;
  }
  return { text: chars.join(""), bytes: used, clipped: false };
}

// ─── Credential scrubbing ───────────────────────────────────────────────────

const SECRET_PLACEHOLDER = "[redacted]";

type RedactionTally = { count: number };

/**
 * 面向引用的凭据清æ´ï¼å®æå¿çºµï¼è¯¯ä¼¤æ®éæå­å¯æ¥åï¼ãåªå**ç¡®å®æ§**æ¨¡å¼å¹éï¼
 * 不猜熵、不做模型判断；命中数进 `truncation.redactions`。
 */
function scrubSecrets(input: string, tally: RedactionTally): string {
  let text = input;

  text = text.replace(
    /-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z ]*PRIVATE KEY-----/g,
    () => {
      tally.count += 1;
      return SECRET_PLACEHOLDER;
    },
  );

  text = text.replace(
    /\b(https?:\/\/)[^\s/@]+(:[^\s/@]*)?@/gi,
    (_match, prefix: string) => {
      tally.count += 1;
      return `${prefix}${SECRET_PLACEHOLDER}@`;
    },
  );

  text = text.replace(
    /((?:api[_-]?key|secret|token|password|passwd|authorization|access[_-]?key|private[_-]?key|client[_-]?secret)\s*[:=]\s*)(["'`]?)([^\s"'`,;]{6,})\2/gi,
    (_match, prefix: string, quote: string) => {
      tally.count += 1;
      return `${prefix}${quote}${SECRET_PLACEHOLDER}${quote}`;
    },
  );

  text = text.replace(
    /\bBearer\s+[A-Za-z0-9._~+/=-]{12,}/g,
    () => {
      tally.count += 1;
      return `Bearer ${SECRET_PLACEHOLDER}`;
    },
  );

  text = text.replace(
    /\bBasic\s+[A-Za-z0-9+/=]{16,}/g,
    () => {
      tally.count += 1;
      return `Basic ${SECRET_PLACEHOLDER}`;
    },
  );

  text = text.replace(
    /\bsk-[A-Za-z0-9_-]{16,}/g,
    () => {
      tally.count += 1;
      return SECRET_PLACEHOLDER;
    },
  );

  text = text.replace(
    /\bgh[pousr]_[A-Za-z0-9]{20,}/g,
    () => {
      tally.count += 1;
      return SECRET_PLACEHOLDER;
    },
  );

  text = text.replace(
    /\bxox[abprs]-[A-Za-z0-9-]{10,}/g,
    () => {
      tally.count += 1;
      return SECRET_PLACEHOLDER;
    },
  );

  text = text.replace(
    /\bAKIA[0-9A-Z]{16}\b/g,
    () => {
      tally.count += 1;
      return SECRET_PLACEHOLDER;
    },
  );

  text = text.replace(
    /\beyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}/g,
    () => {
      tally.count += 1;
      return SECRET_PLACEHOLDER;
    },
  );

  return text;
}

/** ç»ä¸æ­£æå¥å£ï¼å» ANSI/æ§å¶å­ç¬¦ â è§è²æ¸洗 → 隐去凭据 → 去首尾空白。 */
function sanitizeText(
  raw: string,
  role: "user" | "assistant" | "plain",
  tally: RedactionTally,
): string {
  const plain = ansiToPlainText(raw);
  const cleaned = role === "assistant" ? cleanAssistantText(plain) : plain;
  return scrubSecrets(cleaned, tally).trim();
}

/** 单行字段（标题 / 代理名 / 路径…）：压成一行再按字节截断。 */
function sanitizeInline(raw: string, maxBytes: number, tally: RedactionTally): string {
  const oneLine = sanitizeText(raw, "plain", tally).replace(/\s+/g, " ").trim();
  return clipToBytes(oneLine, maxBytes).text;
}

// ─── Event selection ────────────────────────────────────────────────────────

/** å¼ç¨å¿«ç§åè®¸çäºä»¶ç±»åï¼å¶ä½ï¼thought/notice/file_change/â¦ï¼ä¸å¾æé¤ã */
export function isSessionReferenceEvent(event: SessionEvent): boolean {
  return (
    event.type === "user_message" ||
    event.type === "assistant_message" ||
    event.type === "tool_call"
  );
}

const FAILED_TOOL_STATUS = /fail|error|cancel|reject|deny|denied|declin|abort|timeout|expired/i;
const SUCCESS_TOOL_STATUS = /complet|success|succeed|done|finished|ok$/i;

/**
 * åªæç¶ææç¡®åçæåæç®ãæåçå·¥å·ç»æãã`completedAt` åªè½è¯æç»æï¼
 * 不能证明成功——状态缺失或模棱两可的一律不收（fail-closed）。
 */
function toolSucceeded(event: Extract<SessionEvent, { type: "tool_call" }>): boolean {
  const status = (event.status ?? "").trim();
  if (!status) return false;
  if (FAILED_TOOL_STATUS.test(status)) return false;
  return SUCCESS_TOOL_STATUS.test(status);
}

const DIFF_NOISE_LINE = /^(diff --git|index [0-9a-f]{6,}|@@|\+\+\+ |--- |Binary files )/;

/** 输出正文的第一行非 diff 噪音摘要（不看 `input`，也不整段搬运输出体）。 */
function toolSummaryLine(
  event: Extract<SessionEvent, { type: "tool_call" }>,
  tally: RedactionTally,
): string {
  const plain = sanitizeText(event.detail ?? "", "plain", tally);
  if (!plain) return "";
  for (const line of plain.split("\n")) {
    const trimmed = line.trim();
    if (!trimmed) continue;
    if (DIFF_NOISE_LINE.test(trimmed)) continue;
    return trimmed;
  }
  return "";
}

// ─── Draft shapes ───────────────────────────────────────────────────────────

type DraftMessage = SessionReferenceMessage & { sourceIndex: number };

type DraftTurn = {
  user: DraftMessage | null;
  replies: DraftMessage[];
};

function toMessage(message: DraftMessage): SessionReferenceMessage {
  return {
    role: message.role,
    text: message.text,
    createdAt: message.createdAt,
    bytes: message.bytes,
    clipped: message.clipped,
  };
}

function constrainDraft(message: DraftMessage, maxBytes: number): DraftMessage {
  if (message.bytes <= maxBytes) return message;
  const clipped = clipToBytes(message.text, maxBytes);
  return { ...message, text: clipped.text, bytes: clipped.bytes, clipped: true };
}

function buildUserDraft(
  event: Extract<SessionEvent, { type: "user_message" }>,
  sourceIndex: number,
  maxBytes: number,
  tally: RedactionTally,
): DraftMessage | null {
  const body = sanitizeText(event.text, "user", tally);
  const names = (event.attachments ?? [])
    .map((attachment) => sanitizeInline(attachment.name || attachment.path || "", 80, tally))
    .filter((name) => name.length > 0);
  const shown = names.slice(0, 6);
  const attachmentNote =
    shown.length > 0
      ? "[attachment] " + shown.join(", ") + (names.length > shown.length ? " and " + names.length + " total" : "")
      : "";
  const combined = [body, attachmentNote].filter((part) => part.length > 0).join("\n\n");
  if (!combined) return null;
  const clipped = clipToBytes(combined, maxBytes);
  return {
    role: "user",
    text: clipped.text,
    createdAt: event.createdAt,
    bytes: clipped.bytes,
    clipped: clipped.clipped,
    sourceIndex,
  };
}

function buildAssistantDraft(
  event: Extract<SessionEvent, { type: "assistant_message" }>,
  sourceIndex: number,
  maxBytes: number,
  tally: RedactionTally,
): DraftMessage | null {
  const body = sanitizeText(event.text, "assistant", tally);
  if (!body) return null;
  const clipped = clipToBytes(body, maxBytes);
  return {
    role: "assistant",
    text: clipped.text,
    createdAt: event.createdAt,
    bytes: clipped.bytes,
    clipped: clipped.clipped,
    sourceIndex,
  };
}

function buildToolOutcome(
  event: Extract<SessionEvent, { type: "tool_call" }>,
  maxBytes: number,
  tally: RedactionTally,
): SessionReferenceToolOutcome {
  const title = event.title ? sanitizeInline(event.title, 120, tally) || null : null;
  const toolName = sanitizeInline(event.toolName || event.title || "tool", 80, tally) || "tool";
  const path = event.path ? sanitizeInline(event.path, 240, tally) || null : null;
  const summaryRaw = toolSummaryLine(event, tally) || title || "";
  return {
    toolName,
    title,
    path,
    status: sanitizeInline(event.status ?? "", 40, tally),
    summary: clipToBytes(summaryRaw, maxBytes).text,
    createdAt: event.createdAt,
    completedAt: event.completedAt ?? null,
  };
}

function collectAttachmentCount(turns: readonly DraftTurn[]): number {
  let count = 0;
  for (const turn of turns) {
    if (turn.user?.clipped) count += 1;
    for (const reply of turn.replies) {
      if (reply.clipped) count += 1;
    }
  }
  return count;
}

// ─── Reason bookkeeping ─────────────────────────────────────────────────────

const REASON_ORDER: readonly SessionReferenceTruncationReason[] = [
  "max_scan_events",
  "max_turns",
  "max_replies_per_turn",
  "max_tool_outcomes",
  "max_bytes",
  "message_clipped",
  "first_request_clipped",
  "source_head_truncated",
  "credentials_redacted",
  "first_request_missing",
  "no_events",
];

const LOSS_REASONS: ReadonlySet<SessionReferenceTruncationReason> = new Set([
  "max_scan_events",
  "max_turns",
  "max_replies_per_turn",
  "max_tool_outcomes",
  "max_bytes",
  "message_clipped",
  "first_request_clipped",
  "source_head_truncated",
  "credentials_redacted",
]);

// ─── Builder ────────────────────────────────────────────────────────────────

type TailEntry = { event: SessionEvent; index: number };

type RenderState = {
  turns: DraftTurn[];
  tools: SessionReferenceToolOutcome[];
  first: DraftMessage | null;
  firstExact: boolean;
  firstDroppedForBytes: boolean;
  droppedTurnsByLimit: number;
  droppedTurnsByBytes: number;
  droppedToolsByLimit: number;
  droppedToolsByBytes: number;
  droppedReplies: number;
  scanCut: boolean;
  scanned: number;
  redactions: number;
};

/**
 * ä»è°ç¨æ¹æéå·²æç `Session` åæ°æ®ä¸è½¬å½äºä»¶æå»ºæçå¼ç¨å¿«ç§ã
 * 纯函数：不读盘、不联网、不看时钟；`session.id` 缺失是调用方的编程错误。
 */
export function buildSessionReference(
  input: BuildSessionReferenceInput,
): SessionReferenceSnapshot {
  const session = input.session;
  const sessionId = typeof session.id === "string" ? session.id : "";
  if (!sessionId) throw new Error("buildSessionReference: session.id is required");

  const events = Array.isArray(input.events) ? input.events : [];
  const limits = resolveSessionReferenceLimits(input.limits);
  const tally: RedactionTally = { count: 0 };
  const headTruncated = input.sourceHeadTruncated === true;

  // åæ°æ®ï¼åªæ¬ç½ååå­æ®µï¼cwd / transcriptPath / nativeSessionIds ç­ä¸è¿æ¥ï¼ã
  const label = sanitizeInline(session.label ?? "", 160, tally) || sessionId;
  const metadata: SessionReferenceMetadata = {
    sessionId,
    label,
    projectId: sanitizeInline(session.projectId ?? "", 120, tally),
    agentId: sanitizeInline(session.agentId ?? "", 120, tally),
    status: session.status,
    origin: session.origin ? sanitizeInline(session.origin, 48, tally) || null : null,
    startedAt: session.startedAt,
    lastActiveAt: session.lastActiveAt,
    exitedAt: session.exitedAt ?? null,
    preferredModel: session.preferredModel
      ? sanitizeInline(session.preferredModel, 160, tally) || null
      : null,
    preferredMode: session.preferredMode
      ? sanitizeInline(session.preferredMode, 80, tally) || null
      : null,
  };

  // ååæ¾âæåè¯·æ±âï¼è·³è¿ Marionette èªå·±æ³¨å¥çåå¯¼ï¼æ«æéåæ ·æä¸éï¼ã
  let firstEvent: Extract<SessionEvent, { type: "user_message" }> | null = null;
  let firstEventIndex = -1;
  let firstScanned = 0;
  let preamblesSkipped = 0;
  for (let index = 0; index < events.length && firstScanned < limits.maxScanEvents; index += 1) {
    const event = events[index];
    if (!event) continue;
    firstScanned += 1;
    if (event.sessionId !== sessionId) continue;
    if (event.type !== "user_message") continue;
    if (isInjectedPromptText(event.text)) {
      preamblesSkipped += 1;
      continue;
    }
    firstEvent = event;
    firstEventIndex = index;
    break;
  }

  // 尾部扫描：从后往前拿，最多 maxScanEvents 条原始事件（跨会话事件也计数）。
  const tail: TailEntry[] = [];
  let scanned = 0;
  let unrelatedEvents = 0;
  let excludedByType = 0;
  let lowestScanned = events.length;
  for (let index = events.length - 1; index >= 0; index -= 1) {
    if (scanned >= limits.maxScanEvents) break;
    const event = events[index];
    if (!event) {
      lowestScanned = index;
      continue;
    }
    scanned += 1;
    lowestScanned = index;
    if (event.sessionId !== sessionId) {
      unrelatedEvents += 1;
      continue;
    }
    if (!isSessionReferenceEvent(event)) {
      excludedByType += 1;
      continue;
    }
    tail.push({ event, index });
  }
  const scanCut = lowestScanned > 0;
  tail.reverse();

  // æåè¯·æ±åå»ºä¸æ¬¡ï¼è¥å®è½å¨å°¾é¨çªå£éï¼åé¢ç´æ¥å¤ç¨ï¼é¿åéå¤æ¸洗与重复洗凭据计数）。
  const firstDraft = firstEvent
    ? buildUserDraft(firstEvent, firstEventIndex, limits.maxFirstRequestBytes, tally)
    : null;

  // 拼轮次：user_message 开一轮，assistant_message 追加到当前轮。
  const draftTurns: DraftTurn[] = [];
  let currentTurn: DraftTurn | null = null;
  for (const entry of tail) {
    const { event, index } = entry;
    if (event.type === "user_message") {
      const user =
        index === firstEventIndex && firstDraft
          ? constrainDraft(firstDraft, limits.maxMessageBytes)
          : buildUserDraft(event, index, limits.maxMessageBytes, tally);
      currentTurn = { user, replies: [] };
      draftTurns.push(currentTurn);
      continue;
    }
    if (event.type === "assistant_message") {
      if (!currentTurn) {
        currentTurn = { user: null, replies: [] };
        draftTurns.push(currentTurn);
      }
      const reply = buildAssistantDraft(event, index, limits.maxMessageBytes, tally);
      if (reply) currentTurn.replies.push(reply);
    }
  }

  // 每轮回复条数上限：保留最近的。
  let repliesOmitted = 0;
  for (const turn of draftTurns) {
    if (turn.replies.length <= limits.maxRepliesPerTurn) continue;
    repliesOmitted += turn.replies.length - limits.maxRepliesPerTurn;
    turn.replies = turn.replies.slice(-limits.maxRepliesPerTurn);
  }

  const windowTurns = draftTurns.filter((turn) => turn.user !== null || turn.replies.length > 0);
  const turnsInWindow = windowTurns.length;
  const limitedTurns = windowTurns.slice(-limits.maxTurns);
  const droppedTurnsByLimit = turnsInWindow - limitedTurns.length;

  // å·¥å·ç»æï¼åå»ä»·è®¡æ°ï¼ååªæå»ºææ°ç N æ¡æåç»æã
  let toolOutcomesInWindow = 0;
  for (const entry of tail) {
    if (entry.event.type === "tool_call" && toolSucceeded(entry.event)) toolOutcomesInWindow += 1;
  }
  const newestOutcomes: SessionReferenceToolOutcome[] = [];
  for (
    let index = tail.length - 1;
    index >= 0 && newestOutcomes.length < limits.maxToolOutcomes;
    index -= 1
  ) {
    const event = tail[index]?.event;
    if (!event || event.type !== "tool_call" || !toolSucceeded(event)) continue;
    newestOutcomes.push(buildToolOutcome(event, limits.maxToolOutcomeBytes, tally));
  }
  newestOutcomes.reverse();
  const droppedToolsByLimit = toolOutcomesInWindow - newestOutcomes.length;

  // ── 渲染 ─────────────────────────────────────────────────────────────────
  const render = (state: RenderState): string => {
    const lines: string[] = [];
    const push = (line = "") => {
      lines.push(line);
    };
    const first = state.first;
    const firstInTurns =
      first !== null && state.turns.some((turn) => turn.user?.sourceIndex === first.sourceIndex);
    const messageClips = collectAttachmentCount(state.turns);
    const droppedTurns = state.droppedTurnsByLimit + state.droppedTurnsByBytes;
    const droppedTools = state.droppedToolsByLimit + state.droppedToolsByBytes;

    push(
      `<marionette-session-reference version="${SESSION_REFERENCE_VERSION}" session="${sessionId}">`,
    );
    push();
    push(`## Session reference - ${metadata.label}`);
    push(`- Session ID: ${metadata.sessionId}`);
    push(`- Project: ${metadata.projectId}`);
    push(`- Agent: ${metadata.agentId} - status: ${metadata.status}`);
    const exited = metadata.exitedAt ? ` (exited at ${metadata.exitedAt})` : "";
    push(`- Time: ${metadata.startedAt} -> ${metadata.lastActiveAt}${exited}`);
    const extras: string[] = [];
    if (metadata.origin) extras.push(`origin ${metadata.origin}`);
    if (metadata.preferredModel) extras.push(`recent model ${metadata.preferredModel}`);
    if (metadata.preferredMode) extras.push(`recent mode ${metadata.preferredMode}`);
    if (extras.length > 0) push(`- ${extras.join(" - ")}`);

    if (first && !firstInTurns && first.text) {
      push();
      push(`### ${state.firstExact ? "Initial request" : "Earliest visible user request"}`);
      push();
      push(first.text);
    }

    if (state.turns.length > 0) {
      push();
      push(`### Recent conversation (last ${state.turns.length} turns)`);
      for (const turn of state.turns) {
        push();
        if (turn.user && turn.user.text) {
          push(`**You** - ${clipToBytes(turn.user.createdAt, 40).text}`);
          push();
          push(turn.user.text);
        } else {
          push("**(The prompt for this turn is outside the reference window.)**");
        }
        for (const reply of turn.replies) {
          if (!reply.text) continue;
          push();
          push(`**Assistant** - ${clipToBytes(reply.createdAt, 40).text}`);
          push();
          push(reply.text);
        }
      }
    }

    if (state.tools.length > 0) {
      push();
      push(`### Successful tool outcomes (${state.tools.length})`);
      for (const outcome of state.tools) {
        push();
        const path = outcome.path ? ` - ${outcome.path}` : "";
        const done = outcome.completedAt ? ` - ${clipToBytes(outcome.completedAt, 40).text}` : "";
        push(`- ${outcome.toolName}${path} - ${outcome.status}${done}`);
        if (outcome.summary) push(`  ${outcome.summary}`);
      }
    }

    if (state.turns.length === 0 && state.tools.length === 0 && (!first || firstInTurns)) {
      push();
      push("(This session has no quotable conversation content in the reference window.)");
    }

    push();
    push("### Included scope");
    push(`- Only events from session ${sessionId}; other sessions and subagents are excluded.`);
    push(
      "- Excluded: thoughts, notices, file changes, subtasks, plan/permission/question cards, raw tool input and full tool output bodies.",
    );
    if (state.redactions > 0) push(`- Redacted ${state.redactions} possible credential value(s).`);
    if (droppedTurns > 0) push(`- Omitted ${droppedTurns} earlier conversation turn(s).`);
    if (state.droppedReplies > 0) push(`- Omitted ${state.droppedReplies} earlier assistant reply item(s).`);
    if (droppedTools > 0) push(`- Omitted ${droppedTools} earlier successful tool outcome(s).`);
    if (messageClips > 0) push(`- Clipped ${messageClips} message(s) at the per-message byte limit.`);
    if (first?.clipped) push("- Clipped the initial request at the per-message byte limit.");
    if (state.firstDroppedForBytes) push("- Omitted the initial request to satisfy the total snapshot byte limit.");
    if (state.scanCut) push(`- Scanned only the most recent ${state.scanned} event(s) in the window.`);
    push(`- Snapshot byte limit: ${limits.maxBytes}.`);
    push();
    push("</marionette-session-reference>");
    return lines.join("\n");
  };
  let includedTurns = limitedTurns;
  let includedTools = newestOutcomes;
  let firstForRender: DraftMessage | null = firstDraft;
  let droppedTurnsByBytes = 0;
  let droppedToolsByBytes = 0;
  let firstDroppedForBytes = false;
  let hardClipped = false;

  const composeState = (): RenderState => ({
    turns: includedTurns,
    tools: includedTools,
    first: firstForRender,
    firstExact: firstEventIndex >= 0 && !headTruncated,
    firstDroppedForBytes,
    droppedTurnsByLimit,
    droppedTurnsByBytes,
    droppedToolsByLimit,
    droppedToolsByBytes,
    droppedReplies: repliesOmitted,
    scanCut,
    scanned,
    redactions: tally.count,
  });

  let text = render(composeState());
  let guard = 0;
  while (utf8ByteLength(text) > limits.maxBytes && guard < 64) {
    guard += 1;
    if (includedTurns.length > 0) {
      includedTurns = includedTurns.slice(1);
      droppedTurnsByBytes += 1;
    } else if (includedTools.length > 0) {
      includedTools = includedTools.slice(1);
      droppedToolsByBytes += 1;
    } else if (firstForRender) {
      firstForRender = null;
      firstDroppedForBytes = true;
    } else {
      hardClipped = true;
      text = clipToBytes(text, limits.maxBytes).text;
      break;
    }
    text = render(composeState());
  }
  if (utf8ByteLength(text) > limits.maxBytes) {
    hardClipped = true;
    text = clipToBytes(text, limits.maxBytes).text;
  }

  // ── 事实与理由 ───────────────────────────────────────────────────────────
  const messageClips = collectAttachmentCount(includedTurns);
  const reasons = new Set<SessionReferenceTruncationReason>();
  if (scanCut) reasons.add("max_scan_events");
  if (droppedTurnsByLimit > 0) reasons.add("max_turns");
  if (repliesOmitted > 0) reasons.add("max_replies_per_turn");
  if (droppedToolsByLimit > 0) reasons.add("max_tool_outcomes");
  if (droppedTurnsByBytes > 0 || droppedToolsByBytes > 0 || firstDroppedForBytes || hardClipped) {
    reasons.add("max_bytes");
  }
  if (messageClips > 0) reasons.add("message_clipped");
  if (firstDraft?.clipped) reasons.add("first_request_clipped");
  if (!firstDraft) reasons.add("first_request_missing");
  if (headTruncated) reasons.add("source_head_truncated");
  if (tally.count > 0) reasons.add("credentials_redacted");
  if (tail.length === 0) reasons.add("no_events");

  const orderedReasons = REASON_ORDER.filter((reason) => reasons.has(reason));
  const truncated = orderedReasons.some((reason) => LOSS_REASONS.has(reason));

  const firstUserRequestInTurns =
    firstDraft !== null &&
    includedTurns.some((turn) => turn.user?.sourceIndex === firstDraft.sourceIndex);

  return {
    kind: "marionette.session-reference",
    version: SESSION_REFERENCE_VERSION,
    sessionId,
    metadata,
    firstUserRequest: firstDraft ? toMessage(firstDraft) : null,
    firstUserRequestExact: firstDraft !== null && !headTruncated,
    firstUserRequestInTurns,
    turns: includedTurns.map((turn) => ({
      user: turn.user ? toMessage(turn.user) : null,
      replies: turn.replies.map(toMessage),
    })),
    toolOutcomes: includedTools.map((outcome) => ({ ...outcome })),
    truncation: {
      truncated,
      reasons: orderedReasons,
      limits,
      eventsProvided: events.length,
      eventsScanned: scanned,
      firstRequestScanned: firstScanned,
      unrelatedEvents,
      excludedByType,
      preamblesSkipped,
      eligibleEvents: tail.length,
      turnsInWindow,
      turnsKept: includedTurns.length,
      repliesOmitted,
      toolOutcomesInWindow,
      toolOutcomesKept: includedTools.length,
      messageClips,
      redactions: tally.count,
      bytes: utf8ByteLength(text),
      sourceHeadTruncated: headTruncated,
      scanCutOffEvents: scanCut,
    },
    text,
  };
}
