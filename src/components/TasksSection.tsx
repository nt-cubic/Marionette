import { MoreHorizontal, Plus, X } from "lucide-react";
import { memo, useEffect, useState } from "react";
import { useT } from "../lib/i18n";
import type { PlanEntry } from "../lib/acpPlan";
import type { BackgroundTaskRow } from "../lib/backgroundTasks";
import type { TodoItem, TodoMergePreview, TodoStatus } from "../lib/todos";

/** memo 默认值要保持同一引用，否则每次渲染都击穿子组件 memo。 */
const EMPTY_BG_TASKS: BackgroundTaskRow[] = [];

type TasksSectionProps = {
  activeAgentId?: string;
  activeAgentLabel?: string;
  planEntries?: PlanEntry[];
  planModeActive?: boolean;
  todoItems?: TodoItem[];
  onTodosChange?: (items: TodoItem[]) => void;
  onAbsorbPlan?: () => void;
  onSendTodosToAi?: () => void;
  onRequestAiTodoUpdate?: () => void;
  onPrepareAiTodoMerge?: () => TodoMergePreview | { error: string } | null;
  backgroundTasks?: BackgroundTaskRow[];
  sessionLabels?: Map<string, string>;
  onOpenBackgroundTask?: (row: BackgroundTaskRow) => void;
  onKillBackgroundTerminal?: (terminalId: string) => void;
};

type Mark = "done" | "doing" | "todo";

function planMark(status: PlanEntry["status"]): Mark {
  if (status === "completed") return "done";
  if (status === "in_progress") return "doing";
  return "todo";
}

function todoMark(status: TodoStatus): Mark {
  return status === "done" ? "done" : status === "doing" ? "doing" : "todo";
}

const MARK_GLYPH: Record<Mark, string> = { done: "✓", doing: "◐", todo: "○" };

function cycleTodoStatus(status: TodoStatus): TodoStatus {
  if (status === "todo") return "doing";
  if (status === "doing") return "done";
  return "todo";
}

/** `42s` / `3m` / `1h` — a running background task's age, ticking once a second. */
function Elapsed({ since }: { since: number }) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, []);
  const s = Math.max(0, Math.floor((now - since) / 1000));
  if (s < 60) return <>{s}s</>;
  if (s < 3600) return <>{Math.floor(s / 60)}m</>;
  return <>{Math.floor(s / 3600)}h</>;
}

function bgStateLabel(row: BackgroundTaskRow, t: ReturnType<typeof useT>): string {
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
      return "";
  }
}

export const TasksSection = memo(function TasksSection({
  activeAgentId,
  activeAgentLabel,
  planEntries,
  planModeActive = false,
  todoItems = [],
  onTodosChange,
  onAbsorbPlan,
  onSendTodosToAi,
  onRequestAiTodoUpdate,
  onPrepareAiTodoMerge,
  backgroundTasks = EMPTY_BG_TASKS,
  sessionLabels,
  onOpenBackgroundTask,
  onKillBackgroundTerminal,
}: TasksSectionProps) {
  const t = useT();
  const planList = planEntries && planEntries.length > 0 ? planEntries : null;
  const agentName = activeAgentLabel || activeAgentId || "agent";
  const [adding, setAdding] = useState(false);
  const [draft, setDraft] = useState("");
  const [moreOpen, setMoreOpen] = useState(false);
  const [mergePreview, setMergePreview] = useState<TodoMergePreview | null>(null);
  const [mergeError, setMergeError] = useState<string | null>(null);

  const total = (planList?.length ?? 0) + todoItems.length;
  const done =
    (planList?.filter((entry) => entry.status === "completed").length ?? 0) +
    todoItems.filter((item) => item.status === "done").length;

  const toggleTodo = (id: string) => {
    if (!onTodosChange) return;
    const now = new Date().toISOString();
    onTodosChange(
      todoItems.map((it) => {
        if (it.id !== id) return it;
        const status = cycleTodoStatus(it.status);
        return { ...it, status, updatedAt: now, doneAt: status === "done" ? now : undefined };
      }),
    );
  };

  const removeTodo = (id: string) => onTodosChange?.(todoItems.filter((it) => it.id !== id));

  const addTodo = () => {
    if (!onTodosChange) return;
    const text = draft.trim();
    if (!text) return;
    const now = new Date().toISOString();
    onTodosChange([
      ...todoItems,
      {
        id: `todo-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`,
        text,
        status: "todo",
        source: "user",
        createdAt: now,
        updatedAt: now,
      },
    ]);
    setDraft("");
  };

  const runPrepareMerge = () => {
    setMergeError(null);
    const result = onPrepareAiTodoMerge?.() ?? null;
    if (!result) {
      setMergeError(t("context.noAiTodoList"));
      setMergePreview(null);
      return;
    }
    if ("error" in result) {
      setMergeError(result.error);
      setMergePreview(null);
      return;
    }
    setMergePreview(result);
  };

  const applyMerge = () => {
    if (!mergePreview || !onTodosChange) return;
    onTodosChange(mergePreview.nextItems);
    setMergePreview(null);
    setMergeError(null);
  };

  const hasActions = Boolean(
    (planList && onAbsorbPlan) || (onSendTodosToAi && todoItems.length > 0) || onRequestAiTodoUpdate || onPrepareAiTodoMerge,
  );
  const empty = total === 0 && backgroundTasks.length === 0 && !planModeActive;

  return (
    <section className="rail-sec" id="context-tasks" aria-label={t("context.tasks")}>
      <div className="rail-sh">
        <span>{t("context.tasks")}</span>
        {total > 0 && (
          <span className="rail-sh__meta">
            {done} / {total}
          </span>
        )}
        <span className="rail-act">
          {hasActions && (
            <button
              type="button"
              className={`rail-ib${moreOpen ? " is-on" : ""}`}
              title={t("rail.taskMore")}
              aria-label={t("rail.taskMore")}
              aria-expanded={moreOpen}
              onClick={() => setMoreOpen((open) => !open)}
            >
              <MoreHorizontal size={13} strokeWidth={1.5} />
            </button>
          )}
          {onTodosChange && (
            <button
              type="button"
              className={`rail-ib${adding ? " is-on" : ""}`}
              title={t("rail.addTask")}
              aria-label={t("rail.addTask")}
              onClick={() => setAdding((open) => !open)}
            >
              <Plus size={13} strokeWidth={1.5} />
            </button>
          )}
        </span>
      </div>

      {moreOpen && hasActions && (
        <div className="rail-textbtns">
          {planList && onAbsorbPlan && (
            <button type="button" className="rail-tb" onClick={onAbsorbPlan}>
              {t("context.absorbPlan")}
            </button>
          )}
          {onSendTodosToAi && todoItems.length > 0 && (
            <button type="button" className="rail-tb" onClick={onSendTodosToAi}>
              {t("context.sendToAi")}
            </button>
          )}
          {onRequestAiTodoUpdate && (
            <button type="button" className="rail-tb" onClick={onRequestAiTodoUpdate}>
              {t("context.askAiUpdate")}
            </button>
          )}
          {onPrepareAiTodoMerge && (
            <button type="button" className="rail-tb" onClick={runPrepareMerge}>
              {t("context.previewAiChanges")}
            </button>
          )}
        </div>
      )}

      {mergeError && <p className="rail-empty">{mergeError}</p>}
      {mergePreview && (
        <div className="rail-merge">
          <div className="rail-ph">{t("context.aiSuggestedChanges")}</div>
          <ul className="rail-merge__list">
            {mergePreview.added.length > 0 && <li>{t("context.mergeAdded", { count: mergePreview.added.length })}</li>}
            {mergePreview.completed.length > 0 && (
              <li>{t("context.mergeCompleted", { count: mergePreview.completed.length })}</li>
            )}
            {mergePreview.statusChanged.length > 0 && (
              <li>{t("context.mergeStatusChanged", { count: mergePreview.statusChanged.length })}</li>
            )}
            {mergePreview.untouched.length > 0 && (
              <li>{t("context.mergeUntouched", { count: mergePreview.untouched.length })}</li>
            )}
            {mergePreview.added.length === 0 &&
              mergePreview.completed.length === 0 &&
              mergePreview.statusChanged.length === 0 && <li>{t("context.mergeNoChanges")}</li>}
          </ul>
          <div className="rail-textbtns">
            <button type="button" className="rail-tb is-primary" onClick={applyMerge}>
              {t("context.apply")}
            </button>
            <button
              type="button"
              className="rail-tb"
              onClick={() => {
                setMergePreview(null);
                setMergeError(null);
              }}
            >
              {t("common.cancel")}
            </button>
          </div>
        </div>
      )}

      {adding && onTodosChange && (
        <div className="rail-add">
          <input
            type="text"
            autoFocus
            className="rail-add__input"
            placeholder={t("rail.addTaskPlaceholder")}
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                addTodo();
              } else if (e.key === "Escape") {
                e.preventDefault();
                setAdding(false);
                setDraft("");
              }
            }}
          />
        </div>
      )}

      {planList && todoItems.length > 0 && <div className="rail-ph">{t("rail.planFrom", { agent: agentName })}</div>}
      {planList && (
        <ul className="rail-list">
          {planList.map((entry, index) => {
            const mark = planMark(entry.status);
            return (
              <li
                key={`${index}:${entry.content.slice(0, 40)}`}
                className={`rail-ln rail-task is-${mark}`}
                title={entry.priority ? `priority: ${entry.priority}` : entry.content}
              >
                <span className="rail-task__mark" aria-hidden>
                  {MARK_GLYPH[mark]}
                </span>
                <span className="rail-task__text">{entry.content}</span>
              </li>
            );
          })}
        </ul>
      )}
      {planList && todoItems.length > 0 && <div className="rail-ph">{t("rail.projectTodos")}</div>}

      {!planList && planModeActive && (
        <p className="rail-empty" title={t("context.planEmptyHint1")}>
          {t("context.planModeMeta", { agent: agentName })}
        </p>
      )}

      {todoItems.length > 0 && (
        <ul className="rail-list">
          {todoItems.map((item) => {
            const mark = todoMark(item.status);
            return (
              <li key={item.id} className={`rail-ln rail-task is-${mark} is-editable`}>
                <button
                  type="button"
                  className="rail-task__mark rail-task__toggle"
                  title="Cycle status"
                  aria-label="Cycle status"
                  onClick={() => toggleTodo(item.id)}
                >
                  {MARK_GLYPH[mark]}
                </button>
                <span className="rail-task__text" title={item.text}>
                  {item.text}
                </span>
                {onTodosChange && (
                  <button
                    type="button"
                    className="rail-ib rail-ib--sm rail-task__remove"
                    title="Remove"
                    aria-label="Remove task"
                    onClick={() => removeTodo(item.id)}
                  >
                    <X size={11} strokeWidth={1.5} />
                  </button>
                )}
              </li>
            );
          })}
        </ul>
      )}

      {backgroundTasks.length > 0 && (
        <ul className="rail-list">
          {backgroundTasks.map((row) => {
            const label = bgStateLabel(row, t);
            const killable = row.kind === "terminal" && row.state === "running";
            // 排队/定时发送在会话回合结束后自动冲走，行不可点；其余行点击切到发起会话。
            const clickable = row.kind !== "queued" && row.kind !== "scheduled";
            const sessionLabel = sessionLabels?.get(row.sessionId);
            const tip = [row.title, row.detail, sessionLabel].filter(Boolean).join("\n");
            return (
              <li key={row.key} className={`rail-ln rail-task rail-bg is-${row.state}`}>
                <button
                  type="button"
                  className="rail-bg__main"
                  title={tip}
                  disabled={!clickable}
                  onClick={clickable ? () => onOpenBackgroundTask?.(row) : undefined}
                >
                  <span className="rail-task__mark" aria-hidden>
                    ⋯
                  </span>
                  <span className="rail-task__text">
                    {t("rail.background")} · {row.title}
                  </span>
                  <span className="rail-bg__state">
                    {row.state === "running" ? <Elapsed since={row.startedAtMs} /> : label}
                  </span>
                </button>
                {killable && onKillBackgroundTerminal && (
                  <button
                    type="button"
                    className="rail-ib rail-ib--sm"
                    title={t("context.killTask")}
                    aria-label={t("rail.backgroundKill", { title: row.title })}
                    onClick={() => onKillBackgroundTerminal(row.key.slice("terminal:".length))}
                  >
                    <X size={11} strokeWidth={1.5} />
                  </button>
                )}
              </li>
            );
          })}
        </ul>
      )}

      {empty && <p className="rail-empty">{t("rail.tasksEmpty")}</p>}
    </section>
  );
});
