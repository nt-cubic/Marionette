import { ListTodo, Loader2 } from "lucide-react";
import { useCallback, useRef, useState } from "react";
import type { PlanApprovalDecision } from "../lib/api";
import { MarkdownBody } from "./MarkdownBody";

export type PlanApprovalPrompt = {
  requestId: string;
  sessionId: string;
  toolCallId?: string | null;
  /** Plan markdown from Grok (`planContent`). Often empty — Grok reads plan.md itself. */
  planMarkdown: string;
};

type PlanApprovalCardProps = {
  prompt: PlanApprovalPrompt;
  busy?: boolean;
  onAnswer: (decision: PlanApprovalDecision, feedback?: string) => void | Promise<void>;
};

/**
 * Grok plan-mode approval (mirrors Codeg `plan-approval-card`).
 * Agent calls `exit_plan_mode` and blocks until Approve / Request changes / Abandon.
 */
export function PlanApprovalCard({ prompt, busy = false, onAnswer }: PlanApprovalCardProps) {
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState(false);
  const [changesOpen, setChangesOpen] = useState(false);
  const [feedback, setFeedback] = useState("");
  const inFlight = useRef(false);

  const plan = prompt.planMarkdown.trim();
  const locked = busy || submitting;

  const run = useCallback(
    async (decision: PlanApprovalDecision, fb?: string) => {
      if (inFlight.current) return;
      inFlight.current = true;
      setSubmitting(true);
      setError(false);
      try {
        await onAnswer(decision, fb);
        // Parent clears the prompt on success; keep disabled if unmount lags.
      } catch {
        setError(true);
        setSubmitting(false);
        inFlight.current = false;
      }
    },
    [onAnswer],
  );

  return (
    <div className="plan-approval-card" role="region" aria-label="Plan approval">
      <div className="plan-approval-card__header">
        <ListTodo size={16} aria-hidden />
        <div>
          <strong>计划待批准</strong>
          <span>批准后离开 Plan 模式并开始执行；要求修改则继续规划</span>
        </div>
      </div>

      {plan ? (
        <div className="plan-approval-card__plan">
          <MarkdownBody text={plan} className="plan-approval-card__md" />
        </div>
      ) : (
        <p className="plan-approval-card__empty">
          计划正文未随请求附带（Grok 常在批准后再读 plan.md）。可在右侧任务栏查看步骤，或直接批准。
        </p>
      )}

      {changesOpen ? (
        <div className="plan-approval-card__changes">
          <textarea
            className="plan-approval-card__textarea"
            value={feedback}
            onChange={(e) => setFeedback(e.target.value)}
            placeholder="说明要改什么…"
            disabled={locked}
            rows={3}
            autoFocus
          />
          <div className="plan-approval-card__actions">
            <button
              type="button"
              className="plan-approval-card__btn plan-approval-card__btn--ghost"
              disabled={locked}
              onClick={() => setChangesOpen(false)}
            >
              取消
            </button>
            <button
              type="button"
              className="plan-approval-card__btn plan-approval-card__btn--primary"
              disabled={locked || !feedback.trim()}
              onClick={() => void run("request_changes", feedback.trim())}
            >
              {submitting && <Loader2 size={14} className="spin" aria-hidden />}
              发送修改意见
            </button>
          </div>
        </div>
      ) : (
        <div className="plan-approval-card__actions">
          <button
            type="button"
            className="plan-approval-card__btn plan-approval-card__btn--primary"
            disabled={locked}
            onClick={() => void run("approve")}
          >
            {submitting && <Loader2 size={14} className="spin" aria-hidden />}
            批准并执行
          </button>
          <button
            type="button"
            className="plan-approval-card__btn plan-approval-card__btn--ghost"
            disabled={locked}
            onClick={() => setChangesOpen(true)}
          >
            要求修改
          </button>
          <button
            type="button"
            className="plan-approval-card__btn plan-approval-card__btn--danger"
            disabled={locked}
            onClick={() => void run("abandon")}
          >
            放弃计划
          </button>
        </div>
      )}

      {error && (
        <p className="plan-approval-card__error">提交失败，请重试</p>
      )}
    </div>
  );
}
