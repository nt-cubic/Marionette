import { ArrowUpCircle, ChevronRight, PanelRightClose, PanelRightOpen, RefreshCw } from "lucide-react";
import { memo, useState } from "react";
import { createPortal } from "react-dom";
import { t, useT } from "../lib/i18n";
import { ChangesSection } from "./ChangesSection";
import { MemorySection } from "./MemorySection";
import { PluginsSection } from "./PluginsSection";
import { QuotaSection } from "./QuotaSection";
import { TasksSection } from "./TasksSection";
import { UniversalPluginsSection } from "./UniversalPluginsSection";
import { dockFooterHost, useDocked } from "../lib/dockFooter";
import type { PlanEntry } from "../lib/acpPlan";
import type { BackgroundTaskRow } from "../lib/backgroundTasks";
import type { SessionStats } from "../lib/sessionStats";
import type { TodoItem, TodoMergePreview } from "../lib/todos";
import type {
  McpServerSpec,
  ChangedFile,
  HandoffResult,
  ProjectContext,
  UsageSnapshot,
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
  /** Model the current conversation runs on — decides who is billed. */
  activeModelId?: string | null;
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

export const ContextPanel = memo(function ContextPanel({
  collapsed,
  onCollapse,
  onExpand,
  usage,
  onUsageRefresh,
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
  activeModelId = null,
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
  /** Docked: the update control joins the strip's footer bar below the composer. */
  const docked = useDocked();
  const [sharedOpen, setSharedOpen] = useState(false);
  const [memoryCount, setMemoryCount] = useState(0);
  // Same "enabled" logic as the MCP / skills list: mcp defaults off, skills default on.
  const sharedMcpCount = projectContext
    ? projectContext.inventory.mcpServers.filter((server) => projectContext.selection.mcpServers[server.id] ?? false)
        .length
    : 0;
  const sharedSkillCount = projectContext
    ? projectContext.inventory.skills.filter((skill) => projectContext.selection.skills[skill.id] ?? true).length
    : 0;
  const sharedCount = sharedMcpCount + sharedSkillCount + memoryCount;

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
      <TasksSection
        activeAgentId={activeAgentId}
        activeAgentLabel={activeAgentLabel}
        planEntries={planEntries}
        planModeActive={planModeActive}
        todoItems={todoItems}
        onTodosChange={onTodosChange}
        onAbsorbPlan={onAbsorbPlan}
        onSendTodosToAi={onSendTodosToAi}
        onRequestAiTodoUpdate={onRequestAiTodoUpdate}
        onPrepareAiTodoMerge={onPrepareAiTodoMerge}
        backgroundTasks={backgroundTasks}
        sessionLabels={sessionLabels}
        onOpenBackgroundTask={onOpenBackgroundTask}
        onKillBackgroundTerminal={onKillBackgroundTerminal}
      />

      <QuotaSection usage={usage} agentId={activeAgentId} modelId={activeModelId} onRefresh={onUsageRefresh} />

      <ChangesSection
        changedFiles={changedFiles}
        changedFilesNote={changedFilesNote}
        onRefresh={onRefreshChangedFiles}
        gitBranch={gitBranch}
        onCommitAndPush={onCommitAndPush}
        onOpenDiff={onOpenDiff}
        projectRoot={projectRoot}
      />

      <section className="rail-sec rail-shared">
        <button
          type="button"
          className={"rail-shared__toggle" + (sharedOpen ? " is-open" : "")}
          aria-expanded={sharedOpen}
          onClick={() => setSharedOpen(!sharedOpen)}
        >
          <ChevronRight size={12} className={"rail-shared__chevron" + (sharedOpen ? " is-open" : "")} aria-hidden />
          <span>{t("rail.shared")}</span>
          <span className="rail-shared__summary">
            {t("rail.sharedSummary", { count: sharedCount, agent: activeAgentLabel ?? "" })}
          </span>
        </button>
        {sharedOpen && (
          <>
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

      <PluginsSection activeAgentId={activeAgentId} />

      <UniversalPluginsSection />

      <MemorySection activeSessionId={activeSessionId} onCount={setMemoryCount} />

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
          </>
        )}
      </section>

      <div className="context-panel__footer">
        <button className="pill-action pill-action--icon pill-action--sm" type="button" title={collapsed ? "Pin information panel open" : "Collapse information panel"} aria-label={collapsed ? "Pin information panel open" : "Collapse information panel"} onClick={collapsed ? onExpand : onCollapse}>
          {collapsed ? <PanelRightOpen size={13} /> : <PanelRightClose size={13} />}
        </button>
        {docked && updateButton ? createPortal(updateButton, dockFooterHost()) : updateButton}
      </div>
    </aside>
  );
});
