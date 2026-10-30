import { cleanAssistantText, stripSectionMarkers } from "./markdownText";
import type { SessionEvent } from "./types";
import { parseWebReply } from "./webReply";
import { t } from "./i18n";

const PERSISTABLE = new Set([
  "user_message",
  "assistant_message",
  "thought",
  "tool_call",
  "handoff_prepared",
  "file_change",
  "subtask_started",
  "subtask_result",
  "plan_prompt",
  "ask_prompt",
]);

export function isPersistableEvent(event: SessionEvent): boolean {
  return PERSISTABLE.has(event.type);
}

export function persistableEventsForSession(
  events: SessionEvent[],
  sessionId: string,
): SessionEvent[] {
  return events
    .filter((e) => e.sessionId === sessionId && isPersistableEvent(e))
    .map((event) => {
      if (event.type === "assistant_message") {
        return { ...event, text: cleanAssistantText(event.text) };
      }
      if (event.type === "thought") {
        return { ...event, text: stripSectionMarkers(event.text) };
      }
      if (event.type === "tool_call" && event.images && event.images.length > 0) {
        const images = event.images.filter((src) => src && !src.startsWith("data:"));
        return { ...event, images: images.length > 0 ? images : undefined };
      }
      return event;
    });
}

/** Best-effort revive of a persisted turnStats blob; null when unusable. */
function parseTurnStats(raw: unknown): import("./types").TurnStats | null {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return null;
  const e = raw as Record<string, unknown>;
  const num = (v: unknown): number | null =>
    typeof v === "number" && Number.isFinite(v) ? v : null;
  const stats: import("./types").TurnStats = {};
  for (const key of [
    "input",
    "output",
    "cached",
    "reasoning",
    "total",
    "ttftMs",
    "durationMs",
    "outputTps",
    "ppTps",
  ] as const) {
    const n = num(e[key]);
    if (n != null) stats[key] = n;
  }
  return Object.keys(stats).length > 0 ? stats : null;
}

/** Best-effort revive of JSONL rows written by write_transcript. */
export function parseTranscriptEvents(raw: unknown[]): SessionEvent[] {
  const out: SessionEvent[] = [];
  for (const row of raw) {
    if (!row || typeof row !== "object") continue;
    const e = row as Record<string, unknown>;
    const type = typeof e.type === "string" ? e.type : "";
    const sessionId = typeof e.sessionId === "string" ? e.sessionId : "";
    const createdAt = typeof e.createdAt === "string" ? e.createdAt : new Date().toISOString();
    if (!sessionId || !PERSISTABLE.has(type)) continue;

    if (type === "user_message" || type === "assistant_message" || type === "thought") {
      const rawText = typeof e.text === "string" ? e.text : "";
      const text =
        type === "user_message"
          ? rawText
          : type === "assistant_message"
            ? cleanAssistantText(rawText)
            : stripSectionMarkers(rawText);
      if (type !== "user_message" && !text.trim()) continue;
      const messageId = typeof e.messageId === "string" ? e.messageId : undefined;
      out.push({
        type,
        sessionId,
        text,
        createdAt,
        ...(messageId ? { messageId } : {}),
        // Optional metadata — old transcripts won't have them, new ones will restore correctly
        ...(typeof e.agentId === "string" ? { agentId: e.agentId } : {}),
        ...(typeof e.agentLabel === "string" ? { agentLabel: e.agentLabel } : {}),
        ...(typeof e.modelId === "string" ? { modelId: e.modelId } : {}),
        ...(typeof e.modelLabel === "string" ? { modelLabel: e.modelLabel } : {}),
        ...(typeof e.modeLabel === "string" ? { modeLabel: e.modeLabel } : {}),
        ...(typeof e.effortLabel === "string" ? { effortLabel: e.effortLabel } : {}),
        ...(typeof e.durationMs === "number" ? { durationMs: e.durationMs } : {}),
        ...(typeof e.endedAt === "string" ? { endedAt: e.endedAt } : {}),
        ...(type === "user_message" && typeof e.sentAt === "string" ? { sentAt: e.sentAt } : {}),
        ...(type === "assistant_message"
          ? { turnStats: parseTurnStats(e.turnStats), webReply: parseWebReply(e.webReply) }
          : {}),
        ...(type === "user_message" && Array.isArray(e.attachments)
          ? { attachments: e.attachments as import("./imageAttachments").ImageAttachment[] }
          : {}),
        ...(type === "user_message" && e.forceWebSearch === true
          ? { forceWebSearch: true }
          : {}),
      } as SessionEvent);
      continue;
    }
    if (type === "tool_call") {
      out.push({
        type: "tool_call",
        sessionId,
        text: typeof e.text === "string" ? e.text : "",
        createdAt,
        toolCallId: typeof e.toolCallId === "string" ? e.toolCallId : undefined,
        status: typeof e.status === "string" ? e.status : undefined,
        title: typeof e.title === "string" ? e.title : undefined,
        toolName: typeof e.toolName === "string" ? e.toolName : undefined,
        // Older transcripts only have `text`; the card falls back to it.
        path: typeof e.path === "string" ? e.path : undefined,
        detail: typeof e.detail === "string" ? e.detail : undefined,
        input: typeof e.input === "string" ? e.input : undefined,
        ...(typeof e.completedAt === "string" ? { completedAt: e.completedAt } : {}),
        ...(Array.isArray(e.images)
          ? (() => {
              const images = e.images.filter(
                (src): src is string =>
                  typeof src === "string" && src.length > 0 && !src.startsWith("data:"),
              );
              return images.length > 0 ? { images } : {};
            })()
          : {}),
      });
      continue;
    }
    if (type === "handoff_prepared") {
      out.push({
        type: "handoff_prepared",
        sessionId,
        targetAgentId: typeof e.targetAgentId === "string" ? e.targetAgentId : "",
        handoffPath: typeof e.handoffPath === "string" ? e.handoffPath : "",
        prompt: typeof e.prompt === "string" ? e.prompt : "",
        createdAt,
      });
      continue;
    }
    if (type === "file_change") {
      const changeType = e.changeType;
      if (
        changeType === "added" ||
        changeType === "modified" ||
        changeType === "deleted" ||
        changeType === "untracked"
      ) {
        out.push({
          type: "file_change",
          sessionId,
          path: typeof e.path === "string" ? e.path : "",
          changeType,
          createdAt,
          ...(typeof e.revision === "number" ? { revision: e.revision } : {}),
          ...(typeof e.oldPath === "string" && e.oldPath ? { oldPath: e.oldPath } : {}),
          ...(typeof e.add === "number" ? { add: e.add } : {}),
          ...(typeof e.del === "number" ? { del: e.del } : {}),
          ...(e.binary === true ? { binary: true } : {}),
        });
      }
      continue;
    }
    if (type === "subtask_started") {
      out.push({
        type: "subtask_started",
        sessionId,
        childSessionId: typeof e.childSessionId === "string" ? e.childSessionId : "",
        agentId: typeof e.agentId === "string" ? e.agentId : "",
        agentLabel: typeof e.agentLabel === "string" ? e.agentLabel : "",
        modelId: typeof e.modelId === "string" ? e.modelId : undefined,
        prompt: typeof e.prompt === "string" ? e.prompt : "",
        createdAt,
      });
      continue;
    }
    if (type === "subtask_result") {
      const st = e.status;
      const status =
        st === "done" || st === "failed" || st === "cancelled" || st === "timeout"
          ? st
          : "failed";
      out.push({
        type: "subtask_result",
        sessionId,
        childSessionId: typeof e.childSessionId === "string" ? e.childSessionId : "",
        agentId: typeof e.agentId === "string" ? e.agentId : "",
        status,
        summary: typeof e.summary === "string" ? e.summary : "",
        durationMs: typeof e.durationMs === "number" ? e.durationMs : undefined,
        error: typeof e.error === "string" ? e.error : undefined,
        createdAt,
      });
      continue;
    }
    if (type === "plan_prompt") {
      const source = e.source === "permission" ? "permission" : "plan_approval";
      const st = e.status;
      const status =
        st === "answered" || st === "declined" || st === "timeout" ? st : "pending";
      const options = Array.isArray(e.options)
        ? e.options.flatMap((row) => {
            if (!row || typeof row !== "object") return [];
            const o = row as Record<string, unknown>;
            const optionId = typeof o.optionId === "string" ? o.optionId : "";
            if (!optionId) return [];
            return [
              {
                optionId,
                name: typeof o.name === "string" ? o.name : optionId,
                kind: typeof o.kind === "string" ? o.kind : "",
              },
            ];
          })
        : undefined;
      out.push({
        type: "plan_prompt",
        sessionId,
        requestId: typeof e.requestId === "string" ? e.requestId : "",
        source,
        ...(typeof e.title === "string" && e.title ? { title: e.title } : {}),
        planMarkdown: typeof e.planMarkdown === "string" ? e.planMarkdown : "",
        ...(options && options.length > 0 ? { options } : {}),
        status,
        ...(typeof e.decision === "string" ? { decision: e.decision } : {}),
        ...(typeof e.decisionLabel === "string" ? { decisionLabel: e.decisionLabel } : {}),
        ...(typeof e.feedback === "string" ? { feedback: e.feedback } : {}),
        createdAt,
        ...(typeof e.answeredAt === "string" ? { answeredAt: e.answeredAt } : {}),
      });
      continue;
    }
    if (type === "ask_prompt") {
      const st = e.status;
      const status =
        st === "answered" || st === "declined" || st === "timeout" ? st : "pending";
      const questions = Array.isArray(e.questions)
        ? e.questions.flatMap((row) => {
            if (!row || typeof row !== "object") return [];
            const q = row as Record<string, unknown>;
            const question = typeof q.question === "string" ? q.question : "";
            const optionsRaw = Array.isArray(q.options) ? q.options : [];
            const options = optionsRaw.flatMap((opt) => {
              if (!opt || typeof opt !== "object") return [];
              const o = opt as Record<string, unknown>;
              const label = typeof o.label === "string" ? o.label : "";
              if (!label) return [];
              return [
                {
                  label,
                  ...(typeof o.description === "string" && o.description
                    ? { description: o.description }
                    : {}),
                },
              ];
            });
            if (!question && options.length === 0) return [];
            return [
              {
                question: question || t("transcript.pleaseSelect"),
                ...(typeof q.header === "string" && q.header ? { header: q.header } : {}),
                ...(q.multiSelect === true || q.multi_select === true
                  ? { multiSelect: true }
                  : {}),
                options,
              },
            ];
          })
        : [];
      const answers = Array.isArray(e.answers)
        ? e.answers.flatMap((row) => {
            if (!row || typeof row !== "object") return [];
            const a = row as Record<string, unknown>;
            const question = typeof a.question === "string" ? a.question : "";
            const selected = Array.isArray(a.selected)
              ? a.selected.filter((s): s is string => typeof s === "string")
              : [];
            return [{ question, selected }];
          })
        : undefined;
      out.push({
        type: "ask_prompt",
        sessionId,
        requestId: typeof e.requestId === "string" ? e.requestId : "",
        ...(typeof e.mode === "string" ? { mode: e.mode } : {}),
        questions,
        status,
        ...(answers && answers.length > 0 ? { answers } : {}),
        createdAt,
        ...(typeof e.answeredAt === "string" ? { answeredAt: e.answeredAt } : {}),
      });
    }
  }
  return out;
}

export function titleFromUserText(text: string): string {
  const oneLine = text.replace(/\s+/g, " ").trim();
  if (!oneLine) return "New session";
  if (oneLine.length <= 48) return oneLine;
  return `${oneLine.slice(0, 46)}…`;
}

/**
 * Marionette prefixes three prompts of its own: the rehydrated-history header
 * (`sessionHistory.ts`), the project skills list (`context_inventory.rs`), and
 * the shared-memory card that opens every agent process's first prompt
 * (`memory.rs`). An agent that titles a conversation from its first prompt
 * hands one back, so shelves read "[Marionette — …" or
 * "<marionette-memory> 以下是用户的长期偏好…". Titles and labels derived from
 * that scaffolding are refused.
 */
const INJECTED_PREAMBLES: RegExp[] = [
  /^\[\s*Marionette\s*[—–-]/,
  /^<marionette-memory>/,
];

/** True when `text` is, or begins with, a prompt Marionette injected itself. */
export function isInjectedPromptText(text: string): boolean {
  const trimmed = text.trim();
  return INJECTED_PREAMBLES.some((pattern) => pattern.test(trimmed));
}

export function shouldAutoRenameLabel(label: string): boolean {
  const t = label.trim();
  return (
    t === "" ||
    t === "New session" ||
    t === "New conversation" ||
    t === "新对话" ||
    t === "新会话" ||
    t === "Chat" ||
    t.startsWith("Chat · ") ||
    /^session-\d+$/i.test(t)
  );
}
