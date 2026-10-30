import { Fragment, useCallback, useEffect, useMemo, useRef, useState, type PointerEvent as ReactPointerEvent, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { listen } from "@tauri-apps/api/event";
import { Archive, Bell, BellOff, ChevronRight, CloudMoon, Folder, FolderOpen, Globe, GripVertical, KeyRound, Languages, MessageSquare, Network, PanelLeftClose, PanelLeftOpen, Pause, Pencil, Pin, PinOff, Plus, RefreshCw, Save, Search, Send, SlidersHorizontal, SunMedium, Trash2, Users, X, Zap } from "lucide-react";
// Refresh on a project row folds CLI history into the normal session list.
import type { AgentConfig, Project, ProxyConfig, ProxyTestResult, Session } from "../lib/types";
import { applyLocale, getLocale, localeToggleTitle, nextLocale } from "../lib/i18n";
import { t, useLocale, useT, type DictKey } from "../lib/i18n";
import { themeToggleTitle, type ThemeMode } from "../lib/theme";
import { collapsedShelfSessions, pinStamp, sortSessionsNewestFirst } from "../lib/sessionOrder";
import { suspendControl } from "../lib/sessionSuspend";
import { showToast } from "../lib/toast";
import { loadCollapsedProjectIds, saveCollapsedProjectIds } from "../lib/uiRestore";
import {
  engineCheckUpdate,
  engineEnable,
  engineRestoreFactory,
  engineStatus,
  type EngineDownloadProgress,
  engineStop,
  engineUpdate,
  telegramAcceptPairing,
  telegramDismissPairing,
  telegramResetPairing,
  telegramSave,
  telegramStatus,
  type EngineStatus,
  type EngineUpdateInfo,
  type TelegramPrefs,
  type TelegramStatus,
} from "../lib/api";
import { ConfirmDialog } from "./ConfirmDialog";
import { CredentialsDialog } from "./CredentialsDialog";
import { ModelLibraryDialog } from "./ModelLibraryDialog";
import { SplitBesideMenu, type OpenBesideEdge } from "./SplitBesideMenu";
import type { DropEdge } from "../lib/splitWorkspace";
import { dockFooterHost, useDocked } from "../lib/dockFooter";

/**
 * Docked mode: the footer buttons are portaled below the composer. Their
 * popovers travel with them — inside a `translateX(100%)` rail overlay a
 * `position: fixed` popover would be parked off-screen too.
 */
function DockedLayer({ docked, children }: { docked: boolean; children: ReactNode }) {
  return docked ? createPortal(children, document.body) : <>{children}</>;
}

/** Visible sessions per project before "Show more". Chat/memo lists use the same count as a scroll window. */
const SESSION_LIST_PREVIEW = 5;
const EMPTY_DONE_UNREAD: ReadonlySet<string> = new Set();

/** Per-project memo cache: a keyed derived value recomputes only when a dep changes. */
function memoPerProject<T>(
  cache: Map<string, { deps: unknown[]; value: unknown }>,
  key: string,
  deps: unknown[],
  compute: () => T,
): T {
  const hit = cache.get(key);
  if (hit && hit.deps.length === deps.length && hit.deps.every((dep, i) => dep === deps[i])) {
    return hit.value as T;
  }
  const value = compute();
  cache.set(key, { deps, value });
  return value;
}

function magpieCredit(version?: string | null): string {
  const raw = version?.trim();
  if (!raw) return t("shelf.poweredBy");
  return t("shelf.poweredByVersion", { version: raw.replace(/^v/i, "") });
}

/** Pin toggle shared by project session rows and chat rows. */
function SessionPinButton({
  pinned,
  className,
  onToggle,
}: {
  pinned: boolean;
  className: string;
  onToggle: (next: boolean) => void;
}) {
  const t = useT();
  const name = pinned ? t("shelf.unpin") : t("shelf.pin");
  return (
    <button
      className={pinned ? `${className} is-pinned` : className}
      type="button"
      title={name}
      aria-label={name}
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
  const t = useT();
  return (
    <span className={className} title={t("shelf.pinned")} aria-hidden>
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
  /** Split this session to the right or below the focused pane. */
  onOpenBeside?: (session: Session, edge: DropEdge) => void;
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
  /** 收纳盒-grown dialogs (origin "memo") — their own section below 聊天. */
  memoSessions?: Session[];
  /** Open a chat session from the Chat section. */
  onChatSessionSelect?: (session: Session) => void;
  /** Create a new chat session. */
  onNewChat?: () => void;
  /** Open the memo page from the entry above the Chat section. */
  onOpenMemo?: () => void;
  /** Council Mode(多模型审议)整页入口。 */
  onOpenCouncil?: () => void;
  /** Scan this project's agent CLI stores into the normal session list. */
  externalScanning?: boolean;
  onRefreshExternal?: (projectId: string) => void;
};

type DropHint = { targetId: string; place: "before" | "after" };

/** Shelf recency: `now`, `5m`, `3h`, `2d`, then `Oct 18` — fits the row's tail. */
function shelfRecency(iso: string | undefined, nowMs: number): string {
  const ms = iso ? Date.parse(iso) : NaN;
  if (!Number.isFinite(ms)) return "";
  const min = Math.floor((nowMs - ms) / 60_000);
  if (min < 1) return "now";
  if (min < 60) return `${min}m`;
  const hr = Math.floor(min / 60);
  if (hr < 24) return `${hr}h`;
  const day = Math.floor(hr / 24);
  if (day < 7) return `${day}d`;
  return new Date(ms).toLocaleDateString(undefined, { month: "short", day: "numeric" });
}

function sessionIsBusy(status: Session["status"]): boolean {
  return status === "running" || status === "starting";
}

/** Day bucket for the shelf's group labels; pinned rows keep their own group. */
function shelfDayLabel(session: Session, nowMs: number): string {
  if (pinStamp(session) != null) return t("shelf.pin");
  const ms = Date.parse(session.lastActiveAt);
  if (!Number.isFinite(ms)) return t("common.earlier");
  const start = new Date(nowMs);
  start.setHours(0, 0, 0, 0);
  const today = start.getTime();
  if (ms >= today) return t("common.today");
  if (ms >= today - 86_400_000) return t("common.yesterday");
  if (ms >= today - 6 * 86_400_000) return t("common.thisWeek");
  return t("common.earlier");
}

/** First row of each day bucket → its label; only long lists get labels. */
function shelfDayHeaders(list: Session[], nowMs: number, minRows: number): Map<string, string> {
  const heads = new Map<string, string>();
  if (list.length <= minRows) return heads;
  let last = "";
  for (const session of list) {
    const label = shelfDayLabel(session, nowMs);
    if (label !== last) heads.set(session.id, label);
    last = label;
  }
  return heads;
}

const SESSION_STATUS_WORD: Record<string, DictKey> = {
  starting: "shelf.starting",
  running: "shelf.running",
  waiting: "shelf.waiting",
  exited: "shelf.idle",
  error: "shelf.error",
};

/** Shelf-top filter: which dialogs need a look right now. */
type ShelfStatusFilter = "running" | "waiting" | "done" | "error";

const SHELF_STATUS_ORDER: { id: ShelfStatusFilter; label: DictKey }[] = [
  { id: "running", label: "shelf.running" },
  { id: "waiting", label: "shelf.waiting" },
  { id: "done", label: "shelf.done" },
  { id: "error", label: "shelf.error" },
];

function sessionMatchesStatus(
  session: Session,
  filter: ShelfStatusFilter,
  doneUnread: ReadonlySet<string>,
): boolean {
  switch (filter) {
    case "running":
      return sessionIsBusy(session.status);
    case "waiting":
      return session.status === "waiting";
    case "done":
      return doneUnread.has(session.id) && !sessionIsBusy(session.status);
    case "error":
      return session.status === "error";
  }
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
  onOpenBeside,
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
  defaultFolderPath = "",
  chatSessions = [],
  memoSessions = [],
  onChatSessionSelect,
  onNewChat,
  onOpenMemo,
  onOpenCouncil,
  externalScanning = false,
  onRefreshExternal,
}: ProjectShelfProps) {
  const unreadDone = doneUnreadIds ?? EMPTY_DONE_UNREAD;
  const t = useT();
  const locale = useLocale();
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
  const [besideMenu, setBesideMenu] = useState<{
    session: Session;
    x: number;
    y: number;
  } | null>(null);

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
      setProxyError(t("shelf.proxyNeedUrlFirst"));
      return;
    }
    setProxyBusy("test");
    setProxyError(null);
    setProxyTest(null);
    try {
      const result = await onTestProxy(url);
      setProxyTest(result ? result.message : t("shelf.proxyTestUnavailable"));
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
      setProxyError(t("shelf.proxyNeedUrl"));
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
  const [modelLibraryOpen, setModelLibraryOpen] = useState(false);

  /**
   * Cross-use gateway (engine) popover. The engine auto-starts with the app
   * (2026-10-16 product decision: open the app and everything works); this
   * button is its always-visible status + one-click start, NOT a dialog —
   * key management stays one hop deeper (footer link → CredentialsDialog).
   */
  const [engineOpen, setEngineOpen] = useState(false);
  const [engineUpdateInfo, setEngineUpdateInfo] = useState<EngineUpdateInfo | null>(null);
  const [engineUpdateBusy, setEngineUpdateBusy] = useState<"check" | "update" | "restore" | null>(
    null,
  );
  const [engineUpdateError, setEngineUpdateError] = useState("");
  const [engineAnchor, setEngineAnchor] = useState<{ left: number; top: number } | null>(null);
  const [engineState, setEngineState] = useState<EngineStatus | null>(null);
  const engineBtnRef = useRef<HTMLButtonElement | null>(null);
  const enginePopRef = useRef<HTMLDivElement | null>(null);

  /**
   * Telegram 远程桥（docs/telegram-bridge.md）。按钮常显状态，浮层承担：
   * token 粘贴（getMe 校验）→ 配对确认（bot 收到第一条私聊消息后挂起，
   * 1Hz 轮询把它带进浮层）→ 通知偏好开关。所有动作直连 api，不经 App。
   */
  const [tgOpen, setTgOpen] = useState(false);
  const [tgAnchor, setTgAnchor] = useState<{ left: number; top: number } | null>(null);
  const [tgState, setTgState] = useState<TelegramStatus | null>(null);
  const [tgTokenDraft, setTgTokenDraft] = useState("");
  const [tgPrefsDraft, setTgPrefsDraft] = useState<TelegramPrefs | null>(null);
  const [tgBusy, setTgBusy] = useState<"save" | "pair" | "prefs" | null>(null);
  const [tgError, setTgError] = useState<string | null>(null);
  const tgBtnRef = useRef<HTMLButtonElement | null>(null);
  const tgPopRef = useRef<HTMLDivElement | null>(null);

  const refreshTg = useCallback(() => {
    void telegramStatus().then((next) => {
      if (next) {
        setTgState(next);
        // 偏好草稿只在浮层关闭时跟随外部变化（和 proxy 草稿同款约定）。
        setTgPrefsDraft((current) => current ?? { ...next.prefs });
      }
    });
  }, []);
  useEffect(refreshTg, [refreshTg]);

  /** Open popover: anchor + 1Hz status polling（等待配对请求出现）。 */
  useEffect(() => {
    if (!tgOpen) {
      setTgPrefsDraft(null);
      return;
    }
    refreshTg();
    const timer = window.setInterval(refreshTg, 1000);
    const onDown = (event: MouseEvent) => {
      const target = event.target as Node;
      if (tgPopRef.current?.contains(target) || tgBtnRef.current?.contains(target)) return;
      setTgOpen(false);
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") setTgOpen(false);
    };
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      window.clearInterval(timer);
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [tgOpen, refreshTg]);

  const tgSaveToken = async () => {
    const token = tgTokenDraft.trim();
    if (!token) {
      setTgError(t("shelf.tgNeedToken"));
      return;
    }
    setTgBusy("save");
    setTgError(null);
    try {
      const next = await telegramSave({ token });
      setTgState(next);
      setTgTokenDraft("");
      setTgPrefsDraft({ ...next.prefs });
    } catch (error) {
      setTgError(error instanceof Error ? error.message : String(error));
    } finally {
      setTgBusy(null);
    }
  };

  const tgSetEnabled = async (enabled: boolean) => {
    setTgBusy("save");
    setTgError(null);
    try {
      const next = await telegramSave({ enabled });
      setTgState(next);
    } catch (error) {
      setTgError(error instanceof Error ? error.message : String(error));
    } finally {
      setTgBusy(null);
    }
  };

  const tgSavePrefs = async (patch: Partial<TelegramPrefs>) => {
    if (!tgPrefsDraft) return;
    const prefs = { ...tgPrefsDraft, ...patch };
    setTgPrefsDraft(prefs);
    setTgBusy("prefs");
    setTgError(null);
    try {
      const next = await telegramSave({ prefs });
      setTgState(next);
    } catch (error) {
      setTgError(error instanceof Error ? error.message : String(error));
    } finally {
      setTgBusy(null);
    }
  };

  const tgPair = async (accept: boolean) => {
    setTgBusy("pair");
    setTgError(null);
    try {
      const next = accept ? await telegramAcceptPairing() : await telegramDismissPairing();
      setTgState(next);
    } catch (error) {
      setTgError(error instanceof Error ? error.message : String(error));
    } finally {
      setTgBusy(null);
    }
  };

  const tgUnpair = async () => {
    setTgBusy("pair");
    setTgError(null);
    try {
      const next = await telegramResetPairing();
      setTgState(next);
    } catch (error) {
      setTgError(error instanceof Error ? error.message : String(error));
    } finally {
      setTgBusy(null);
    }
  };

  const refreshEngine = useCallback(() => {
    void engineStatus().then(setEngineState);
  }, []);
  useEffect(refreshEngine, [refreshEngine]);

  // 引擎在后台起步/就绪/回收时状态行自动跟随——曾经只在挂载时拉一次，
  // 后台拉起完成后 UI 停在「启动中」，要手动点一下才刷（2026-10-31 用户
  // 实锤）。下载进度是独立事件，合并进 downloading 态。
  useEffect(() => {
    let disposed = false;
    const unlisteners: Array<() => void> = [];
    const keep = (fn: () => void) => {
      if (disposed) fn();
      else unlisteners.push(fn);
    };
    void listen<EngineStatus>("engine-state-changed", (event) => {
      if (event.payload) setEngineState(event.payload);
    }).then(keep);
    void listen<EngineDownloadProgress>("engine-download-progress", (event) => {
      setEngineState((prev) =>
        prev?.state === "downloading" && event.payload
          ? { ...prev, downloaded: event.payload.downloaded, total: event.payload.total }
          : prev,
      );
    }).then(keep);
    return () => {
      disposed = true;
      for (const fn of unlisteners) fn();
    };
  }, []);

  const handleEngineCheckUpdate = useCallback(async () => {
    setEngineUpdateBusy("check");
    setEngineUpdateError("");
    try {
      const info = await engineCheckUpdate();
      if (!info) {
        setEngineUpdateError(t("shelf.updateCheckUnavailable"));
        return;
      }
      setEngineUpdateInfo(info);
      if (info.note) setEngineUpdateError(info.note);
    } catch (error) {
      setEngineUpdateError(error instanceof Error ? error.message : String(error));
    } finally {
      setEngineUpdateBusy(null);
    }
  }, []);

  const handleEngineUpdate = useCallback(async () => {
    setEngineUpdateBusy("update");
    setEngineUpdateError("");
    try {
      const snapshot = await engineUpdate();
      if (snapshot) setEngineState(snapshot);
      setEngineUpdateInfo((prev) =>
        prev
          ? {
              ...prev,
              updateAvailable: false,
              currentVersion: prev.latestVersion ?? prev.currentVersion,
            }
          : prev,
      );
      refreshEngine();
    } catch (error) {
      setEngineUpdateError(error instanceof Error ? error.message : String(error));
    } finally {
      setEngineUpdateBusy(null);
    }
  }, [refreshEngine]);

  const handleEngineRestoreFactory = useCallback(async () => {
    setEngineUpdateBusy("restore");
    setEngineUpdateError("");
    try {
      const snapshot = await engineRestoreFactory();
      if (snapshot) setEngineState(snapshot);
      setEngineUpdateInfo((prev) =>
        prev
          ? {
              ...prev,
              canRestoreFactory: false,
              currentVersion: prev.embeddedVersion ?? prev.currentVersion,
              updateAvailable: Boolean(
                prev.latestVersion &&
                  prev.embeddedVersion &&
                  prev.latestVersion !== prev.embeddedVersion,
              ),
            }
          : prev,
      );
      refreshEngine();
    } catch (error) {
      setEngineUpdateError(error instanceof Error ? error.message : String(error));
    } finally {
      setEngineUpdateBusy(null);
    }
  }, [refreshEngine]);

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
  /** Hover preview: which row, anchored where. Opens after a short dwell. */
  const [hoverCard, setHoverCard] = useState<{ session: Session; top: number; left: number } | null>(null);
  const hoverTimerRef = useRef<number | undefined>(undefined);
  useEffect(() => () => window.clearTimeout(hoverTimerRef.current), []);
  const hoverRow = (session: Session, el: HTMLElement | null) => {
    window.clearTimeout(hoverTimerRef.current);
    if (!el) {
      setHoverCard(null);
      return;
    }
    hoverTimerRef.current = window.setTimeout(() => {
      const rect = el.getBoundingClientRect();
      setHoverCard({ session, top: rect.top, left: rect.right + 8 });
    }, 350);
  };
  const [renameDraft, setRenameDraft] = useState("");
  const renameInputRef = useRef<HTMLInputElement>(null);
  /** Pending destructive action — rendered as an in-app window instead of window.confirm. */
  const [pendingDelete, setPendingDelete] = useState<
    { kind: "project" | "session" | "chat"; id: string; label: string } | null
  >(null);
  /**
   * 内联删除确认（2026-10-31）：会话/对话行的删除不再弹窗——第一次点删除
   * 按钮原地变红进入「确认删除」态，再点即删，鼠标离开该行自动复位。连点
   * 多行清理时不用反复关弹窗。项目移除仍走弹窗（低频且更重）。
   */
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);
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
  /** Latest grip pointer position; the drop-target check runs once per frame. */
  const gripPointRef = useRef({ clientX: 0, clientY: 0 });
  const dragFrameRef = useRef(0);
  /** Per-project derived-list cache (filter+sort once per input change, not per render). */
  const projectShelfCacheRef = useRef(new Map<string, { deps: unknown[]; value: unknown }>());

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

  const [statusFilter, setStatusFilter] = useState<ShelfStatusFilter | null>(null);
  const statusCounts = useMemo(() => {
    const counts: Record<ShelfStatusFilter, number> = { running: 0, waiting: 0, done: 0, error: 0 };
    for (const session of [...sessions, ...chatSessions, ...memoSessions]) {
      for (const { id } of SHELF_STATUS_ORDER) {
        if (sessionMatchesStatus(session, id, unreadDone)) counts[id] += 1;
      }
    }
    return counts;
  }, [sessions, chatSessions, memoSessions, unreadDone]);
  // A filter whose count drops to zero (the run finished) lets go by itself.
  const activeStatusFilter = statusFilter && statusCounts[statusFilter] > 0 ? statusFilter : null;
  // 归零不只是显示上放手——状态本身也要清掉。旧实现把 statusFilter 留在
  // state 里：下次任何对话再进入该状态，过滤会一声不响地重新挂上，用户
  // 看到的就是「列表突然又少了一截」（2026-10-25 反馈）。
  useEffect(() => {
    if (statusFilter && statusCounts[statusFilter] === 0) setStatusFilter(null);
  }, [statusFilter, statusCounts]);

  /** Search / status filter always reveal matching projects; otherwise respect saved collapse. */
  const isProjectExpanded = (projectId: string) =>
    Boolean(query.trim()) || activeStatusFilter != null || !collapsedProjects.has(projectId);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    // 纯状态过滤（无搜索词）：项目行全部保留，没有匹配的置灰——把项目整行
    // 藏掉会让用户以为项目/对话丢了（2026-10-25 反馈）。搜索仍然只留命中项。
    if (!q && activeStatusFilter) {
      const sessionsByProject = new Map<string, Session[]>();
      for (const session of sessions) {
        if (sessionMatchesStatus(session, activeStatusFilter, unreadDone)) {
          const list = sessionsByProject.get(session.projectId) ?? [];
          list.push(session);
          sessionsByProject.set(session.projectId, list);
        }
      }
      return { projects, sessionsByProject };
    }
    if (!q && !activeStatusFilter) {
      return { projects, sessionsByProject: null as Map<string, Session[]> | null };
    }
    const statusOk = (session: Session) =>
      !activeStatusFilter || sessionMatchesStatus(session, activeStatusFilter, unreadDone);
    const sessionsByProject = new Map<string, Session[]>();
    const projectIds = new Set<string>();

    // Titles only: the box filters the names drawn in the shelf (project name,
    // session title). Transcript content is out of scope here — matching it
    // meant reading every transcript on each keystroke.
    for (const session of sessions) {
      if (statusOk(session) && (!q || session.label.toLowerCase().includes(q))) {
        projectIds.add(session.projectId);
        const list = sessionsByProject.get(session.projectId) ?? [];
        list.push(session);
        sessionsByProject.set(session.projectId, list);
      }
    }

    for (const project of projects) {
      if (q && project.name.toLowerCase().includes(q)) {
        const own = sessions.filter((s) => s.projectId === project.id && statusOk(s));
        // Under a status filter a name match alone does not list the project.
        if (activeStatusFilter && own.length === 0) continue;
        projectIds.add(project.id);
        if (!sessionsByProject.has(project.id)) sessionsByProject.set(project.id, own);
      }
    }

    return {
      projects: projects.filter((p) => projectIds.has(p.id)),
      sessionsByProject,
    };
  }, [projects, query, sessions, activeStatusFilter, unreadDone]);

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
    if (dragFrameRef.current) {
      window.cancelAnimationFrame(dragFrameRef.current);
      dragFrameRef.current = 0;
    }
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
    if (dragFrameRef.current) {
      window.cancelAnimationFrame(dragFrameRef.current);
      dragFrameRef.current = 0;
    }
  };

  const computeGripDropHint = () => {
    dragFrameRef.current = 0;
    const drag = projectDragRef.current;
    if (!drag || !drag.armed) return;
    const { clientX, clientY } = gripPointRef.current;
    // elementFromPoint under capture still sees targets below the grip.
    const el = document.elementFromPoint(clientX, clientY);
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
      clientY < box.top + box.height / 2 ? "before" : "after";
    const next: DropHint = { targetId: overId, place };
    const prev = dropHintRef.current;
    if (!prev || prev.targetId !== next.targetId || prev.place !== next.place) {
      dropHintRef.current = next;
      setDropHint(next);
    }
  };

  const onProjectGripPointerMove = (event: ReactPointerEvent<HTMLButtonElement>) => {
    const drag = projectDragRef.current;
    if (!drag || drag.pointerId !== event.pointerId) return;
    const dist = Math.hypot(event.clientX - drag.startX, event.clientY - drag.startY);
    if (!drag.armed && dist < 4) return;
    drag.armed = true;
    // Keep the latest pointer position; the drop-target computation coalesces per frame.
    gripPointRef.current = { clientX: event.clientX, clientY: event.clientY };
    if (dragFrameRef.current === 0) {
      dragFrameRef.current = window.requestAnimationFrame(computeGripDropHint);
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

  const languageButton = (
    <button
      className="pill-action pill-action--icon pill-action--sm sidebar-footer__button"
      type="button"
      title={localeToggleTitle(getLocale())}
      aria-label={localeToggleTitle(getLocale())}
      onClick={() => void applyLocale(nextLocale(getLocale()))}
    >
      <Languages size={13} />
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
          ? t("shelf.proxyTitleOn", { url: proxyConfig.url })
          : t("shelf.proxyTitleOff")
      }
      aria-label={proxyConfig?.enabled ? t("shelf.proxyAriaOn") : t("shelf.proxyAriaOff")}
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
    if (!engineState) return t("shelf.engineStateUnknown");
    switch (engineState.state) {
      case "running":
        return t("shelf.engineRunningAt", { port: engineState.port });
      case "downloading": {
        const { downloaded, total } = engineState;
        const mb = (n: number) => `${(n / 1024 / 1024).toFixed(1)} MB`;
        return total > 0
          ? t("shelf.engineDownloading", {
              done: mb(downloaded),
              total: mb(total),
              pct: Math.min(100, Math.round((downloaded / total) * 100)),
            })
          : t("shelf.downloadingSoon");
      }
      case "starting":
        return t("shelf.startingSoon");
      case "failed":
        return t("shelf.engineFailedWith", { error: engineState.error.slice(0, 120) });
      default:
        return t("shelf.engineOff");
    }
  })();

  /** running → on, downloading/starting → busy, failed → fail, else off. */
  const engineTone =
    engineState?.state === "running"
      ? "on"
      : engineBusyTransient
        ? "busy"
        : engineState?.state === "failed"
          ? "fail"
          : "off";
  const engineHeadline =
    engineTone === "on"
      ? t("shelf.running")
      : engineState?.state === "downloading"
        ? t("shelf.downloading")
        : engineState?.state === "starting"
          ? t("shelf.starting")
          : engineTone === "fail"
            ? t("shelf.engineFailed")
            : t("shelf.engineOff");
  const engineEntryHint = (() => {
    if (!engineState) return "";
    switch (engineState.state) {
      case "running":
        return t("shelf.engineRunningPort", { port: engineState.port });
      case "downloading":
        return engineState.total > 0
          ? t("shelf.engineDownloadPct", {
              pct: Math.min(100, Math.round((engineState.downloaded / engineState.total) * 100)),
            })
          : t("shelf.downloadingSoon");
      case "starting":
        return t("shelf.startingSoon");
      case "failed":
        return t("shelf.engineFailed");
      default:
        return t("shelf.engineClickToStart");
    }
  })();

  const gatewayRow = (
    <button
      ref={engineBtnRef}
      type="button"
      className={`engine-entry is-${engineTone}${engineOpen ? " is-open" : ""}`}
      aria-label={t("shelf.modelShareStatus", { state: engineHeadline })}
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
            // The popover may be gone by now; say how the start went.
            if (snapshot?.state === "running") showToast({ text: t("shelf.modelShareStarted"), tone: "ok" });
            else if (snapshot === null || snapshot.state === "failed")
              showToast({ text: t("shelf.modelShareFailed"), detail: t("shelf.modelShareFailedDetail"), tone: "error" });
          });
        }
      }}
    >
      <Network size={13} aria-hidden className="engine-entry__icon" />
      <span className="engine-entry__label">{t("shelf.modelShare")}</span>
      <span className="engine-entry__dot" aria-hidden />
      <span className="engine-entry__hint">{engineEntryHint}</span>
    </button>
  );

  const tgActive = Boolean(tgState?.enabled);
  const telegramButton = (
    <button
      ref={tgBtnRef}
      type="button"
      className={
        tgActive
          ? "pill-action pill-action--icon pill-action--sm sidebar-footer__button is-notify-on"
          : "pill-action pill-action--icon pill-action--sm sidebar-footer__button is-notify-off"
      }
      title={
        tgActive
          ? t("shelf.tgTitleOn", {
              bot: tgState?.botUsername ? ` · @${tgState.botUsername}` : "",
            })
          : t("shelf.tgTitleOff")
      }
      aria-label={tgActive ? t("shelf.tgAriaOn") : t("shelf.tgAriaOff")}
      aria-pressed={tgActive}
      aria-haspopup="dialog"
      aria-expanded={tgOpen}
      onClick={() => {
        const rect = tgBtnRef.current?.getBoundingClientRect();
        setTgAnchor(rect ? { left: rect.left, top: rect.top } : null);
        setTgError(null);
        setTgOpen((open) => !open);
      }}
    >
      <Send size={13} />
    </button>
  );

  /**
   * The rail's bottom block. Docked it is portaled into the strip's footer bar
   * (below the composer); the collapse chevron only makes sense in the rail, so
   * it stays here.
   */
  const docked = useDocked();
  const dockFooter = (
    <div className="sidebar-dock">
      {gatewayRow}
      <div className="sidebar-footer">
        {themeButton}
        {languageButton}
        {notifyButton}
        {proxyButton}
        {telegramButton}
        {!docked && (
          <button className="pill-action pill-action--icon pill-action--sm sidebar-footer__button sidebar-footer__button--collapse" type="button" title={collapsed ? "Pin projects and sessions open" : "Collapse projects and sessions"} aria-label={collapsed ? "Pin projects and sessions open" : "Collapse projects and sessions"} onClick={collapsed ? onExpand : onCollapse}>
            {collapsed ? <PanelLeftOpen size={13} /> : <PanelLeftClose size={13} />}
          </button>
        )}
      </div>
    </div>
  );

  const searching = Boolean(query.trim()) || activeStatusFilter != null;
  const visibleChatSessions = useMemo(() => {
    if (!searching) return chatSessions;
    const q = query.trim().toLowerCase();
    return chatSessions.filter(
      (session) =>
        (!q || session.label.toLowerCase().includes(q)) &&
        (!activeStatusFilter || sessionMatchesStatus(session, activeStatusFilter, unreadDone)),
    );
  }, [chatSessions, query, searching, activeStatusFilter, unreadDone]);
  const visibleMemoSessions = useMemo(() => {
    if (!searching) return memoSessions;
    const q = query.trim().toLowerCase();
    return memoSessions.filter(
      (session) =>
        (!q || session.label.toLowerCase().includes(q)) &&
        (!activeStatusFilter || sessionMatchesStatus(session, activeStatusFilter, unreadDone)),
    );
  }, [memoSessions, query, searching, activeStatusFilter, unreadDone]);

  /** One chat-row, shared by the 聊天 section and the 收纳盒 dialog group. */
  const renderChatRow = (session: Session) => {
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
        title={doneUnread ? t("shelf.taskDone") : undefined}
        aria-label={doneUnread ? t("shelf.taskDone") : undefined}
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
        onContextMenu={(event) => {
          if (!onOpenBeside) return;
          event.preventDefault();
          event.stopPropagation();
          setBesideMenu({ session, x: event.clientX, y: event.clientY });
        }}
        onMouseLeave={() => {
          setConfirmDeleteId((cur) => (cur === session.id ? null : cur));
        }}
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
            onClick={() => {
              setStatusFilter(null);
              onChatSessionSelect?.(session);
            }}
            onDoubleClick={(event) => {
              event.preventDefault();
              event.stopPropagation();
              beginRename();
            }}
          >
            {chatDot}
            <span className="chat-row__content" title={session.label}>
              {pinned && <PinnedMark className="chat-row__pin" />}
              <strong>{session.label}</strong>
            </span>
          </button>
        )}
        <span className="chat-row__actions">
          {onToggleSessionPin && !isRenaming && (
            <SessionPinButton
              pinned={pinned}
              className="chat-row__action"
              onToggle={(next) => onToggleSessionPin(session.id, next)}
            />
          )}
          {onRenameSession && !isRenaming && (
            <button
              className="chat-row__action"
              type="button"
              title={t("shelf.rename")}
              aria-label={t("shelf.rename")}
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
              title={t("shelf.park")}
              aria-label={t("shelf.park")}
              onClick={(event) => {
                event.stopPropagation();
                onSuspendSession(session.id);
              }}
            >
              <Pause size={12} />
            </button>
          )}
          <button
            className={`chat-row__action chat-row__action--danger${confirmDeleteId === session.id ? " is-armed" : ""}`}
            type="button"
            title={confirmDeleteId === session.id ? t("shelf.confirmDelete") : t("common.delete")}
            aria-label={confirmDeleteId === session.id ? t("shelf.confirmDelete") : t("common.delete")}
            onClick={(event) => {
              event.stopPropagation();
              if (confirmDeleteId === session.id) {
                setConfirmDeleteId(null);
                onDeleteSession(session.id);
              } else {
                setConfirmDeleteId(session.id);
              }
            }}
          >
            {confirmDeleteId === session.id ? (
              <span className="row-action-confirm">{t("shelf.confirmDelete")}</span>
            ) : (
              <Trash2 size={12} />
            )}
          </button>
        </span>
      </div>
    );
  };
  const hasStatusCounts = SHELF_STATUS_ORDER.some(({ id }) => statusCounts[id] > 0);
  // Pin the Explorer "open here" project to the top of the shelf (still filterable).
  const allVisibleProjects = useMemo(() => {
    const list = filtered.projects;
    if (!pinnedProjectId) return list;
    const pinned = list.find((p) => p.id === pinnedProjectId);
    if (!pinned) return list;
    return [pinned, ...list.filter((p) => p.id !== pinnedProjectId)];
  }, [filtered.projects, pinnedProjectId]);
  // Projects always fully listed; only sessions inside a project use Show more/less.

  /**
   * A query that matched nothing anywhere in the shelf. The "no match" line is
   * shelf-wide but lives in the project list, so it must stay quiet while the
   * Chat list below is showing its own hits.
   */
  const noShelfMatch =
    searching && allVisibleProjects.length === 0 && visibleChatSessions.length === 0 && visibleMemoSessions.length === 0;

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
            aria-label="Search thread titles"
            onChange={(event) => setQuery(event.target.value)}
          />
        </label>
      </div>

      <div className="project-tree__body">
      {hasStatusCounts && (
        <div className="shelf-status" role="toolbar" aria-label={t("shelf.filterByStatus")}>
          {SHELF_STATUS_ORDER.filter(({ id }) => statusCounts[id] > 0).map(({ id, label }) => (
            <button
              key={id}
              type="button"
              className={`shelf-status__chip is-${id}${activeStatusFilter === id ? " is-active" : ""}`}
              aria-pressed={activeStatusFilter === id}
              title={activeStatusFilter === id ? t("shelf.showAll") : t("shelf.showOnly", { label: t(label) })}
              onClick={() => setStatusFilter(activeStatusFilter === id ? null : id)}
            >
              <span className="shelf-status__dot" aria-hidden />
              {t(label)}
              <span className="shelf-status__count">{statusCounts[id]}</span>
            </button>
          ))}
        </div>
      )}
      {activeStatusFilter && (
        <div className="shelf-filter-note">
          <span className="shelf-filter-note__text">
            {t("shelf.filterNote", {
              label: t(SHELF_STATUS_ORDER.find(({ id }) => id === activeStatusFilter)!.label),
              n: statusCounts[activeStatusFilter],
            })}
          </span>
          <button
            type="button"
            className="shelf-filter-note__clear"
            title={t("shelf.clearFilterAll")}
            aria-label={t("shelf.clearFilter")}
            onClick={() => setStatusFilter(null)}
          >
            <X size={12} />
          </button>
        </div>
      )}
      <button className="project-add" type="button" title="Add project" onClick={onAddProject}>
        <Plus size={13} />
        Add project
      </button>

      <div className="project-list custom-scrollbar scrollbar-autohide" aria-label="Projects with sessions">
        {allVisibleProjects.length === 0 && !searching && (
          <div className="project-list__empty">No projects yet</div>
        )}
        {noShelfMatch && (
          <div className="project-list__empty">
            {query.trim() ? `No threads match “${query.trim()}”` : t("shelf.noMatchInFilter")}
          </div>
        )}
        {allVisibleProjects.map((project) => {
          const derivedShelf = memoPerProject(
            projectShelfCacheRef.current,
            project.id,
            [filtered.sessionsByProject, sessions, unreadDone, sessionsExpandedByProject, searching, activeStatusFilter, Math.floor(Date.now() / 60000), locale],
            () => {
              const rawSessions = activeStatusFilter
                ? // 状态过滤下取不到 = 没有匹配（项目行置灰、不列会话）；搜索与
                  // 无过滤才回退全量。搜索路径 filtered 只含命中项目，不会到这。
                  (filtered.sessionsByProject?.get(project.id) ?? [])
                : sessions.filter((session) => session.projectId === project.id);
              // Pinned first, then recency — click-to-select must not jump the row.
              const projectSessions = sortSessionsNewestFirst(rawSessions);
              const sessionsFullyExpanded = searching || sessionsExpandedByProject.has(project.id);
              const shouldCollapseSessions =
                !sessionsFullyExpanded && projectSessions.length > SESSION_LIST_PREVIEW;
              const visibleSessions = shouldCollapseSessions
                ? collapsedShelfSessions(projectSessions, SESSION_LIST_PREVIEW, unreadDone)
                : projectSessions;
              const hiddenSessionCount = Math.max(0, projectSessions.length - visibleSessions.length);
              // Long, fully listed projects get 今天 / 昨天 / 本周 / 更早 labels.
              const dayHeads = shelfDayHeaders(visibleSessions, Date.now(), SESSION_LIST_PREVIEW);
              return { projectSessions, visibleSessions, hiddenSessionCount, dayHeads };
            },
          );
          const { projectSessions, visibleSessions, hiddenSessionCount, dayHeads } = derivedShelf;
          const projectExpanded = isProjectExpanded(project.id);
          const filteredOutByStatus = activeStatusFilter != null && projectSessions.length === 0;

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
            filteredOutByStatus ? "is-filtered-out" : "",
          ]
            .filter(Boolean)
            .join(" ");

          return (
            <div
              className={`project-group${filteredOutByStatus ? " is-filtered-out" : ""}`}
              key={project.id}
              data-project-id={project.id}
            >
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
                  <ChevronRight size={13} className={`project-row__chev${projectExpanded ? " is-open" : ""}`} />
                </button>
                <button
                  className="project-row__select"
                  type="button"
                  title={
                    isPinnedHere
                      ? t("shelf.hereTitle", { path: project.rootPath })
                      : project.rootPath
                  }
                  onClick={() => {
                    // 点具体项目/对话 = 「带我去这里」，状态过滤随之解除——
                    // 过滤是用来找东西的，不是要一直留着的视图（2026-10-25）。
                    setStatusFilter(null);
                    onProjectSelect(project.id);
                  }}
                >
                  <Folder size={15} />
                  <span className="project-row__content">
                    <strong>{project.name}</strong>
                    {isPinnedHere && (
                      <em className="project-row__here-badge" title={t("shelf.hereBadgeTitle")}>
                        {t("shelf.here")}
                      </em>
                    )}
                    {hasDoneUnread && (
                      <em className="project-row__done-badge" title={t("shelf.doneBadgeTitle")}>
                        {t("shelf.doneBadge")}
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
                      title={t("shelf.refreshExternal")}
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
                  {projectSessions.length === 0 && !activeStatusFilter && (
                    <div className="session-row session-row--empty">No sessions</div>
                  )}
                  {visibleSessions.map((session) => {
                    const dayLabel = dayHeads.get(session.id);
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
                        aria-label={session.status}
                      />
                    ) : doneUnread ? (
                      <span
                        className="session-row__dot is-done"
                        aria-label={t("shelf.taskDone")}
                      />
                    ) : (
                      <span
                        className={`session-row__dot is-${session.status}`}
                        aria-hidden
                      />
                    );
                    return (
                      <Fragment key={session.id}>
                      {dayLabel && <div className="shelf-day">{dayLabel}</div>}
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
                        onMouseEnter={(event) => {
                          if (!isRenaming) hoverRow(session, event.currentTarget);
                        }}
                        onMouseLeave={() => {
                          hoverRow(session, null);
                          setConfirmDeleteId((cur) => (cur === session.id ? null : cur));
                        }}
                        onMouseDown={() => hoverRow(session, null)}
                        onContextMenu={(event) => {
                          if (!onOpenBeside) return;
                          event.preventDefault();
                          event.stopPropagation();
                          setBesideMenu({ session, x: event.clientX, y: event.clientY });
                        }}
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
                            aria-label={`${agentLabel(session.agentId)} · ${session.label}`}
                            onClick={() => {
                              setStatusFilter(null);
                              onSessionSelect(session);
                            }}
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
                            <span className="session-row__time">
                              {shelfRecency(session.lastActiveAt, Date.now())}
                            </span>
                          </button>
                        )}
                        <span className="session-row__actions">
                          {onToggleSessionPin && !isRenaming && (
                            <SessionPinButton
                              pinned={pinned}
                              className="session-row__action"
                              onToggle={(next) => onToggleSessionPin(session.id, next)}
                            />
                          )}
                          {onRenameSession && !isRenaming && (
                            <button
                              className="session-row__action"
                              type="button"
                              title={t("shelf.rename")}
                              aria-label={t("shelf.rename")}
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
                              title={t("shelf.park")}
                              aria-label={t("shelf.park")}
                              onClick={(e) => {
                                e.stopPropagation();
                                onSuspendSession(session.id);
                              }}
                            >
                              <Pause size={12} />
                            </button>
                          )}
                          <button
                            className={`session-row__action session-row__action--danger${confirmDeleteId === session.id ? " is-armed" : ""}`}
                            type="button"
                            title={confirmDeleteId === session.id ? t("shelf.confirmDelete") : t("common.delete")}
                            aria-label={confirmDeleteId === session.id ? t("shelf.confirmDelete") : t("common.delete")}
                            onClick={() => {
                              if (confirmDeleteId === session.id) {
                                setConfirmDeleteId(null);
                                onDeleteSession(session.id);
                              } else {
                                setConfirmDeleteId(session.id);
                              }
                            }}
                          >
                            {confirmDeleteId === session.id ? (
                              <span className="row-action-confirm">{t("shelf.confirmDelete")}</span>
                            ) : (
                              <Trash2 size={12} />
                            )}
                          </button>
                        </span>
                      </div>
                      </Fragment>
                    );
                  })}

                  {hiddenSessionCount > 0 && (
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

      {onOpenMemo && (
        <button className="shelf-memo-entry" type="button" onClick={onOpenMemo}>
          <Archive size={13} aria-hidden />
          {t("shelf.memoBox")}
        </button>
      )}
      {onOpenCouncil && (
        <button className="shelf-memo-entry" type="button" onClick={onOpenCouncil} title="多智能体协作(Council)">
          <Users size={13} aria-hidden />
          Council 协作
        </button>
      )}

      {/* ── Chat section ── */}
      {onChatSessionSelect && (
        <section className="chat-section">
          <div
            className="chat-section__header"
            title={defaultFolderPath ? t("shelf.chatWorkdir", { path: defaultFolderPath }) : t("shelf.chatWorkdirDefault")}
          >
            <span className="chat-section__title">
              <MessageSquare size={13} aria-hidden />
              {t("common.chat")}
            </span>
            {onNewChat && (
              <button
                className="pill-action pill-action--icon pill-action--sm chat-section__new"
                type="button"
                title={t("shelf.newChat")}
                aria-label="New chat"
                onClick={() => onNewChat()}
              >
                <Plus size={13} />
              </button>
            )}
          </div>
          <div
            className="chat-list custom-scrollbar scrollbar-autohide"
            aria-label="Chat conversations"
            style={{ ["--chat-list-preview" as string]: String(SESSION_LIST_PREVIEW) }}
          >
            {visibleChatSessions.length === 0 && !searching && (
              <div className="chat-list__empty">{t("shelf.noChats")}</div>
            )}
            {sortSessionsNewestFirst(visibleChatSessions).map(renderChatRow)}
          </div>
        </section>
      )}

      {/* ── 收纳盒 dialogs: conversations grown from memo materials ── */}
      {onChatSessionSelect && visibleMemoSessions.length > 0 && (
        <section className="chat-section chat-section--memo" aria-label={t("shelf.memoChats")}>
          <div className="chat-section__header" title={t("shelf.memoChatsTitle")}>
            <span className="chat-section__title">
              <Archive size={13} aria-hidden />
              {t("shelf.memoBox")}
            </span>
          </div>
          <div
            className="chat-list custom-scrollbar scrollbar-autohide"
            aria-label={t("shelf.memoChats")}
            style={{ ["--chat-list-preview" as string]: String(SESSION_LIST_PREVIEW) }}
          >
            {sortSessionsNewestFirst(visibleMemoSessions).map(renderChatRow)}
          </div>
        </section>
      )}

      {docked ? null : dockFooter}
      <DockedLayer docked={docked}>
      {proxyOpen && proxyAnchor && (
        <div
          ref={proxyPopRef}
          className="proxy-popover"
          role="dialog"
          aria-label={t("shelf.proxySettings")}
          style={{ left: proxyAnchor.left, bottom: window.innerHeight - proxyAnchor.top + 6 }}
        >
          <div className="proxy-popover__header">
            <span className="proxy-popover__title">{t("shelf.proxySettings")}</span>
            <button
              type="button"
              className="pill-action pill-action--icon pill-action--sm"
              aria-label={t("common.close")}
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
            <span className="proxy-popover__row-label">{t("shelf.proxyEnable")}</span>
          </div>
          <div className="proxy-popover__row">
            <label className="proxy-popover__field">
              <span className="proxy-popover__field-label">{t("shelf.proxyAddress")}</span>
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
          <div className="proxy-popover__hint">{t("shelf.proxyHint")}</div>
          <div className="proxy-popover__actions">
            <button
              type="button"
              className="pill-action pill-action--icon pill-action--sm"
              title={t("shelf.proxyTest")}
              aria-label={t("shelf.proxyTest")}
              disabled={proxyBusy !== null || !proxyDraft.enabled}
              onClick={() => void runProxyTest()}
            >
              {proxyBusy === "test" ? <span className="proxy-popover__spinner" aria-hidden /> : <Zap size={13} />}
            </button>
            <button
              type="button"
              className="pill-action pill-action--icon pill-action--sm"
              title={t("shelf.proxySave")}
              aria-label={t("shelf.proxySave")}
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
          className="proxy-popover engine-panel"
          role="dialog"
          aria-label={t("shelf.modelShare")}
          style={{ left: engineAnchor.left, bottom: window.innerHeight - engineAnchor.top + 6 }}
        >
          <div className="proxy-popover__header">
            <span className="proxy-popover__title">{t("shelf.modelShare")}</span>
            <button
              type="button"
              className="pill-action pill-action--icon pill-action--sm"
              aria-label={t("common.close")}
              onClick={() => setEngineOpen(false)}
            >
              <X size={12} />
            </button>
          </div>

          {/* Status, and the one action that matters right now. */}
          <section className={`engine-panel__status is-${engineTone}`}>
            <div className="engine-panel__state">
              <span className="engine-panel__dot" aria-hidden />
              <strong>{engineHeadline}</strong>
              {engineState?.state === "running" && (
                <span className="engine-panel__where">127.0.0.1:{engineState.port}</span>
              )}
            </div>
            {engineState?.state === "running" && (
              <p className="engine-panel__note">
                {engineState.reused
                  ? t("shelf.engineReused")
                  : t("shelf.engineSharedModels", { version: engineState.version ?? "" }).replace("  ", " ")}
              </p>
            )}
            {engineState?.state === "downloading" && (
              <div className="engine-panel__progress" aria-label={engineStatusLine}>
                <i
                  style={{
                    width: `${engineState.total > 0 ? Math.min(100, (engineState.downloaded / engineState.total) * 100) : 8}%`,
                  }}
                />
              </div>
            )}
            {engineState?.state === "failed" && (
              <p className="engine-panel__error">{engineState.error.slice(0, 240)}</p>
            )}
            {(!engineState || engineState.state === "absent") && (
              <p className="engine-panel__note">{t("shelf.engineIntro")}</p>
            )}
            {engineRunning ? (
              !engineState?.reused && (
                <button
                  type="button"
                  className="engine-panel__action is-quiet"
                  title={t("shelf.engineStopTitle")}
                  onClick={() => {
                    void engineStop().then(() => refreshEngine());
                  }}
                >
                  {t("shelf.engineStop")}
                </button>
              )
            ) : (
              <button
                type="button"
                className="engine-panel__action"
                disabled={engineBusyTransient}
                onClick={() => {
                  void engineEnable().then((snapshot) => {
                    if (snapshot) setEngineState(snapshot);
                    refreshEngine();
                  });
                }}
              >
                {engineBusyTransient ? t("shelf.enginePreparing") : engineState?.state === "failed" ? t("shelf.engineRetryStart") : t("shelf.engineStart")}
              </button>
            )}
          </section>

          {/* Engine version: where it stands, and how to move it. */}
          <section className="engine-panel__section">
            <div className="engine-panel__section-head">
              <span>{t("shelf.engineVersion")}</span>
              <button
                type="button"
                className="engine-panel__link"
                disabled={engineUpdateBusy !== null || engineBusyTransient}
                onClick={() => void handleEngineCheckUpdate()}
              >
                <RefreshCw size={11} aria-hidden className={engineUpdateBusy === "check" ? "is-spinning" : undefined} />
                {engineUpdateBusy === "check" ? t("shelf.checking") : t("shelf.checkUpdate")}
              </button>
            </div>
            {engineUpdateInfo ? (
              <dl className="engine-panel__kv">
                <dt>{t("shelf.engineLocal")}</dt>
                <dd>{engineUpdateInfo.currentVersion ?? t("shelf.engineNotInstalled")}</dd>
                {engineUpdateInfo.latestVersion && (
                  <>
                    <dt>{t("shelf.engineLatest")}</dt>
                    <dd className={engineUpdateInfo.updateAvailable ? "is-new" : undefined}>
                      {engineUpdateInfo.latestVersion}
                    </dd>
                  </>
                )}
                {engineUpdateInfo.embeddedVersion && (
                  <>
                    <dt>{t("shelf.engineFactory")}</dt>
                    <dd>{engineUpdateInfo.embeddedVersion}</dd>
                  </>
                )}
              </dl>
            ) : (
              <p className="engine-panel__note">{t("shelf.engineUpdateHint")}</p>
            )}
            {(engineUpdateInfo?.updateAvailable || engineUpdateInfo?.canRestoreFactory) && (
              <div className="engine-panel__row">
                {engineUpdateInfo?.updateAvailable && (
                  <button
                    type="button"
                    className="engine-panel__action is-small"
                    disabled={engineUpdateBusy !== null || engineBusyTransient}
                    title={t("shelf.engineUpdateTitle")}
                    onClick={() => void handleEngineUpdate()}
                  >
                    {engineUpdateBusy === "update" ? t("shelf.updating") : t("shelf.updateTo", { version: engineUpdateInfo.latestVersion })}
                  </button>
                )}
                {engineUpdateInfo?.canRestoreFactory && (
                  <button
                    type="button"
                    className="engine-panel__link"
                    disabled={engineUpdateBusy !== null || engineBusyTransient}
                    title={t("shelf.engineRestoreTitle")}
                    onClick={() => void handleEngineRestoreFactory()}
                  >
                    {engineUpdateBusy === "restore" ? t("shelf.restoring") : t("shelf.engineRestore")}
                  </button>
                )}
              </div>
            )}
            {engineUpdateError && (
              <p className="engine-panel__error" role="status">
                {engineUpdateError}
              </p>
            )}
          </section>

          {/* Where the models and keys are managed. */}
          <nav className="engine-panel__nav">
            <button
              type="button"
              className="engine-panel__nav-row"
              onClick={() => {
                setEngineOpen(false);
                setModelLibraryOpen(true);
              }}
            >
              <SlidersHorizontal size={12} aria-hidden />
              <span>{t("shelf.modelsAndSources")}</span>
              <small>{t("shelf.modelsAndSourcesHint")}</small>
              <ChevronRight size={12} aria-hidden />
            </button>
            <button
              type="button"
              className="engine-panel__nav-row"
              onClick={() => {
                setEngineOpen(false);
                setCredentialsOpen(true);
              }}
            >
              <KeyRound size={12} aria-hidden />
              <span>{t("shelf.keysAndSources")}</span>
              <small>{t("shelf.keysAndSourcesHint")}</small>
              <ChevronRight size={12} aria-hidden />
            </button>
          </nav>
          {engineRunning && (
            <p className="engine-panel__credit">
              {magpieCredit(engineState?.state === "running" ? engineState.version : undefined)}
            </p>
          )}
        </div>
      )}
      {tgOpen && tgAnchor && (
        <div
          ref={tgPopRef}
          className="proxy-popover tg-popover"
          role="dialog"
          aria-label={t("shelf.tgPanelAria")}
          style={{ left: tgAnchor.left, bottom: window.innerHeight - tgAnchor.top + 6 }}
        >
          <div className="proxy-popover__header">
            <span className="proxy-popover__title">{t("shelf.tgRemote")}</span>
            <button
              type="button"
              className="pill-action pill-action--icon pill-action--sm"
              aria-label={t("common.close")}
              onClick={() => setTgOpen(false)}
            >
              <X size={12} />
            </button>
          </div>

          {!tgState?.configured ? (
            <>
              <div className="proxy-popover__row">
                <label className="proxy-popover__field">
                  <span className="proxy-popover__field-label">Bot token</span>
                  <input
                    className="proxy-popover__input"
                    type="text"
                    spellCheck={false}
                    placeholder={t("shelf.tgTokenPlaceholder")}
                    value={tgTokenDraft}
                    onChange={(event) => setTgTokenDraft(event.target.value)}
                    onKeyDown={(event) => {
                      if (event.key === "Enter") void tgSaveToken();
                    }}
                  />
                </label>
              </div>
              <div className="proxy-popover__hint">{t("shelf.tgSetupHint")}</div>
              <div className="proxy-popover__actions">
                <button
                  type="button"
                  className="pill-action pill-action--icon pill-action--sm"
                  title={t("shelf.tgSaveToken")}
                  aria-label={t("shelf.tgSaveToken")}
                  disabled={tgBusy !== null}
                  onClick={() => void tgSaveToken()}
                >
                  {tgBusy === "save" ? (
                    <span className="proxy-popover__spinner" aria-hidden />
                  ) : (
                    <Save size={13} />
                  )}
                </button>
              </div>
            </>
          ) : !tgState.paired ? (
            <>
              <p className="engine-popover__status" data-state={tgState.pendingPairing ? "running" : "unknown"}>
                {tgState.pendingPairing
                  ? t("shelf.tgPairRequest", {
                      user: tgState.pendingPairing.username || tgState.pendingPairing.firstName,
                    })
                  : t("shelf.tgWaitingFirstMessage", { bot: tgState.botUsername ?? "bot" })}
              </p>
              {tgState.pendingPairing && (
                <div className="proxy-popover__actions tg-popover__pair">
                  <button
                    type="button"
                    className="engine-popover__start"
                    disabled={tgBusy !== null}
                    onClick={() => void tgPair(true)}
                  >
                    {t("shelf.tgAccept")}
                  </button>
                  <button
                    type="button"
                    className="engine-popover__stop"
                    disabled={tgBusy !== null}
                    onClick={() => void tgPair(false)}
                  >
                    {t("shelf.tgIgnore")}
                  </button>
                </div>
              )}
              <div className="proxy-popover__hint">{t("shelf.tgPairHint")}</div>
            </>
          ) : (
            <>
              <div className="proxy-popover__row">
                <button
                  type="button"
                  role="switch"
                  aria-checked={tgState.enabled}
                  className={
                    tgState.enabled ? "proxy-popover__switch is-on" : "proxy-popover__switch"
                  }
                  disabled={tgBusy !== null}
                  onClick={() => void tgSetEnabled(!tgState.enabled)}
                >
                  <span className="proxy-popover__switch-track" />
                  <span className="proxy-popover__switch-thumb" />
                </button>
                <span className="proxy-popover__row-label">
                  {tgState.enabled ? t("shelf.tgEnabled") : t("shelf.tgPaused")}
                  {tgState.pairedUsername ? ` · @${tgState.pairedUsername}` : ""}
                </span>
              </div>
              {(tgPrefsDraft ?? tgState.prefs) && (
                <div className="tg-popover__prefs">
                  {(
                    [
                      ["notifyTurn", "shelf.tgNotifyTurn"],
                      ["notifyWait", "shelf.tgNotifyWait"],
                      ["notifyError", "shelf.tgNotifyError"],
                      ["contentPreview", "shelf.tgNotifyPreview"],
                    ] as [keyof TelegramPrefs, DictKey][]
                  ).map(([key, label]) => {
                    const prefs = tgPrefsDraft ?? tgState.prefs;
                    return (
                      <div className="proxy-popover__row" key={key}>
                        <button
                          type="button"
                          role="switch"
                          aria-checked={prefs[key]}
                          className={
                            prefs[key]
                              ? "proxy-popover__switch is-on"
                              : "proxy-popover__switch"
                          }
                          disabled={tgBusy !== null}
                          onClick={() => void tgSavePrefs({ [key]: !prefs[key] })}
                        >
                          <span className="proxy-popover__switch-track" />
                          <span className="proxy-popover__switch-thumb" />
                        </button>
                        <span className="proxy-popover__row-label">{t(label)}</span>
                      </div>
                    );
                  })}
                </div>
              )}
              <div className="proxy-popover__hint">{t("shelf.tgReplyHint")}</div>
              <div className="engine-popover__footer">
                <button
                  type="button"
                  className="engine-popover__link"
                  disabled={tgBusy !== null}
                  onClick={() => void tgUnpair()}
                >
                  <Trash2 size={11} aria-hidden /> {t("shelf.tgUnpair")}
                </button>
              </div>
            </>
          )}
          <div className="proxy-popover__hint">{t("shelf.tgProxyHint")}</div>
          {tgError && <div className="proxy-popover__test is-err">{tgError}</div>}
          {tgState?.lastError && !tgError && (
            <div className="proxy-popover__test is-err">{t("shelf.tgLastError", { error: tgState.lastError })}</div>
          )}
        </div>
      )}
      </DockedLayer>
      </div>
    </section>
    {docked ? createPortal(dockFooter, dockFooterHost()) : null}
    {credentialsOpen && (
      <CredentialsDialog agents={agents} onClose={() => setCredentialsOpen(false)} />
    )}
    {modelLibraryOpen && <ModelLibraryDialog agents={agents} onClose={() => setModelLibraryOpen(false)} onManageSources={() => { setModelLibraryOpen(false); setCredentialsOpen(true); }} />}
    {pendingDelete && (
      <ConfirmDialog
        title={
          pendingDelete.kind === "project"
            ? t("shelf.removeProject")
            : pendingDelete.kind === "chat"
              ? t("shelf.deleteConversation")
              : "Delete session"
        }
        itemName={pendingDelete.label}
        description={
          pendingDelete.kind === "project"
            ? t("shelf.removeProjectDesc")
            : pendingDelete.kind === "chat"
              ? t("shelf.deleteConversationDesc")
              : "This removes its local transcript and cannot be undone."
        }
        confirmLabel={
          pendingDelete.kind === "project"
            ? t("common.remove")
            : pendingDelete.kind === "session"
              ? "Delete"
              : t("common.delete")
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
    {hoverCard && !besideMenu && (
        <div
          className="shelf-peek"
          role="tooltip"
          style={{ top: Math.min(hoverCard.top, window.innerHeight - 190), left: hoverCard.left }}
        >
          <div className="shelf-peek__title">{hoverCard.session.label}</div>
          <div className="shelf-peek__row">
            <span
              className={`shelf-peek__status is-${hoverCard.session.status}${
                unreadDone.has(hoverCard.session.id) && !sessionIsBusy(hoverCard.session.status) ? " is-done" : ""
              }`}
            >
              {unreadDone.has(hoverCard.session.id) && !sessionIsBusy(hoverCard.session.status)
                ? t("shelf.done")
                : SESSION_STATUS_WORD[hoverCard.session.status]
                  ? t(SESSION_STATUS_WORD[hoverCard.session.status]!)
                  : hoverCard.session.status}
            </span>
            <span>{agentLabel(hoverCard.session.agentId)}</span>
          </div>
          {(hoverCard.session.preferredModel || hoverCard.session.preferredMode) && (
            <div className="shelf-peek__meta">
              {[hoverCard.session.preferredModel, hoverCard.session.preferredMode].filter(Boolean).join(" · ")}
            </div>
          )}
          <div className="shelf-peek__meta">
            {projects.find((p) => p.id === hoverCard.session.projectId)?.name ??
              (hoverCard.session.origin === "memo" ? t("shelf.memoBox") : t("common.chat"))}
            {hoverCard.session.origin === "import" ? t("shelf.importedSuffix") : ""}
          </div>
          <div className="shelf-peek__meta">
            {t("shelf.started")}{" "}
            {new Date(hoverCard.session.startedAt).toLocaleString(undefined, {
              month: "numeric",
              day: "numeric",
              hour: "2-digit",
              minute: "2-digit",
            })}
            {" · "}{t("shelf.recent")}{" "}
            {new Date(hoverCard.session.lastActiveAt).toLocaleString(undefined, {
              month: "numeric",
              day: "numeric",
              hour: "2-digit",
              minute: "2-digit",
            })}
          </div>
          <div className="shelf-peek__meta shelf-peek__hint">{t("shelf.peekHint")}</div>
        </div>
      )}
    {besideMenu && onOpenBeside && (
      <SplitBesideMenu
        x={besideMenu.x}
        y={besideMenu.y}
        onPick={(edge: OpenBesideEdge) => onOpenBeside(besideMenu.session, edge)}
        onClose={() => setBesideMenu(null)}
      />
    )}
    </>
  );
}
