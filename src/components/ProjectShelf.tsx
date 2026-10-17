import { useCallback, useEffect, useMemo, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import { Bell, BellOff, ChevronDown, ChevronRight, CloudMoon, Folder, FolderOpen, Globe, GripVertical, KeyRound, MessageSquare, Network, PanelLeftClose, PanelLeftOpen, Pause, Pencil, Pin, PinOff, Plus, RefreshCw, Save, Search, SunMedium, Trash2, X, Zap } from "lucide-react";
// Refresh on a project row folds CLI history into the normal session list.
import type { AgentConfig, Project, ProxyConfig, ProxyTestResult, Session } from "../lib/types";
import { themeToggleTitle, type ThemeMode } from "../lib/theme";
import { collapsedShelfSessions, pinStamp, sortSessionsNewestFirst } from "../lib/sessionOrder";
import { suspendControl } from "../lib/sessionSuspend";
import { loadCollapsedProjectIds, saveCollapsedProjectIds } from "../lib/uiRestore";
import { engineEnable, engineStatus, engineStop, type EngineStatus } from "../lib/api";
import { ConfirmDialog } from "./ConfirmDialog";
import { CredentialsDialog } from "./CredentialsDialog";

/** Visible sessions per project before "Show more". Projects always list fully. */
const SESSION_LIST_PREVIEW = 5;
const EMPTY_DONE_UNREAD: ReadonlySet<string> = new Set();

/** Pin toggle shared by project session rows and chat rows. */
function SessionPinButton({
  label,
  pinned,
  className,
  onToggle,
}: {
  label: string;
  pinned: boolean;
  className: string;
  onToggle: (next: boolean) => void;
}) {
  return (
    <button
      className={pinned ? `${className} is-pinned` : className}
      type="button"
      title={pinned ? `取消置顶 ${label}` : `置顶 ${label}`}
      aria-label={pinned ? `Unpin ${label}` : `Pin ${label} to the top`}
      aria-pressed={pinned}
      onClick={(event) => {
        event.stopPropagation();
        onToggle(!pinned);
      }}
    >
      {pinned ? <PinOff size={12} /> : <Pin size={12} />}
    </button>
  );
}

/** Always-visible marker so a pinned row reads as pinned without hovering. */
function PinnedMark({ className }: { className: string }) {
  return (
    <span className={className} title="已置顶" aria-hidden>
      <Pin size={10} />
    </span>
  );
}

type ProjectShelfProps = {
  agents: AgentConfig[];
  projects: Project[];
  sessions: Session[];
  currentProjectId: string;
  currentSessionId?: string;
  /** Dialogs that finished a turn while this window was looking elsewhere. */
  doneUnreadIds?: ReadonlySet<string>;
  /**
   * Project opened via Explorer "在此处打开 Marionette" — always listed first
   * with a small "此处" badge for this app session.
   */
  pinnedProjectId?: string | null;
  collapsed: boolean;
  theme: ThemeMode;
  /** Session ids matched by transcript search (null = no active query). */
  searchHitIds?: string[] | null;
  onSearchQueryChange?: (query: string) => void;
  onCollapse: () => void;
  onExpand: () => void;
  onToggleTheme: () => void;
  /** Desktop attention (taskbar flash + sound) for agent events and stalls. */
  desktopNotifyEnabled?: boolean;
  onToggleDesktopNotify?: () => void;
  onAddProject: () => void;
  onNewSession: (projectId: string) => void;
  onProjectSelect: (projectId: string) => void;
  onSessionSelect: (session: Session) => void;
  onDeleteSession: (sessionId: string) => void;
  /** Pin / unpin a dialog so it sorts to the top of its project. */
  onToggleSessionPin?: (sessionId: string, pinned: boolean) => void;
  onDeleteProject: (projectId: string) => void;
  /** Manual rename — always persists (like first-message auto-title). */
  onRenameSession?: (sessionId: string, label: string) => void;
  /** Park an idle dialog: stop its agent process, reconnect on the next send. */
  onSuspendSession?: (sessionId: string) => void;
  /**
   * Drag reorder. `place` is where the dragged row lands relative to the target:
   * before = insert above the line, after = insert below the line.
   */
  onReorderProjects?: (
    fromProjectId: string,
    toProjectId: string,
    place: "before" | "after"
  ) => void;
  /** Reveal project folder in the OS file manager. */
  onRevealProject?: (project: Project) => void;
  /** Agent proxy: single exit address injected into spawned agents. */
  proxyConfig?: ProxyConfig | null;
  /** Persist the proxy config (App also restarts the live agent). */
  onSaveProxy?: (config: ProxyConfig) => Promise<void>;
  /** Round-trip through `url` to verify the proxy path is alive. */
  onTestProxy?: (url: string) => Promise<ProxyTestResult | null>;
  /** Default folder path for the Chat section. */
  defaultFolderPath?: string;
  /** Chat sessions to display in the Chat section. */
  chatSessions?: Session[];
  /** Open a chat session from the Chat section. */
  onChatSessionSelect?: (session: Session) => void;
  /** Create a new chat session. */
  onNewChat?: () => void;
  /** Scan this project's agent CLI stores into the normal session list. */
  externalScanning?: boolean;
  onRefreshExternal?: (projectId: string) => void;
};

type DropHint = { targetId: string; place: "before" | "after" };

function sessionIsBusy(status: Session["status"]): boolean {
  return status === "running" || status === "starting";
}

export function ProjectShelf({
  agents,
  projects,
  sessions,
  currentProjectId,
  currentSessionId,
  doneUnreadIds,
  pinnedProjectId = null,
  onProjectSelect,
  onSessionSelect,
  collapsed,
  onCollapse,
  onExpand,
  onAddProject,
  onNewSession,
  theme,
  onToggleTheme,
  desktopNotifyEnabled = true,
  onToggleDesktopNotify,
  onDeleteSession,
  onToggleSessionPin,
  onDeleteProject,
  onRenameSession,
  onSuspendSession,
  onReorderProjects,
  onRevealProject,
  proxyConfig = null,
  onSaveProxy,
  onTestProxy,
  searchHitIds = null,
  onSearchQueryChange,
  defaultFolderPath = "",
  chatSessions = [],
  onChatSessionSelect,
  onNewChat,
  externalScanning = false,
  onRefreshExternal,
}: ProjectShelfProps) {
  const unreadDone = doneUnreadIds ?? EMPTY_DONE_UNREAD;
  /** Collapsed project ids (persisted). Default for unknown ids is expanded. */
  const [collapsedProjects, setCollapsedProjects] = useState<Set<string>>(() => loadCollapsedProjectIds());
  const [query, setQuery] = useState("");
  /** Proxy popover: draft config, anchor + open state. */
  const [proxyOpen, setProxyOpen] = useState(false);
  const [proxyDraft, setProxyDraft] = useState<ProxyConfig>(() =>
    proxyConfig ? { ...proxyConfig } : { enabled: false, url: "" }
  );
  const [proxyBusy, setProxyBusy] = useState<"test" | "save" | null>(null);
  const [proxyTest, setProxyTest] = useState<string | null>(null);
  const [proxyError, setProxyError] = useState<string | null>(null);
  const [proxyAnchor, setProxyAnchor] = useState<{ left: number; top: number } | null>(null);
  const proxyBtnRef = useRef<HTMLButtonElement | null>(null);
  const proxyPopRef = useRef<HTMLDivElement | null>(null);

  /** Keep the draft in sync when the persisted config changes externally. */
  useEffect(() => {
    if (!proxyOpen) {
      setProxyDraft(proxyConfig ? { ...proxyConfig } : { enabled: false, url: "" });
    }
  }, [proxyConfig, proxyOpen]);

  /** Close the popover on outside click / Escape. */
  useEffect(() => {
    if (!proxyOpen) return;
    const onDown = (event: MouseEvent) => {
      const target = event.target as Node;
      if (proxyPopRef.current?.contains(target) || proxyBtnRef.current?.contains(target)) return;
      setProxyOpen(false);
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") setProxyOpen(false);
    };
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [proxyOpen]);

  const openProxyPopover = () => {
    const rect = proxyBtnRef.current?.getBoundingClientRect();
    setProxyAnchor(rect ? { left: rect.left, top: rect.top } : null);
    setProxyTest(null);
    setProxyError(null);
    setProxyOpen((open) => !open);
  };

  const runProxyTest = async () => {
    if (!onTestProxy) return;
    const url = proxyDraft.url.trim();
    if (!url) {
      setProxyError("请先填写代理地址");
      return;
    }
    setProxyBusy("test");
    setProxyError(null);
    setProxyTest(null);
    try {
      const result = await onTestProxy(url);
      setProxyTest(result ? result.message : "无法测试（非桌面环境）");
    } catch (error) {
      setProxyError(error instanceof Error ? error.message : String(error));
    } finally {
      setProxyBusy(null);
    }
  };

  const saveProxy = async () => {
    if (!onSaveProxy) return;
    const url = proxyDraft.url.trim();
    if (proxyDraft.enabled && !url) {
      setProxyError("请填写代理地址");
      return;
    }
    setProxyBusy("save");
    setProxyError(null);
    try {
      await onSaveProxy({ enabled: proxyDraft.enabled, url });
      setProxyOpen(false);
    } catch (error) {
      setProxyError(error instanceof Error ? error.message : String(error));
    } finally {
      setProxyBusy(null);
    }
  };
  /** Credentials (cross-agent bridge) dialog — key scan + manual keys. */
  const [credentialsOpen, setCredentialsOpen] = useState(false);

  /**
   * Cross-use gateway (engine) popover. The engine auto-starts with the app
   * (2026-10-16 product decision: open the app and everything works); this
   * button is its always-visible status + one-click start, NOT a dialog —
   * key management stays one hop deeper (footer link → CredentialsDialog).
   */
  const [engineOpen, setEngineOpen] = useState(false);
  const [engineAnchor, setEngineAnchor] = useState<{ left: number; top: number } | null>(null);
  const [engineState, setEngineState] = useState<EngineStatus | null>(null);
  const engineBtnRef = useRef<HTMLButtonElement | null>(null);
  const enginePopRef = useRef<HTMLDivElement | null>(null);

  const refreshEngine = useCallback(() => {
    void engineStatus().then(setEngineState);
  }, []);
  useEffect(refreshEngine, [refreshEngine]);

  /** Open popover: anchor + 1 Hz status polling (catches download/start transitions). */
  useEffect(() => {
    if (!engineOpen) return;
    refreshEngine();
    const timer = window.setInterval(refreshEngine, 1000);
    const onDown = (event: MouseEvent) => {
      const target = event.target as Node;
      if (enginePopRef.current?.contains(target) || engineBtnRef.current?.contains(target)) return;
      setEngineOpen(false);
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") setEngineOpen(false);
    };
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      window.clearInterval(timer);
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [engineOpen, refreshEngine]);

  /** Project ids whose session list is fully expanded past the preview cap. */
  const [sessionsExpandedByProject, setSessionsExpandedByProject] = useState<Set<string>>(() => new Set());

  // A finished dialog behind a collapsed project would be invisible — open it.
  useEffect(() => {
    if (unreadDone.size === 0) return;
    const projectIds = new Set<string>();
    for (const session of sessions) {
      if (unreadDone.has(session.id)) projectIds.add(session.projectId);
    }
    if (projectIds.size === 0) return;
    setCollapsedProjects((current) => {
      let changed = false;
      const next = new Set(current);
      for (const id of projectIds) {
        if (next.delete(id)) changed = true;
      }
      if (!changed) return current;
      saveCollapsedProjectIds(next);
      return next;
    });
  }, [unreadDone, sessions]);

  const [renamingId, setRenamingId] = useState<string | null>(null);
  const [renameDraft, setRenameDraft] = useState("");
  const renameInputRef = useRef<HTMLInputElement>(null);
  /** Pending destructive action — rendered as an in-app window instead of window.confirm. */
  const [pendingDelete, setPendingDelete] = useState<
    { kind: "project" | "session" | "chat"; id: string; label: string } | null
  >(null);
  /**
   * Pointer-based reorder (not HTML5 DnD).
   * WebView2 often won't start a drag from nested <button>s, and React state
   * set in dragstart is too late for dragover preventDefault — both broke order.
   */
  const [draggingProjectId, setDraggingProjectId] = useState<string | null>(null);
  /** Insertion line: which row edge the drop will land on. */
  const [dropHint, setDropHint] = useState<DropHint | null>(null);
  const projectDragRef = useRef<{
    fromId: string;
    pointerId: number;
    startX: number;
    startY: number;
    armed: boolean;
  } | null>(null);
  const dropHintRef = useRef<DropHint | null>(null);
  dropHintRef.current = dropHint;

  useEffect(() => {
    if (!renamingId) return;
    renameInputRef.current?.focus();
    renameInputRef.current?.select();
  }, [renamingId]);

  const commitRename = () => {
    if (!renamingId || !onRenameSession) {
      setRenamingId(null);
      return;
    }
    const next = renameDraft.trim() || "New session";
    onRenameSession(renamingId, next);
    setRenamingId(null);
  };

  const agentLabel = (agentId: string) =>
    agents.find((a) => a.id === agentId)?.label ?? agentId;

  /** Search always reveals matching projects; otherwise respect saved collapse. */
  const isProjectExpanded = (projectId: string) =>
    Boolean(query.trim()) || !collapsedProjects.has(projectId);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) {
      return { projects, sessionsByProject: null as Map<string, Session[]> | null };
    }
    const hitSet = searchHitIds ? new Set(searchHitIds) : null;
    const sessionsByProject = new Map<string, Session[]>();
    const projectIds = new Set<string>();

    for (const session of sessions) {
      const metaMatch =
        session.label.toLowerCase().includes(q) ||
        session.agentId.toLowerCase().includes(q) ||
        agentLabel(session.agentId).toLowerCase().includes(q);
      const transcriptMatch = hitSet?.has(session.id) ?? false;
      if (metaMatch || transcriptMatch) {
        projectIds.add(session.projectId);
        const list = sessionsByProject.get(session.projectId) ?? [];
        list.push(session);
        sessionsByProject.set(session.projectId, list);
      }
    }

    for (const project of projects) {
      if (project.name.toLowerCase().includes(q) || project.rootPath.toLowerCase().includes(q)) {
        projectIds.add(project.id);
        if (!sessionsByProject.has(project.id)) {
          sessionsByProject.set(
            project.id,
            sessions.filter((s) => s.projectId === project.id)
          );
        }
      }
    }

    return {
      projects: projects.filter((p) => projectIds.has(p.id)),
      sessionsByProject,
    };
  }, [agents, projects, query, searchHitIds, sessions]);

  const toggleProject = (projectId: string) => {
    setCollapsedProjects((current) => {
      const next = new Set(current);
      if (next.has(projectId)) next.delete(projectId);
      else next.add(projectId);
      saveCollapsedProjectIds(next);
      return next;
    });
  };

  const endProjectDrag = (commit: boolean) => {
    const drag = projectDragRef.current;
    projectDragRef.current = null;
    const fromId = drag?.fromId ?? null;
    const hint = dropHintRef.current;
    setDraggingProjectId(null);
    setDropHint(null);
    dropHintRef.current = null;
    document.body.classList.remove("is-project-reordering");
    if (
      !commit ||
      !drag?.armed ||
      !fromId ||
      !hint ||
      !onReorderProjects
    ) {
      return;
    }
    // No-op if the line sits on an edge that leaves order unchanged.
    onReorderProjects(fromId, hint.targetId, hint.place);
  };

  const onProjectGripPointerDown = (projectId: string, event: ReactPointerEvent<HTMLButtonElement>) => {
    if (!onReorderProjects || searching || event.button !== 0) return;
    event.preventDefault();
    event.stopPropagation();
    try {
      event.currentTarget.setPointerCapture(event.pointerId);
    } catch {
      /* capture optional */
    }
    projectDragRef.current = {
      fromId: projectId,
      pointerId: event.pointerId,
      startX: event.clientX,
      startY: event.clientY,
      armed: false,
    };
    setDraggingProjectId(projectId);
    setDropHint(null);
    dropHintRef.current = null;
    document.body.classList.add("is-project-reordering");
  };

  const onProjectGripPointerMove = (event: ReactPointerEvent<HTMLButtonElement>) => {
    const drag = projectDragRef.current;
    if (!drag || drag.pointerId !== event.pointerId) return;
    const dist = Math.hypot(event.clientX - drag.startX, event.clientY - drag.startY);
    if (!drag.armed && dist < 4) return;
    drag.armed = true;
    // elementFromPoint under capture still sees targets below the grip.
    const el = document.elementFromPoint(event.clientX, event.clientY);
    const group = el?.closest("[data-project-id]") as HTMLElement | null;
    const overId = group?.dataset.projectId ?? null;
    if (!overId || overId === drag.fromId) {
      // Still over self (or gap): clear line so we don't lie about the drop.
      if (dropHintRef.current) {
        dropHintRef.current = null;
        setDropHint(null);
      }
      return;
    }
    // Half-row rule: top half → insert before, bottom half → insert after.
    const rowEl = group?.querySelector(".project-row") as HTMLElement | null;
    const box = (rowEl ?? group)!.getBoundingClientRect();
    const place: "before" | "after" =
      event.clientY < box.top + box.height / 2 ? "before" : "after";
    const next: DropHint = { targetId: overId, place };
    const prev = dropHintRef.current;
    if (!prev || prev.targetId !== next.targetId || prev.place !== next.place) {
      dropHintRef.current = next;
      setDropHint(next);
    }
  };

  const onProjectGripPointerUp = (event: ReactPointerEvent<HTMLButtonElement>) => {
    const drag = projectDragRef.current;
    if (!drag || drag.pointerId !== event.pointerId) return;
    try {
      event.currentTarget.releasePointerCapture(event.pointerId);
    } catch {
      /* ignore */
    }
    endProjectDrag(true);
  };

  const onProjectGripPointerCancel = (event: ReactPointerEvent<HTMLButtonElement>) => {
    const drag = projectDragRef.current;
    if (!drag || drag.pointerId !== event.pointerId) return;
    endProjectDrag(false);
  };

  const themeButton = (
    <button
      className="pill-action pill-action--icon pill-action--sm sidebar-footer__button"
      type="button"
      title={themeToggleTitle(theme)}
      aria-label={themeToggleTitle(theme)}
      onClick={onToggleTheme}
    >
      {theme === "dim" ? <CloudMoon size={13} /> : <SunMedium size={13} />}
    </button>
  );

  const notifyButton = onToggleDesktopNotify ? (
    <button
      className={
        desktopNotifyEnabled
          ? "pill-action pill-action--icon pill-action--sm sidebar-footer__button is-notify-on"
          : "pill-action pill-action--icon pill-action--sm sidebar-footer__button is-notify-off"
      }
      type="button"
      title={
        desktopNotifyEnabled
          ? "Desktop notify on · completion chime even while focused; taskbar flash when away"
          : "Desktop notify off · click to enable"
      }
      aria-label={desktopNotifyEnabled ? "Disable desktop notifications" : "Enable desktop notifications"}
      aria-pressed={desktopNotifyEnabled}
      onClick={onToggleDesktopNotify}
    >
      {desktopNotifyEnabled ? <Bell size={13} /> : <BellOff size={13} />}
    </button>
  ) : null;

  const proxyButton = (
    <button
      ref={proxyBtnRef}
      type="button"
      className={
        proxyConfig?.enabled
          ? "pill-action pill-action--icon pill-action--sm sidebar-footer__button is-proxy-on"
          : "pill-action pill-action--icon pill-action--sm sidebar-footer__button is-proxy-off"
      }
      title={
        proxyConfig?.enabled
          ? `代理已启用 · ${proxyConfig.url} · 点此修改`
          : "代理未启用 · 点此配置"
      }
      aria-label={proxyConfig?.enabled ? "配置代理（已启用）" : "配置代理（未启用）"}
      aria-pressed={proxyConfig?.enabled}
      aria-haspopup="dialog"
      aria-expanded={proxyOpen}
      onClick={openProxyPopover}
    >
      <Globe size={13} />
    </button>
  );

  const engineRunning = engineState?.state === "running";
  const engineBusyTransient =
    engineState?.state === "downloading" || engineState?.state === "starting";
  const engineStatusLine = (() => {
    if (!engineState) return "状态未知";
    switch (engineState.state) {
      case "running":
        return `运行中 · 127.0.0.1:${engineState.port}${engineState.reused ? " · 复用自装实例" : ""}${engineState.version ? ` · magpie ${engineState.version}` : ""}`;
      case "downloading": {
        const { downloaded, total } = engineState;
        const mb = (n: number) => `${(n / 1024 / 1024).toFixed(1)} MB`;
        return total > 0
          ? `下载中 ${mb(downloaded)} / ${mb(total)}（${Math.min(100, Math.round((downloaded / total) * 100))}%）`
          : "下载中…";
      }
      case "starting":
        return "启动中…";
      case "failed":
        return `启动失败：${engineState.error.slice(0, 120)}`;
      default:
        return "未启动";
    }
  })();

  const gatewayButton = (
    <button
      ref={engineBtnRef}
      type="button"
      className={
        engineRunning
          ? "pill-action pill-action--icon pill-action--sm sidebar-footer__button is-engine-on"
          : "pill-action pill-action--icon pill-action--sm sidebar-footer__button is-engine-off"
      }
      title={
        engineRunning
          ? `跨用网关 ${engineStatusLine} · 点此查看`
          : "跨用网关未运行 · 点击启动并查看"
      }
      aria-label={engineRunning ? "跨用网关（运行中）" : "跨用网关（未运行）"}
      aria-pressed={engineRunning}
      aria-haspopup="dialog"
      aria-expanded={engineOpen}
      onClick={() => {
        const rect = engineBtnRef.current?.getBoundingClientRect();
        setEngineAnchor(rect ? { left: rect.left, top: rect.top } : null);
        setEngineOpen((open) => !open);
        // 一键启动：不在运行（且不在下载/启动中）时，点击即拉起网关；
        // 弹层只负责展示状态，凭据管理退到弹层底部入口。
        if (!engineRunning && !engineBusyTransient) {
          void engineEnable().then((snapshot) => {
            if (snapshot) setEngineState(snapshot);
          });
        }
      }}
    >
      <Network size={13} />
    </button>
  );

  const searching = Boolean(query.trim());
  const visibleChatSessions = useMemo(() => {
    if (!searching) return chatSessions;
    const q = query.trim().toLowerCase();
    const hitSet = searchHitIds ? new Set(searchHitIds) : null;
    return chatSessions.filter((session) =>
      hitSet?.has(session.id) ||
      session.label.toLowerCase().includes(q) ||
      session.agentId.toLowerCase().includes(q) ||
      session.cwd.toLowerCase().includes(q),
    );
  }, [chatSessions, query, searchHitIds, searching]);
  // Pin the Explorer "open here" project to the top of the shelf (still filterable).
  const allVisibleProjects = useMemo(() => {
    const list = filtered.projects;
    if (!pinnedProjectId) return list;
    const pinned = list.find((p) => p.id === pinnedProjectId);
    if (!pinned) return list;
    return [pinned, ...list.filter((p) => p.id !== pinnedProjectId)];
  }, [filtered.projects, pinnedProjectId]);
  // Projects always fully listed; only sessions inside a project use Show more/less.

  return (
    <>
    <section className="project-tree">
      {/* Same height as center tabs / right Information — flush to window top like Zed. */}
      <div className="sidebar-title titlebar-row">
        {/* Full-bleed drag layer behind the field; input sits above so typing still works. */}
        <div className="titlebar-drag-layer" data-tauri-drag-region />
        <label className="sidebar-search sidebar-search--input sidebar-search--titlebar">
          <Search size={13} aria-hidden />
          <input
            type="search"
            value={query}
            placeholder="Search threads…"
            aria-label="Search threads"
            onChange={(event) => {
              const next = event.target.value;
              setQuery(next);
              onSearchQueryChange?.(next);
            }}
          />
        </label>
      </div>

      <div className="project-tree__body">
      <button className="project-add" type="button" title="Add project" onClick={onAddProject}>
        <Plus size={13} />
        Add project
      </button>

      <div className="project-list custom-scrollbar scrollbar-autohide" aria-label="Projects with sessions">
        {allVisibleProjects.length === 0 && (
          <div className="project-list__empty">
            {searching ? `No threads match “${query.trim()}”` : "No projects yet"}
          </div>
        )}
        {allVisibleProjects.map((project) => {
          const rawSessions =
            filtered.sessionsByProject?.get(project.id) ??
            sessions.filter((session) => session.projectId === project.id);
          // Pinned first, then recency — click-to-select must not jump the row.
          const projectSessions = sortSessionsNewestFirst(rawSessions);
          const projectExpanded = isProjectExpanded(project.id);
          const sessionsFullyExpanded = searching || sessionsExpandedByProject.has(project.id);
          const shouldCollapseSessions =
            !sessionsFullyExpanded && projectSessions.length > SESSION_LIST_PREVIEW;
          const visibleSessions = shouldCollapseSessions
            ? collapsedShelfSessions(projectSessions, SESSION_LIST_PREVIEW, unreadDone)
            : projectSessions;
          const hiddenSessionCount = Math.max(0, projectSessions.length - visibleSessions.length);

          const showLineBefore =
            dropHint?.targetId === project.id &&
            dropHint.place === "before" &&
            draggingProjectId != null &&
            draggingProjectId !== project.id;
          const showLineAfter =
            dropHint?.targetId === project.id &&
            dropHint.place === "after" &&
            draggingProjectId != null &&
            draggingProjectId !== project.id;

          const isPinnedHere = pinnedProjectId === project.id;
          const hasDoneUnread = projectSessions.some((session) => unreadDone.has(session.id));
          const rowClass = [
            "project-row",
            project.id === currentProjectId ? "is-active" : "",
            draggingProjectId === project.id ? "is-dragging" : "",
            isPinnedHere ? "is-open-here" : "",
            hasDoneUnread ? "is-done" : "",
          ]
            .filter(Boolean)
            .join(" ");

          return (
            <div className="project-group" key={project.id} data-project-id={project.id}>
              {showLineBefore && (
                <div className="project-drop-line" aria-hidden>
                  <span className="project-drop-line__dot" />
                  <span className="project-drop-line__bar" />
                </div>
              )}
              <div className={rowClass}>
                {onReorderProjects && !searching ? (
                  <button
                    className="project-row__grip"
                    type="button"
                    title="Drag to reorder projects"
                    aria-label={`Reorder ${project.name}`}
                    onPointerDown={(e) => onProjectGripPointerDown(project.id, e)}
                    onPointerMove={onProjectGripPointerMove}
                    onPointerUp={onProjectGripPointerUp}
                    onPointerCancel={onProjectGripPointerCancel}
                  >
                    <GripVertical size={13} />
                  </button>
                ) : (
                  <span className="project-row__grip project-row__grip--spacer" aria-hidden />
                )}
                <button className="project-row__toggle" type="button" title={projectExpanded ? "Collapse project" : "Expand project"} aria-label={projectExpanded ? `Collapse ${project.name}` : `Expand ${project.name}`} aria-expanded={projectExpanded} onClick={() => toggleProject(project.id)}>
                  {projectExpanded ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
                </button>
                <button
                  className="project-row__select"
                  type="button"
                  title={
                    isPinnedHere
                      ? `${project.rootPath}\n(从资源管理器「在此处打开」)`
                      : project.rootPath
                  }
                  onClick={() => onProjectSelect(project.id)}
                >
                  <Folder size={15} />
                  <span className="project-row__content">
                    <strong>{project.name}</strong>
                    {isPinnedHere && (
                      <em className="project-row__here-badge" title="当前从资源管理器打开的路径">
                        此处
                      </em>
                    )}
                    {hasDoneUnread && (
                      <em className="project-row__done-badge" title="有对话已完成任务">
                        完成
                      </em>
                    )}
                  </span>
                </button>
                <span className="project-row__actions">
                  <button className="project-row__action" type="button" title="New session" onClick={() => onNewSession(project.id)}>
                    <Plus size={13} />
                  </button>
                  {onRefreshExternal && (
                    <button
                      className="project-row__action"
                      type="button"
                      title="把这个目录在各 agent 命令行里的对话收进列表"
                      aria-label={`Refresh conversations for ${project.name}`}
                      disabled={externalScanning}
                      onClick={(e) => {
                        e.stopPropagation();
                        onRefreshExternal(project.id);
                      }}
                    >
                      <RefreshCw size={12} className={externalScanning && project.id === currentProjectId ? "is-spinning" : undefined} />
                    </button>
                  )}
                  <button
                    className="project-row__action project-row__action--danger"
                    type="button"
                    title={`Remove project ${project.name}`}
                    aria-label={`Remove project ${project.name}`}
                    onClick={() => {
                      setPendingDelete({ kind: "project", id: project.id, label: project.name });
                    }}
                  >
                    <Trash2 size={13} />
                  </button>
                  {onRevealProject && (
                    <button
                      className="project-row__action"
                      type="button"
                      title={`Reveal ${project.name} in file manager`}
                      aria-label={`Reveal ${project.name} in file manager`}
                      onClick={(e) => {
                        e.stopPropagation();
                        onRevealProject(project);
                      }}
                    >
                      <FolderOpen size={13} />
                    </button>
                  )}
                </span>
              </div>
              {showLineAfter && (
                <div className="project-drop-line" aria-hidden>
                  <span className="project-drop-line__dot" />
                  <span className="project-drop-line__bar" />
                </div>
              )}

              {projectExpanded && (
                <div className="project-sessions">
                  {projectSessions.length === 0 && (
                    <div className="session-row session-row--empty">No sessions</div>
                  )}
                  {visibleSessions.map((session) => {
                    const busy = sessionIsBusy(session.status);
                    const park = suspendControl(session, session.label);
                    const isRenaming = renamingId === session.id;
                    const pinned = pinStamp(session) != null;
                    const doneUnread = unreadDone.has(session.id);
                    const beginRename = () => {
                      if (!onRenameSession) return;
                      setRenamingId(session.id);
                      setRenameDraft(session.label);
                    };
                    const statusMark = busy ? (
                      <span
                        className={`session-row__pulse is-${session.status}`}
                        title={session.status}
                        aria-label={session.status}
                      />
                    ) : doneUnread ? (
                      <span
                        className="session-row__dot is-done"
                        title="任务已完成"
                        aria-label="任务已完成"
                      />
                    ) : (
                      <span
                        className={`session-row__dot is-${session.status}`}
                        aria-hidden
                      />
                    );
                    return (
                      <div
                        className={[
                          "session-row",
                          session.id === currentSessionId ? "is-active" : "",
                          `is-${session.status}`,
                          pinned ? "is-pinned" : "",
                          doneUnread ? "is-done" : "",
                        ]
                          .filter(Boolean)
                          .join(" ")}
                        key={session.id}
                      >
                        {isRenaming ? (
                          <form
                            className="session-row__rename"
                            onSubmit={(e) => {
                              e.preventDefault();
                              commitRename();
                            }}
                          >
                            {statusMark}
                            <input
                              ref={renameInputRef}
                              className="session-row__rename-input"
                              value={renameDraft}
                              aria-label="Rename session"
                              onChange={(e) => setRenameDraft(e.target.value)}
                              onBlur={() => commitRename()}
                              onKeyDown={(e) => {
                                if (e.key === "Escape") {
                                  e.preventDefault();
                                  setRenamingId(null);
                                }
                              }}
                            />
                          </form>
                        ) : (
                          <button
                            className="session-row__select"
                            type="button"
                            title={
                              doneUnread
                                ? `${session.label} · 任务已完成`
                                : busy
                                  ? `${agentLabel(session.agentId)} · ${session.status} · double-click or pencil to rename`
                                  : `${agentLabel(session.agentId)} · double-click or pencil to rename`
                            }
                            onClick={() => onSessionSelect(session)}
                            onDoubleClick={(e) => {
                              e.preventDefault();
                              e.stopPropagation();
                              beginRename();
                            }}
                          >
                            {statusMark}
                            <span className="session-row__content">
                              {pinned && <PinnedMark className="session-row__pin" />}
                              <strong>{session.label}</strong>
                            </span>
                          </button>
                        )}
                        <span className="session-row__actions">
                          {onToggleSessionPin && !isRenaming && (
                            <SessionPinButton
                              label={session.label}
                              pinned={pinned}
                              className="session-row__action"
                              onToggle={(next) => onToggleSessionPin(session.id, next)}
                            />
                          )}
                          {onRenameSession && !isRenaming && (
                            <button
                              className="session-row__action"
                              type="button"
                              title={`Rename ${session.label}`}
                              aria-label={`Rename ${session.label}`}
                              onClick={(e) => {
                                e.stopPropagation();
                                beginRename();
                              }}
                            >
                              <Pencil size={12} />
                            </button>
                          )}
                          {park.shown && onSuspendSession && !isRenaming && (
                            <button
                              className="session-row__action"
                              type="button"
                              title={park.title}
                              aria-label={park.ariaLabel}
                              onClick={(e) => {
                                e.stopPropagation();
                                onSuspendSession(session.id);
                              }}
                            >
                              <Pause size={12} />
                            </button>
                          )}
                          <button
                            className="session-row__action session-row__action--danger"
                            type="button"
                            title={`Delete ${session.label}`}
                            aria-label={`Delete ${session.label}`}
                            onClick={() => {
                              setPendingDelete({ kind: "session", id: session.id, label: session.label });
                            }}
                          >
                            <Trash2 size={12} />
                          </button>
                        </span>
                      </div>
                    );
                  })}

                  {shouldCollapseSessions && hiddenSessionCount > 0 && (
                    <div className="shelf-clip shelf-clip--sessions">
                      <div className="shelf-clip__fade" aria-hidden />
                      <button
                        className="shelf-clip__more"
                        type="button"
                        title={`Show ${hiddenSessionCount} more session${hiddenSessionCount === 1 ? "" : "s"}`}
                        aria-label={`Show ${hiddenSessionCount} more sessions`}
                        onClick={() => {
                          setSessionsExpandedByProject((current) => {
                            const next = new Set(current);
                            next.add(project.id);
                            return next;
                          });
                        }}
                      >
                        Show more
                        <span className="shelf-clip__count">{hiddenSessionCount}</span>
                      </button>
                    </div>
                  )}
                  {!searching &&
                    sessionsExpandedByProject.has(project.id) &&
                    projectSessions.length > SESSION_LIST_PREVIEW && (
                      <div className="shelf-clip shelf-clip--sessions shelf-clip--expanded">
                        <button
                          className="shelf-clip__more"
                          type="button"
                          title="Show fewer sessions"
                          aria-label="Show fewer sessions"
                          onClick={() => {
                            setSessionsExpandedByProject((current) => {
                              const next = new Set(current);
                              next.delete(project.id);
                              return next;
                            });
                          }}
                        >
                          Show less
                        </button>
                      </div>
                    )}
                </div>
              )}
            </div>
          );
        })}
      </div>

      {/* ── Chat section ── */}
      {onChatSessionSelect && (
        <section className="chat-section">
          <div
            className="chat-section__header"
            title={defaultFolderPath ? `工作目录：${defaultFolderPath}` : "Chat 工作目录"}
          >
            <span className="chat-section__title">
              <MessageSquare size={13} aria-hidden />
              聊天
            </span>
            {onNewChat && (
              <button
                className="pill-action pill-action--icon pill-action--sm chat-section__new"
                type="button"
                title="新建对话"
                aria-label="New chat"
                onClick={() => onNewChat()}
              >
                <Plus size={13} />
              </button>
            )}
          </div>
          <div className="chat-list custom-scrollbar scrollbar-autohide" aria-label="Chat conversations">
            {visibleChatSessions.length === 0 && (
              <div className="chat-list__empty">
                {searching ? `没有匹配的对话 “${query.trim()}”` : "暂无对话"}
              </div>
            )}
            {sortSessionsNewestFirst(visibleChatSessions).map((session) => {
              const busy = sessionIsBusy(session.status);
              const park = suspendControl(session, session.label);
              const isActive = session.id === currentSessionId;
              const isRenaming = renamingId === session.id;
              const pinned = pinStamp(session) != null;
              const doneUnread = unreadDone.has(session.id);
              const beginRename = () => {
                if (!onRenameSession) return;
                setRenamingId(session.id);
                setRenameDraft(session.label);
              };
              const chatDot = (
                <span
                  className={`chat-row__dot${busy ? ` is-${session.status}` : doneUnread ? " is-done" : ""}`}
                  title={doneUnread ? "任务已完成" : undefined}
                  aria-label={doneUnread ? "任务已完成" : undefined}
                  aria-hidden={!doneUnread}
                />
              );
              return (
                <div
                  className={[
                    "chat-row",
                    isActive ? "is-active" : "",
                    busy ? "is-running" : "",
                    pinned ? "is-pinned" : "",
                    doneUnread ? "is-done" : "",
                  ].filter(Boolean).join(" ")}
                  key={session.id}
                >
                  {isRenaming ? (
                    <form
                      className="chat-row__rename"
                      onSubmit={(event) => {
                        event.preventDefault();
                        commitRename();
                      }}
                    >
                      {chatDot}
                      <input
                        ref={renameInputRef}
                        className="chat-row__rename-input"
                        value={renameDraft}
                        aria-label="Rename chat"
                        onChange={(event) => setRenameDraft(event.target.value)}
                        onBlur={() => commitRename()}
                        onKeyDown={(event) => {
                          if (event.key === "Escape") {
                            event.preventDefault();
                            setRenamingId(null);
                          }
                        }}
                      />
                    </form>
                  ) : (
                    <button
                      className="chat-row__select"
                      type="button"
                      title={
                        doneUnread
                          ? `${session.label} · 任务已完成`
                          : `${session.label} · ${session.agentId} · double-click or pencil to rename`
                      }
                      onClick={() => onChatSessionSelect(session)}
                      onDoubleClick={(event) => {
                        event.preventDefault();
                        event.stopPropagation();
                        beginRename();
                      }}
                    >
                      {chatDot}
                      <span className="chat-row__content">
                        {pinned && <PinnedMark className="chat-row__pin" />}
                        <strong>{session.label}</strong>
                      </span>
                    </button>
                  )}
                  <span className="chat-row__actions">
                    {onToggleSessionPin && !isRenaming && (
                      <SessionPinButton
                        label={session.label}
                        pinned={pinned}
                        className="chat-row__action"
                        onToggle={(next) => onToggleSessionPin(session.id, next)}
                      />
                    )}
                    {onRenameSession && !isRenaming && (
                      <button
                        className="chat-row__action"
                        type="button"
                        title={`Rename ${session.label}`}
                        aria-label={`Rename ${session.label}`}
                        onClick={(event) => {
                          event.stopPropagation();
                          beginRename();
                        }}
                      >
                        <Pencil size={12} />
                      </button>
                    )}
                    {park.shown && onSuspendSession && !isRenaming && (
                      <button
                        className="chat-row__action"
                        type="button"
                        title={park.title}
                        aria-label={park.ariaLabel}
                        onClick={(event) => {
                          event.stopPropagation();
                          onSuspendSession(session.id);
                        }}
                      >
                        <Pause size={12} />
                      </button>
                    )}
                    <button
                      className="chat-row__action chat-row__action--danger"
                      type="button"
                      title={`Delete ${session.label}`}
                      aria-label={`Delete ${session.label}`}
                      onClick={(event) => {
                        event.stopPropagation();
                        setPendingDelete({ kind: "chat", id: session.id, label: session.label });
                      }}
                    >
                      <Trash2 size={12} />
                    </button>
                  </span>
                </div>
              );
            })}
          </div>
        </section>
      )}

      <div className="sidebar-footer">
        {themeButton}
        {notifyButton}
        {proxyButton}
        {gatewayButton}
        <button className="pill-action pill-action--icon pill-action--sm sidebar-footer__button sidebar-footer__button--collapse" type="button" title={collapsed ? "Pin projects and sessions open" : "Collapse projects and sessions"} aria-label={collapsed ? "Pin projects and sessions open" : "Collapse projects and sessions"} onClick={collapsed ? onExpand : onCollapse}>
          {collapsed ? <PanelLeftOpen size={13} /> : <PanelLeftClose size={13} />}
        </button>
      </div>
      {proxyOpen && proxyAnchor && (
        <div
          ref={proxyPopRef}
          className="proxy-popover"
          role="dialog"
          aria-label="代理设置"
          style={{ left: proxyAnchor.left, bottom: window.innerHeight - proxyAnchor.top + 6 }}
        >
          <div className="proxy-popover__header">
            <span className="proxy-popover__title">代理设置</span>
            <button
              type="button"
              className="pill-action pill-action--icon pill-action--sm"
              aria-label="关闭"
              onClick={() => setProxyOpen(false)}
            >
              <X size={12} />
            </button>
          </div>
          <div className="proxy-popover__row">
            <button
              type="button"
              role="switch"
              aria-checked={proxyDraft.enabled}
              className={proxyDraft.enabled ? "proxy-popover__switch is-on" : "proxy-popover__switch"}
              onClick={() => setProxyDraft((draft) => ({ ...draft, enabled: !draft.enabled }))}
            >
              <span className="proxy-popover__switch-track" />
              <span className="proxy-popover__switch-thumb" />
            </button>
            <span className="proxy-popover__row-label">启用代理</span>
          </div>
          <div className="proxy-popover__row">
            <label className="proxy-popover__field">
              <span className="proxy-popover__field-label">代理地址</span>
              <input
                className="proxy-popover__input"
                type="text"
                spellCheck={false}
                placeholder="http://127.0.0.1:7890"
                value={proxyDraft.url}
                disabled={!proxyDraft.enabled}
                onChange={(event) => setProxyDraft((draft) => ({ ...draft, url: event.target.value }))}
                onKeyDown={(event) => {
                  if (event.key === "Enter") void saveProxy();
                }}
              />
            </label>
          </div>
          {proxyTest && <div className="proxy-popover__test is-ok">{proxyTest}</div>}
          {proxyError && <div className="proxy-popover__test is-err">{proxyError}</div>}
          <div className="proxy-popover__hint">本地地址自动直连；规则/全局由你的代理客户端决定。</div>
          <div className="proxy-popover__actions">
            <button
              type="button"
              className="pill-action pill-action--icon pill-action--sm"
              title="测试连接"
              aria-label="测试连接"
              disabled={proxyBusy !== null || !proxyDraft.enabled}
              onClick={() => void runProxyTest()}
            >
              {proxyBusy === "test" ? <span className="proxy-popover__spinner" aria-hidden /> : <Zap size={13} />}
            </button>
            <button
              type="button"
              className="pill-action pill-action--icon pill-action--sm"
              title="保存设置"
              aria-label="保存设置"
              disabled={proxyBusy !== null}
              onClick={() => void saveProxy()}
            >
              {proxyBusy === "save" ? <span className="proxy-popover__spinner" aria-hidden /> : <Save size={13} />}
            </button>
          </div>
        </div>
      )}
      {engineOpen && engineAnchor && (
        <div
          ref={enginePopRef}
          className="proxy-popover engine-popover"
          role="dialog"
          aria-label="跨用网关"
          style={{ left: engineAnchor.left, bottom: window.innerHeight - engineAnchor.top + 6 }}
        >
          <div className="proxy-popover__header">
            <span className="proxy-popover__title">跨用网关</span>
            <button
              type="button"
              className="pill-action pill-action--icon pill-action--sm"
              aria-label="关闭"
              onClick={() => setEngineOpen(false)}
            >
              <X size={12} />
            </button>
          </div>
          <p className="engine-popover__status" data-state={engineState?.state ?? "unknown"}>
            {engineStatusLine}
          </p>
          {engineRunning ? (
            <>
              <div className="proxy-popover__hint">
                Grok / Claude / ChatGPT 订阅与本机 key 已并入；各 agent 模型菜单的「跨用」组里直接选用。
              </div>
              {engineState?.reused ? (
                <div className="proxy-popover__hint">
                  复用的自装实例不由 Marionette 停止（在它自己的终端里管理）。
                </div>
              ) : (
                <button
                  type="button"
                  className="engine-popover__stop"
                  title="停止网关进程；正在跨用的会话会断流，随时可从任何跨用入口再次唤醒"
                  onClick={() => {
                    void engineStop().then(() => refreshEngine());
                  }}
                >
                  停止网关
                </button>
              )}
            </>
          ) : (
            <button
              type="button"
              className="engine-popover__start"
              disabled={engineBusyTransient}
              onClick={() => {
                void engineEnable().then((snapshot) => {
                  if (snapshot) setEngineState(snapshot);
                  refreshEngine();
                });
              }}
            >
              {engineBusyTransient ? "准备中…" : engineState?.state === "failed" ? "重试启动" : "启动网关"}
            </button>
          )}
          <div className="engine-popover__footer">
            <button
              type="button"
              className="engine-popover__link"
              onClick={() => {
                setEngineOpen(false);
                setCredentialsOpen(true);
              }}
            >
              <KeyRound size={11} aria-hidden /> 管理密钥与来源…
            </button>
          </div>
        </div>
      )}
      </div>
    </section>
    {credentialsOpen && (
      <CredentialsDialog agents={agents} onClose={() => setCredentialsOpen(false)} />
    )}
    {pendingDelete && (
      <ConfirmDialog
        title={
          pendingDelete.kind === "project"
            ? "移除项目"
            : pendingDelete.kind === "chat"
              ? "删除对话"
              : "Delete session"
        }
        itemName={pendingDelete.label}
        description={
          pendingDelete.kind === "project"
            ? "仅从列表中移除，不会删除磁盘上的文件。"
            : pendingDelete.kind === "chat"
              ? "这会删除本地对话记录，且无法撤销。"
              : "This removes its local transcript and cannot be undone."
        }
        confirmLabel={
          pendingDelete.kind === "project"
            ? "移除"
            : pendingDelete.kind === "session"
              ? "Delete"
              : "删除"
        }
        onCancel={() => setPendingDelete(null)}
        onConfirm={() => {
          const target = pendingDelete;
          setPendingDelete(null);
          if (target.kind === "project") onDeleteProject(target.id);
          else onDeleteSession(target.id);
        }}
      />
    )}
    </>
  );
}
