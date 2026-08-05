import {
  ArrowUpCircle,
  FileDiff,
  PanelRightClose,
  PanelRightOpen,
  Plus,
  RefreshCw,
  X,
} from "lucide-react";
import { useState } from "react";
import type { PlanEntry } from "../lib/acpPlan";
import type { TodoItem, TodoMergePreview, TodoStatus } from "../lib/todos";
import type {
  ChangedFile,
  HandoffResult,
  ProjectContext,
  UsageSnapshot,
  UsageWindow,
} from "../lib/types";


type ContextPanelProps = {
  collapsed: boolean;
  onCollapse: () => void;
  onExpand: () => void;
  usage: UsageSnapshot;
  onUsageRefresh: () => void;
  changedFiles?: ChangedFile[];
  changedFilesNote?: string | null;
  onRefreshChangedFiles?: () => void;
  onOpenDiff?: (path: string) => void;
  handoff?: HandoffResult | null;
  /** MCP servers + skills found on this machine / in this project. */
  projectContext?: ProjectContext | null;
  projectContextScanning?: boolean;
  onRescanProjectContext?: () => void;
  /** Re-run `session/new` so MCP servers that came up late can attach. */
  onReconnectAgent?: () => void;
  reconnecting?: boolean;
  onToggleProjectContext?: (kind: "mcp" | "skill", id: string, enabled: boolean) => void;
  /** Agent of the active dialog — decides what is already native. */
  activeAgentId?: string;
  /** Display label for the active agent (Plan card subtitle). */
  activeAgentLabel?: string;
  /** ACP plan for the active session (full-replace, session-scoped). */
  planEntries?: PlanEntry[];
  /** Agent is in plan mode even if no structured plan entries arrived. */
  planModeActive?: boolean;
  /** Project-level todos. */
  todoItems?: TodoItem[];
  onTodosChange?: (items: TodoItem[]) => void;
  onAbsorbPlan?: () => void;
  onSendTodosToAi?: () => void;
  onRequestAiTodoUpdate?: () => void;
  /** Build a merge preview from latest plan / fenced reply; null if nothing to apply. */
  onPrepareAiTodoMerge?: () => TodoMergePreview | { error: string } | null;
  /** Parent-driven panel width drag. */
  resizeDragging?: boolean;
  onResizeStart?: () => void;
  /** Check GitHub Releases for a newer portable build. */
  onCheckAppUpdate?: () => void;
  checkAppUpdateBusy?: boolean;
  /** Dot badge when a downloadable update is already known. */
  appUpdateAvailable?: boolean;
};

function planStatusGlyph(status: PlanEntry["status"]): string {
  switch (status) {
    case "completed":
      return "●";
    case "in_progress":
      return "◐";
    default:
      return "○";
  }
}

function todoStatusMark(status: TodoStatus): string {
  if (status === "done") return "☑";
  if (status === "doing") return "◐";
  return "☐";
}

function cycleTodoStatus(status: TodoStatus): TodoStatus {
  if (status === "todo") return "doing";
  if (status === "doing") return "done";
  return "todo";
}

function meterTone(window: UsageWindow): "ok" | "warn" | "hot" | "none" {
  if (window.percentage == null) return "none";
  if (window.kind === "cost") return "none";
  if (window.percentage >= 90) return "hot";
  if (window.percentage >= 75) return "warn";
  return "ok";
}

function formatPrimary(window: UsageWindow): string {
  if (window.kind === "cost" && window.detail) return window.detail;
  if (window.percentage === null) {
    // Prefer a short honest label; full sentence stays in detail line.
    if (window.detail) {
      // Absolute money (provider balances / spend) — not a utilization %.
      // e.g. "¥71.85 total · topped-up …" → primary "¥71.85"
      const money = window.detail.match(
        /^(?:[¥$€£]\s*[\d,]+(?:\.\d+)?|[\d,]+(?:\.\d+)?\s*(?:CNY|USD|RMB|\$))/i
      );
      if (money) return money[0].replace(/\s+/g, " ").trim();
      const d = window.detail.toLowerCase();
      if (d.includes("ceiling") || d.includes("plan")) return "Plan only";
      if (d.includes("no public") || d.includes("unavailable") || d.includes("not public")) {
        return "N/A";
      }
      if (d.includes("waiting")) return "…";
    }
    return "N/A";
  }
  return `${window.percentage}%`;
}

function changeBadge(changeType: ChangedFile["changeType"]): string {
  switch (changeType) {
    case "added":
      return "A";
    case "deleted":
      return "D";
    case "untracked":
      return "U";
    default:
      return "M";
  }
}

export function ContextPanel({
  collapsed,
  onCollapse,
  onExpand,
  usage,
  onUsageRefresh,
  changedFiles = [],
  changedFilesNote = null,
  onRefreshChangedFiles,
  onOpenDiff,
  handoff = null,
  projectContext = null,
  projectContextScanning = false,
  onRescanProjectContext,
  onReconnectAgent,
  reconnecting = false,
  onToggleProjectContext,
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
  resizeDragging = false,
  onResizeStart,
  onCheckAppUpdate,
  checkAppUpdateBusy = false,
  appUpdateAvailable = false,
}: ContextPanelProps) {
  const planList = planEntries && planEntries.length > 0 ? planEntries : null;
  const [draftTodo, setDraftTodo] = useState("");
  const [mergePreview, setMergePreview] = useState<TodoMergePreview | null>(null);
  const [mergeError, setMergeError] = useState<string | null>(null);

  const toggleTodo = (id: string) => {
    if (!onTodosChange) return;
    const now = new Date().toISOString();
    onTodosChange(
      todoItems.map((it) => {
        if (it.id !== id) return it;
        const status = cycleTodoStatus(it.status);
        return {
          ...it,
          status,
          updatedAt: now,
          doneAt: status === "done" ? now : undefined,
        };
      }),
    );
  };

  const removeTodo = (id: string) => {
    if (!onTodosChange) return;
    onTodosChange(todoItems.filter((it) => it.id !== id));
  };

  const addTodo = () => {
    if (!onTodosChange) return;
    const text = draftTodo.trim();
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
    setDraftTodo("");
  };

  const runPrepareMerge = () => {
    setMergeError(null);
    const result = onPrepareAiTodoMerge?.() ?? null;
    if (!result) {
      setMergeError("没有可用的 AI 清单（等 plan 更新或 ```marionette-todo 代码块）");
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

  return (
    <aside className={`context-panel custom-scrollbar scrollbar-autohide${collapsed ? " is-collapsed" : ""}`} aria-label="Context panel">
      {onResizeStart && (
        <button
          type="button"
          className={resizeDragging ? "panel-resizer panel-resizer--right is-dragging" : "panel-resizer panel-resizer--right"}
          aria-label="Resize information panel"
          title="Drag to resize"
          onMouseDown={(event) => {
            event.preventDefault();
            onResizeStart();
          }}
        />
      )}
      <section className="context-card" aria-label="Tasks">
        <div className="context-card__heading">
          <span>Tasks</span>
        </div>

        {planList && (
          <div className="plan-block">
            <div className="plan-block__meta">
              Plan · {activeAgentLabel || activeAgentId || "agent"} · this session
            </div>
            <ul className="plan-list custom-scrollbar scrollbar-autohide">
              {planList.map((entry, index) => (
                <li
                  key={`${index}:${entry.content.slice(0, 40)}`}
                  className={`plan-item plan-item--${entry.status}`}
                  title={entry.priority ? `priority: ${entry.priority}` : undefined}
                >
                  <span className="plan-item__glyph" aria-hidden>
                    {planStatusGlyph(entry.status)}
                  </span>
                  <span className="plan-item__text">{entry.content}</span>
                </li>
              ))}
            </ul>
            {onAbsorbPlan && (
              <button type="button" className="tasks-action" onClick={onAbsorbPlan}>
                ↓ 吸收进 Todo
              </button>
            )}
          </div>
        )}

        {!planList && planModeActive && (
          <div className="plan-block plan-block--empty">
            <div className="plan-block__meta">
              Plan 模式 · {activeAgentLabel || activeAgentId || "agent"}
            </div>
            <p className="context-card__empty">
              已切换到 Plan。Grok 等 Agent 多数把方案写在对话里，不会推结构化清单到这里。
              对话中的选择题会弹卡片；方案写完后切回 Build 再动手。
            </p>
          </div>
        )}

        <div className="todo-block">
          <div className="plan-block__meta">Todo · project</div>
          {todoItems.length === 0 ? (
            <p className="context-card__empty">No project todos yet.</p>
          ) : (
            <ul className="todo-list custom-scrollbar scrollbar-autohide">
              {todoItems.map((item) => (
                <li key={item.id} className={`todo-item todo-item--${item.status}`}>
                  <button
                    type="button"
                    className="todo-item__toggle"
                    title="Cycle status"
                    onClick={() => toggleTodo(item.id)}
                  >
                    {todoStatusMark(item.status)}
                  </button>
                  <span className="todo-item__text" title={item.source === "plan" ? "from plan" : item.source}>
                    {item.text}
                  </span>
                  <button
                    type="button"
                    className="todo-item__remove"
                    title="Remove"
                    aria-label="Remove todo"
                    onClick={() => removeTodo(item.id)}
                  >
                    <X size={11} />
                  </button>
                </li>
              ))}
            </ul>
          )}
          {onTodosChange && (
            <div className="todo-add">
              <input
                type="text"
                className="todo-add__input"
                placeholder="Add a task…"
                value={draftTodo}
                onChange={(e) => setDraftTodo(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") {
                    e.preventDefault();
                    addTodo();
                  }
                }}
              />
              <button type="button" className="todo-add__btn" title="Add task" onClick={addTodo}>
                <Plus size={12} />
              </button>
            </div>
          )}
          <div className="tasks-actions">
            {onSendTodosToAi && (
              <button type="button" className="tasks-action" onClick={onSendTodosToAi} disabled={todoItems.length === 0}>
                发给 AI
              </button>
            )}
            {onRequestAiTodoUpdate && (
              <button type="button" className="tasks-action" onClick={onRequestAiTodoUpdate}>
                让 AI 更新…
              </button>
            )}
            {onPrepareAiTodoMerge && (
              <button type="button" className="tasks-action" onClick={runPrepareMerge}>
                预览 AI 变更
              </button>
            )}
          </div>
          {mergeError && <p className="context-card__empty">{mergeError}</p>}
          {mergePreview && (
            <div className="todo-merge-preview">
              <div className="plan-block__meta">AI 建议的变更</div>
              <ul className="todo-merge-preview__list">
                {mergePreview.added.length > 0 && (
                  <li>+ 新增 {mergePreview.added.length} 条</li>
                )}
                {mergePreview.completed.length > 0 && (
                  <li>✓ 标记完成 {mergePreview.completed.length} 条</li>
                )}
                {mergePreview.statusChanged.length > 0 && (
                  <li>~ 状态变更 {mergePreview.statusChanged.length} 条</li>
                )}
                {mergePreview.untouched.length > 0 && (
                  <li>? AI 未提及 {mergePreview.untouched.length} 条（保留）</li>
                )}
                {mergePreview.added.length === 0 &&
                  mergePreview.completed.length === 0 &&
                  mergePreview.statusChanged.length === 0 && (
                    <li>无实质变更</li>
                  )}
              </ul>
              <div className="tasks-actions">
                <button type="button" className="tasks-action tasks-action--primary" onClick={applyMerge}>
                  应用
                </button>
                <button
                  type="button"
                  className="tasks-action"
                  onClick={() => {
                    setMergePreview(null);
                    setMergeError(null);
                  }}
                >
                  取消
                </button>
              </div>
            </div>
          )}
        </div>
      </section>
      <section className="context-card">
        <div className="context-card__heading">
          <span>Usage</span>
          <button className="pill-action pill-action--icon pill-action--sm" type="button" title="Refresh usage" aria-label="Refresh usage" onClick={onUsageRefresh}>
            <RefreshCw size={11} />
          </button>
        </div>
        <div className="usage-agent">{usage.agentLabel}</div>
        {usage.windows.length === 0 ? (
          <p className="context-card__empty">
            暂无用量数据。先发一条消息或点刷新；OpenCode 会查余额，Claude/Codex 在已连接时会拉 /usage、/status。
          </p>
        ) : (
          <div className="usage-list">
            {usage.windows.map((window) => {
              const tone = meterTone(window);
              const primary = formatPrimary(window);
              const isEmpty =
                primary === "N/A" &&
                !window.detail &&
                window.percentage == null;
              return (
                <div className={`usage-row usage-row--${tone}`} key={window.id}>
                  <span>{window.label}</span>
                  <strong className={isEmpty ? "usage-row__muted" : undefined}>
                    {primary}
                  </strong>
                  {window.detail && window.kind !== "cost" && (
                    <span className="usage-row__detail">{window.detail}</span>
                  )}
                  {window.percentage !== null && (
                    <div className="usage-meter__track">
                      <span
                        style={{
                          width: `${Math.min(100, Math.max(0, window.percentage))}%`,
                        }}
                      />
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        )}
        <small className="usage-updated" title={usage.note ?? undefined}>
          {usage.note ? `${usage.note} · ` : ""}
          {usage.refreshedAt || "尚未刷新"}
        </small>
      </section>

      <section className="context-card">
        <div className="context-card__heading">
          <span>Changed Files · project</span>
          {onRefreshChangedFiles && (
            <button className="pill-action pill-action--icon pill-action--sm" type="button" title="Refresh git status" aria-label="Refresh git status" onClick={onRefreshChangedFiles}>
              <RefreshCw size={11} />
            </button>
          )}
        </div>
        {changedFiles.length === 0 ? (
          <p className="context-card__empty">
            {changedFilesNote ?? "No local changes (or not a git repo)."}
          </p>
        ) : (
          <ul className="changed-files-list custom-scrollbar scrollbar-autohide">
            {changedFiles.map((file) => (
              <li key={`${file.changeType}:${file.path}`}>
                <button
                  type="button"
                  className="changed-file-row"
                  title={file.path}
                  onClick={() => onOpenDiff?.(file.path)}
                >
                  <span className={`change-badge change-${file.changeType}`}>{changeBadge(file.changeType)}</span>
                  <span className="changed-file-row__path">{file.path}</span>
                  {onOpenDiff && <FileDiff size={12} className="changed-file-row__icon" aria-hidden />}
                </button>
              </li>
            ))}
          </ul>
        )}
      </section>

      <section className="context-card">
        <div className="context-card__heading">
          <span>Project context</span>
          {onRescanProjectContext && (
            <button
              className="pill-action pill-action--icon pill-action--sm"
              type="button"
              title="Rescan agent configs and skill folders"
              aria-label="Rescan project context"
              onClick={onRescanProjectContext}
            >
              <RefreshCw size={11} />
            </button>
          )}
        </div>
        {projectContext == null ? (
          <p className="context-card__empty">
            {projectContextScanning ? "Scanning…" : "Open a project to scan MCP servers and skills."}
          </p>
        ) : (
          <div className="context-lend">
            <p className="context-lend__hint">
              Lend these to agents that don’t have them. Checked items go into the next
              connection — MCP servers via <code>session/new</code>, skills as a pointer list.
            </p>

            <div className="context-lend__labelrow">
              <span className="context-lend__label">
                MCP servers ({projectContext.inventory.mcpServers.length})
              </span>
              {onReconnectAgent && (
                <button
                  type="button"
                  className="pill-action context-lend__reconnect"
                  disabled={reconnecting}
                  onClick={onReconnectAgent}
                  title={
                    "Restart the agent connection so MCP servers that came up late can attach.\n" +
                    "Servers are only offered at session/new, so one that was not listening then " +
                    "stays unavailable until the session is replaced. Your conversation is kept."
                  }
                >
                  <RefreshCw size={11} className={reconnecting ? "is-spinning" : undefined} aria-hidden />
                  {reconnecting ? "Reconnecting…" : "Reconnect"}
                </button>
              )}
            </div>
            {projectContext.inventory.mcpServers.length === 0 ? (
              <p className="context-card__empty">None found in agent configs.</p>
            ) : (
              <ul className="context-lend__list custom-scrollbar scrollbar-autohide">
                {projectContext.inventory.mcpServers.map((server) => {
                  const enabled = projectContext.selection.mcpServers[server.id] ?? false;
                  const native = activeAgentId ? server.agents.includes(activeAgentId) : false;
                  return (
                    <li key={server.id}>
                      <label className="context-lend__row" title={server.sourcePaths.join("\n")}>
                        <input
                          type="checkbox"
                          checked={enabled}
                          onChange={(event) =>
                            onToggleProjectContext?.("mcp", server.id, event.target.checked)
                          }
                        />
                        <span className="context-lend__name">{server.name}</span>
                        <span className="context-lend__meta">
                          {server.transport}
                          {server.agents.length > 0 ? ` · has: ${server.agents.join(", ")}` : ""}
                          {native ? " · native here" : ""}
                        </span>
                      </label>
                    </li>
                  );
                })}
              </ul>
            )}

            <span className="context-lend__label">
              Skills ({projectContext.inventory.skills.length})
            </span>
            {projectContext.inventory.skills.length === 0 ? (
              <p className="context-card__empty">No SKILL.md folders found.</p>
            ) : (
              <ul className="context-lend__list custom-scrollbar scrollbar-autohide">
                {projectContext.inventory.skills.map((skill) => {
                  const enabled = projectContext.selection.skills[skill.id] ?? true;
                  const native = activeAgentId ? skill.agents.includes(activeAgentId) : false;
                  return (
                    <li key={skill.id}>
                      <label
                        className="context-lend__row"
                        title={`${skill.description || skill.name}\n${skill.file}`}
                      >
                        <input
                          type="checkbox"
                          checked={enabled}
                          onChange={(event) =>
                            onToggleProjectContext?.("skill", skill.id, event.target.checked)
                          }
                        />
                        <span className="context-lend__name">{skill.name}</span>
                        <span className="context-lend__meta">
                          {skill.sources.join(", ")}
                          {native ? " · native here" : ""}
                        </span>
                      </label>
                    </li>
                  );
                })}
              </ul>
            )}
            {projectContext.inventory.notes.map((note) => (
              <p className="context-card__empty" key={note}>
                {note}
              </p>
            ))}
          </div>
        )}
      </section>

      <section className="context-card">
        <div className="context-card__heading">
          <span>Handoff</span>
        </div>
        {handoff ? (
          <div className="handoff-block">
            <strong title={handoff.handoffPath}>{handoff.summary || handoff.prompt.slice(0, 80)}</strong>
            <small>Target: {handoff.targetAgentId}</small>
            <small title={handoff.handoffPath}>{handoff.handoffPath}</small>
            <p className="handoff-block__hint">
              Per-dialog file under `.marionette/handoff/` · prefill in Composer (not auto-sent).
            </p>
          </div>
        ) : (
          <p className="context-card__empty">
            Switch agent to write `.marionette/handoff/&lt;session&gt;.md` and prefill Composer.
          </p>
        )}
      </section>

      <div className="context-panel__footer">
        <button className="pill-action pill-action--icon pill-action--sm" type="button" title={collapsed ? "Pin information panel open" : "Collapse information panel"} aria-label={collapsed ? "Pin information panel open" : "Collapse information panel"} onClick={collapsed ? onExpand : onCollapse}>
          {collapsed ? <PanelRightOpen size={13} /> : <PanelRightClose size={13} />}
        </button>
        {onCheckAppUpdate && (
          <button
            className={`pill-action pill-action--icon pill-action--sm context-panel__button--update${appUpdateAvailable ? " is-update-ready" : ""}`}
            type="button"
            title={appUpdateAvailable ? "有新版本可用 — 点击检查" : "检查更新"}
            aria-label={appUpdateAvailable ? "有新版本可用 — 点击检查" : "检查更新"}
            disabled={checkAppUpdateBusy}
            onClick={onCheckAppUpdate}
          >
            <ArrowUpCircle size={13} strokeWidth={2.25} />
          </button>
        )}
      </div>
    </aside>
  );
}
