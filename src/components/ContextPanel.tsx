import {
  ArrowUpCircle,
  ExternalLink,
  FolderOpen,
  PanelRightClose,
  PanelRightOpen,
  Plus,
  RefreshCw,
  Upload,
  X,
} from "lucide-react";
import { memo, useState } from "react";
import { createPortal } from "react-dom";
import { openExternal, revealInFileManager } from "../lib/api";
import { t, useT } from "../lib/i18n";
import { ConfirmDialog } from "./ConfirmDialog";
import { MemorySection } from "./MemorySection";
import { PluginsSection } from "./PluginsSection";
import { SessionStatsSection } from "./SessionStatsSection";
import { UsageLedgerSection } from "./UsageLedgerSection";
import { dockFooterHost, useDocked } from "../lib/dockFooter";
import type { PlanEntry } from "../lib/acpPlan";
import type { BackgroundTaskRow } from "../lib/backgroundTasks";
import type { SessionStats } from "../lib/sessionStats";
import type { TodoItem, TodoMergePreview, TodoStatus } from "../lib/todos";
import type {
  McpServerSpec,
  ChangedFile,
  HandoffResult,
  ProjectContext,
  UsageSnapshot,
  UsageWindow,
} from "../lib/types";

/** memo 默认值要保持同一引用，否则每次渲染都击穿子组件 memo。 */
const EMPTY_BG_TASKS: BackgroundTaskRow[] = [];



type ContextPanelProps = {
  collapsed: boolean;
  onCollapse: () => void;
  onExpand: () => void;
  usage: UsageSnapshot;
  onUsageRefresh: () => void;
  /** Whole-dialog totals folded from the active dialog's transcript. */
  sessionStats?: SessionStats | null;
  changedFiles?: ChangedFile[];
  changedFilesNote?: string | null;
  onRefreshChangedFiles?: () => void;
  /** Current git branch for the active project (null = not a repo / unknown). */
  gitBranch?: string | null;
  /** Ask the active agent to commit local changes and push. */
  onCommitAndPush?: () => void;
  onOpenDiff?: (path: string) => void;
  /** Project root used to open / reveal changed files. */
  projectRoot?: string | null;
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
  /** Active session id — 会话蒸馏（memory_distill）用。 */
  activeSessionId?: string | null;
  /** 后台任务面板：聚合好的行（子代理/后台命令/排队发送/需要你），空数组 = 不渲染。 */
  backgroundTasks?: BackgroundTaskRow[];
  /** sessionId → 会话名（行的归属说明）。 */
  sessionLabels?: Map<string, string>;
  /** 点击行：打开发起会话。 */
  onOpenBackgroundTask?: (row: BackgroundTaskRow) => void;
  /** 终止一个 ACP terminal/* 后台命令。 */
  onKillBackgroundTerminal?: (terminalId: string) => void;
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

/**
 * One readable line per MCP entry: where it runs (本地 · command / 远程 · host)
 * and who already loads it. This is a config scan, not a live connection, so
 * it never claims connected / failed.
 */
function mcpServerLine(server: McpServerSpec, nativeHere: boolean): string {
  let where: string;
  if (server.transport === "stdio") {
    const cmd = (server.command ?? "").split(/[\\/]/).pop() ?? "";
    const pkg = server.args.find((a) => !a.startsWith("-") && a !== "-y") ?? "";
    where = [t("context.local"), [cmd, pkg].filter(Boolean).join(" ")].filter(Boolean).join(" · ");
  } else {
    let host = server.url ?? "";
    try {
      host = server.url ? new URL(server.url).host : "";
    } catch {
      // keep the raw url
    }
    where = [t("context.remote"), host].filter(Boolean).join(" · ");
  }
  const owners = nativeHere
    ? t("context.bundledHere")
    : server.agents.length > 0
      ? t("context.bundledIn", { agents: server.agents.join(t("context.listSep")) })
      : "";
  return [where, owners].filter(Boolean).join(" · ");
}

type UsageGroup = {
  key: string;
  name: string;
  windows: UsageWindow[];
  /** Highest used % across the source's windows. */
  peak: number | null;
  tone: ReturnType<typeof meterTone>;
};

/**
 * Which source a window belongs to. Engine rows carry it in the id
 * (`OpenCode Go · 7 days`); the dialog's own agent reports its limits as bare
 * `5-hour limit` / `codex Weekly limit`, so those group under the agent's name.
 */
function usageSource(window: UsageWindow, agentLabel: string): { key: string; name: string } {
  if (window.id.startsWith("engine:")) {
    const name = window.label.split(" · ")[0] || window.label;
    return { key: window.id.split(":")[1] ?? name, name };
  }
  if (window.kind === "rate_limit") return { key: "agent", name: agentLabel || t("context.currentAgent") };
  const name = window.label.split(" · ")[0] || window.label;
  return { key: `label:${name}`, name };
}

/**
 * The window's own part of the label: `7 days`, `MCP · Month`; an agent's
 * `codex 5-hour limit` reads `5 hours`, `Weekly (Opus)` reads `Weekly · Opus`.
 */
function usagePeriod(window: UsageWindow, source: string): string {
  const label = window.label;
  if (label.startsWith(`${source} · `)) return label.slice(source.length + 3);
  if (window.kind !== "rate_limit" || window.id.startsWith("engine:")) return label;
  return label
    .replace(/^(?:codex|chatgpt|claude|grok)\s+/i, "")
    .replace(/\s*limit$/i, "")
    .replace(/^(\d+)[\s-]*h(?:ours?)?$/i, (_, n: string) => `${n} hours`)
    .replace(/\s*\(([^()]+)\)\s*$/, " · $1")
    .trim() || label;
}

/** Rough window length in hours for ordering; unknown keeps its place. */
function periodHours(period: string): number | null {
  const p = period.toLowerCase();
  const n = Number(/(\d+(?:\.\d+)?)/.exec(p)?.[1] ?? 1);
  if (/hour|小时/.test(p)) return n;
  if (/day|天/.test(p)) return n * 24;
  if (/week|周/.test(p)) return n * 24 * 7;
  if (/month|月/.test(p)) return 24 * 31;
  return null;
}

function groupUsageBySource(windows: UsageWindow[], agentLabel: string): UsageGroup[] {
  const groups: UsageGroup[] = [];
  const byKey = new Map<string, UsageGroup>();
  for (const window of windows) {
    const { key, name } = usageSource(window, agentLabel);
    let group = byKey.get(key);
    if (!group) {
      group = { key, name, windows: [], peak: null, tone: "none" };
      byKey.set(key, group);
      groups.push(group);
    }
    group.windows.push(window);
  }
  for (const group of groups) {
    const order = group.windows.map((w, i) => ({ w, i, h: periodHours(usagePeriod(w, group.name)) }));
    order.sort((x, y) => (x.h != null && y.h != null ? x.h - y.h : x.i - y.i));
    group.windows = order.map((entry) => entry.w);
    const percents = group.windows
      .filter((w) => w.percentage != null && w.kind !== "cost")
      .map((w) => w.percentage as number);
    group.peak = percents.length > 0 ? Math.max(...percents) : null;
    group.tone = group.peak == null ? "none" : group.peak >= 90 ? "hot" : group.peak >= 75 ? "warn" : "ok";
  }
  return groups;
}

function formatPrimary(window: UsageWindow): string {
  // cost and tokens rows carry their whole message in `detail` (no % to show).
  if ((window.kind === "cost" || window.kind === "tokens") && window.detail) {
    return window.detail;
  }
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
  // At most one decimal: providers report 30.3395… and it pushed the column out.
  return `${Math.round(window.percentage * 10) / 10}%`;
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

/** 后台任务行的状态徽标与 aria 文案。 */
function backgroundTaskStateMark(row: BackgroundTaskRow): { glyph: string; label: string } {
  switch (row.state) {
    case "attention":
      return { glyph: "!", label: t("context.needYou") };
    case "running":
      return { glyph: "◐", label: t("context.stateRunning") };
    case "queued":
      return { glyph: "…", label: t("context.stateQueued") };
    case "failed":
      return { glyph: "×", label: t("context.stateFailed") };
    case "stopped":
      return { glyph: "■", label: t("context.stateStopped") };
    default:
      return { glyph: "✓", label: t("context.stateDone") };
  }
}

function backgroundTaskKindLabel(row: BackgroundTaskRow): string {
  switch (row.kind) {
    case "delegate":
      return t("context.subagent");
    case "delegate_queued":
      return t("context.subagentQueued");
    case "terminal":
      return t("context.backgroundCommand");
    case "queued":
      return t("context.queuedSend");
    case "subagent":
      return t("context.subagent");
    case "scheduled":
      return t("context.scheduledSend");
    default:
      return t("context.needYou");
  }
}

const BackgroundTaskRowView = memo(function BackgroundTaskRowView({
  row,
  sessionLabel,
  onOpen,
  onKill,
}: {
  row: BackgroundTaskRow;
  sessionLabel?: string;
  onOpen: (row: BackgroundTaskRow) => void;
  onKill?: (terminalId: string) => void;
}) {
  const t = useT();
  const mark = backgroundTaskStateMark(row);
  const kind = backgroundTaskKindLabel(row);
  const killable = row.kind === "terminal" && row.state === "running";
  // 排队/定时发送在会话回合结束后自动冲走，行不可点；其余行点击切到发起会话。
  const clickable = row.kind !== "queued" && row.kind !== "scheduled";
  const meta = [kind, sessionLabel].filter(Boolean).join(" · ");
  return (
    <li className={`bg-task-row bg-task-row--${row.state}`}>
      <button
        type="button"
        className="bg-task-row__main"
        title={row.detail ? `${row.title}\n${row.detail}` : row.title}
        onClick={clickable ? () => onOpen(row) : undefined}
        disabled={!clickable}
      >
        <span className={`bg-task-row__mark bg-task-row__mark--${row.state}`} aria-hidden>
          {mark.glyph}
        </span>
        <span className="bg-task-row__body">
          <span className="bg-task-row__title">{row.title}</span>
          {(row.detail || meta) && (
            <span className="bg-task-row__meta">
              {meta}
              {meta && row.detail ? " — " : ""}
              {row.detail}
            </span>
          )}
        </span>
        <span className="bg-task-row__state" title={mark.label}>
          {mark.label}
        </span>
      </button>
      {killable && onKill && (
        <button
          type="button"
          className="pill-action pill-action--icon pill-action--sm bg-task-row__kill"
          title={t("context.killTask")}
          aria-label={t("context.killTaskNamed", { title: row.title })}
          onClick={() => onKill(row.key.slice("terminal:".length))}
        >
          <X size={11} />
        </button>
      )}
    </li>
  );
});


function parentRelativePath(path: string): string {
  const normalized = path.replace(/\\/g, "/").replace(/\/+$/, "");
  const slash = normalized.lastIndexOf("/");
  if (slash <= 0) return ".";
  return normalized.slice(0, slash);
}

function ChangedFileRow({
  file,
  cwd,
  onOpenDiff,
  onRisky,
  onNote,
}: {
  file: ChangedFile;
  cwd: string | null;
  onOpenDiff?: (path: string) => void;
  onRisky: (path: string) => void;
  onNote: (note: string | null) => void;
}) {
  const t = useT();
  const deleted = file.changeType === "deleted";
  const canAct = Boolean(cwd);

  const openFile = async () => {
    if (!cwd || deleted) return;
    onNote(null);
    try {
      const result = await openExternal(file.path, cwd);
      if (result.reason === "risky") {
        onRisky(file.path);
        return;
      }
      if (!result.opened) {
        onNote(result.message ?? t("context.cannotOpenFile"));
      }
    } catch (error) {
      onNote(error instanceof Error ? error.message : String(error));
    }
  };

  const reveal = async () => {
    if (!cwd) return;
    onNote(null);
    const target = deleted ? parentRelativePath(file.path) : file.path;
    try {
      await revealInFileManager(target, cwd);
    } catch (error) {
      onNote(error instanceof Error ? error.message : String(error));
    }
  };

  return (
    <li>
      <div className="changed-file-row">
        <button
          type="button"
          className="changed-file-row__main"
          title={t("context.viewDiff", { path: file.path })}
          onClick={() => onOpenDiff?.(file.path)}
        >
          <span className={`change-badge change-${file.changeType}`}>{changeBadge(file.changeType)}</span>
          <span className="changed-file-row__path">{file.path}</span>
        </button>
        <div className="changed-file-row__actions">
          <button
            type="button"
            className="pill-action pill-action--icon pill-action--sm changed-file-row__action"
            title={deleted ? t("context.fileDeleted") : t("context.openDefault")}
            aria-label={deleted ? t("context.fileDeleted") : t("context.openDefault")}
            disabled={!canAct || deleted}
            onClick={() => void openFile()}
          >
            <ExternalLink size={11} />
          </button>
          <button
            type="button"
            className="pill-action pill-action--icon pill-action--sm changed-file-row__action"
            title={deleted ? t("context.openParentFolder") : t("context.revealInExplorer")}
            aria-label={deleted ? t("context.openParentFolder") : t("context.revealInExplorer")}
            disabled={!canAct}
            onClick={() => void reveal()}
          >
            <FolderOpen size={11} />
          </button>
        </div>
      </div>
    </li>
  );
}

export const ContextPanel = memo(function ContextPanel({
  collapsed,
  onCollapse,
  onExpand,
  usage,
  onUsageRefresh,
  sessionStats = null,
  changedFiles = [],
  changedFilesNote = null,
  onRefreshChangedFiles,
  gitBranch = null,
  onCommitAndPush,
  onOpenDiff,
  projectRoot = null,
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
  activeSessionId = null,
  backgroundTasks = EMPTY_BG_TASKS,
  sessionLabels,
  onOpenBackgroundTask,
  onKillBackgroundTerminal,
}: ContextPanelProps) {
  const t = useT();
  const planList = planEntries && planEntries.length > 0 ? planEntries : null;
  const [draftTodo, setDraftTodo] = useState("");
  /** Docked: the update control joins the strip's footer bar below the composer. */
  const docked = useDocked();
  const [mergePreview, setMergePreview] = useState<TodoMergePreview | null>(null);
  const [mergeError, setMergeError] = useState<string | null>(null);
  const [riskyOpenPath, setRiskyOpenPath] = useState<string | null>(null);
  const [fileActionNote, setFileActionNote] = useState<string | null>(null);

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

  // Group windows by their source so one provider's 5 hours / 7 days / month
  // read together, shortest first; a source with one window keeps the plain row.
  const usageGroups = groupUsageBySource(usage.windows, usage.agentLabel);
  const attentionCount = backgroundTasks.filter((row) => row.state === "attention").length;
  const usageFooter = [usage.note, usage.refreshedAt || t("context.notRefreshed")]
    .filter((part, i, all): part is string => Boolean(part) && all.indexOf(part) === i)
    .join(" · ");
  const renderUsageRow = (window: UsageWindow) => {
    const tone = meterTone(window);
    const primary = formatPrimary(window);
    // tokens rows (Last turn / Session total) carry their whole
    // message as the value; inline it overflows the row, so the
    // value drops to a small line under the label instead.
    const stacked = window.kind === "tokens" && window.detail != null;
    const isEmpty =
      primary === "N/A" &&
      !window.detail &&
      window.percentage == null;
    const detailLine = stacked
      ? primary
      : window.detail && window.kind !== "cost" && window.kind !== "tokens"
        ? window.detail
        : null;
    return (
      <div className={`usage-row usage-row--${tone}`} key={window.id}>
        <span>{window.label}</span>
        {!stacked && (
          <strong className={isEmpty ? "usage-row__muted" : undefined}>
            {primary}
          </strong>
        )}
        {detailLine != null && (
          <span className="usage-row__detail">{detailLine}</span>
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
  };

  /** Right rail's only footer control besides collapse; docked it moves below. */
  const updateButton = onCheckAppUpdate ? (
    <button
      className={`pill-action pill-action--icon pill-action--sm context-panel__button--update${appUpdateAvailable ? " is-update-ready" : ""}`}
      type="button"
      title={appUpdateAvailable ? t("context.updateReady") : t("context.checkUpdate")}
      aria-label={appUpdateAvailable ? t("context.updateReady") : t("context.checkUpdate")}
      disabled={checkAppUpdateBusy}
      onClick={onCheckAppUpdate}
    >
      <ArrowUpCircle size={13} strokeWidth={2.25} />
    </button>
  ) : null;

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
      <section className="context-card" id="context-tasks" aria-label="Tasks">
        <div className="context-card__heading">
          <span>{t("context.tasks")}</span>
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
                {t("context.absorbPlan")}
              </button>
            )}
          </div>
        )}

        {!planList && planModeActive && (
          <div className="plan-block plan-block--empty">
            <div className="plan-block__meta">
              {t("context.planModeMeta", { agent: activeAgentLabel || activeAgentId || "agent" })}
            </div>
            <p className="context-card__empty">
              {t("context.planEmptyHint1")}
              {t("context.planEmptyHint2")}
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
            {onSendTodosToAi && todoItems.length > 0 && (
              <button type="button" className="tasks-action" onClick={onSendTodosToAi}>
                {t("context.sendToAi")}
              </button>
            )}
            {onRequestAiTodoUpdate && (
              <button type="button" className="tasks-action" onClick={onRequestAiTodoUpdate}>
                {t("context.askAiUpdate")}
              </button>
            )}
            {onPrepareAiTodoMerge && (
              <button type="button" className="tasks-action" onClick={runPrepareMerge}>
                {t("context.previewAiChanges")}
              </button>
            )}
          </div>
          {mergeError && <p className="context-card__empty">{mergeError}</p>}
          {mergePreview && (
            <div className="todo-merge-preview">
              <div className="plan-block__meta">{t("context.aiSuggestedChanges")}</div>
              <ul className="todo-merge-preview__list">
                {mergePreview.added.length > 0 && (
                  <li>{t("context.mergeAdded", { count: mergePreview.added.length })}</li>
                )}
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
                  mergePreview.statusChanged.length === 0 && (
                    <li>{t("context.mergeNoChanges")}</li>
                  )}
              </ul>
              <div className="tasks-actions">
                <button type="button" className="tasks-action tasks-action--primary" onClick={applyMerge}>
                  {t("context.apply")}
                </button>
                <button
                  type="button"
                  className="tasks-action"
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
        </div>
      </section>
      {backgroundTasks.length > 0 && (
        <section className="context-card" id="context-background" aria-label="Background tasks">
          <div className="context-card__heading">
            <span>{t("context.backgroundTasks")}</span>
            {attentionCount > 0 && (
              <span className="bg-task-attention-badge" title={t("context.attentionCount", { count: attentionCount })}>
                {attentionCount}
              </span>
            )}
          </div>
          <ul className="bg-task-list custom-scrollbar scrollbar-autohide">
            {backgroundTasks.map((row) => (
              <BackgroundTaskRowView
                key={row.key}
                row={row}
                sessionLabel={sessionLabels?.get(row.sessionId)}
                onOpen={(target) => onOpenBackgroundTask?.(target)}
                onKill={onKillBackgroundTerminal}
              />
            ))}
          </ul>
        </section>
      )}
      <section className="context-card" id="context-usage">
        <div className="context-card__heading">
          <span>{t("context.usage")}</span>
          <button className="pill-action pill-action--icon pill-action--sm" type="button" title="Refresh usage" aria-label="Refresh usage" onClick={onUsageRefresh}>
            <RefreshCw size={11} />
          </button>
        </div>
        <div className="usage-agent">{usage.agentLabel}</div>
        {usage.windows.length === 0 ? (
          <p className="context-card__empty">
            {t("context.usageEmpty")}
          </p>
        ) : (
          <div className="usage-list">
            {usageGroups.map((group) =>
              group.windows.length === 1 ? (
                renderUsageRow(group.windows[0]!)
              ) : (
                <div className="usage-group" key={group.key}>
                  <div className={`usage-group__head usage-row--${group.tone}`}>
                    <span>{group.name}</span>
                    {group.peak != null && <strong>{Math.round(group.peak)}%</strong>}
                  </div>
                  {group.windows.map((window) => {
                    const tone = meterTone(window);
                    const period = usagePeriod(window, group.name);
                    if (window.percentage == null) {
                      return (
                        <div className="usage-line usage-line--text" key={window.id} title={window.detail ?? undefined}>
                          <span className="usage-line__period">{period || window.label}</span>
                          <span className="usage-line__value">{formatPrimary(window)}</span>
                        </div>
                      );
                    }
                    return (
                      <div className={`usage-line usage-row--${tone}`} key={window.id} title={window.detail ?? undefined}>
                        <span className="usage-line__period">{period}</span>
                        <span className="usage-meter__track">
                          <span style={{ width: `${Math.min(100, Math.max(0, window.percentage))}%` }} />
                        </span>
                        <strong className="usage-line__value">{formatPrimary(window)}</strong>
                        {(tone === "hot" || tone === "warn") && window.detail && (
                          <span className="usage-row__detail">{window.detail}</span>
                        )}
                      </div>
                    );
                  })}
                </div>
              ),
            )}
          </div>
        )}
        <small className="usage-updated" title={usage.note ?? undefined}>
          {usageFooter}
        </small>
        {sessionStats && <SessionStatsSection stats={sessionStats} />}
      </section>

      <UsageLedgerSection />

      <MemorySection activeSessionId={activeSessionId} />

      <PluginsSection activeAgentId={activeAgentId} />

      <section className="context-card" id="context-changes">
        <div className="context-card__heading">
          <span>{t("context.changedFilesProject")}</span>
          <div className="context-card__heading-actions">
            {onCommitAndPush && changedFiles.length > 0 && (
              <button
                className="pill-action pill-action--icon pill-action--sm"
                type="button"
                title={t("context.aiCommitPush")}
                aria-label="Commit and push via AI"
                onClick={onCommitAndPush}
              >
                <Upload size={11} />
              </button>
            )}
            {onRefreshChangedFiles && (
              <button className="pill-action pill-action--icon pill-action--sm" type="button" title="Refresh git status" aria-label="Refresh git status" onClick={onRefreshChangedFiles}>
                <RefreshCw size={11} />
              </button>
            )}
          </div>
        </div>
        {gitBranch ? (
          <div className="git-branch-row" title={`Current branch: ${gitBranch}`}>
            <span className="git-branch-row__label">branch</span>
            <code className="git-branch-row__name">{gitBranch}</code>
          </div>
        ) : null}
        {changedFiles.length === 0 ? (
          <p className="context-card__empty">
            {changedFilesNote ?? "No local changes (or not a git repo)."}
          </p>
        ) : (
          <ul className="changed-files-list custom-scrollbar scrollbar-autohide">
            {changedFiles.map((file) => (
              <ChangedFileRow
                key={`${file.changeType}:${file.path}`}
                file={file}
                cwd={projectRoot}
                onOpenDiff={onOpenDiff}
                onRisky={setRiskyOpenPath}
                onNote={setFileActionNote}
              />
            ))}
          </ul>
        )}
        {fileActionNote ? <p className="changed-files-note">{fileActionNote}</p> : null}
      </section>

      <section className="context-card" id="context-project">
        <div className="context-card__heading">
          <span>{t("context.projectContext")}</span>
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
              connection — MCP servers via <code>session/new</code>, skills as a pointer
              list. ZCode’s adapter rejects stdio servers over ACP, so checked stdio
              servers are also merged into <code>~/.zcode/cli/config.json</code> for
              ZCode to load natively (machine-wide, all projects’ checked servers).
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
                          {mcpServerLine(server, native)}
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
          <span>{t("context.handoff")}</span>
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

      {riskyOpenPath && (
        <ConfirmDialog
          title={t("context.openExecutableTitle")}
          itemName={riskyOpenPath}
          description={t("context.openExecutableDesc")}
          confirmLabel={t("common.open")}
          cancelLabel={t("common.cancel")}
          onCancel={() => setRiskyOpenPath(null)}
          onConfirm={() => {
            const path = riskyOpenPath;
            setRiskyOpenPath(null);
            if (!path || !projectRoot) return;
            void openExternal(path, projectRoot, true).catch((error) => {
              setFileActionNote(error instanceof Error ? error.message : String(error));
            });
          }}
        />
      )}

      <div className="context-panel__footer">
        <button className="pill-action pill-action--icon pill-action--sm" type="button" title={collapsed ? "Pin information panel open" : "Collapse information panel"} aria-label={collapsed ? "Pin information panel open" : "Collapse information panel"} onClick={collapsed ? onExpand : onCollapse}>
          {collapsed ? <PanelRightOpen size={13} /> : <PanelRightClose size={13} />}
        </button>
        {docked && updateButton ? createPortal(updateButton, dockFooterHost()) : updateButton}
      </div>
    </aside>
  );
});
