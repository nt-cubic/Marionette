import { Bot, X } from "lucide-react";
import { memo } from "react";
import {
  summarizeBackgroundTasks,
  type BackgroundTaskRow,
} from "../lib/backgroundTasks";
import { useT } from "../lib/i18n";

type AgentsSectionProps = {
  tasks?: BackgroundTaskRow[];
  onOpenTask?: (row: BackgroundTaskRow) => void;
  onStopTask?: (taskId: string) => void;
};

const EMPTY_AGENT_TASKS: BackgroundTaskRow[] = [];

function stateLabel(row: BackgroundTaskRow, t: ReturnType<typeof useT>): string {
  switch (row.state) {
    case "attention":
      return t("context.needYou");
    case "queued":
      return t("context.stateQueued");
    case "failed":
      return t("context.stateFailed");
    case "stopped":
      return t("context.stateStopped");
    case "done":
      return t("context.stateDone");
    default:
      return t("attention.running");
  }
}

/** Parent-thread subagents, kept separate from project tasks and commands. */
export const AgentsSection = memo(function AgentsSection({
  tasks = EMPTY_AGENT_TASKS,
  onOpenTask,
  onStopTask,
}: AgentsSectionProps) {
  const t = useT();
  if (tasks.length === 0) return null;
  const summary = summarizeBackgroundTasks(tasks);

  return (
    <section className="rail-sec" id="context-agents" aria-label={t("agents.title")}>
      <div className="rail-sh">
        <span>{t("agents.title")}</span>
        {summary && <span className="rail-sh__meta">{summary}</span>}
      </div>
      <ul className="rail-list rail-agents">
        {tasks.map((row) => {
          const label = stateLabel(row, t);
          const canStop =
            row.state === "running" ||
            row.state === "queued" ||
            row.state === "attention";
          const tip = [row.title, row.detail].filter(Boolean).join("\n");
          return (
            <li key={row.key} className={`rail-ln rail-agent-row is-${row.state}`}>
              <button
                type="button"
                className="rail-agent__main"
                title={tip}
                onClick={() => onOpenTask?.(row)}
              >
                <Bot className="rail-agent__icon" size={13} strokeWidth={1.5} aria-hidden />
                <span className="rail-agent__copy">
                  <span className="rail-agent__title">{row.title}</span>
                  {row.detail && <span className="rail-agent__detail">{row.detail}</span>}
                </span>
                <span className="rail-agent__state">{label}</span>
              </button>
              {canStop && onStopTask && (
                <button
                  type="button"
                  className="rail-ib rail-ib--sm rail-agent__stop"
                  title={t("context.killTask")}
                  aria-label={t("rail.backgroundKill", { title: row.title })}
                  onClick={() => onStopTask(row.key.slice("agent-delegate:".length))}
                >
                  <X size={11} strokeWidth={1.5} />
                </button>
              )}
            </li>
          );
        })}
      </ul>
    </section>
  );
});
