import type { AskQuestionItem, AskQuestionPrompt } from "../components/AskQuestionCard";

function asRecord(value: unknown): Record<string, unknown> | null {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

function asString(value: unknown): string {
  return typeof value === "string" ? value : "";
}

/**
 * Parse a `question/prompt` ACP event (from `_x.ai/ask_user_question`).
 * Returns null when the payload is unusable.
 */
export function parseAskQuestionPrompt(
  sessionId: string,
  data: unknown,
): AskQuestionPrompt | null {
  const root = asRecord(data);
  if (!root) return null;
  const requestId = asString(root.requestId);
  if (!requestId) return null;

  let questionsRaw = root.questions;
  if (!Array.isArray(questionsRaw)) {
    // Nested params fallback
    const params = asRecord(root.params);
    questionsRaw = params?.questions;
  }
  if (!Array.isArray(questionsRaw) || questionsRaw.length === 0) return null;

  const questions: AskQuestionItem[] = [];
  for (const item of questionsRaw) {
    const rec = asRecord(item);
    if (!rec) continue;
    const question = asString(rec.question);
    const optionsRaw = Array.isArray(rec.options) ? rec.options : [];
    const options = optionsRaw
      .map((o) => {
        const or = asRecord(o);
        if (!or) return null;
        const label = asString(or.label);
        if (!label) return null;
        const description = asString(or.description);
        return description ? { label, description } : { label };
      })
      .filter((o): o is NonNullable<typeof o> => o != null);
    if (!question && options.length === 0) continue;
    questions.push({
      question: question || "请选择",
      header: asString(rec.header) || undefined,
      multiSelect: rec.multiSelect === true || rec.multi_select === true,
      options,
    });
  }
  if (questions.length === 0) return null;

  return {
    requestId,
    sessionId: asString(root.sessionId) || sessionId,
    toolCallId:
      typeof root.toolCallId === "string"
        ? root.toolCallId
        : typeof root.tool_call_id === "string"
          ? root.tool_call_id
          : null,
    mode: typeof root.mode === "string" ? root.mode : null,
    questions,
  };
}
