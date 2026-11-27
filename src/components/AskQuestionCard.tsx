import { HelpCircle, Paperclip, X } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useT } from "../lib/i18n";
import {
  buildQuestionAttachmentFollowUp,
  clearQuestionAttachmentDrafts,
  pickQuestionAttachments,
  questionAcceptsAttachments,
  readQuestionAttachmentDrafts,
  writeQuestionAttachmentDrafts,
  type QuestionAttachment,
  type QuestionAttachmentFollowUp,
} from "../lib/questionAttachments";

export type AskQuestionOption = {
  label: string;
  description?: string;
};

export type AskQuestionItem = {
  question: string;
  header?: string;
  multiSelect?: boolean;
  options: AskQuestionOption[];
  /**
   * Whether this question has a free-text answer that *replaces* the options
   * (「其他」). Decided by the agent's own declaration; absent = infer from
   * `options` (Grok's `x.ai/ask_user_question` always allows a typed answer).
   */
  other?: boolean;
  /**
   * Whether this question has a separate note channel: Codex's `_note`, or
   * Claude's `_custom` on a single-select question.
   */
  note?: boolean;
  /**
   * UI hint from the payload (elicitation `questions_for_ui` / Grok bridge).
   * There is no native ACP attachment field; attachments are only offered on
   * questions whose schema accepts a custom text answer, and `false` is a
   * veto. Absent = infer from `other` / `options`.
   */
  attachmentsAllowed?: boolean;
};

export type AskQuestionPrompt = {
  requestId: string;
  sessionId: string;
  toolCallId?: string | null;
  mode?: string | null;
  questions: AskQuestionItem[];
};

export type AskQuestionAnswer = {
  question: string;
  selected: string[];
  /** 「其他」：替代选项的自由文本答案。 */
  typed?: string;
  /** 「补充」：附加在答案旁边的一句说明。 */
  note?: string;
};

type AskQuestionCardProps = {
  prompt: AskQuestionPrompt;
  busy?: boolean;
  variant?: "dock" | "stream";
  /**
   * `followUp` is the one explicit message carrying attachment paths. It must
   * be sent only after the answer itself was accepted - the card cannot know
   * that, so the host owns the ordering (see `sendQuestionAttachmentFollowUp`).
   */
  onSubmit: (answers: AskQuestionAnswer[], followUp?: QuestionAttachmentFollowUp) => void;
  onDecline: () => void;
};

function splitRecommended(label: string): { text: string; recommended: boolean } {
  const m = label.match(/^(.*?)\s*\(recommended\)\s*$/i);
  const text = m?.[1]?.trim();
  return text ? { text, recommended: true } : { text: label, recommended: false };
}

function hasOptions(q: AskQuestionItem): boolean {
  return q.options.length > 0;
}

/** 「其他」可用：有 wire 上的自由文本槽（纯文本题天然可用）。 */
function otherAllowed(q: AskQuestionItem): boolean {
  return q.other ?? hasOptions(q);
}

/** 「补充」可用：有独立的附注槽。 */
function noteAllowed(q: AskQuestionItem): boolean {
  return q.note === true;
}

function questionCanProceed(
  q: AskQuestionItem,
  selected: string[],
  other: string,
): boolean {
  if (!hasOptions(q)) return other.trim().length > 0;
  if (otherAllowed(q) && other.trim()) return true;
  if (q.multiSelect) return selected.length > 0;
  return selected.length === 1;
}

/**
 * Full-width composer replacement: all questions + options visible at once.
 * Compact rows (todo-list density) so many choices fit without a wizard.
 *
 * Per-question capabilities come from the agent's schema: 「其他」 replaces the
 * selection, 「补充」 rides beside it, and a question without options is just a
 * text input. Single-select can be cleared by clicking the pick again — a
 * mistaken pick must not be a one-way door.
 */
export function AskQuestionCard({
  prompt,
  busy = false,
  variant = "stream",
  onSubmit,
  onDecline,
}: AskQuestionCardProps) {
  const questions = prompt.questions;
  const total = questions.length;
  const [picks, setPicks] = useState<string[][]>(() => questions.map(() => []));
  const [otherText, setOtherText] = useState<string[]>(() => questions.map(() => ""));
  const [noteText, setNoteText] = useState<string[]>(() => questions.map(() => ""));
  /**
   * Per-question attachment drafts. Paths only: nothing is copied, so leaving
   * the form can never delete a user file. The registry survives remounts of
   * this card while the same request is pending.
   */
  const [attachmentDrafts, setAttachmentDrafts] = useState<QuestionAttachment[][]>(() =>
    readQuestionAttachmentDrafts(prompt, questions.length),
  );
  /** Index of the question whose file picker is open, or null. */
  const [attaching, setAttaching] = useState<number | null>(null);
  const t = useT();

  const questionsRef = useRef(questions);
  questionsRef.current = questions;

  const draftKey = `${prompt.sessionId}\u0000${prompt.requestId}`;
  const draftKeyRef = useRef(draftKey);

  useEffect(() => {
    const current = questionsRef.current;
    setPicks(current.map(() => []));
    setOtherText(current.map(() => ""));
    setNoteText(current.map(() => ""));
    setAttachmentDrafts(readQuestionAttachmentDrafts(prompt, current.length));
    setAttaching(null);
  }, [draftKey]); // only reset on new request

  // Persist drafts as they change. The first pass after a request swap must not
  // write the previous request's rows under the new key; the reset effect above
  // repopulates state and the next pass stores the right rows.
  useEffect(() => {
    if (draftKeyRef.current !== draftKey) {
      draftKeyRef.current = draftKey;
      return;
    }
    writeQuestionAttachmentDrafts(
      { sessionId: prompt.sessionId, requestId: prompt.requestId },
      attachmentDrafts,
    );
  }, [attachmentDrafts, draftKey, prompt.requestId, prompt.sessionId]);

  const allReady = useMemo(() => {
    if (total === 0) return false;
    return questions.every((q, i) =>
      questionCanProceed(q, picks[i] ?? [], otherText[i] ?? ""),
    );
  }, [questions, picks, otherText, total]);

  const toggle = useCallback((qi: number, label: string, multi: boolean) => {
    setPicks((prev) => {
      const next = prev.map((row) => [...row]);
      const row = next[qi] ?? [];
      if (multi) {
        next[qi] = row.includes(label) ? row.filter((x) => x !== label) : [...row, label];
      } else {
        // Radio semantics, except a second click clears the choice.
        next[qi] = row.includes(label) ? [] : [label];
      }
      return next;
    });
    // Picking an option and typing 「其他」 are mutually exclusive.
    setOtherText((prev) => {
      if (!(prev[qi] ?? "").trim()) return prev;
      const next = [...prev];
      next[qi] = "";
      return next;
    });
  }, []);

  const changeOther = useCallback((qi: number, value: string) => {
    setOtherText((prev) => {
      const next = [...prev];
      next[qi] = value;
      return next;
    });
    // 「其他」 replaces the answer: clear any picked option while it has text.
    if (value.trim()) {
      setPicks((prev) => {
        if ((prev[qi] ?? []).length === 0) return prev;
        const next = prev.map((row) => [...row]);
        next[qi] = [];
        return next;
      });
    }
  }, []);

  const changeNote = useCallback((qi: number, value: string) => {
    setNoteText((prev) => {
      const next = [...prev];
      next[qi] = value;
      return next;
    });
  }, []);

  const addAttachments = useCallback(
    async (qi: number) => {
      if (busy || attaching !== null) return;
      setAttaching(qi);
      try {
        const next = await pickQuestionAttachments(attachmentDrafts[qi] ?? []);
        setAttachmentDrafts((prev) =>
          prev.map((row, index) => (index === qi ? next : row)),
        );
      } finally {
        setAttaching(null);
      }
    },
    [attachmentDrafts, attaching, busy],
  );

  const removeAttachment = useCallback((qi: number, id: string) => {
    setAttachmentDrafts((prev) =>
      prev.map((row, index) =>
        index === qi ? row.filter((attachment) => attachment.id !== id) : row,
      ),
    );
  }, []);

  const buildAnswers = useCallback((): AskQuestionAnswer[] => {
    return questions.map((q, i) => {
      const typed = (otherText[i] ?? "").trim();
      const note = (noteText[i] ?? "").trim();
      const answer: AskQuestionAnswer = {
        question: q.question,
        selected: [...(picks[i] ?? [])],
      };
      if (typed) answer.typed = typed;
      if (note && noteAllowed(q)) answer.note = note;
      return answer;
    });
  }, [questions, picks, otherText, noteText]);

  const submit = useCallback(() => {
    if (!allReady || busy || attaching !== null) return;
    // Paths never enter `selected`/`typed`: the answer body stays schema-valid,
    // and the files ride in one explicit follow-up message built here.
    const followUp = buildQuestionAttachmentFollowUp(questions, attachmentDrafts) ?? undefined;
    clearQuestionAttachmentDrafts(prompt);
    setAttachmentDrafts(questions.map(() => []));
    onSubmit(buildAnswers(), followUp);
  }, [
    allReady,
    attaching,
    attachmentDrafts,
    buildAnswers,
    busy,
    onSubmit,
    prompt,
    questions,
  ]);

  const decline = useCallback(() => {
    if (busy) return;
    clearQuestionAttachmentDrafts(prompt);
    setAttachmentDrafts(questions.map(() => []));
    onDecline();
  }, [busy, onDecline, prompt, questions]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Enter" || e.shiftKey || e.ctrlKey || e.metaKey || e.altKey) return;
      const tag = (e.target as HTMLElement | null)?.tagName?.toLowerCase();
      if (tag === "textarea") return;
      if (tag === "input") return;
      if (!allReady || busy || attaching !== null) return;
      e.preventDefault();
      submit();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [allReady, attaching, busy, submit]);

  if (total === 0) return null;

  return (
    <section
      className={
        variant === "stream"
          ? "ask-card ask-card--in-stream"
          : "ask-card ask-card--composer-slot"
      }
      role="region"
      aria-label={t("ask.needed")}
      aria-labelledby="ask-card-title"
    >
      <div className="ask-card__header">
        <HelpCircle size={14} aria-hidden />
        <div>
          <strong id="ask-card-title">{t("ask.needed")}</strong>
          <span>
            {prompt.mode ? t("ask.modeLabel", { mode: prompt.mode }) : t("ask.multipleChoice")}
            {total > 1 ? t("ask.questionCount", { count: total }) : ""}
            {t("ask.replyOnCard")}
          </span>
        </div>
      </div>

      <div className="ask-card__body custom-scrollbar scrollbar-autohide">
        {questions.map((q, qi) => {
          const multi = Boolean(q.multiSelect);
          const selected = picks[qi] ?? [];
          const other = otherText[qi] ?? "";
          const note = noteText[qi] ?? "";
          const ready = questionCanProceed(q, selected, other);
          const acceptsAttachments = questionAcceptsAttachments(q);
          const attachments = attachmentDrafts[qi] ?? [];
          const attachingHere = attaching === qi;
          return (
            <section
              key={`${qi}:${q.question.slice(0, 48)}`}
              className={ready ? "ask-card__question is-ready" : "ask-card__question"}
            >
              <div className="ask-card__qhead">
                {total > 1 ? (
                  <span className="ask-card__qnum" aria-hidden>
                    {qi + 1}
                  </span>
                ) : null}
                <div className="ask-card__qtext">
                  {q.header ? <span className="ask-card__chip">{q.header}</span> : null}
                  <p className="ask-card__prompt">{q.question}</p>
                  {multi ? <span className="ask-card__hint">{t("ask.multiHint")}</span> : null}
                </div>
              </div>

              {hasOptions(q) ? (
                <ul className="ask-card__options" role={multi ? "group" : "radiogroup"}>
                  {q.options.map((opt) => {
                    const { text, recommended } = splitRecommended(opt.label);
                    const on = selected.includes(opt.label);
                    return (
                      <li key={opt.label}>
                        <button
                          type="button"
                          className={on ? "ask-card__option is-selected" : "ask-card__option"}
                          disabled={busy}
                          aria-pressed={on}
                          onClick={() => toggle(qi, opt.label, multi)}
                        >
                          <span
                            className={
                              multi
                                ? on
                                  ? "ask-card__mark ask-card__mark--check is-on"
                                  : "ask-card__mark ask-card__mark--check"
                                : on
                                  ? "ask-card__mark ask-card__mark--radio is-on"
                                  : "ask-card__mark ask-card__mark--radio"
                            }
                            aria-hidden
                          />
                          <span className="ask-card__option-main">
                            <span className="ask-card__option-label">
                              {text}
                              {recommended ? (
                                <em className="ask-card__rec">{t("ask.recommendedBadge")}</em>
                              ) : null}
                            </span>
                            {opt.description ? (
                              <span className="ask-card__option-desc">{opt.description}</span>
                            ) : null}
                          </span>
                        </button>
                      </li>
                    );
                  })}
                </ul>
              ) : null}

              {hasOptions(q) ? (
                <>
                  {otherAllowed(q) ? (
                    <label className="ask-card__other">
                      <span>{t("ask.other")}</span>
                      <input
                        type="text"
                        value={other}
                        disabled={busy}
                        placeholder={t("ask.otherPlaceholder")}
                        onChange={(e) => changeOther(qi, e.target.value)}
                      />
                    </label>
                  ) : null}
                  {noteAllowed(q) ? (
                    <label className="ask-card__other ask-card__note">
                      <span>{t("ask.note")}</span>
                      <input
                        type="text"
                        value={note}
                        disabled={busy}
                        placeholder={t("ask.notePlaceholder")}
                        onChange={(e) => changeNote(qi, e.target.value)}
                      />
                    </label>
                  ) : null}
                </>
              ) : (
                <label className="ask-card__other">
                  <span>{t("ask.answer")}</span>
                  <input
                    type="text"
                    value={other}
                    disabled={busy}
                    placeholder={t("ask.answerPlaceholder")}
                    onChange={(e) => changeOther(qi, e.target.value)}
                  />
                </label>
              )}

              {acceptsAttachments ? (
                <div className="ask-card__attach">
                  <div className="ask-card__attach-head">
                    <span>{t("ask.attachments")}</span>
                    <button
                      type="button"
                      className="ask-card__attach-add"
                      disabled={busy || attaching !== null}
                      onClick={() => void addAttachments(qi)}
                    >
                      <Paperclip size={11} aria-hidden />
                      {attachingHere ? t("ask.attachAdding") : t("ask.attachFile")}
                    </button>
                  </div>
                  {attachments.length > 0 ? (
                    <ul className="ask-card__attach-list">
                      {attachments.map((attachment) => (
                        <li key={attachment.id} className="ask-card__attach-item">
                          <span className="ask-card__attach-name" title={attachment.path}>
                            {attachment.name}
                          </span>
                          <span className="ask-card__attach-path" title={attachment.path}>
                            {attachment.path}
                          </span>
                          <button
                            type="button"
                            className="ask-card__attach-x"
                            disabled={busy}
                            aria-label={t("ask.removeAttachment", { name: attachment.name })}
                            onClick={() => removeAttachment(qi, attachment.id)}
                          >
                            <X size={11} aria-hidden />
                          </button>
                        </li>
                      ))}
                    </ul>
                  ) : null}
                  <span className="ask-card__attach-hint">{t("ask.attachmentsHint")}</span>
                </div>
              ) : null}
            </section>
          );
        })}
      </div>

      <div className="ask-card__actions">
        <button
          type="button"
          className="ask-card__btn ask-card__btn--ghost"
          disabled={busy}
          onClick={decline}
        >
          {t("ask.skip")}
        </button>
        <button
          type="button"
          className="ask-card__btn ask-card__btn--primary"
          disabled={busy || !allReady || attaching !== null}
          onClick={submit}
        >
          {t("ask.confirm")}
        </button>
      </div>
    </section>
  );
}
