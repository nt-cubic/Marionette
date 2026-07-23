import { CheckCircle2, Circle, PanelRightClose, PanelRightOpen, RefreshCw } from "lucide-react";
import type { UsageSnapshot } from "../lib/types";

type ContextPanelProps = {
  collapsed: boolean;
  onCollapse: () => void;
  onExpand: () => void;
  usage: UsageSnapshot;
  onUsageRefresh: () => void;
};

export function ContextPanel({ collapsed, onCollapse, onExpand, usage, onUsageRefresh }: ContextPanelProps) {
  const todos = [
    { id: "todo-layout", label: "Refine shell layout", done: true },
    { id: "todo-project-tree", label: "Project/session tree", done: true },
    { id: "todo-m2", label: "Wire project storage", done: false },
    { id: "todo-pty", label: "Raw terminal service", done: false }
  ];

  if (collapsed) {
    return (
      <aside className="context-panel is-collapsed" aria-label="Context panel">
        <div className="context-panel__collapsed-rail">
          <button className="icon-button icon-button--small context-panel__button" type="button" title="Show information panel" aria-label="Show information panel" onClick={onExpand}>
            <PanelRightOpen size={14} />
          </button>
        </div>
      </aside>
    );
  }

  return (
    <aside className="context-panel" aria-label="Context panel">
      <div className="context-panel__top">
        <span>Information</span>
      </div>
      <section className="context-card">
        <div className="context-card__heading">
          <span>Usage</span>
          <button className="icon-button icon-button--small context-panel__button" type="button" title="Refresh usage" aria-label="Refresh usage" onClick={onUsageRefresh}>
            <RefreshCw size={13} />
          </button>
        </div>
        <div className="usage-agent">{usage.agentLabel}</div>
        <div className="usage-list">
          {usage.windows.map((window) => (
            <div className="usage-row" key={window.id}>
              <span>{window.label}</span>
              <strong>{window.percentage === null ? "Unavailable" : `${window.percentage}%`}</strong>
              {window.percentage !== null && <div className="usage-meter__track"><span style={{ width: `${window.percentage}%` }} /></div>}
            </div>
          ))}
        </div>
        <small className="usage-updated">Updated {usage.refreshedAt}. Usage is provided by the active Agent adapter.</small>
      </section>
      <section className="context-card">
        <div className="context-card__heading"><span>Todo</span></div>
        <div className="todo-list">
          {todos.map((todo) => (
            <div className={todo.done ? "todo-row is-done" : "todo-row"} key={todo.id}>
              {todo.done ? <CheckCircle2 size={14} /> : <Circle size={14} />}
              <span>{todo.label}</span>
            </div>
          ))}
        </div>
      </section>
      <div className="context-panel__footer">
        <button className="icon-button icon-button--small context-panel__button" type="button" title="Collapse information panel" aria-label="Collapse information panel" onClick={onCollapse}>
          <PanelRightClose size={14} />
        </button>
      </div>
    </aside>
  );
}
