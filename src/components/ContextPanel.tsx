import { ArrowUpCircle, ChevronRight, RefreshCw } from "lucide-react";
import { memo, useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { t, useT } from "../lib/i18n";
import { ChangesSection } from "./ChangesSection";
import { MemorySection } from "./MemorySection";
import { QuotaSection } from "./QuotaSection";
import { TasksSection } from "./TasksSection";
import { UniversalPluginsSection } from "./UniversalPluginsSection";
import { UniversalSkillsSection } from "./UniversalSkillsSection";
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

/** Visible MCP / skill rows before the fade + "Show more", matching the left shelf clip. */
const LEND_LIST_PREVIEW = 8;

function previewSlice<T>(items: T[], expanded: boolean, limit = LEND_LIST_PREVIEW): { visible: T[]; hidden: number } {
  if (expanded || items.length <= limit) return { visible: items, hidden: 0 };
  return { visible: items.slice(0, limit), hidden: items.length - limit };
}

function LendClip({
  total,
  hidden,
  expanded,
  onExpand,
  onCollapse,
}: {
  total: number;
  hidden: number;
  expanded: boolean;
  onExpand: () => void;
  onCollapse: () => void;
}) {
  const t = useT();
  if (!expanded && hidden > 0) {
    return (
      <div className="shelf-clip shelf-clip--lend">
        <div className="shelf-clip__fade" aria-hidden />
        <button
          className="shelf-clip__more"
          type="button"
          title={t("rail.showMoreItems", { n: hidden })}
          aria-label={t("rail.showMoreItems", { n: hidden })}
          onClick={onExpand}
        >
          {t("rail.showMore")}
          <span className="shelf-clip__count">{hidden}</span>
        </button>
      </div>
    );
  }
  if (expanded && total > LEND_LIST_PREVIEW) {
    return (
      <div className="shelf-clip shelf-clip--lend shelf-clip--expanded">
        <button
          className="shelf-clip__more"
          type="button"
          title={t("rail.showLess")}
          aria-label={t("rail.showLess")}
          onClick={onCollapse}
        >
          {t("rail.showLess")}
        </button>
      </div>
    );
  }
  return null;
}

export const ContextPanel = memo(function ContextPanel({
  collapsed,
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
  const [mcpExpanded, setMcpExpanded] = useState(false);
  const [skillsExpanded, setSkillsExpanded] = useState(false);
  // Plugin / memory counts are only known once their sections mounted (memory syncs on
  // mount); null = not loaded yet, so the summary leaves them out instead of showing 0.
  const [memoryCount, setMemoryCount] = useState<number | null>(null);
  const [universalPluginCount, setUniversalPluginCount] = useState<number | null>(null);
  const [skillCount, setSkillCount] = useState<number | null>(null);
  const pluginCount =
    universalPluginCount == null && skillCount == null
      ? null
      : (universalPluginCount ?? 0) + (skillCount ?? 0);
  const pluginsLoaded = universalPluginCount != null && skillCount != null;
  const pluginsEmpty = pluginCount === 0;

  /**
   * Ctrl K「插件」的落点（P3，2026-11-06）：插件卡在「共享」折叠区内，
   * 未展开时 #context-plugins 根本不在 DOM——App 侧只 scrollTo 会落空。
   * App 派发 marionette:reveal-plugins，这里展开共享卡并滚到位（贴边
   * context 整页是同一组件实例，一并修好）。
   */
  useEffect(() => {
    const reveal = () => {
      setSharedOpen(true);
      requestAnimationFrame(() => {
        requestAnimationFrame(() => {
          document
            .getElementById("context-plugins")
            ?.scrollIntoView({ block: "start", behavior: "smooth" });
        });
      });
    };
    window.addEventListener("marionette:reveal-plugins", reveal);
    return () => window.removeEventListener("marionette:reveal-plugins", reveal);
  }, []);
  const mcpServers = projectContext?.inventory.mcpServers ?? [];
  const mcpSlice = previewSlice(mcpServers, mcpExpanded);
  const skillItems = projectContext?.inventory.skills ?? [];
  const skillsSlice = previewSlice(skillItems, skillsExpanded);
  // Same "enabled" logic as the MCP / skills list: mcp defaults off, skills default on.
  const sharedMcpCount = projectContext
    ? projectContext.inventory.mcpServers.filter((server) => projectContext.selection.mcpServers[server.id] ?? false)
        .length
    : 0;
  const sharedSkillCount = projectContext
    ? projectContext.inventory.skills.filter((skill) => projectContext.selection.skills[skill.id] ?? true).length
    : 0;
  const sharedSummary = [
    projectContext ? t("rail.sharedMcp", { count: sharedMcpCount }) : null,
    projectContext ? t("rail.sharedSkills", { count: sharedSkillCount }) : null,
    pluginCount != null ? t("rail.sharedPlugins", { count: pluginCount }) : null,
    memoryCount != null ? t("rail.sharedMemory", { count: memoryCount }) : null,
  ]
    .filter(Boolean)
    .join(" · ");

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
          <span className="rail-shared__title">{t("rail.shared")}</span>
          <span
            className="rail-shared__summary"
            title={activeAgentLabel ? t("rail.sharedTo", { agent: activeAgentLabel }) : undefined}
          >
            {sharedSummary}
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
              title={t("rail.shareRescanTitle")}
              aria-label={t("rail.shareRescanAria")}
              onClick={onRescanProjectContext}
            >
              <RefreshCw size={11} />
            </button>
          )}
        </div>
        {projectContext == null ? (
          <p className="context-card__empty">
            {projectContextScanning ? t("rail.shareScanning") : t("rail.shareOpenProject")}
          </p>
        ) : (
          <div className="context-lend">
            <p className="context-lend__hint" title={t("rail.shareHint")}>
              {t("rail.shareHint")}
            </p>

            <div className="context-lend__labelrow">
              <span className="context-lend__label">{t("rail.shareMcp")}</span>
              <span className="context-lend__count">
                {mcpServers.length}
              </span>
              {onReconnectAgent && (
                <button
                  type="button"
                  className="pill-action context-lend__reconnect"
                  disabled={reconnecting}
                  onClick={onReconnectAgent}
                  title={t("rail.shareReconnectTitle")}
                >
                  <RefreshCw size={11} className={reconnecting ? "is-spinning" : undefined} aria-hidden />
                  {reconnecting ? t("rail.shareReconnecting") : t("rail.shareReconnect")}
                </button>
              )}
            </div>
            {mcpServers.length === 0 ? (
              <p className="context-card__empty">{t("rail.shareNoMcp")}</p>
            ) : (
              <>
                <ul className="context-lend__list custom-scrollbar scrollbar-autohide">
                  {mcpSlice.visible.map((server) => {
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
                <LendClip
                  total={mcpServers.length}
                  hidden={mcpSlice.hidden}
                  expanded={mcpExpanded}
                  onExpand={() => setMcpExpanded(true)}
                  onCollapse={() => setMcpExpanded(false)}
                />
              </>
            )}

            <div className="context-lend__labelrow">
              <span className="context-lend__label">{t("rail.shareSkills")}</span>
              <span className="context-lend__count">
                {skillItems.length}
              </span>
            </div>
            {skillItems.length === 0 ? (
              <p className="context-card__empty">{t("rail.shareNoSkills")}</p>
            ) : (
              <>
                <ul className="context-lend__list custom-scrollbar scrollbar-autohide">
                  {skillsSlice.visible.map((skill) => {
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
                            {native ? ` · ${t("rail.shareNativeHere")}` : ""}
                          </span>
                        </label>
                      </li>
                    );
                  })}
                </ul>
                <LendClip
                  total={skillItems.length}
                  hidden={skillsSlice.hidden}
                  expanded={skillsExpanded}
                  onExpand={() => setSkillsExpanded(true)}
                  onCollapse={() => setSkillsExpanded(false)}
                />
              </>
            )}
            {projectContext.inventory.notes.map((note) => (
              <p className="context-card__empty" key={note}>
                {note}
              </p>
            ))}
          </div>
        )}
      </section>

      <section className="context-card" id="context-plugins">
        <div className="context-card__heading">
          <span>{t("plugins.title")}</span>
          {pluginCount != null ? (
            <span className="context-lend__count">{pluginCount}</span>
          ) : null}
        </div>
        {pluginsLoaded && pluginsEmpty ? (
          <p className="context-card__empty" style={{ marginTop: 8 }}>
            {t("rail.pluginsEmpty")}
          </p>
        ) : null}
        <UniversalPluginsSection embedded onCount={setUniversalPluginCount} />
        <UniversalSkillsSection embedded onCount={setSkillCount} />
        <p className="panel-note panel-note--hint" title={t("allPlugins.applyHint")}>
          {t("allPlugins.applyHint")}
        </p>
      </section>

      <MemorySection activeSessionId={activeSessionId} onCount={setMemoryCount} />

      <section className="context-card">
        <div className="context-card__heading">
          <span>{t("context.handoff")}</span>
        </div>
        {handoff ? (
          <div className="handoff-block">
            <strong title={handoff.handoffPath}>{handoff.summary || handoff.prompt.slice(0, 80)}</strong>
            <small>{t("rail.handoffTarget", { agent: handoff.targetAgentId })}</small>
            <small title={handoff.handoffPath}>{handoff.handoffPath}</small>
            <p className="handoff-block__hint" title={t("rail.handoffHint")}>
              {t("rail.handoffHint")}
            </p>
          </div>
        ) : (
          <p className="context-card__empty">{t("rail.handoffEmpty")}</p>
        )}
      </section>
          </>
        )}
      </section>

      <div className="context-panel__footer">
        {docked && updateButton ? createPortal(updateButton, dockFooterHost()) : updateButton}
      </div>
    </aside>
  );
});
