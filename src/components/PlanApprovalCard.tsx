import { ListTodo, Loader2 } from "lucide-react";
import { useCallback, useRef, useState } from "react";
import type { PlanApprovalDecision } from "../lib/api";
import { permissionOptionTone } from "../lib/permissionPlan";
import { MarkdownBody } from "./MarkdownBody";
import { useT } from "../lib/i18n";
import type { PermissionOption } from "./PermissionDialog";

export type PlanApprovalPrompt = {
  requestId: string;
  sessionId: string;
  toolCallId?: string | null;
  /** Plan markdown from Grok (`planContent`). Often empty — Grok reads plan.md itself. */
  planMarkdown: string;
  title?: string;
  /** Claude Code / Codex: permission-shaped buttons instead of Grok's trio. */
  permissionOptions?: PermissionOption[] | null;
};

type PlanApprovalCardProps = {
  prompt: PlanApprovalPrompt;
  busy?: boolean;
  variant?: "dock" | "stream";
  onAnswer: (decision: PlanApprovalDecision, feedback?: string) => void | Promise<void>;
  onPermissionChoose?: (optionId: string) => void;
};

/**
 * Grok plan-mode approval (mirrors Codeg `plan-approval-card`).
 * Agent calls `exit_plan_mode` and blocks until Approve / Request changes / Abandon.
 */
export function PlanApprovalCard({
  prompt,
  busy = false,
  variant = "stream",
  onAnswer,
  onPermissionChoose,
}: PlanApprovalCardProps) {
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [changesOpen, setChangesOpen] = useState(false);
  const [feedback, setFeedback] = useState("");
  const inFlight = useRef(false);
  const t = useT();

  const plan = prompt.planMarkdown.trim();
  const locked = busy || submitting;
  const permissionOptions = prompt.permissionOptions ?? [];
  const isPermission = permissionOptions.length > 0;

  const run = useCallback(
    async (decision: PlanApprovalDecision, fb?: string) => {
      if (inFlight.current) return;
      inFlight.current = true;
      setSubmitting(true);
      setError(null);
      try {
        await onAnswer(decision, fb);
        // Parent clears the prompt on success; keep disabled if unmount lags.
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err));
        setSubmitting(false);
        inFlight.current = false;
      }
    },
    [onAnswer],
  );

  return (
    <div
      className={
        variant === "stream"
          ? "plan-approval-card plan-approval-card--in-stream"
          : "plan-approval-card"
      }
      role="region"
      aria-label="Plan approval"
    >
      <div className="plan-approval-card__header">
        <ListTodo size={16} aria-hidden />
        <div>
          <strong>{prompt.title?.trim() || t("plan.pendingTitle")}</strong>
          <span>
            {isPermission
              ? t("plan.permissionHint")
              : t("plan.hint")}
          </span>
        </div>
      </div>

      {plan ? (
        <div className="plan-approval-card__plan">
          <MarkdownBody text={plan} className="plan-approval-card__md" />
        </div>
      ) : (
        <p className="plan-approval-card__empty">
          {t("plan.emptyWithTip")}
        </p>
      )}

      {isPermission ? (
        <div className="plan-approval-card__actions">
          {permissionOptions.map((opt) => {
            const tone = permissionOptionTone(opt.kind, opt.name);
            return (
              <button
                key={opt.optionId}
                type="button"
                className={`plan-approval-card__btn plan-approval-card__btn--${tone === "allow" ? "primary" : tone === "deny" ? "danger" : "ghost"}`}
                disabled={locked}
                onClick={() => onPermissionChoose?.(opt.optionId)}
              >
                {opt.name || opt.optionId}
              </button>
            );
          })}
        </div>
      ) : changesOpen ? (
        <div className="plan-approval-card__changes">
          <textarea
            className="plan-approval-card__textarea"
            value={feedback}
            onChange={(e) => setFeedback(e.target.value)}
            placeholder={t("plan.changesPlaceholder")}
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
              {t("common.cancel")}
            </button>
            <button
              type="button"
              className="plan-approval-card__btn plan-approval-card__btn--primary"
              disabled={locked || !feedback.trim()}
              onClick={() => void run("request_changes", feedback.trim())}
            >
              {submitting && <Loader2 size={14} className="spin" aria-hidden />}
              {t("plan.sendFeedback")}
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
            {t("plan.approveAndRun")}
          </button>
          <button
            type="button"
            className="plan-approval-card__btn plan-approval-card__btn--ghost"
            disabled={locked}
            onClick={() => setChangesOpen(true)}
          >
            {t("plan.requestChanges")}
          </button>
          <button
            type="button"
            className="plan-approval-card__btn plan-approval-card__btn--danger"
            disabled={locked}
            onClick={() => void run("abandon")}
          >
            {t("plan.abandon")}
          </button>
        </div>
      )}

      {error && (
        <p className="plan-approval-card__error">{t("plan.submitFailed")}{error}</p>
      )}
    </div>
  );
}
