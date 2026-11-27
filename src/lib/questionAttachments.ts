/**
 * Attachments for ask-card answers (P1-2).
 *
 * Neither wire channel (Grok `_x.ai/ask_user_question` nor ACP
 * `elicitation/create` form) has an attachment field. Putting a path in an
 * option value would corrupt the answer schema, so files stay *beside* the
 * answer:
 *
 *   1. the card collects a per-question draft (paths only - nothing is copied,
 *      so cancelling can never delete a user's file);
 *   2. on confirm the host sends ONE explicit follow-up user message that lists
 *      the saved paths (see `buildQuestionAttachmentFollowUp`).
 *
 * The draft registry below is module-level and keyed by `sessionId + requestId`
 * so switching questions (or remounting the card while the same request is
 * still pending) never loses what the user already attached. It is bounded and
 * cleared on submit/decline.
 */

import type { AskQuestionItem, AskQuestionPrompt } from "../components/AskQuestionCard";
import { sendAcpPrompt, pickFiles } from "./api";
import { fileNameFromPath, imagePathKey } from "./imageAttachments";
import { t } from "./i18n";

export type QuestionAttachment = {
  id: string;
  /** Absolute filesystem path, exactly as picked. Never rewritten. */
  path: string;
  name: string;
};

/** The single follow-up message that carries attachment paths. */
export type QuestionAttachmentFollowUp = {
  kind: "question-attachments";
  /** Human-readable message body; paths are always spelled out in it. */
  text: string;
  /** Deduped paths, also usable as `_meta` file hints. */
  paths: string[];
};

/** Does this question accept a free-text answer at all? */
export function questionAcceptsCustomText(q: AskQuestionItem): boolean {
  if (q.options.length === 0) return true;
  return q.other ?? true;
}

/**
 * Attachment UI is allowed only when the schema accepts a custom text answer.
 * The payload flag can *veto* (an agent that wants a pure pick), never enable
 * attachments on a fixed-choice-only question.
 */
export function questionAcceptsAttachments(q: AskQuestionItem): boolean {
  if (!questionAcceptsCustomText(q)) return false;
  return q.attachmentsAllowed !== false;
}

const MAX_DRAFT_KEYS = 24;
const DRAFT_SEPARATOR = "\u0000";

type DraftPrompt = Pick<AskQuestionPrompt, "sessionId" | "requestId">;

const DRAFTS = new Map<string, QuestionAttachment[][]>();

function draftKey(prompt: DraftPrompt): string {
  return `${prompt.sessionId}${DRAFT_SEPARATOR}${prompt.requestId}`;
}

function cloneRows(rows: readonly (readonly QuestionAttachment[])[]): QuestionAttachment[][] {
  return rows.map((row) => row.map((attachment) => ({ ...attachment })));
}

/** Per-question drafts for a request, padded to `count` rows. */
export function readQuestionAttachmentDrafts(
  prompt: DraftPrompt,
  count: number,
): QuestionAttachment[][] {
  const stored = DRAFTS.get(draftKey(prompt));
  return Array.from({ length: Math.max(0, count) }, (_, index) =>
    (stored?.[index] ?? []).map((attachment) => ({ ...attachment })),
  );
}

/**
 * Persist drafts. Empty rows are not stored, so a card that was opened and left
 * alone never keeps a registry entry. Keys for older requests of the same
 * session are dropped - only the live form can still be submitted.
 */
export function writeQuestionAttachmentDrafts(
  prompt: DraftPrompt,
  drafts: readonly (readonly QuestionAttachment[])[],
): void {
  const key = draftKey(prompt);
  if (drafts.every((row) => row.length === 0)) {
    DRAFTS.delete(key);
    return;
  }
  const sessionPrefix = `${prompt.sessionId}${DRAFT_SEPARATOR}`;
  for (const existing of [...DRAFTS.keys()]) {
    if (existing !== key && existing.startsWith(sessionPrefix)) DRAFTS.delete(existing);
  }
  DRAFTS.set(key, cloneRows(drafts));
  while (DRAFTS.size > MAX_DRAFT_KEYS) {
    const oldest = DRAFTS.keys().next().value;
    if (oldest === undefined) break;
    DRAFTS.delete(oldest);
  }
}

export function clearQuestionAttachmentDrafts(prompt: DraftPrompt): void {
  DRAFTS.delete(draftKey(prompt));
}

/** Session teardown hook (wiring gap: App's delete path should call this). */
export function forgetQuestionAttachmentDrafts(sessionId: string): void {
  const prefix = `${sessionId}${DRAFT_SEPARATOR}`;
  for (const key of [...DRAFTS.keys()]) {
    if (key.startsWith(prefix)) DRAFTS.delete(key);
  }
}

function questionAttachmentId(): string {
  return `qa-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 7)}`;
}

/**
 * Native file pick, merged into the existing row.
 *
 * Files are referenced in place (same ownership model as Composer drops):
 * Marionette copies nothing, so there is nothing to delete on cancel/decline,
 * and a duplicate pick keeps a single path.
 */
export async function pickQuestionAttachments(
  existing: readonly QuestionAttachment[],
): Promise<QuestionAttachment[]> {
  const picked = await pickFiles();
  if (picked.length === 0) return [...existing];
  const next = [...existing];
  const seen = new Set(next.map((attachment) => imagePathKey(attachment.path)));
  for (const raw of picked) {
    const path = raw.trim();
    if (!path) continue;
    const key = imagePathKey(path);
    if (!key || seen.has(key)) continue;
    seen.add(key);
    next.push({ id: questionAttachmentId(), path, name: fileNameFromPath(path) });
  }
  return next;
}

function oneLine(value: string): string {
  return value.replace(/[\r\n]+/g, " ").trim();
}

/**
 * Build the one explicit follow-up message. Returns null when nothing was
 * attached (the normal case), so callers can skip the extra prompt entirely.
 */
export function buildQuestionAttachmentFollowUp(
  questions: readonly AskQuestionItem[],
  drafts: readonly (readonly QuestionAttachment[])[],
): QuestionAttachmentFollowUp | null {
  const groups: { index: number; question: string; attachments: QuestionAttachment[] }[] = [];
  questions.forEach((question, index) => {
    if (!questionAcceptsAttachments(question)) return;
    const attachments = (drafts[index] ?? []).filter((attachment) => attachment.path.trim());
    if (attachments.length === 0) return;
    groups.push({ index, question: oneLine(question.question), attachments });
  });
  if (groups.length === 0) return null;

  const paths: string[] = [];
  const seen = new Set<string>();
  for (const group of groups) {
    for (const attachment of group.attachments) {
      const key = imagePathKey(attachment.path);
      if (seen.has(key)) continue;
      seen.add(key);
      paths.push(attachment.path);
    }
  }

  const lines: string[] = [t("ask.attachFollowUpHeader")];
  for (const group of groups) {
    lines.push("");
    lines.push(
      t("ask.attachFollowUpQuestion", {
        index: group.index + 1,
        question: group.question,
      }),
    );
    for (const attachment of group.attachments) {
      lines.push(`- ${oneLine(attachment.name)} - ${oneLine(attachment.path)}`);
    }
  }
  lines.push("");
  lines.push(t("ask.attachFollowUpNote"));

  return {
    kind: "question-attachments",
    text: lines.join("\n"),
    paths,
  };
}

/**
 * Send the follow-up after `respond_acp_question` has been accepted.
 *
 * ACP rejects a second `session/prompt` while the answered turn is still in
 * flight, so the message is queued first: call this right after the answer is
 * accepted and call `flushQuestionAttachmentFollowUps` on `turn/complete`.
 * Returns "sent" when it went out immediately, "queued" when the turn is still
 * running (the normal case).
 */
export async function sendQuestionAttachmentFollowUp(
  sessionId: string,
  followUp: QuestionAttachmentFollowUp | null | undefined,
): Promise<"sent" | "queued" | "skipped"> {
  if (!followUp || !followUp.text.trim()) return "skipped";
  queueQuestionAttachmentFollowUp(sessionId, followUp);
  try {
    await flushQuestionAttachmentFollowUps(sessionId);
    return "sent";
  } catch {
    return "queued";
  }
}

const PENDING_FOLLOW_UPS = new Map<string, QuestionAttachmentFollowUp[]>();
const MAX_PENDING_PER_SESSION = 8;
const MAX_PENDING_SESSIONS = 24;

/** Queue one follow-up for a session (dedup by text). */
export function queueQuestionAttachmentFollowUp(
  sessionId: string,
  followUp: QuestionAttachmentFollowUp | null | undefined,
): boolean {
  if (!followUp || !followUp.text.trim()) return false;
  const list = PENDING_FOLLOW_UPS.get(sessionId) ?? [];
  if (list.some((item) => item.text === followUp.text)) return false;
  list.push(followUp);
  while (list.length > MAX_PENDING_PER_SESSION) list.shift();
  PENDING_FOLLOW_UPS.set(sessionId, list);
  while (PENDING_FOLLOW_UPS.size > MAX_PENDING_SESSIONS) {
    const oldest = PENDING_FOLLOW_UPS.keys().next().value;
    if (oldest === undefined) break;
    PENDING_FOLLOW_UPS.delete(oldest);
  }
  return true;
}

/**
 * Deliver queued follow-ups, one prompt per message. On failure the failed and
 * remaining items stay queued (so a turn-boundary error is retryable) and the
 * error is rethrown for the host to log.
 */
export async function flushQuestionAttachmentFollowUps(sessionId: string): Promise<number> {
  const list = PENDING_FOLLOW_UPS.get(sessionId);
  if (!list || list.length === 0) return 0;
  let sent = 0;
  while (list.length > 0) {
    const followUp = list[0]!;
    try {
      await sendAcpPrompt(sessionId, followUp.text, [], false, followUp.paths);
    } catch (error) {
      PENDING_FOLLOW_UPS.set(sessionId, list);
      throw error;
    }
    list.shift();
    sent += 1;
  }
  PENDING_FOLLOW_UPS.delete(sessionId);
  return sent;
}

/** Drop queued follow-ups (session delete / answers cancelled). */
export function forgetQuestionAttachmentFollowUps(sessionId: string): void {
  PENDING_FOLLOW_UPS.delete(sessionId);
}

