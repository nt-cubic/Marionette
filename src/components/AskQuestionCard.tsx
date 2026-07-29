import { HelpCircle } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";

export type AskQuestionOption = {
  label: string;
  description?: string;
};

export type AskQuestionItem = {
  question: string;
  header?: string;
  multiSelect?: boolean;
  options: AskQuestionOption[];
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
};

type AskQuestionCardProps = {
  prompt: AskQuestionPrompt;
  busy?: boolean;
  onSubmit: (answers: AskQuestionAnswer[]) => void;
  onDecline: () => void;
};

function splitRecommended(label: string): { text: string; recommended: boolean } {
  const m = label.match(/^(.*?)\s*\(recommended\)\s*$/i);
  const text = m?.[1]?.trim();
  return text ? { text, recommended: true } : { text: label, recommended: false };
}

function questionCanProceed(
  q: AskQuestionItem,
  selected: string[],
  other: string,
): boolean {
  const multi = Boolean(q.multiSelect);
  if (other.trim()) return true;
  if (multi) return selected.length > 0;
  return selected.length === 1;
}

/**
 * Full-width composer replacement: all questions + options visible at once.
 * Compact rows (todo-list density) so many choices fit without a wizard.
 */
export function AskQuestionCard({ prompt, busy = false, onSubmit, onDecline }: AskQuestionCardProps) {
  const questions = prompt.questions;
  const total = questions.length;
  const [picks, setPicks] = useState<string[][]>(() => questions.map(() => []));
  const [otherText, setOtherText] = useState<string[]>(() => questions.map(() => ""));

  useEffect(() => {
    setPicks(prompt.questions.map(() => []));
    setOtherText(prompt.questions.map(() => ""));
  }, [prompt.requestId]); // only reset on new request

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
        next[qi] = [label];
      }
      return next;
    });
  }, []);

  const buildAnswers = useCallback((): AskQuestionAnswer[] => {
    return questions.map((q, i) => {
      const sel = [...(picks[i] ?? [])];
      const o = (otherText[i] ?? "").trim();
      if (o) sel.push(o);
      return { question: q.question, selected: sel };
    });
  }, [questions, picks, otherText]);

  const submit = useCallback(() => {
    if (!allReady || busy) return;
    onSubmit(buildAnswers());
  }, [allReady, busy, onSubmit, buildAnswers]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Enter" || e.shiftKey || e.ctrlKey || e.metaKey || e.altKey) return;
      const tag = (e.target as HTMLElement | null)?.tagName?.toLowerCase();
      if (tag === "textarea") return;
      if (tag === "input") return;
      if (!allReady || busy) return;
      e.preventDefault();
      submit();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [allReady, busy, submit]);

  if (total === 0) return null;

  return (
    <section
      className="ask-card ask-card--composer-slot"
      role="region"
      aria-label="Agent 需要你选择"
      aria-labelledby="ask-card-title"
    >
      <div className="ask-card__header">
        <HelpCircle size={14} aria-hidden />
        <div>
          <strong id="ask-card-title">Agent 需要你选择</strong>
          <span>
            {prompt.mode ? `模式 · ${prompt.mode}` : "选择题"}
            {total > 1 ? ` · ${total} 题` : ""}
            {" · 替代输入框，全部选项同屏"}
          </span>
        </div>
      </div>

      <div className="ask-card__body custom-scrollbar scrollbar-autohide">
        {questions.map((q, qi) => {
          const multi = Boolean(q.multiSelect);
          const selected = picks[qi] ?? [];
          const other = otherText[qi] ?? "";
          const ready = questionCanProceed(q, selected, other);
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
                  {multi ? <span className="ask-card__hint">可多选</span> : null}
                </div>
              </div>

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
                              <em className="ask-card__rec">荐</em>
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

              <label className="ask-card__other">
                <span>其他</span>
                <input
                  type="text"
                  value={other}
                  disabled={busy}
                  placeholder="可选：自己写答案"
                  onChange={(e) => {
                    const value = e.target.value;
                    setOtherText((prev) => {
                      const next = [...prev];
                      next[qi] = value;
                      return next;
                    });
                  }}
                />
              </label>
            </section>
          );
        })}
      </div>

      <div className="ask-card__actions">
        <button
          type="button"
          className="ask-card__btn ask-card__btn--ghost"
          disabled={busy}
          onClick={onDecline}
        >
          跳过，让 Agent 自己定
        </button>
        <button
          type="button"
          className="ask-card__btn ask-card__btn--primary"
          disabled={busy || !allReady}
          onClick={submit}
        >
          确认选择
        </button>
      </div>
    </section>
  );
}
