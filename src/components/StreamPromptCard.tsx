import { HelpCircle, ListTodo } from "lucide-react";
import type { PlanApprovalDecision } from "../lib/api";
import { planDecisionLabel } from "../lib/interactionEvents";
import type { SessionEvent } from "../lib/types";
import { AskQuestionCard, type AskQuestionAnswer, type AskQuestionPrompt } from "./AskQuestionCard";
import { ClippedBody } from "./ClippedBody";
import { MarkdownBody } from "./MarkdownBody";
import type { PlanApprovalPrompt } from "./PlanApprovalCard";
import { PlanApprovalCard } from "./PlanApprovalCard";
import { t, useT } from "../lib/i18n";

export type StreamPromptHandlers = {
  liveAsk?: AskQuestionPrompt | null;
  livePlan?: PlanApprovalPrompt | null;
  askBusy?: boolean;
  planBusy?: boolean;
  onAskSubmit?: (answers: AskQuestionAnswer[]) => void;
  onAskDecline?: () => void;
  onPlanAnswer?: (decision: PlanApprovalDecision, feedback?: string) => void | Promise<void>;
  onPlanPermissionChoose?: (optionId: string) => void;
};

function statusChip(status: string): string {
  if (status === "pending") return t("stream.pendingReply");
  if (status === "declined") return t("stream.skipped");
  if (status === "timeout") return t("stream.timedOut");
  return t("stream.answered");
}

export function StreamPromptCard({
  event,
  handlers,
}: {
  event: Extract<SessionEvent, { type: "plan_prompt" | "ask_prompt" }>;
  handlers?: StreamPromptHandlers;
}) {
  if (event.type === "plan_prompt") {
    const live =
      event.status === "pending" &&
      handlers?.livePlan?.requestId === event.requestId
        ? handlers.livePlan
        : null;
    const pending = Boolean(live);

    return (
      <article
        className={`event-card event-card--plan_prompt${pending ? " is-pending" : ""}`}
        id={`plan-${event.requestId}`}
      >
        {pending && live ? (
          <PlanApprovalCard
            prompt={{
              ...live,
              planMarkdown: live.planMarkdown.trim() || event.planMarkdown,
              permissionOptions: live.permissionOptions ?? event.options,
              title: live.title || event.title,
            }}
            variant="stream"
            busy={handlers?.planBusy}
            onAnswer={(decision, feedback) => {
              void handlers?.onPlanAnswer?.(decision, feedback);
            }}
            onPermissionChoose={handlers?.onPlanPermissionChoose}
          />
        ) : (
          <SettledPlan event={event} />
        )}
      </article>
    );
  }

  const live =
    event.status === "pending" &&
    handlers?.liveAsk?.requestId === event.requestId
      ? handlers.liveAsk
      : null;
  const pending = Boolean(live);

  return (
    <article
      className={`event-card event-card--ask_prompt${pending ? " is-pending" : ""}`}
      id={`ask-${event.requestId}`}
    >
      {pending && live ? (
        <AskQuestionCard
          prompt={live}
          variant="stream"
          busy={handlers?.askBusy}
          onSubmit={(answers) => handlers?.onAskSubmit?.(answers)}
          onDecline={() => handlers?.onAskDecline?.()}
        />
      ) : (
        <SettledAsk event={event} />
      )}
    </article>
  );
}

function SettledPlan({
  event,
}: {
  event: Extract<SessionEvent, { type: "plan_prompt" }>;
}) {
  const plan = event.planMarkdown.trim();
  const t = useT();
  const label =
    event.decisionLabel ||
    planDecisionLabel(event.decision, event.options) ||
    statusChip(event.status);
  return (
    <>
      <div className="stream-prompt__head">
        <ListTodo size={15} aria-hidden />
        <div>
          <strong>{event.title?.trim() || t("plan.defaultTitle")}</strong>
          <span className="stream-prompt__chip">{label}</span>
        </div>
      </div>
      {plan ? (
        <ClippedBody className="event-card__clip stream-prompt__body" maxHeight={360}>
          <MarkdownBody text={plan} className="plan-approval-card__md" />
        </ClippedBody>
      ) : (
        <p className="plan-approval-card__empty">{t("plan.bodyMissing")}</p>
      )}
      {event.feedback?.trim() ? (
        <p className="stream-prompt__feedback">{t("plan.feedbackLabel")}{event.feedback.trim()}</p>
      ) : null}
    </>
  );
}

function SettledAsk({
  event,
}: {
  event: Extract<SessionEvent, { type: "ask_prompt" }>;
}) {
  const t = useT();
  const label =
    event.status === "answered"
      ? t("stream.selected")
      : event.status === "declined"
        ? t("stream.skipped")
        : statusChip(event.status);
  return (
    <>
      <div className="stream-prompt__head">
        <HelpCircle size={15} aria-hidden />
        <div>
          <strong>{t("ask.title")}</strong>
          <span className="stream-prompt__chip">{label}</span>
        </div>
      </div>
      <div className="stream-prompt__qa">
        {event.questions.map((q, i) => {
          const picked =
            event.answers?.find((a) => a.question === q.question)?.selected ??
            event.answers?.[i]?.selected ??
            [];
          return (
            <div key={`${i}:${q.question.slice(0, 48)}`} className="stream-prompt__q">
              <p className="stream-prompt__qtext">{q.question}</p>
              {picked.length > 0 ? (
                <p className="stream-prompt__atext">{picked.join(t("turn.nameSeparator"))}</p>
              ) : event.status === "declined" ? (
                <p className="stream-prompt__atext">{t("stream.letAgentDecide")}</p>
              ) : null}
            </div>
          );
        })}
      </div>
    </>
  );
}
