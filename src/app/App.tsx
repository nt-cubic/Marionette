import { emit, listen } from "@tauri-apps/api/event";
import { ChevronLeft, ChevronRight, FolderOpen, X } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties } from "react";
import { agents, projects, sessions } from "../lib/mockData";
import {
  applyAcpPartToEvents,
  coalesceAdjacentAssistantFragments,
  coalesceAdjacentThoughts,
  collapseIntermediateAssistantAsThought,
  extractAcpSessionTitle,
  extractAcpUpdateText,
  findLastIndexForSession,
  getSessionUpdate,
  getSessionUpdateKind,
  sealOpenAssistantReplies,
  turnEndedSilently,
  userMessageEvent,
} from "../lib/acpTranscript";
import { addProject, appendDebugLog, applyAppUpdateAndRelaunch, cancelAcpSession, checkAppUpdate, checkOutsideProjectPaths, createChildSession, createSession as createSessionApi, createChatSession, deleteProject as deleteProjectApi, deleteSession as deleteSessionApi, downloadAppUpdate, getDefaultFolder, generateHandoff, getProxyConfig, getChangedFiles, getCurrentBranch, getFileDiff, getSessionCapabilities, grantWorkspaceRoot, isTauriRuntime, listAgentCommands, listAgents, listProjects, listSessions, listTodos, loadTranscript, listChatSessions, OPEN_PATH_EVENT, openHere, pickFolder, probeAcpBilling, probeAgentAuth, probeProviderUsage, projectContextPrompt, reorderProjects as reorderProjectsApi, respondAcpPermission, respondAcpPlanApproval, respondAcpQuestion, revealInFileManager, openExternal, saveTodos, scanProjectContext, searchSessions, sendAcpPrompt, setProjectContextEnabled, setProxyConfig, setSessionPinned, startAcpSession, startAgentLogin, stopAcpSession, takeLaunchOpenPath, testProxy, updateAcpSession, updateSessionAgent, updateSessionLabel, updateSessionPrefs, updateSessionStatus, writeTranscript, type AppUpdateInfo, type OutsidePath, type PlanApprovalDecision, CHAT_PROJECT_ID } from "../lib/api";
import {
  bindDetachedWindowReaper,
  DETACHED_HIDDEN_EVENT,
  DETACHED_READY_EVENT,
  hideDetachedWindowForSession,
  listenMergeBackRequests,
  listenMergeHighlights,
  openDetachedSessionWindow,
  readDetachedSessionId,
  requestMergeBack,
  sessionIdFromDetachedLabel,
  setDetachedSessionOwner,
  shouldHandleAcpSession,
  type DetachedReadyPayload,
} from "../lib/detachedWindow";
import { broadcastSessionPatch, listenSessionPatches } from "../lib/sessionBus";
import {
  coldSessionIdsWithEvents,
  collectHotSessionIds,
  dropEventsForSessions,
} from "../lib/memoryHygiene";
import { agentAuthSpec } from "../lib/agentAuth";
import type { AcpEvent, AvailableCommand, CapabilitySnapshot, ChangedFile, HandoffResult, Project, ProjectContext, ProxyConfig, ProxyTestResult, Session, SessionComposerPrefs, SessionEvent, SessionViewMode, TurnStats, UsageSnapshot } from "../lib/types";
import {
  applyGrokLiveContext,
  applyModelContextSize,
  buildTurnStats,
  buildUsageSnapshot,
  cumulativeFromEvents,
  emptyGrokTurnUsage,
  emptySessionUsage,
  extractTurnTokens,
  grokTurnUsageToTurnTokens,
  mergeGrokBilling,
  mergeGrokCallUsage,
  mergeProviderProbe,
  mergeUsageFromAcp,
  mergeUsageFromPromptResult,
  mergeUsageFromText,
  parseGrokCallUsage,
  type GrokTurnUsage,
  type SessionUsageState,
} from "../lib/usage";
import {
  bindDesktopNotifyFocusHandlers,
  cancelScheduledDesktopNotify,
  DESKTOP_NOTIFY_SETTLE_MS,
  isDesktopNotifyEnabled,
  raiseDesktopNotify,
  scheduleDesktopNotify,
  setDesktopNotifyEnabled,
} from "../lib/desktopNotify";
import { parseAvailableCommandsUpdate } from "../lib/slashCommands";
import type { PlanEntry } from "../lib/acpPlan";
import { parseAcpPlanUpdate } from "../lib/acpPlan";
import {
  absorbPlanIntoTodos,
  formatAiUpdatePrompt,
  formatTodosForPrompt,
  parseMarionetteTodoFence,
  planToProposed,
  previewMergeFromAi,
  type TodoItem,
} from "../lib/todos";
import { parseDelegateLine } from "../lib/delegate";
import {
  expandAcpConfigAttempts,
  mergeAcpCapabilities,
  normalizeAgentModeId,
} from "../lib/acpSupplements";
import {
  formatImageMarksForSend,
  type ImageAttachment,
} from "../lib/imageAttachments";
import { foldSessionStats } from "../lib/sessionStats";
import { isRuntimeMetadataOnly } from "../lib/markdownText";
import {
  parseTranscriptEvents,
  persistableEventsForSession,
  shouldAutoRenameLabel,
  titleFromUserText,
} from "../lib/transcript";
import {
  activityHealth,
  isSubagentTool,
  isToolInProgress,
} from "../lib/activityHealth";
import {
  buildHistoryInjection,
  pendingHandoff,
  withHandoffAttachment,
  withHistoryInjection,
} from "../lib/sessionHistory";
import { formatPinsForSend } from "../lib/quoteComment";
import { findLinkTargets } from "../lib/linkTargets";
import { classifyAgentError, formatClassifiedError } from "../lib/errors";
import { prettyEffortLabel } from "../lib/modelLabel";
import { getLastUsedDefaults } from "../lib/recentModels";
import { pickRestoredSession, saveUiRestore, saveQueuedSends, loadQueuedSends, saveClosedTabs, loadClosedTabs } from "../lib/uiRestore";
import { AskQuestionCard, type AskQuestionPrompt } from "../components/AskQuestionCard";
import { Composer } from "../components/Composer";
import { ComposerErrorStrip } from "../components/ComposerErrorStrip";
import { ContextPanel } from "../components/ContextPanel";
import { PermissionDialog, type PermissionPrompt } from "../components/PermissionDialog";
import { PlanApprovalCard, type PlanApprovalPrompt } from "../components/PlanApprovalCard";
import { UnifiedDiffView } from "../components/UnifiedDiffView";
import { ProjectShelf } from "../components/ProjectShelf";
import { SessionTabs, SessionView, type UserMessageAnchor } from "../components/SessionView";
import { WindowControls } from "../components/WindowControls";
import { parseAskQuestionPrompt } from "../lib/askQuestion";
import { initScrollbarAutoHide } from "../lib/scrollbarAutoHide";
import { nextTheme, parseStoredTheme, themeColorScheme, type ThemeMode } from "../lib/theme";

const LEFT_PANEL_MIN = 180;
const LEFT_PANEL_MAX = 420;
const LEFT_PANEL_DEFAULT = 224;
const RIGHT_PANEL_MIN = 220;
const RIGHT_PANEL_MAX = 480;
const RIGHT_PANEL_DEFAULT = 270;
const LAYOUT_STORAGE_KEY = "marionette-layout";

/** Popped-out dialog windows load `?detached=<sessionId>`. */
const DETACHED_SESSION_ID = readDetachedSessionId();
const IS_DETACHED_WINDOW = Boolean(DETACHED_SESSION_ID);

function clamp(n: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, n));
}

function readStoredPanelWidth(key: "leftWidth" | "rightWidth", fallback: number, min: number, max: number): number {
  try {
    const raw =
      window.localStorage.getItem(LAYOUT_STORAGE_KEY) ??
      window.localStorage.getItem("agentshell-layout");
    if (!raw) return fallback;
    const parsed = JSON.parse(raw) as { leftWidth?: number; rightWidth?: number };
    const value = parsed[key];
    return typeof value === "number" && Number.isFinite(value) ? clamp(value, min, max) : fallback;
  } catch {
    return fallback;
  }
}

/** Pull a human-readable error from ACP JSON-RPC error payloads. */
function formatAcpRpcError(data: unknown): string | null {
  if (data == null) return null;
  if (typeof data === "string") return data;
  if (typeof data !== "object") return String(data);
  const root = data as Record<string, unknown>;
  const err = (root.error && typeof root.error === "object"
    ? (root.error as Record<string, unknown>)
    : root) as Record<string, unknown>;
  const message =
    (typeof err.message === "string" && err.message) ||
    (typeof root.message === "string" && root.message) ||
    null;
  const details =
    err.data && typeof err.data === "object"
      ? (err.data as Record<string, unknown>).details
      : typeof err.data === "string"
        ? err.data
        : null;
  if (message && typeof details === "string") return `${message}: ${details}`;
  if (message) return message;
  if (typeof details === "string") return details;
  try {
    return JSON.stringify(data);
  } catch {
    return "Unknown agent error";
  }
}

/** Keep native title/taskbar details readable when an agent returns JSON. */
function compactNotifyDetail(value: string | null | undefined, max = 160): string {
  const compact = value?.replace(/\s+/g, " ").trim() ?? "";
  return compact.length > max ? `${compact.slice(0, max - 1)}…` : compact;
}

/**
 * Status is runtime state, not history. On desktop, Rust `list_sessions_healed`
 * already keeps mid-turn / warm sessions live and demotes dead ones to exited —
 * so we must not force idle here (detached windows would lose Interrupt).
 * Browser mock has no process ownership: never restore as running.
 */
function asIdleOnLoad(session: Session): Session {
  if (isTauriRuntime()) return session;
  if (session.status !== "starting" && session.status !== "running" && session.status !== "waiting") {
    return session;
  }
  return { ...session, status: "exited", processId: null };
}

/**
 * How long an agent may stay silent after `session/cancel` before we call the
 * turn unrecoverable. Cancel gets no reply, so renewed output is the only ack;
 * an agent that honours it reacts within a second or two.
 */
const CANCEL_ACK_GRACE_MS = 6_000;

/**
 * True turn boundary (Codeg-aligned). Generic `rpc/response` is used for
 * set_config / model / mode / probes and must NOT end a turn.
 */
function isTurnCompleteMethod(method: string | null | undefined): boolean {
  return method === "turn/complete";
}

function turnStopReason(data: unknown): string {
  if (!data || typeof data !== "object") return "end_turn";
  const d = data as Record<string, unknown>;
  const raw = d.stopReason ?? d.stop_reason;
  return typeof raw === "string" && raw.trim() ? raw.trim() : "end_turn";
}

/**
 * Close out tools that never received a terminal status (stuck in_progress).
 *
 * `endedAtMs` is the moment the turn actually ended, and is only passed when we
 * observed it. Loading a dialog whose agent process is gone closes its tools
 * too, but days later — stamping that as the tool's end would put a bogus
 * duration into the session statistics, so it stays unstamped.
 */
function markOpenTools(
  events: SessionEvent[],
  sessionId: string,
  status: "cancelled" | "failed",
  endedAtMs?: number
): SessionEvent[] {
  return events.map((event) => {
    if (event.sessionId !== sessionId || event.type !== "tool_call") return event;
    if (!isToolInProgress(event.status)) return event;
    const title = event.title ?? "tool";
    const line1 = `${title} · ${status}`;
    const rest = event.text.includes("\n") ? event.text.slice(event.text.indexOf("\n")) : "";
    return {
      ...event,
      status,
      ...(endedAtMs != null && event.completedAt == null
        ? { completedAt: new Date(endedAtMs).toISOString() }
        : {}),
      text: rest ? `${line1}${rest}` : line1,
    };
  });
}

function effortLabel(value: number): string {
  if (value <= 0.1) return "Low";
  if (value >= 0.9) return "High";
  if (Math.abs(value - 0.5) < 0.1) return "Auto";
  return value < 0.5 ? "Medium" : "High";
}

type ProjectFileSnapshot = {
  projectId: string;
  changedFiles: ChangedFile[];
  files: Record<string, { changeType: ChangedFile["changeType"]; fingerprint: string }>;
};

/** Small deterministic fingerprint so a turn snapshot does not retain full diff text. */
function fingerprintDiff(text: string): string {
  let hash = 2166136261;
  for (let i = 0; i < text.length; i += 1) {
    hash ^= text.charCodeAt(i);
    hash = Math.imul(hash, 16777619);
  }
  return `${text.length}:${hash >>> 0}`;
}

function isToolCompletionStatus(status?: string): boolean {
  const normalized = status?.trim().toLowerCase().replace(/[\s-]+/g, "_");
  return normalized === "completed" ||
    normalized === "complete" ||
    normalized === "succeeded" ||
    normalized === "success" ||
    normalized === "failed" ||
    normalized === "error" ||
    normalized === "cancelled" ||
    normalized === "canceled";
}

function isChatgptUsageModel(modelId: string | null | undefined): boolean {
  const provider = modelId?.trim().split("/", 1)[0]?.toLowerCase();
  return provider === "openai" || provider === "codex" || provider === "chatgpt";
}

export function App() {
  const [availableProjects, setAvailableProjects] = useState<Project[]>(projects);
  const [availableAgents, setAvailableAgents] = useState(agents);
  const [availableSessions, setAvailableSessions] = useState<Session[]>(sessions);
  const [currentProjectId, setCurrentProjectId] = useState(projects[0]?.id ?? "");
  const [currentSessionId, setCurrentSessionId] = useState(sessions[0]?.id ?? "");
  const [openSessionIds, setOpenSessionIds] = useState<string[]>([sessions[0]?.id ?? ""]);
  /** Project opened via Explorer "在此处打开" / --open-path — pinned at shelf top. */
  const [openHereProjectId, setOpenHereProjectId] = useState<string | null>(null);
  /** Default folder for the Chat section (from --open-path or CWD). */
  const [defaultFolderPath, setDefaultFolderPath] = useState<string>("");
  /** Chat rows created before the initial async storage restore completed. */
  const startupCreatedChatsRef = useRef<Session[]>([]);
  /** Manual rename wins over any later title emitted by an agent. */
  const manuallyRenamedSessionIdsRef = useRef<Set<string>>(new Set());
  /** Ignore stale filesystem-search responses when the user keeps typing. */
  const searchRequestRef = useRef(0);
  const [viewMode, setViewMode] = useState<SessionViewMode>("clean");
  const [leftCollapsed, setLeftCollapsed] = useState(IS_DETACHED_WINDOW);
  const [rightCollapsed, setRightCollapsed] = useState(false);
  /** Whether the mouse is hovering near the collapsed sidebar edge — shows the floating trigger */
  const [leftEdgeHover, setLeftEdgeHover] = useState(false);
  const [rightEdgeHover, setRightEdgeHover] = useState(false);
  const edgeHoverTimers = useRef<{ left?: ReturnType<typeof setTimeout>; right?: ReturnType<typeof setTimeout> }>({});
  const wasNearLeft = useRef(false);
  const wasNearRight = useRef(false);

  const [leftWidth, setLeftWidth] = useState(() =>
    readStoredPanelWidth("leftWidth", LEFT_PANEL_DEFAULT, LEFT_PANEL_MIN, LEFT_PANEL_MAX)
  );
  const [rightWidth, setRightWidth] = useState(() =>
    readStoredPanelWidth("rightWidth", RIGHT_PANEL_DEFAULT, RIGHT_PANEL_MIN, RIGHT_PANEL_MAX)
  );
  /** Which rail is being dragged — set once per gesture (not every mouse move). */
  const [resizingSide, setResizingSide] = useState<"left" | "right" | null>(null);
  const workspaceGridRef = useRef<HTMLDivElement>(null);
  const leftWidthRef = useRef(leftWidth);
  const rightWidthRef = useRef(rightWidth);
  leftWidthRef.current = leftWidth;
  rightWidthRef.current = rightWidth;
  const [projectDialogOpen, setProjectDialogOpen] = useState(false);
  const [projectPath, setProjectPath] = useState("");
  const [projectError, setProjectError] = useState("");
  const [projectAdding, setProjectAdding] = useState(false);
  const [theme, setTheme] = useState<ThemeMode>(() => {
    const stored =
      window.localStorage.getItem("marionette-theme") ??
      window.localStorage.getItem("agentshell-theme");
    return parseStoredTheme(stored);
  });
  /** Taskbar flash + chime when AI replies / may be stuck (off while focused). */
  const [desktopNotifyOn, setDesktopNotifyOn] = useState(() => isDesktopNotifyEnabled());
  /** Agent proxy config (single exit address), loaded on mount. */
  const [proxyConfig, setProxyConfigState] = useState<ProxyConfig | null>(null);
  /** Main window only: a detached tab drag is hovering the tab strip. */
  const [mergeTargetActive, setMergeTargetActive] = useState(false);
  const [sessionCapabilities, setSessionCapabilities] = useState<CapabilitySnapshot | null>(null);
  const [liveEvents, setLiveEvents] = useState<SessionEvent[]>([]);
  /** Per-session usage from ACP `usage_update` + opportunistic rate-limit text. */
  const [sessionUsageById, setSessionUsageById] = useState<Record<string, SessionUsageState>>({});
  /** Per-session ACP-advertised slash commands (`available_commands_update`). */
  const [slashCommandsById, setSlashCommandsById] = useState<Record<string, AvailableCommand[]>>({});
  /**
   * Per-session ACP `plan` update (full-replace). Session-scoped only —
   * not persisted; restart clears it (honest: live turn state).
   */
  const [planBySessionId, setPlanBySessionId] = useState<Record<string, PlanEntry[]>>({});
  /** Project-level todos (`.marionette/todos.json`). */
  const [todoItems, setTodoItems] = useState<TodoItem[]>([]);
  /** Active model id from Composer (`provider/model` for OpenCode). */
  const [activeModelId, setActiveModelId] = useState<string | null>(null);
  const providerProbeInflight = useRef(false);
  const lastProviderProbeKey = useRef("");
  /** Throttle auto usage refresh after each turn end. */
  const lastUsageRefreshAt = useRef<Record<string, number>>({});
  /** In-flight ACP bootstrap promises (lazy warm / ensure-on-send). */
  const acpBootstrapRef = useRef<Map<string, Promise<CapabilitySnapshot | null>>>(new Map());
  /** Snapshot of Composer config taken at send time, used to stamp user_message events. */
  const sendMetaRef = useRef<{
    agentId?: string;
    agentLabel?: string;
    modelId?: string;
    modelLabel?: string;
    modeLabel?: string;
    effortLabel?: string;
  } | null>(null);
  /** When the current turn's first assistant_message chunk arrived (for duration tracking). */
  const turnStartedAtRef = useRef<Record<string, number>>({});
  /** When the current turn's prompt was handed to ACP (for TTFT). */
  const turnSentAtRef = useRef<Record<string, number>>({});
  /** Per-turn Grok `response_completed` accumulation (one per model call). */
  const grokTurnUsageRef = useRef<Record<string, GrokTurnUsage>>({});
  /** Fresh ACP process needs local transcript injected once (no session/load yet). */
  const acpNeedsHistoryRef = useRef<Set<string>>(new Set());
  /** When each session's turn last actually finished (`turn/complete` / process end). */
  const turnEndedAtRef = useRef<Record<string, number>>({});
  /**
   * Sessions whose `session/cancel` went unanswered. Cancel is a fire-and-forget
   * notification, so an agent wedged inside a tool or a model call never reads
   * it — the process is unrecoverable from that point and the next send has to
   * replace it rather than write into a pipe nobody drains.
   */
  const cancelIgnoredRef = useRef<Set<string>>(new Set());
  /**
   * After the user interrupts, late agent_message / thought chunks must not
   * append onto the sealed "**Interrupted.**" card (or open a new Thought).
   * Cleared on turn/complete, process end, or the next send.
   */
  const streamSuppressedRef = useRef<Set<string>>(new Set());
  const cancelWatchdogsRef = useRef<Map<string, ReturnType<typeof setTimeout>>>(new Map());
  const transcriptSaveTimers = useRef<Map<string, ReturnType<typeof setTimeout>>>(new Map());
  const transcriptLoadedRef = useRef<Set<string>>(new Set());
  /** Session ids currently shown in a visible detached OS window (main only). */
  const detachedOwnedIdsRef = useRef<Set<string>>(new Set());
  /** Main is reloading disk transcript after a detached hide — skip live events. */
  const reloadingSessionsRef = useRef<Set<string>>(new Set());
  /**
   * Follow-up prompts typed while a turn is live. ACP allows only one
   * `session/prompt` in flight; sending mid-turn cancels the open prompt and
   * the message waits here (strip above the composer, not a timeline card —
   * a card inserted mid-stream would split the running reply) until
   * `turn/complete` frees the slot, then it is wired immediately.
   */
  const pendingSendsRef = useRef<
    Map<
      string,
      Array<{
        composed: string;
        imageAttachments: import("../lib/imageAttachments").ImageAttachment[];
        composerSnap?: {
          modeId?: string | null;
          modeLabel?: string | null;
          modelId?: string | null;
          modelLabel?: string | null;
          effortLabel?: string | null;
        };
      }>
    >
  >(new Map());
  /** Bumps to re-render the queued-strip (source of truth is pendingSendsRef). */
  const [queuedStripTick, setQueuedStripTick] = useState(0);
  const lastClosedTabsRef = useRef<string[]>(loadClosedTabs());
  const lastSendBySessionRef = useRef<
    Map<
      string,
      {
        composed: string;
        imageAttachments: ImageAttachment[];
        composerSnap?: {
          modeId?: string | null;
          modeLabel?: string | null;
          modelId?: string | null;
          modelLabel?: string | null;
          effortLabel?: string | null;
        };
      }
    >
  >(new Map());
  const [composerFailure, setComposerFailure] = useState<{
    sessionId: string;
    error: import("../lib/errors").ClassifiedError;
  } | null>(null);
  const flushingSendRef = useRef<Set<string>>(new Set());
  /** Set each render so turn/complete can drain the queue without stale closures. */
  const drainQueuedSendRef = useRef<(sessionId: string) => void>(() => undefined);
  /**
   * @-delegate children: childSessionId → meta.
   * Not in availableSessions (shelf filters them); ACP is started directly.
   */
  const delegateMetaRef = useRef<
    Map<
      string,
      {
        parentId: string;
        agentId: string;
        agentLabel: string;
        modelId?: string;
        prompt: string;
        startedAt: number;
        finished: boolean;
        idleTimer?: ReturnType<typeof setTimeout>;
      }
    >
  >(new Map());
  /** Parent id → queued delegate jobs waiting for a free concurrency slot. */
  const delegateQueueRef = useRef<
    Map<
      string,
      Array<{
        agentId: string;
        agentLabel: string;
        modelId?: string;
        prompt: string;
        projectId: string;
      }>
    >
  >(new Map());
  const MAX_DELEGATE_CONCURRENT = 2;
  const DELEGATE_IDLE_TIMEOUT_MS = 600_000;
  /** Stable ref so the ACP listener can finalize children without rebinding. */
  const finalizeDelegateChildRef = useRef<
    (childId: string, status: "done" | "failed" | "cancelled" | "timeout", error?: string) => void
  >(() => undefined);
  const [searchHitIds, setSearchHitIds] = useState<string[] | null>(null);
  /** agentId → banner text (null = no banner). Probe-driven or error-driven. */
  const [agentAuthHint, setAgentAuthHint] = useState<Record<string, string | null>>({});
  const [signInBusy, setSignInBusy] = useState(false);
  const authPollRef = useRef<ReturnType<typeof setInterval> | null>(null);
  /** Stable per-agent setter so the ACP listener can raise/clear banners. */
  const setAuthHintFor = useCallback((agentId: string, hint: string | null) => {
    setAgentAuthHint((current) => {
      if (hint === null) {
        if (!current[agentId]) return current;
        const next = { ...current };
        delete next[agentId];
        return next;
      }
      if (current[agentId] === hint) return current;
      return { ...current, [agentId]: hint };
    });
  }, []);
  const [composerPrefill, setComposerPrefill] = useState<{ text: string; token: number } | null>(null);
  /** Inline Clean quote-comments (numbered pins) for the active dialog. */
  const [quotePins, setQuotePins] = useState<import("../lib/quoteComment").QuotePin[]>([]);
  const [lastHandoff, setLastHandoff] = useState<HandoffResult | null>(null);
  const [changedFiles, setChangedFiles] = useState<ChangedFile[]>([]);
  const [changedFilesNote, setChangedFilesNote] = useState<string | null>(null);
  /** Current git branch for the active project (null = none / unknown). */
  const [gitBranch, setGitBranch] = useState<string | null>(null);
  const [diffPreview, setDiffPreview] = useState<{ path: string; text: string } | null>(null);
  const [permissionPrompt, setPermissionPrompt] = useState<PermissionPrompt | null>(null);
  const [permissionBusy, setPermissionBusy] = useState(false);
  const [askPrompt, setAskPrompt] = useState<AskQuestionPrompt | null>(null);
  const [askBusy, setAskBusy] = useState(false);
  const [planApproval, setPlanApproval] = useState<PlanApprovalPrompt | null>(null);
  const [planApprovalBusy, setPlanApprovalBusy] = useState(false);
  const [appUpdate, setAppUpdate] = useState<AppUpdateInfo | null>(null);
  const [appUpdateBusy, setAppUpdateBusy] = useState(false);
  /** MCP servers + skills found for the active project (needs 5). */
  const [projectContext, setProjectContext] = useState<ProjectContext | null>(null);
  const [projectContextScanning, setProjectContextScanning] = useState(false);
  /** A reconnect tears down and re-warms the agent — the button must show it. */
  const [reconnecting, setReconnecting] = useState(false);
  /** Draft held back because it points outside the project. */
  const [pathGrantPrompt, setPathGrantPrompt] = useState<{
    paths: OutsidePath[];
    text: string;
    sessionId: string;
    imageAttachments?: ImageAttachment[];
    /** Composer chips at original submit — mode lags on caps, so keep the snapshot. */
    composerSnap?: {
      modeId?: string | null;
      modeLabel?: string | null;
      modelId?: string | null;
      modelLabel?: string | null;
      effortLabel?: string | null;
    };
  } | null>(null);
  const [pathGrantBusy, setPathGrantBusy] = useState(false);
  const lastEscAtRef = useRef(0);
  /** Last ACP activity timestamp per session — for heartbeat / stale-working UI. */
  const [lastActivityById, setLastActivityById] = useState<Record<string, number>>({});
  const lastActivityByIdRef = useRef(lastActivityById);
  lastActivityByIdRef.current = lastActivityById;
  const sessionsRef = useRef(availableSessions);
  const agentsRef = useRef(availableAgents);
  const projectsRef = useRef(availableProjects);
  const currentSessionIdRef = useRef(currentSessionId);
  const currentProjectIdRef = useRef(currentProjectId);
  const openSessionIdsRef = useRef(openSessionIds);
  const liveEventsRef = useRef(liveEvents);
  const openSessionRef = useRef<(s: Session) => void>(() => undefined);
  const loadSessionTranscriptRef = useRef<(id: string) => Promise<void>>(async () => undefined);
  sessionsRef.current = availableSessions;
  agentsRef.current = availableAgents;
  projectsRef.current = availableProjects;
  currentSessionIdRef.current = currentSessionId;
  currentProjectIdRef.current = currentProjectId;
  openSessionIdsRef.current = openSessionIds;
  liveEventsRef.current = liveEvents;

  const parentIdOfSession = (id: string): string | null => {
    const row = sessionsRef.current.find((session) => session.id === id);
    if (row?.parentSessionId) return row.parentSessionId;
    return delegateMetaRef.current.get(id)?.parentId ?? null;
  };

  const ownsAcpSession = (sessionId: string): boolean =>
    shouldHandleAcpSession(sessionId, {
      detachedSessionId: DETACHED_SESSION_ID,
      detachedOwnedIds: detachedOwnedIdsRef.current,
      parentIdOf: parentIdOfSession,
    });

  /** One workspace snapshot per prompt; used to turn real edits into timeline cards. */
  const fileChangeSnapshotsRef = useRef<Record<string, ProjectFileSnapshot>>({});
  const fileChangeDetectionRef = useRef<Set<string>>(new Set());
  const fileChangeFinalizeRef = useRef<Set<string>>(new Set());
  const fileChangePublishedRef = useRef<Record<string, Record<string, {
    fingerprint: string;
    changeType: ChangedFile["changeType"];
    createdAt: string;
    revision: number;
  }>>>({});

  const captureProjectFileSnapshot = useCallback(async (projectId: string): Promise<ProjectFileSnapshot | null> => {
    if (!isTauriRuntime() || !projectId || projectId === CHAT_PROJECT_ID) return null;
    const changedFiles = await getChangedFiles(projectId);
    const entries = await Promise.all(
      changedFiles.map(async (file) => {
        const diff = await getFileDiff(projectId, file.path);
        return [file.path, { changeType: file.changeType, fingerprint: fingerprintDiff(diff) }] as const;
      }),
    );
    return {
      projectId,
      changedFiles,
      files: Object.fromEntries(entries),
    };
  }, []);

  const syncFileChangesForTurn = useCallback(async (sessionId: string, finalize = false) => {
    const before = fileChangeSnapshotsRef.current[sessionId];
    if (!before) return;
    if (fileChangeDetectionRef.current.has(sessionId)) {
      if (finalize) fileChangeFinalizeRef.current.add(sessionId);
      return;
    }
    fileChangeDetectionRef.current.add(sessionId);
    try {
      // Let the agent's final file write settle before asking Git for its status.
      await new Promise<void>((resolve) => window.setTimeout(resolve, 120));
      const after = await captureProjectFileSnapshot(before.projectId);
      if (!after) return;

      const published = fileChangePublishedRef.current[sessionId] ?? {};
      fileChangePublishedRef.current[sessionId] = published;
      const changedDuringTurn = after.changedFiles.flatMap((file) => {
        const previous = before.files[file.path];
        const current = after.files[file.path];
        const changed = current && (!previous ||
          previous.changeType !== current.changeType ||
          previous.fingerprint !== current.fingerprint);
        if (!changed || !current) return [];
        const priorPublication = published[file.path];
        if (
          priorPublication &&
          priorPublication.changeType === file.changeType &&
          priorPublication.fingerprint === current.fingerprint
        ) {
          return [];
        }
        const publication = {
          fingerprint: current.fingerprint,
          changeType: file.changeType,
          createdAt: priorPublication?.createdAt ?? new Date().toISOString(),
          revision: (priorPublication?.revision ?? 0) + 1,
        };
        published[file.path] = publication;
        return [{ file, publication }];
      });
      if (changedDuringTurn.length > 0) {
        setLiveEvents((current) => {
          let next = [...current];
          for (const { file, publication } of changedDuringTurn) {
            const index = next.findIndex(
              (event) => event.type === "file_change" &&
                event.sessionId === sessionId &&
                event.path === file.path &&
                event.createdAt === publication.createdAt,
            );
            const updated = {
              type: "file_change" as const,
              sessionId,
              path: file.path,
              changeType: file.changeType,
              createdAt: publication.createdAt,
              revision: publication.revision,
            };
            if (index >= 0) next[index] = updated;
            else next = [...next, updated];
          }
          return next;
        });
      }
      if (currentProjectIdRef.current === before.projectId) {
        setChangedFiles(after.changedFiles);
        setChangedFilesNote(
          after.changedFiles.length === 0 ? "No local changes (or not a git repo)." : null,
        );
      }
    } finally {
      const finalizeRequested = fileChangeFinalizeRef.current.delete(sessionId);
      fileChangeDetectionRef.current.delete(sessionId);
      if (finalizeRequested && !finalize) {
        // A final turn signal can race with a tool-completion refresh. Run one
        // last snapshot after the in-flight refresh before releasing the base.
        void syncFileChangesForTurn(sessionId, true);
      } else if (finalize) {
        delete fileChangeSnapshotsRef.current[sessionId];
        delete fileChangePublishedRef.current[sessionId];
      }
    }
  }, [captureProjectFileSnapshot]);

  const syncFileChangesRef = useRef(syncFileChangesForTurn);
  syncFileChangesRef.current = syncFileChangesForTurn;

  const touchActivity = useCallback((sessionId: string) => {
    if (!sessionId) return;
    const now = Date.now();
    lastActivityByIdRef.current = { ...lastActivityByIdRef.current, [sessionId]: now };
    setLastActivityById((current) => ({ ...current, [sessionId]: now }));
  }, []);

  /** Dev diary on disk only — never surface in product UI. */
  const pushDebug = useCallback((entry: {
    sessionId?: string;
    level?: "info" | "warn" | "error";
    source: string;
    summary: string;
    detail?: string;
  }) => {
    void appendDebugLog({
      source: entry.source,
      level: entry.level,
      sessionId: entry.sessionId,
      summary: entry.summary,
      detail: entry.detail,
    });
  }, []);

  /** Apply an ACP-provided conversation title without overriding a manual rename. */
  const applyAgentSessionTitle = useCallback((sessionId: string, rawTitle: string) => {
    const title = titleFromUserText(rawTitle);
    if (!title || manuallyRenamedSessionIdsRef.current.has(sessionId)) return;
    const known = sessionsRef.current.find((session) => session.id === sessionId);
    if (!known || known.labelSource === "manual") return;

    setAvailableSessions((current) => {
      let changed = false;
      const next = current.map((session) => {
        if (session.id !== sessionId || session.labelSource === "manual") return session;
        if (session.label === title && session.labelSource === "agent") return session;
        changed = true;
        return { ...session, label: title, labelSource: "agent" as const };
      });
      return changed ? next : current;
    });
    void updateSessionLabel(sessionId, title, "agent").catch(() => undefined);
    void broadcastSessionPatch({ sessionId, label: title, labelSource: "agent" });
    if (IS_DETACHED_WINDOW && DETACHED_SESSION_ID === sessionId) {
      void import("@tauri-apps/api/window")
        .then(({ getCurrentWindow }) => getCurrentWindow().setTitle(title))
        .catch(() => undefined);
    }
  }, []);

  const usage = useMemo<UsageSnapshot>(() => {
    const activeSession = availableSessions.find((session) => session.id === currentSessionId);
    const agent =
      availableAgents.find((candidate) => candidate.id === activeSession?.agentId) ??
      availableAgents[0] ??
      agents[0];
    const connected =
      !!activeSession &&
      (activeSession.status === "starting" ||
        activeSession.status === "running" ||
        activeSession.status === "waiting");
    return buildUsageSnapshot({
      agentId: agent.id,
      agentLabel: agent.label,
      state: activeSession ? sessionUsageById[activeSession.id] : undefined,
      connected,
    });
  }, [availableAgents, availableSessions, currentSessionId, sessionUsageById]);

  // Portable app update check — delayed so it never stalls first paint.
  useEffect(() => {
    if (!isTauriRuntime()) return;
    const t = window.setTimeout(() => {
      void checkAppUpdate().then((info) => {
        if (info?.updateAvailable) setAppUpdate(info);
      });
    }, 4000);
    return () => window.clearTimeout(t);
  }, []);

  const handleCheckAppUpdate = useCallback(() => {
    if (!isTauriRuntime() || appUpdateBusy) return;
    setAppUpdateBusy(true);
    void (async () => {
      try {
        const info = await checkAppUpdate();
        if (!info) {
          pushDebug({
            source: "update",
            level: "warn",
            summary: "check update returned null",
          });
          setAppUpdate({
            currentVersion: "",
            latestVersion: null,
            updateAvailable: false,
            releaseUrl: null,
            assetName: null,
            assetUrl: null,
            notes: null,
            note: "无法检查更新（非桌面运行时或请求失败）",
          });
          return;
        }
        if (info.updateAvailable) {
          setAppUpdate(info);
          return;
        }
        // Manual check with no update: brief status banner, auto-dismiss.
        const note =
          info.note ??
          `已是最新版本 ${info.currentVersion}${
            info.latestVersion && info.latestVersion !== info.currentVersion
              ? `（远端 ${info.latestVersion}）`
              : ""
          }`;
        setAppUpdate({
          ...info,
          updateAvailable: false,
          note,
        });
        window.setTimeout(() => {
          setAppUpdate((cur) => (cur && !cur.updateAvailable ? null : cur));
        }, 4500);
      } catch (error) {
        pushDebug({
          source: "update",
          level: "error",
          summary: "check update failed",
          detail: error instanceof Error ? error.message : String(error),
        });
        setAppUpdate({
          currentVersion: "",
          latestVersion: null,
          updateAvailable: false,
          releaseUrl: null,
          assetName: null,
          assetUrl: null,
          notes: null,
          note: error instanceof Error ? error.message : String(error),
        });
      } finally {
        setAppUpdateBusy(false);
      }
    })();
  }, [appUpdateBusy, pushDebug]);

  /**
   * Explorer "在此处打开 Marionette" / `--open-path`:
   * if folder has .marionette → project; otherwise → chat session.
   */
  const openProjectAtPath = useCallback(async (rawPath: string) => {
    if (IS_DETACHED_WINDOW) return;
    const path = rawPath.trim();
    if (!path) return;
    try {
      const agentId =
        agentsRef.current[0]?.id ??
        availableAgents[0]?.id ??
        agents[0].id;
      const result = await openHere(path, agentId);
      if (result.kind === "project") {
        const project = result.project;
        const prior = projectsRef.current.filter((p) => p.id !== project.id);
        let nextProjects = [project, ...prior];
        try {
          nextProjects = await reorderProjectsApi([
            project.id,
            ...prior.map((p) => p.id),
          ]);
        } catch {
          /* order is best-effort */
        }
        setAvailableProjects(nextProjects);
        setOpenHereProjectId(project.id);
        setCurrentProjectId(project.id);
        const projectSessions = (await listSessions(project.id)).map(asIdleOnLoad);
        setAvailableSessions((current) => {
          const others = current.filter(
            (s) => s.projectId !== project.id && s.projectId !== CHAT_PROJECT_ID,
          );
          return [...projectSessions, ...current.filter((s) => s.projectId === CHAT_PROJECT_ID), ...others];
        });
        await createSessionForProject(project.id, agentId, project);
        void refreshProjectContext(project.id);
        pushDebug({
          level: "info",
          source: "shell",
          summary: "open here",
          detail: project.rootPath,
        });
      } else if (result.kind === "chat") {
        const session = asIdleOnLoad(result.session);
        startupCreatedChatsRef.current = [
          session,
          ...startupCreatedChatsRef.current.filter((item) => item.id !== session.id),
        ];
        setDefaultFolderPath(session.cwd || path);
        setAvailableSessions((current) => [
          session,
          ...current.filter((s) => s.id !== session.id),
        ]);
        setOpenHereProjectId(null);
        setCurrentProjectId(CHAT_PROJECT_ID);
        setCurrentSessionId(session.id);
        setOpenSessionIds((current) => [
          session.id,
          ...current.filter((id) => id !== session.id),
        ]);
        setViewMode("clean");
        // The ref is populated by the current render; this also loads an
        // already-persisted transcript when the event arrives mid-session.
        openSessionRef.current(session);
        pushDebug({
          level: "info",
          source: "shell",
          summary: "open here → chat",
          detail: path,
        });
      }
    } catch (error) {
      pushDebug({
        level: "error",
        source: "shell",
        summary: "open here failed",
        detail: error instanceof Error ? error.message : String(error),
      });
    }
  // createSessionForProject / refreshProjectContext are stable enough via refs+latest render
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [availableAgents, pushDebug]);

  useEffect(() => {
    void (async () => {
      const launchPath =
        !IS_DETACHED_WINDOW && isTauriRuntime()
          ? await takeLaunchOpenPath()
          : null;
      const defaultFolder = launchPath ?? (await getDefaultFolder());
      if (defaultFolder) setDefaultFolderPath(defaultFolder);

      const [nextProjects, nextAgents, nextProxy] = await Promise.all([
        listProjects(),
        listAgents(),
        getProxyConfig(),
      ]);
      let resolvedProjects =
        nextProjects.length > 0 || isTauriRuntime() ? nextProjects : projects;
      const resolvedAgents = nextAgents.length > 0 ? nextAgents : agents;
      setAvailableAgents(resolvedAgents);
      setProxyConfigState(nextProxy);

      const loadAllSessions = async (projectList: Project[]) => {
        const [projectSessionGroups, chats] = await Promise.all([
          Promise.all(projectList.map((project) => listSessions(project.id))),
          listChatSessions(),
        ]);
        return [
          ...projectSessionGroups.flat(),
          ...chats,
        ].map(asIdleOnLoad);
      };
      // A user can click “新建对话” while the initial storage reads are still
      // in flight. Keep any such Chat rows that were not present in the read
      // result; otherwise the later restore pass would erase the new dialog.
      const setLoadedSessions = (loaded: Session[]): Session[] => {
        const loadedById = new Map(loaded.map((session) => [session.id, session]));
        // Prefer the in-memory row for a Chat created during startup: the
        // backend read may have raced the create, and the async prefs write
        // may not have landed yet.
        for (const session of startupCreatedChatsRef.current) {
          loadedById.set(session.id, session);
        }
        const loadedIds = new Set(loaded.map((session) => session.id));
        const pendingChats = startupCreatedChatsRef.current.filter(
          (session) => !loadedIds.has(session.id),
        );
        const merged = [...pendingChats, ...loadedById.values()];
        setAvailableSessions(merged);
        return merged;
      };

      // Cold start from Explorer / CLI. A folder that already owns
      // `.marionette` is a project; a plain folder becomes one Chat session.
      if (launchPath) {
        try {
          const agentId = resolvedAgents[0]?.id ?? agents[0].id;
          const result = await openHere(launchPath, agentId);
          if (result.kind === "project") {
            const project = result.project;
            const prior = resolvedProjects.filter((p) => p.id !== project.id);
            resolvedProjects = [project, ...prior];
            try {
              resolvedProjects = await reorderProjectsApi([
                project.id,
                ...prior.map((p) => p.id),
              ]);
            } catch {
              /* best-effort pin */
            }
            setAvailableProjects(resolvedProjects);
            setOpenHereProjectId(project.id);
            setCurrentProjectId(project.id);

            const loadedSessions = await loadAllSessions(resolvedProjects);
            setLoadedSessions(loadedSessions);

            const newSession = await createSessionApi(project.id, agentId);
            if (newSession) {
              const last = getLastUsedDefaults(agentId);
              let sessionWithPrefs = newSession;
              if (last) {
                sessionWithPrefs = {
                  ...newSession,
                  preferredModel: last.modelId,
                  preferredMode: last.modeId,
                  preferredEffort: last.effort,
                  preferredEffortId: last.effortId,
                  preferredAlwaysApprove: last.alwaysApprove,
                };
                void updateSessionPrefs(newSession.id, {
                  preferredModel: sessionWithPrefs.preferredModel,
                  preferredMode: sessionWithPrefs.preferredMode,
                  preferredEffort: sessionWithPrefs.preferredEffort,
                  preferredEffortId: sessionWithPrefs.preferredEffortId,
                  preferredAlwaysApprove: sessionWithPrefs.preferredAlwaysApprove,
                }).catch(() => undefined);
              }
              setAvailableSessions((current) => [
                sessionWithPrefs,
                ...current.filter((s) => s.id !== sessionWithPrefs.id),
              ]);
              setOpenSessionIds([sessionWithPrefs.id]);
              setCurrentSessionId(sessionWithPrefs.id);
              setViewMode("clean");
            }
          } else {
            const session = asIdleOnLoad(result.session);
            const loadedSessions = await loadAllSessions(resolvedProjects);
            setAvailableProjects(resolvedProjects);
            setLoadedSessions([
              session,
              ...loadedSessions.filter((item) => item.id !== session.id),
            ]);
            setOpenHereProjectId(null);
            setCurrentProjectId(CHAT_PROJECT_ID);
            setCurrentSessionId(session.id);
            setOpenSessionIds([session.id]);
            setViewMode("clean");
          }
          // Context scan runs via currentProjectId effect after state commits.
          return;
        } catch (error) {
          pushDebug({
            level: "error",
            source: "shell",
            summary: "launch open-path failed",
            detail: error instanceof Error ? error.message : String(error),
          });
          // Fall through to normal restore.
        }
      }

      setAvailableProjects(resolvedProjects);
      if (resolvedProjects.length > 0) {
        setCurrentProjectId((current) =>
          resolvedProjects.some((project) => project.id === current)
            ? current
            : resolvedProjects[0].id,
        );
      } else {
        setCurrentProjectId("");
      }

      const loadedSessions = await loadAllSessions(resolvedProjects);
      const effectiveSessions = setLoadedSessions(loadedSessions);
      if (effectiveSessions.length > 0 || isTauriRuntime()) {
        // Focus the dialog the user last used (agent/model/mode/effort already
        // ride on the session row). Detached windows still pin to their URL id.
        const startupChat = startupCreatedChatsRef.current.find((session) =>
          effectiveSessions.some((candidate) => candidate.id === session.id),
        );
        const restored = startupChat
          ? { session: startupChat, openSessionIds: [startupChat.id] }
          : pickRestoredSession(effectiveSessions, {
              detachedSessionId: DETACHED_SESSION_ID,
            });
        if (restored) {
          const preferred = restored.session;
          setCurrentProjectId(preferred.projectId);
          setCurrentSessionId(preferred.id);
          setOpenSessionIds(restored.openSessionIds);
          setViewMode("clean");
          if (IS_DETACHED_WINDOW) {
            setLeftCollapsed(true);
            try {
              const { getCurrentWindow } = await import("@tauri-apps/api/window");
              void getCurrentWindow().setTitle(preferred.label || "Marionette");
            } catch {
              /* browser preview */
            }
          }
        } else {
          setCurrentSessionId("");
          setOpenSessionIds([]);
        }
      }

    })();
  }, [pushDebug]);

  // Second Marionette process handed us a folder while this UI is already open.
  useEffect(() => {
    if (!isTauriRuntime() || IS_DETACHED_WINDOW) return;
    let unlisten: (() => void) | undefined;
    void listen<string>(OPEN_PATH_EVENT, (event) => {
      const path = typeof event.payload === "string" ? event.payload.trim() : "";
      if (path) void openProjectAtPath(path);
    }).then((fn) => {
      unlisten = fn;
    });
    return () => unlisten?.();
  }, [openProjectAtPath]);

  // Keep "last window" fresh so a crash / quit mid-session still restores.
  // Detached shells must not overwrite the main window's restore target.
  useEffect(() => {
    if (IS_DETACHED_WINDOW) return;
    if (!currentSessionId || currentSessionId.startsWith("session-empty-")) return;
    saveUiRestore({
      sessionId: currentSessionId,
      projectId: currentProjectId,
      openSessionIds,
    });
  }, [currentSessionId, currentProjectId, openSessionIds]);

  // Reap aged hidden detached windows (about:blank) — never blanks recent closes.
  useEffect(() => {
    if (!isTauriRuntime()) return;
    let unlisten: (() => void) | undefined;
    void bindDetachedWindowReaper().then((fn) => {
      unlisten = fn;
    });
    return () => unlisten?.();
  }, []);

  // Main: take / release stream ownership when a detached SPA is ready or hidden.
  useEffect(() => {
    if (!isTauriRuntime() || IS_DETACHED_WINDOW) return;
    let unlistenReady: (() => void) | undefined;
    let unlistenHidden: (() => void) | undefined;
    void listen<DetachedReadyPayload>(DETACHED_READY_EVENT, (event) => {
      const sessionId = event.payload?.sessionId;
      if (!sessionId) return;
      detachedOwnedIdsRef.current.add(sessionId);
      const pending = transcriptSaveTimers.current.get(sessionId);
      if (pending) {
        clearTimeout(pending);
        transcriptSaveTimers.current.delete(sessionId);
      }
      setLiveEvents((current) => {
        const next = dropEventsForSessions(current, new Set([sessionId]));
        liveEventsRef.current = next;
        return next;
      });
      transcriptLoadedRef.current.delete(sessionId);
    }).then((fn) => {
      unlistenReady = fn;
    });
    void listen<string>(DETACHED_HIDDEN_EVENT, (event) => {
      const label = typeof event.payload === "string" ? event.payload : "";
      const sessionId = sessionIdFromDetachedLabel(label);
      if (!sessionId) return;
      detachedOwnedIdsRef.current.delete(sessionId);
      reloadingSessionsRef.current.add(sessionId);
      const pending = transcriptSaveTimers.current.get(sessionId);
      if (pending) {
        clearTimeout(pending);
        transcriptSaveTimers.current.delete(sessionId);
      }
      transcriptLoadedRef.current.delete(sessionId);
      setLiveEvents((current) => {
        const next = dropEventsForSessions(current, new Set([sessionId]));
        liveEventsRef.current = next;
        return next;
      });
      void loadSessionTranscriptRef.current(sessionId).finally(() => {
        reloadingSessionsRef.current.delete(sessionId);
      });
    }).then((fn) => {
      unlistenHidden = fn;
    });
    return () => {
      unlistenReady?.();
      unlistenHidden?.();
    };
  }, []);

  // Main window only: detached shells hand their dialog back (merge button /
  // drag-over-main drop) → re-adopt the tab and hide the shell.
  useEffect(() => {
    if (IS_DETACHED_WINDOW) return;
    let unlistenMerge: (() => void) | undefined;
    let unlistenGlow: (() => void) | undefined;
    void listenMergeBackRequests(async (sessionId) => {
      detachedOwnedIdsRef.current.delete(sessionId);
      const session = sessionsRef.current.find((s) => s.id === sessionId);
      if (session) {
        setOpenSessionIds((current) =>
          current.includes(session.id) ? current : [...current, session.id]
        );
        openSessionRef.current(session);
        // Re-push status/label so the restored tab matches the live turn.
        void broadcastSessionPatch({
          sessionId: session.id,
          status: session.status,
          label: session.label,
        });
      }
      await hideDetachedWindowForSession(sessionId);
      try {
        const { getCurrentWindow } = await import("@tauri-apps/api/window");
        const win = getCurrentWindow();
        await win.show();
        await win.unminimize();
        await win.setFocus();
      } catch {
        /* optional */
      }
    }).then((fn) => {
      unlistenMerge = fn;
    });
    void listenMergeHighlights(setMergeTargetActive).then((fn) => {
      unlistenGlow = fn;
    });
    return () => {
      unlistenMerge?.();
      unlistenGlow?.();
    };
  }, []);

  /**
   * Drop in-memory transcript for cold sessions (not open, not live ACP).
   * Disk is source of truth; re-open reloads via loadSessionTranscript.
   * Never touches open tabs / streaming / delegate children.
   */
  const pruneColdSessionMemory = useCallback(async () => {
    const hot = collectHotSessionIds({
      currentSessionId: currentSessionIdRef.current,
      openSessionIds: openSessionIdsRef.current,
      sessions: sessionsRef.current,
      delegateMeta: delegateMetaRef.current,
    });
    const cold = coldSessionIdsWithEvents(liveEventsRef.current, hot);
    if (cold.length === 0) return;

    const flushed: string[] = [];
    for (const sessionId of cold) {
      // Don't drop while a debounced save is still pending — flush first.
      const pending = transcriptSaveTimers.current.get(sessionId);
      if (pending) {
        clearTimeout(pending);
        transcriptSaveTimers.current.delete(sessionId);
      }
      const events = persistableEventsForSession(liveEventsRef.current, sessionId);
      try {
        if (isTauriRuntime() && events.length > 0) {
          await writeTranscript(sessionId, events);
        }
        flushed.push(sessionId);
      } catch {
        // Keep memory if disk write failed — never lose the only copy.
      }
    }
    if (flushed.length === 0) return;

    const drop = new Set(flushed);
    for (const id of drop) {
      transcriptLoadedRef.current.delete(id);
    }
    setLiveEvents((current) => {
      const next = dropEventsForSessions(current, drop);
      liveEventsRef.current = next;
      return next;
    });
    setSessionUsageById((current) => {
      let changed = false;
      const next = { ...current };
      for (const id of drop) {
        if (id in next) {
          delete next[id];
          changed = true;
        }
      }
      return changed ? next : current;
    });
    // Activity stamps are tiny; still drop cold ones to avoid unbounded maps.
    setLastActivityById((current) => {
      let changed = false;
      const next = { ...current };
      for (const id of drop) {
        if (id in next) {
          delete next[id];
          changed = true;
        }
      }
      return changed ? next : current;
    });
  }, []);

  // Periodic cold-session prune (open tabs / live turns are never dropped).
  useEffect(() => {
    if (!isTauriRuntime()) return;
    const tick = () => {
      void pruneColdSessionMemory();
    };
    // First pass after idle boot; then every 2 minutes.
    const first = window.setTimeout(tick, 45_000);
    const interval = window.setInterval(tick, 120_000);
    return () => {
      window.clearTimeout(first);
      window.clearInterval(interval);
    };
  }, [pruneColdSessionMemory]);

  // Keep shelf/tabs/title in sync across main + detached windows.
  useEffect(() => {
    if (!isTauriRuntime()) return;
    let unlisten: (() => void) | undefined;
    void listenSessionPatches((patch) => {
      setAvailableSessions((current) => {
        let changed = false;
        const next = current.map((s) => {
          if (s.id !== patch.sessionId) return s;
          const label = patch.label ?? s.label;
          const labelSource = patch.labelSource ?? s.labelSource;
          const status = patch.status ?? s.status;
          // Pin travels as an explicit field: `null` means unpinned, so it cannot
          // use the `?? s.x` fallback the others rely on.
          const pinnedAt =
            patch.pinnedAt === undefined ? s.pinnedAt : patch.pinnedAt;
          if (
            label === s.label &&
            labelSource === s.labelSource &&
            status === s.status &&
            pinnedAt === s.pinnedAt
          ) return s;
          changed = true;
          const processId =
            status === "exited" || status === "error" ? null : s.processId;
          return { ...s, label, labelSource, status, processId, pinnedAt };
        });
        return changed ? next : current;
      });
      if (
        IS_DETACHED_WINDOW &&
        DETACHED_SESSION_ID === patch.sessionId &&
        patch.label
      ) {
        void import("@tauri-apps/api/window")
          .then(({ getCurrentWindow }) => getCurrentWindow().setTitle(patch.label!))
          .catch(() => undefined);
      }
    }).then((fn) => {
      unlisten = fn;
    });
    return () => unlisten?.();
  }, []);

  useEffect(() => {
    if (!isTauriRuntime()) return;
    let disposed = false;
    let unlistenAcp: (() => void) | undefined;

    void listen<AcpEvent>("acp-event", (event) => {
      if (disposed) return;
      const payload = event.payload;
      // Other windows' streams must not rebuild this SPA's liveEvents / disk.
      if (payload.sessionId && !ownsAcpSession(payload.sessionId)) return;
      if (payload.sessionId && reloadingSessionsRef.current.has(payload.sessionId)) return;
      // ACP wire already logged in Rust emit_event → dev.log
      if (payload.sessionId) touchActivity(payload.sessionId);

      // @-delegate children: any event re-arms the 600s idle timeout.
      if (payload.sessionId && delegateMetaRef.current.has(payload.sessionId)) {
        const meta = delegateMetaRef.current.get(payload.sessionId);
        if (meta && !meta.finished) {
          if (meta.idleTimer) clearTimeout(meta.idleTimer);
          meta.idleTimer = setTimeout(() => {
            finalizeDelegateChildRef.current(payload.sessionId, "timeout", "600s 无事件");
          }, DELEGATE_IDLE_TIMEOUT_MS);
        }
      }

      // Did the *turn* end? That — not mere set_config rpc/response — is what
      // proves a cancel landed / a prompt finished (Codeg: TurnComplete only).
      if (
        payload.sessionId &&
        (isTurnCompleteMethod(payload.method) ||
          payload.method === "process/ended" ||
          payload.method === "process/stopped")
      ) {
        turnEndedAtRef.current[payload.sessionId] = Date.now();
        cancelIgnoredRef.current.delete(payload.sessionId);
        // Keep suppressing late Thinking/Reply until a queued follow-up is
        // actually wired — cancel emits turn/complete immediately, before
        // drain creates the next You card. Process death drops the queue.
        const keepSuppressed =
          payload.method !== "process/ended" &&
          payload.method !== "process/stopped" &&
          (pendingSendsRef.current.get(payload.sessionId) ?? []).length > 0;
        if (!keepSuppressed) {
          streamSuppressedRef.current.delete(payload.sessionId);
        }
        void syncFileChangesRef.current(payload.sessionId, true);
        // Finalize @-delegate child when its turn ends.
        if (delegateMetaRef.current.has(payload.sessionId)) {
          if (payload.method === "process/ended" || payload.method === "process/stopped") {
            finalizeDelegateChildRef.current(
              payload.sessionId,
              "cancelled",
              "应用关闭时中断或进程退出",
            );
          } else if (payload.kind === "error" || turnStopReason(payload.data) === "error") {
            finalizeDelegateChildRef.current(
              payload.sessionId,
              "failed",
              formatAcpRpcError(payload.data) || "rpc error",
            );
          } else if (turnStopReason(payload.data) === "cancelled") {
            finalizeDelegateChildRef.current(
              payload.sessionId,
              "cancelled",
              "interrupted",
            );
          } else {
            finalizeDelegateChildRef.current(payload.sessionId, "done");
          }
        }
      }

      // Per-model context ceiling, so `used / size` works for agents that never
      // send usage_update (Grok). Never overwrites a live usage_update size.
      if (payload.kind === "system" && payload.method === "session/ready") {
        const size = (payload.data as { contextSize?: unknown } | null)?.contextSize;
        setSessionUsageById((current) => {
          const seeded = applyModelContextSize(
            current[payload.sessionId],
            typeof size === "number" ? size : null
          );
          if (!seeded) return current;
          return { ...current, [payload.sessionId]: seeded };
        });
        const title = extractAcpSessionTitle(payload.data);
        if (title) applyAgentSessionTitle(payload.sessionId, title);
      }

      // Live transcript: thinking / tool / assistant stream as events arrive
      if (payload.method === "session/update") {
        // Context window / cost / vendor rate-limit meta
        setSessionUsageById((current) => {
          const merged = mergeUsageFromAcp(current[payload.sessionId], payload.data);
          if (!merged) return current;
          return { ...current, [payload.sessionId]: merged };
        });

        const update = getSessionUpdate(payload.data);
        const title = extractAcpSessionTitle(payload.data);
        if (title) applyAgentSessionTitle(payload.sessionId, title);

        // ACP slash command catalogue for Composer `/` autocomplete.
        const slashList = parseAvailableCommandsUpdate(payload.data);
        if (slashList) {
          setSlashCommandsById((current) => ({
            ...current,
            [payload.sessionId]: slashList,
          }));
        }

        // ACP plan (TodoWrite / plan tool) — full replace per update.
        const planEntries = parseAcpPlanUpdate(payload.data);
        if (planEntries) {
          setPlanBySessionId((current) => ({
            ...current,
            [payload.sessionId]: planEntries,
          }));
        }

        const extracted = extractAcpUpdateText(payload.data);
        if (
          extracted &&
          (extracted.role === "assistant" ||
            extracted.role === "thought" ||
            extracted.role === "tool" ||
            extracted.role === "system")
        ) {
          // User hit Interrupt: drop late Thinking/Reply so they cannot glue
          // onto the sealed Interrupted notification. Tool status still lands.
          const streamSuppressed =
            streamSuppressedRef.current.has(payload.sessionId) &&
            (extracted.role === "assistant" || extracted.role === "thought");
          if (!streamSuppressed) {
            // Codex `/status` / Claude `/usage` embed rate-limit lines in assistant text.
            if (extracted.role === "assistant" && extracted.text) {
              setSessionUsageById((current) => {
                const merged = mergeUsageFromText(current[payload.sessionId], extracted.text);
                if (!merged) return current;
                return { ...current, [payload.sessionId]: merged };
              });
            }
            if (extracted.role === "tool" && isToolCompletionStatus(extracted.toolStatus)) {
              // Match codeg-main's live tool-result path: refresh the workspace
              // as soon as an edit/write/apply_patch-like tool settles.
              void syncFileChangesRef.current(payload.sessionId);
            }
            if (extracted.text) {
              // Status-only blocks — usage already merged above; keep the chat
              // rail free of Codex `/status` and Claude `/usage` dumps.
              const isRuntimeStatusOnly =
                isRuntimeMetadataOnly(extracted.text) ||
                (/^\s*\*{0,2}Model\*{0,2}:/i.test(extracted.text) &&
                  /\n\s*\*{0,2}Directory\*{0,2}:/i.test(extracted.text));
              if (!isRuntimeStatusOnly) {
                setLiveEvents((current) => {
                  const sid = payload.sessionId;
                  const prevIdx = findLastIndexForSession(current, sid);
                  const prevLast = prevIdx >= 0 ? current[prevIdx] : undefined;
                  const prevLen = current.length;
                  let next = applyAcpPartToEvents(current, sid, extracted);
                  // Track turn start for duration: first new assistant_message card
                  // for *this* session (other sessions may sit at the rail tail).
                  if (extracted.role === "assistant") {
                    const newIdx = findLastIndexForSession(next, sid);
                    const newLast = newIdx >= 0 ? next[newIdx] : undefined;
                    if (
                      newLast?.type === "assistant_message" &&
                      newLast.sessionId === sid &&
                      (prevLast?.type !== "assistant_message" ||
                        prevLast.sessionId !== sid) &&
                      !turnStartedAtRef.current[sid]
                    ) {
                      turnStartedAtRef.current[sid] = Date.now();
                      // Clear sendMetaRef — snapshot consumed by the first assistant chunk
                      sendMetaRef.current = null;
                    }
                  }
                  // Glue fragment thoughts / token-split Replies only when a new
                  // card appeared. applyAcpPartToEvents already appends onto an
                  // open Thought/Reply (including Grok messageId churn); running
                  // full-rail coalesce on every token was O(n) per chunk and
                  // helped freeze the window during long Grok streams.
                  if (
                    next.length > prevLen &&
                    (extracted.role === "thought" || extracted.role === "assistant")
                  ) {
                    next = coalesceAdjacentThoughts(next, sid);
                    if (extracted.role === "assistant") {
                      next = coalesceAdjacentAssistantFragments(next, sid);
                    }
                  }
                  return next;
                });
              }
            }
          }
        }
      }

      // Grok reports usage on `_x.ai/session_notification`.
      // `response_completed` arrives once per *model call* (a turn with a tool
      // loop emits several) — accumulate into per-turn totals. Any other
      // payload (e.g. a turn-level `turn_completed.usage`) merges as before
      // and overrides the accumulation at turn end.
      if (payload.method === "_x.ai/session_notification") {
        const call = parseGrokCallUsage(payload.data);
        if (call) {
          const sid = payload.sessionId;
          grokTurnUsageRef.current[sid] = mergeGrokCallUsage(
            grokTurnUsageRef.current[sid],
            call
          );
          const tokens = grokTurnUsageToTurnTokens(grokTurnUsageRef.current[sid]);
          if (tokens) {
            setSessionUsageById((current) => ({
              ...current,
              [sid]: applyGrokLiveContext(current[sid], tokens),
            }));
          }
          return;
        }
        setSessionUsageById((current) => {
          const merged = mergeUsageFromPromptResult(current[payload.sessionId], payload.data);
          if (!merged) return current;
          return { ...current, [payload.sessionId]: merged };
        });
      }

      // Codeg-aligned: only `turn/complete` ends a prompt turn. Generic
      // `rpc/response` (set_config / model / mode / effort) must not unlock
      // the composer or clear the Working bar while the agent is still busy.
      if (isTurnCompleteMethod(payload.method)) {
        const stopReason = turnStopReason(payload.data);
        const wasRunning =
          sessionsRef.current.find((s) => s.id === payload.sessionId)?.status === "running";

        // End-of-turn token split. Most agents put it in the prompt RPC
        // response; Grok puts nothing there — its usage arrives per model call
        // on `_x.ai/session_notification`, so fall back to that accumulation.
        const promptTokens = extractTurnTokens(payload.data);
        const grokAccum = grokTurnUsageToTurnTokens(
          grokTurnUsageRef.current[payload.sessionId]
        );
        const usedGrokAccum = promptTokens == null && grokAccum != null;
        const turnTokens = promptTokens ?? grokAccum;
        delete grokTurnUsageRef.current[payload.sessionId];
        const firstChunkAt = turnStartedAtRef.current[payload.sessionId] ?? null;
        const sentAt = turnSentAtRef.current[payload.sessionId] ?? null;
        delete turnSentAtRef.current[payload.sessionId];
        const endedAt = Date.now();
        // Timings are local and may be missing (e.g. a turn with no streamed
        // assistant text); build stats whenever tokens exist — buildTurnStats
        // leaves the absent timings null rather than inventing numbers.
        const turnStats =
          turnTokens != null
            ? buildTurnStats(turnTokens, { sentAt, firstChunkAt, endedAt })
            : null;

        // Stamp events first (and sync the ref) so later transcript restore
        // can seed lastTurnStats from this turn.
        const startedAt = firstChunkAt;
        const toolClose =
          stopReason === "cancelled"
            ? ("cancelled" as const)
            : stopReason === "error" || payload.kind === "error"
              ? ("failed" as const)
              : null;
        if (startedAt) {
          const durationMs = Date.now() - startedAt;
          delete turnStartedAtRef.current[payload.sessionId];
          setLiveEvents((current) => {
            const sid = payload.sessionId;
            let next = current;
            if (toolClose) next = markOpenTools(next, sid, toolClose, endedAt);
            // Stamp this session's open Reply — not the absolute rail tail
            // (another window's stream may be last in the shared array).
            const lastIdx = findLastIndexForSession(next, sid);
            const last = lastIdx >= 0 ? next[lastIdx] : undefined;
            // The turn's last card is where its final step ends, so it carries
            // the absolute end. `durationMs` keeps its own meaning (it seals a
            // Reply for the coalescer), which is why a Thought card gets the
            // end stamp only.
            const isReply = last?.type === "assistant_message" && last.sessionId === sid;
            const isThought = last?.type === "thought" && last.sessionId === sid;
            const sealedReply = isReply && last.durationMs == null;
            const endsHere = (isReply || isThought) && last.endedAt == null;
            if (sealedReply || endsHere) {
              const stamped = [...next];
              stamped[lastIdx] = {
                ...last,
                ...(sealedReply ? { durationMs, ...(turnStats ? { turnStats } : {}) } : {}),
                ...(endsHere ? { endedAt: new Date(endedAt).toISOString() } : {}),
              };
              next = collapseIntermediateAssistantAsThought(stamped, sid);
            } else {
              next = collapseIntermediateAssistantAsThought(next, sid);
            }
            liveEventsRef.current = next;
            return next;
          });
        } else {
          setLiveEvents((current) => {
            const next = collapseIntermediateAssistantAsThought(
              toolClose
                ? markOpenTools(current, payload.sessionId, toolClose)
                : current,
              payload.sessionId,
            );
            liveEventsRef.current = next;
            return next;
          });
        }

        setSessionUsageById((current) => {
          const base = current[payload.sessionId];
          const merged = mergeUsageFromPromptResult(base, payload.data);
          let state = merged ?? base;
          if (!state && (turnTokens || turnStats)) {
            state = emptySessionUsage();
          }
          if (!state) return current;
          const tokens = state.turnTokens ?? turnTokens;
          if (!tokens && !turnStats) return current;
          // Grok has no usage_update; the latest call's input (incl. cache)
          // is tokens currently in context and must overwrite, not seed-once.
          const next = usedGrokAccum && tokens
            ? applyGrokLiveContext(state, tokens)
            : { ...state, turnTokens: tokens };
          return {
            ...current,
            [payload.sessionId]: {
              ...next,
              lastTurnStats: turnStats ?? next.lastTurnStats,
            },
          };
        });

        // Auth / hard turn failures → error; cancel & clean end → waiting.
        // Use setSessionStatusById so disk + detached windows leave Interrupt mode.
        let turnedIdle = false;
        {
          const prev = sessionsRef.current.find((s) => s.id === payload.sessionId);
          if (prev && (prev.status === "running" || prev.status === "starting")) {
            const nextStatus: Session["status"] =
              payload.kind === "error" || stopReason === "error" || stopReason === "refusal"
                ? "error"
                : "waiting";
            setSessionStatusById(payload.sessionId, nextStatus);
            turnedIdle = nextStatus === "waiting";
          }
        }

        // A clean turn proves auth is fine — drop any auth banner for this agent.
        if (payload.kind !== "error" && stopReason !== "error" && stopReason !== "refusal") {
          const sessAgentId = sessionsRef.current.find((s) => s.id === payload.sessionId)?.agentId;
          if (sessAgentId) setAuthHintFor(sessAgentId, null);
        }

        // Usage panel: refresh once after each completed Reply turn.
        refreshUsageAfterTurnRef.current(payload.sessionId);
        // Desktop notify only for a real completed turn (was running, not cancel).
        // Clean completion follows OpenCode's 350ms idle-settle window so a
        // queued follow-up can cancel the intermediate completion alert.
        if (wasRunning && stopReason !== "cancelled") {
          const sess = sessionsRef.current.find((s) => s.id === payload.sessionId);
          const label = sess?.label?.trim() || "Session";
          const isDelegateChild = delegateMetaRef.current.has(payload.sessionId);
          const turnFailed =
            payload.kind === "error" ||
            stopReason === "error" ||
            stopReason === "refusal";
          // No Reply card at the tail means the agent stopped on its own without
          // anything to read (empty turn, or tools and then silence). That stop is
          // invisible on screen, so its chime also plays while the window is focused.
          const stoppedSilently = turnEndedSilently(
            liveEventsRef.current,
            payload.sessionId,
          );
          if (!isDelegateChild) {
            if (turnFailed) {
              const errorDetail = compactNotifyDetail(formatAcpRpcError(payload.data));
              void raiseDesktopNotify(
                "error",
                errorDetail ? `${label} · ${errorDetail}` : `${label} · Agent error`,
              );
            } else if (stoppedSilently) {
              scheduleDesktopNotify(
                payload.sessionId,
                "idle",
                `${label} · agent stopped`,
                DESKTOP_NOTIFY_SETTLE_MS,
                { audibleWhenFocused: true },
              );
            } else {
              scheduleDesktopNotify(payload.sessionId, "reply", label);
            }
          }
        }
        pushDebug({
          sessionId: payload.sessionId,
          level: payload.kind === "error" ? "error" : "info",
          source: "acp",
          summary: `turn/complete stopReason=${stopReason}`,
        });
        // Drain follow-ups the user typed while this turn was live.
        // Interrupt may already have set status to waiting before this event —
        // still try drain; the helper no-ops when empty or still busy.
        if (turnedIdle || stopReason === "cancelled" || stopReason === "end_turn" || !stopReason) {
          queueMicrotask(() => drainQueuedSendRef.current(payload.sessionId));
        }
      }
      // Agent process stdout closed (crash / exit) — never leave the UI "Working" forever.
      if (payload.method === "process/ended" || payload.method === "process/stopped") {
        const endedHard = payload.method === "process/ended";
        const detail =
          payload.data && typeof payload.data === "object"
            ? String((payload.data as { message?: unknown }).message ?? "Agent process ended")
            : endedHard
              ? "Agent process ended"
              : "Agent process stopped";
        // Drop queued follow-ups — the process cannot receive them anymore.
        pendingSendsRef.current.delete(payload.sessionId);
        flushingSendRef.current.delete(payload.sessionId);
        setQueuedStripTick((t) => t + 1);
        // Zombie Ask / Plan / Permission cards block the composer — clear for this session.
        setAskPrompt((cur) => (cur?.sessionId === payload.sessionId ? null : cur));
        setPlanApproval((cur) => (cur?.sessionId === payload.sessionId ? null : cur));
        setPlanApprovalBusy(false);
        setPermissionPrompt((cur) => (cur?.sessionId === payload.sessionId ? null : cur));
        if (endedHard) {
          setLiveEvents((current) => {
            const sid = payload.sessionId;
            const lastIdx = findLastIndexForSession(current, sid);
            const last = lastIdx >= 0 ? current[lastIdx] : undefined;
            if (
              last?.type === "assistant_message" &&
              last.sessionId === sid &&
              last.text.includes("Agent process ended")
            ) {
              return markOpenTools(current, sid, "failed");
            }
            return [
              ...markOpenTools(current, sid, "failed"),
              {
                type: "assistant_message" as const,
                sessionId: sid,
                text: `**Agent process ended.**\n\n${detail}\n\nThe turn is no longer live. Warm the agent again (focus composer / send) or start a new session.`,
                createdAt: new Date().toISOString(),
              },
            ];
          });
        }
        // Always leave running/starting — intentional stop (agent switch) or crash.
        // turn/complete usually arrived first; this is the belt for races.
        {
          const prev = sessionsRef.current.find((s) => s.id === payload.sessionId);
          if (
            prev &&
            (prev.status === "running" ||
              prev.status === "starting" ||
              prev.status === "waiting" ||
              (endedHard && prev.status === "error"))
          ) {
            setSessionStatusById(payload.sessionId, "exited");
          }
        }
        if (payload.sessionId === currentSessionIdRef.current) {
          setSessionCapabilities(null);
        }
        if (endedHard) {
          // A hard process exit is an error even when it raced the turn-end
          // event. Reuse the error channel so the 1s debounce coalesces the
          // two signals instead of chiming twice for one crash.
          cancelScheduledDesktopNotify(payload.sessionId);
          const sess = sessionsRef.current.find((s) => s.id === payload.sessionId);
          const label = sess?.label?.trim() || "Session";
          const processDetail = compactNotifyDetail(detail);
          void raiseDesktopNotify(
            "error",
            processDetail
              ? `${label} · Agent process ended: ${processDetail}`
              : `${label} · Agent process ended`,
          );
        }
        // Drop dead process bookkeeping so the next warm can respawn cleanly.
        void stopAcpSession(payload.sessionId).catch(() => undefined);
        pushDebug({
          sessionId: payload.sessionId,
          level: endedHard ? "error" : "info",
          source: "acp",
          summary: payload.method,
          detail,
        });
      }

      // Turn-level failures only. set_config / probe rpc errors must not flip
      // the whole session to "error" or spam Clean (that was part of the fragile
      // "model switch ended the chat" experience).
      if (payload.kind === "error" && isTurnCompleteMethod(payload.method)) {
        // Surface auth/turn failures in Clean (Claude often returns
        // { error: { message: "Authentication required" } } with no message chunks).
        const errText = formatAcpRpcError(payload.data);
        if (errText) {
          const sessAgentId = sessionsRef.current.find((s) => s.id === payload.sessionId)?.agentId;
          const sessAgent = sessAgentId
            ? agentsRef.current.find((a) => a.id === sessAgentId)
            : undefined;
          const classified = classifyAgentError(errText, {
            agentId: sessAgentId,
            agentLabel: sessAgent?.label,
          });
          let body = formatClassifiedError(classified);
          setComposerFailure({ sessionId: payload.sessionId, error: classified });
          if (classified.kind === "auth") {
            // Per-agent banner: some agents (CodeBuddy) only prove auth via ACP error.
            const spec = sessAgentId ? agentAuthSpec(sessAgentId) : undefined;
            if (spec) {
              setAuthHintFor(
                sessAgentId!,
                spec.login
                  ? `需要登录 — 点 Sign in，或终端执行 \`${spec.loginCommand}\``
                  : `需要登录 — 终端执行 \`${spec.loginCommand}\``
              );
            }
          }
          setLiveEvents((current) => {
            // Avoid spamming the same auth error on every retry.
            // Session-scoped: another window's event may own the absolute tail.
            const lastIdx = findLastIndexForSession(current, payload.sessionId);
            const last = lastIdx >= 0 ? current[lastIdx] : undefined;
            if (
              last?.type === "assistant_message" &&
              last.sessionId === payload.sessionId &&
              last.text.includes(errText)
            ) {
              return current;
            }
            return [
              ...current,
              {
                type: "assistant_message" as const,
                sessionId: payload.sessionId,
                text: body,
                createdAt: new Date().toISOString(),
              },
            ];
          });
          pushDebug({
            sessionId: payload.sessionId,
            level: "error",
            source: "acp",
            summary: `surface error to Clean: ${classified.kind}: ${errText}`,
          });
        }
        // status already set in the turn/complete branch above when kind=error
      } else if (payload.kind === "error" && payload.method === "rpc/response") {
        pushDebug({
          sessionId: payload.sessionId,
          level: "warn",
          source: "acp",
          summary: `non-turn RPC error (ignored for session status): ${formatAcpRpcError(payload.data) || "unknown"}`,
        });
      }

      // ACP permission prompt (P1-D) — not auto-allowed.
      if (payload.method === "permission/prompt" && payload.data && typeof payload.data === "object") {
        const data = payload.data as Record<string, unknown>;
        const requestId = typeof data.requestId === "string" ? data.requestId : "";
        if (requestId) {
          const rawOptions = Array.isArray(data.options) ? data.options : [];
          const options = rawOptions
            .map((opt) => {
              if (!opt || typeof opt !== "object") return null;
              const o = opt as Record<string, unknown>;
              const optionId = typeof o.optionId === "string" ? o.optionId : "";
              if (!optionId) return null;
              return {
                optionId,
                name: typeof o.name === "string" ? o.name : optionId,
                kind: typeof o.kind === "string" ? o.kind : "",
              };
            })
            .filter((o): o is NonNullable<typeof o> => o != null);
          const title = typeof data.title === "string" ? data.title : "Permission required";
          const detail = typeof data.detail === "string" ? data.detail : null;
          setPermissionPrompt({
            requestId,
            sessionId: payload.sessionId,
            title,
            detail,
            options,
          });
          cancelScheduledDesktopNotify(payload.sessionId);
          void raiseDesktopNotify(
            "permission",
            compactNotifyDetail(detail ? `${title}: ${detail}` : title),
          );
        }
      }
      if (payload.method === "permission/timeout") {
        setPermissionPrompt((current) => {
          if (!current) return current;
          const data = payload.data as { requestId?: string } | null;
          if (data?.requestId && data.requestId === current.requestId) return null;
          return current;
        });
      }

      // Grok `_x.ai/ask_user_question` → interactive choice card
      if (payload.method === "question/prompt" && payload.data) {
        const parsed = parseAskQuestionPrompt(payload.sessionId, payload.data);
        if (parsed) {
          setAskPrompt(parsed);
          // Receipt on disk: an ask that never reaches the screen is otherwise
          // invisible in dev.log (Rust only logs that it emitted the request).
          pushDebug({
            sessionId: payload.sessionId,
            level: "info",
            source: "question",
            summary: `ask card ready · ${parsed.questions.length} question(s)`,
            detail: parsed.requestId,
          });
          cancelScheduledDesktopNotify(payload.sessionId);
          const question = compactNotifyDetail(parsed.questions[0]?.question);
          void raiseDesktopNotify(
            "question",
            question ? `Agent question: ${question}` : "Agent has a question",
          );
        } else {
          pushDebug({
            sessionId: payload.sessionId,
            level: "warn",
            source: "question",
            summary: "question/prompt arrived but did not parse — no card shown",
            detail: JSON.stringify(payload.data).slice(0, 800),
          });
        }
      }
      if (payload.method === "question/timeout") {
        setAskPrompt((current) => {
          if (!current) return current;
          const data = payload.data as { requestId?: string } | null;
          if (data?.requestId && data.requestId === current.requestId) return null;
          return current;
        });
      }

      // Grok `_x.ai/exit_plan_mode` → plan approval card (Codeg wire format)
      if (payload.method === "plan/approval" && payload.data && typeof payload.data === "object") {
        const data = payload.data as Record<string, unknown>;
        const requestId = typeof data.requestId === "string" ? data.requestId : "";
        if (requestId) {
          const planMarkdown =
            typeof data.planMarkdown === "string"
              ? data.planMarkdown
              : typeof data.plan_markdown === "string"
                ? data.plan_markdown
                : "";
          setPlanApproval({
            requestId,
            sessionId: payload.sessionId,
            toolCallId:
              typeof data.toolCallId === "string"
                ? data.toolCallId
                : typeof data.tool_call_id === "string"
                  ? data.tool_call_id
                  : null,
            planMarkdown,
          });
          cancelScheduledDesktopNotify(payload.sessionId);
          void raiseDesktopNotify("plan", "Plan ready for review");
        }
      }
      if (payload.method === "plan/timeout") {
        setPlanApproval((current) => {
          if (!current) return current;
          const data = payload.data as { requestId?: string } | null;
          if (data?.requestId && data.requestId === current.requestId) return null;
          return current;
        });
      }

      // Live mode chip: Grok emits current_mode_update after session/set_mode
      if (payload.method === "session/update" && payload.data) {
        const update = getSessionUpdate(payload.data);
        const kind = update
          ? getSessionUpdateKind(update).toLowerCase().replace(/-/g, "_")
          : "";
        if (update && (kind === "current_mode_update" || kind === "currentmodeupdate")) {
          const modeId =
            (typeof update.currentModeId === "string" && update.currentModeId) ||
            (typeof update.current_mode_id === "string" && update.current_mode_id) ||
            null;
          if (modeId && payload.sessionId === currentSessionIdRef.current) {
            setSessionCapabilities((caps) =>
              caps ? { ...caps, currentMode: modeId } : caps,
            );
          }
        }
      }
    }).then((dispose) => {
      if (disposed) {
        dispose();
        return;
      }
      unlistenAcp = dispose;
      if (IS_DETACHED_WINDOW && DETACHED_SESSION_ID) {
        void setDetachedSessionOwner(DETACHED_SESSION_ID, true);
        void emit(DETACHED_READY_EVENT, {
          sessionId: DETACHED_SESSION_ID,
        } satisfies DetachedReadyPayload);
      }
    });

    return () => {
      disposed = true;
      if (IS_DETACHED_WINDOW && DETACHED_SESSION_ID) {
        void setDetachedSessionOwner(DETACHED_SESSION_ID, false);
      }
      unlistenAcp?.();
      for (const t of cancelWatchdogsRef.current.values()) clearTimeout(t);
      cancelWatchdogsRef.current.clear();
    };
  }, [applyAgentSessionTitle, pushDebug, touchActivity, setAuthHintFor]);
  // note: pushDebug is stable via useCallback

  // Detached close: flush JSONL then give the ACP stream back to main.
  useEffect(() => {
    if (!isTauriRuntime() || !IS_DETACHED_WINDOW || !DETACHED_SESSION_ID) return;
    let unlisten: (() => void) | undefined;
    void listen<string>(DETACHED_HIDDEN_EVENT, (event) => {
      const label = typeof event.payload === "string" ? event.payload : "";
      if (sessionIdFromDetachedLabel(label) !== DETACHED_SESSION_ID) return;
      const events = persistableEventsForSession(
        liveEventsRef.current,
        DETACHED_SESSION_ID,
      );
      const release = () => {
        void setDetachedSessionOwner(DETACHED_SESSION_ID, false);
      };
      if (events.length === 0) {
        release();
        return;
      }
      void writeTranscript(DETACHED_SESSION_ID, events).finally(release);
    }).then((fn) => {
      unlisten = fn;
    });
    return () => unlisten?.();
  }, []);

  /** Debounced rewrite of Clean transcript JSONL per session. */
  useEffect(() => {
    if (!isTauriRuntime()) return;
    const sessionIds = new Set(liveEvents.map((e) => e.sessionId));
    for (const sessionId of sessionIds) {
      if (!ownsAcpSession(sessionId)) continue;
      if (reloadingSessionsRef.current.has(sessionId)) continue;
      const prev = transcriptSaveTimers.current.get(sessionId);
      if (prev) clearTimeout(prev);
      const timer = setTimeout(() => {
        transcriptSaveTimers.current.delete(sessionId);
        const events = persistableEventsForSession(liveEventsRef.current, sessionId);
        void writeTranscript(sessionId, events).catch((error) => {
          pushDebug({
            sessionId,
            level: "warn",
            source: "transcript",
            summary: "write transcript failed",
            detail: error instanceof Error ? error.message : String(error),
          });
        });
      }, 450);
      transcriptSaveTimers.current.set(sessionId, timer);
    }
  }, [liveEvents, pushDebug]);

  const renameSessionFromText = useCallback((sessionId: string, text: string) => {
    const session = sessionsRef.current.find((s) => s.id === sessionId);
    if (!session || session.labelSource === "manual" || session.labelSource === "user" || session.labelSource === "agent") return;
    // New rows carry an explicit default source. Legacy rows still use the
    // old placeholder-label heuristic so an existing manual title is not
    // unexpectedly replaced after upgrading.
    if (session.labelSource !== "default" && !shouldAutoRenameLabel(session.label)) return;
    const label = titleFromUserText(text);
    setAvailableSessions((current) =>
      current.map((s) => (s.id === sessionId ? { ...s, label, labelSource: "user" } : s))
    );
    void updateSessionLabel(sessionId, label, "user").catch(() => undefined);
    void broadcastSessionPatch({ sessionId, label, labelSource: "user" });
    if (IS_DETACHED_WINDOW && DETACHED_SESSION_ID === sessionId) {
      void import("@tauri-apps/api/window")
        .then(({ getCurrentWindow }) => getCurrentWindow().setTitle(label))
        .catch(() => undefined);
    }
  }, []);

  /** Manual rename (shelf / tab) — always persists, stops future auto-title. */
  const handleRenameSession = useCallback((sessionId: string, label: string) => {
    const next = label.trim() || "New session";
    manuallyRenamedSessionIdsRef.current.add(sessionId);
    setAvailableSessions((current) =>
      current.map((s) => (s.id === sessionId ? { ...s, label: next, labelSource: "manual" } : s))
    );
    void updateSessionLabel(sessionId, next, "manual").catch(() => undefined);
    void broadcastSessionPatch({ sessionId, label: next, labelSource: "manual" });
    if (IS_DETACHED_WINDOW && DETACHED_SESSION_ID === sessionId) {
      void import("@tauri-apps/api/window")
        .then(({ getCurrentWindow }) => getCurrentWindow().setTitle(next))
        .catch(() => undefined);
    }
  }, []);

  /**
   * Pin / unpin a dialog in the left shelf.
   *
   * Optimistic: the shelf sorts on `pinnedAt`, so the row has to move on click.
   * The disk write is the source of truth — its timestamp replaces the local
   * guess, and a failure rolls the row back instead of leaving a lie on screen.
   */
  const handleToggleSessionPin = useCallback((sessionId: string, pinned: boolean) => {
    const optimistic = pinned ? String(Date.now()) : null;
    const before = sessionsRef.current.find((s) => s.id === sessionId)?.pinnedAt ?? null;
    setAvailableSessions((current) =>
      current.map((s) => (s.id === sessionId ? { ...s, pinnedAt: optimistic } : s))
    );
    void setSessionPinned(sessionId, pinned)
      .then((saved) => {
        if (saved) {
          setAvailableSessions((current) =>
            current.map((s) => (s.id === sessionId ? { ...s, ...saved } : s))
          );
          void broadcastSessionPatch({ sessionId, pinnedAt: saved.pinnedAt ?? null });
        }
      })
      .catch((error) => {
        setAvailableSessions((current) =>
          current.map((s) => (s.id === sessionId ? { ...s, pinnedAt: before } : s))
        );
        pushDebug({
          sessionId,
          level: "warn",
          source: "shelf",
          summary: pinned ? "pin session failed" : "unpin session failed",
          detail: error instanceof Error ? error.message : String(error),
        });
      });
  }, [pushDebug]);

  const loadSessionTranscript = useCallback(async (sessionId: string) => {
    if (!isTauriRuntime()) return;
    if (transcriptLoadedRef.current.has(sessionId)) return;
    transcriptLoadedRef.current.add(sessionId);
    const raw = await loadTranscript(sessionId);
    let parsed = parseTranscriptEvents(raw);
    if (parsed.length === 0) return;
    // If the agent process is gone, close orphan pending/in_progress tools so
    // Clean View does not treat a multi-hundred-tool history as still working
    // (that path used to force-open shell cards and freeze the WebView).
    const sess = sessionsRef.current.find((s) => s.id === sessionId);
    const live =
      sess != null &&
      (sess.status === "running" ||
        sess.status === "starting" ||
        sess.status === "waiting");
    if (
      !live &&
      parsed.some(
        (e) => e.type === "tool_call" && isToolInProgress(e.status),
      )
    ) {
      parsed = markOpenTools(parsed, sessionId, "cancelled");
      void writeTranscript(
        sessionId,
        persistableEventsForSession(parsed, sessionId),
      ).catch(() => undefined);
    }
    setLiveEvents((current) => {
      const hasLive = current.some((e) => e.sessionId === sessionId);
      if (hasLive) return current;
      const next = [...current.filter((e) => e.sessionId !== sessionId), ...parsed];
      liveEventsRef.current = next;
      return next;
    });
    // Seed lastTurnStats from persisted transcript so a restored dialog
    // still has per-turn token splits in memory (the Usage panel no longer
    // renders those rows).
    let lastStats: TurnStats | null = null;
    for (let i = parsed.length - 1; i >= 0; i -= 1) {
      const e = parsed[i];
      if (e.type === "assistant_message" && e.sessionId === sessionId && e.turnStats) {
        lastStats = e.turnStats;
        break;
      }
    }
    if (lastStats || cumulativeFromEvents(parsed, sessionId)) {
      setSessionUsageById((current) => {
        const base = current[sessionId] ?? emptySessionUsage();
        // Live session already has fresher in-memory usage — don't clobber.
        if (current[sessionId]?.lastTurnStats && current[sessionId]?.turnTokens) {
          return current;
        }
        return {
          ...current,
          [sessionId]: {
            ...base,
            lastTurnStats: base.lastTurnStats ?? lastStats,
            turnTokens:
              base.turnTokens ??
              (lastStats
                ? {
                    input: lastStats.input ?? null,
                    output: lastStats.output ?? null,
                    cached: lastStats.cached ?? null,
                    reasoning: lastStats.reasoning ?? null,
                    total: lastStats.total ?? null,
                  }
                : null),
            source: base.source ?? "transcript turnStats",
          },
        };
      });
    }
  }, []);
  loadSessionTranscriptRef.current = loadSessionTranscript;

  // Restore Clean history whenever the active dialog changes.
  useEffect(() => {
    if (!currentSessionId || currentSessionId.startsWith("session-empty-")) return;
    void loadSessionTranscript(currentSessionId);
  }, [currentSessionId, loadSessionTranscript]);

  // Quote pins belong to one dialog only.
  useEffect(() => {
    setQuotePins([]);
  }, [currentSessionId]);

  const handleShelfSearch = useCallback(async (query: string) => {
    const q = query.trim();
    const requestId = ++searchRequestRef.current;
    if (!q) {
      setSearchHitIds(null);
      return;
    }
    // Metadata filtering is immediate in ProjectShelf. Clear the previous
    // transcript result while this query is being read from disk.
    setSearchHitIds(null);
    const hits = await searchSessions(q);
    if (requestId !== searchRequestRef.current) return;

    // The transcript writer is debounced, so include the current in-memory
    // turn as well. This makes a just-sent phrase searchable immediately.
    const needle = q.toLowerCase();
    const liveHits = new Set<string>();
    for (const event of liveEventsRef.current) {
      let searchable = "";
      switch (event.type) {
        case "user_message":
        case "assistant_message":
        case "thought":
        case "tool_call":
          searchable = event.text;
          break;
        case "file_change":
          searchable = event.path;
          break;
        case "handoff_prepared":
          searchable = `${event.prompt} ${event.targetAgentId}`;
          break;
        case "subtask_started":
          searchable = `${event.prompt} ${event.agentLabel}`;
          break;
        case "subtask_result":
          searchable = `${event.summary} ${event.error ?? ""}`;
          break;
        default:
          break;
      }
      if (searchable.toLowerCase().includes(needle)) liveHits.add(event.sessionId);
    }
    setSearchHitIds([...new Set([...hits, ...liveHits])]);
  }, []);

  const refreshAgentAuth = useCallback(
    async (agentId: string) => {
      const spec = agentAuthSpec(agentId);
      if (!spec?.probe) return;
      const probe = await probeAgentAuth(agentId);
      if (!probe) return;
      if (probe.status === "logged_in") {
        setAuthHintFor(agentId, null);
        if (authPollRef.current) {
          clearInterval(authPollRef.current);
          authPollRef.current = null;
        }
        setSignInBusy(false);
      } else if (probe.status === "logged_out") {
        setAuthHintFor(
          agentId,
          probe.message ||
            (spec.login
              ? `需要登录 — 点 Sign in，或终端执行 \`${spec.loginCommand}\``
              : `需要登录 — 终端执行 \`${spec.loginCommand}\``)
        );
      }
      // "unknown" (e.g. CodeBuddy OS-keyring) keeps whatever banner is up.
    },
    [setAuthHintFor]
  );

  // Probe the active agent's login state whenever the session/agent changes.
  useEffect(() => {
    const agentId =
      availableSessions.find((s) => s.id === currentSessionId)?.agentId ??
      availableAgents[0]?.id ??
      "";
    if (!agentId) return;
    if (!agentAuthSpec(agentId)?.probe) {
      setAuthHintFor(agentId, null);
      return;
    }
    let cancelled = false;
    void refreshAgentAuth(agentId).then(() => {
      if (cancelled) return;
    });
    return () => {
      cancelled = true;
      if (authPollRef.current) {
        clearInterval(authPollRef.current);
        authPollRef.current = null;
      }
    };
  }, [availableAgents, availableSessions, currentSessionId, refreshAgentAuth, setAuthHintFor]);

  const handleAgentSignIn = useCallback(
    async (agentId: string) => {
      const spec = agentAuthSpec(agentId);
      if (!spec?.login) return;
      setSignInBusy(true);
      pushDebug({
        level: "info",
        source: "auth",
        summary: `start ${agentId} login`,
      });
      const result = await startAgentLogin(agentId);
      if (!result?.started) {
        setSignInBusy(false);
        setAuthHintFor(
          agentId,
          result?.message ||
            `无法启动登录。请在终端执行 \`${spec.loginCommand}\`。`
        );
        return;
      }
      setAuthHintFor(
        agentId,
        result.message || "请在浏览器/终端完成登录，然后回到这里（会自动检测）。"
      );
      // Poll until logged in or timeout (~2 min).
      if (authPollRef.current) clearInterval(authPollRef.current);
      let ticks = 0;
      authPollRef.current = setInterval(() => {
        ticks += 1;
        void refreshAgentAuth(agentId);
        if (ticks >= 40) {
          if (authPollRef.current) {
            clearInterval(authPollRef.current);
            authPollRef.current = null;
          }
          setSignInBusy(false);
        }
      }, 3000);
    },
    [pushDebug, refreshAgentAuth, setAuthHintFor]
  );

  const refreshProviderBalance = useCallback(
    async (sessionId: string, modelId: string | null | undefined) => {
      if (!sessionId || providerProbeInflight.current) return;
      providerProbeInflight.current = true;
      try {
        const probe = await probeProviderUsage(modelId);
        if (!probe) return;
        setSessionUsageById((current) => ({
          ...current,
          [sessionId]: mergeProviderProbe(current[sessionId], probe),
        }));
        pushDebug({
          sessionId,
          level: probe.ok ? "info" : "warn",
          source: "usage",
          summary: `provider probe: ${probe.providerLabel}`,
          detail: `${probe.source}; model=${probe.model ?? modelId ?? "?"} windows=${probe.windows.length}`,
        });
      } catch (error) {
        pushDebug({
          sessionId,
          level: "warn",
          source: "usage",
          summary: "provider probe failed",
          detail: error instanceof Error ? error.message : String(error),
        });
      } finally {
        providerProbeInflight.current = false;
      }
    },
    [pushDebug]
  );

  const handleUsageRefresh = useCallback(() => {
    const sid = currentSessionId;
    if (!sid) return;
    const active = availableSessions.find((s) => s.id === sid);
    const agent = availableAgents.find((a) => a.id === active?.agentId);

    // OpenCode: probe the selected model’s provider balance API.
    if (agent?.id === "opencode") {
      const model =
        activeModelId ??
        sessionCapabilities?.currentModel ??
        null;
      void refreshProviderBalance(sid, model);
      return;
    }

    setSessionUsageById((current) => {
      const prev = current[sid] ?? emptySessionUsage();
      return {
        ...current,
        [sid]: {
          ...prev,
          refreshedAt: new Date().toISOString(),
          source: prev.source ?? "manual refresh",
        },
      };
    });

    // Codex (/status) and Claude (/usage) only surface account rate limits as
    // command text. Both are local slash commands — no model turn, no tokens.
    // Claude's `_claude/rateLimit` meta is not a substitute: it rides on a
    // `rate_limit_event`, which never fires while you are comfortably inside
    // your plan, so the panel would sit empty exactly when nothing is wrong.
    const limitCommand =
      agent?.id === "codex" ? "/status" : agent?.id === "claude-code" ? "/usage" : null;
    if (
      agent?.transport === "acp" &&
      limitCommand &&
      active &&
      (active.status === "running" || active.status === "waiting")
    ) {
      void sendAcpPrompt(sid, limitCommand).catch((error) => {
        pushDebug({
          sessionId: sid,
          level: "warn",
          source: "usage",
          summary: `usage refresh ${limitCommand} failed`,
          detail: error instanceof Error ? error.message : String(error),
        });
      });
    }

    // Grok weekly credits: `_x.ai/billing` (same data as TUI /usage). Sending
    // `/usage` over session/prompt just chats the model — the slash is TUI-only.
    if (
      agent?.transport === "acp" &&
      (agent.id === "grok-build" || agent.id === "grok") &&
      active &&
      (active.status === "running" || active.status === "waiting" || active.status === "starting")
    ) {
      void (async () => {
        try {
          const billing = await probeAcpBilling(sid);
          if (!billing) return;
          setSessionUsageById((current) => {
            const merged = mergeGrokBilling(current[sid], billing);
            if (!merged) return current;
            return { ...current, [sid]: merged };
          });
          pushDebug({
            sessionId: sid,
            level: "info",
            source: "usage",
            summary: "grok billing probe ok",
            detail: JSON.stringify(billing).slice(0, 240),
          });
        } catch (error) {
          pushDebug({
            sessionId: sid,
            level: "warn",
            source: "usage",
            summary: "grok billing probe failed",
            detail: error instanceof Error ? error.message : String(error),
          });
        }
      })();
    }
  }, [
    activeModelId,
    availableAgents,
    availableSessions,
    currentSessionId,
    pushDebug,
    refreshProviderBalance,
    sessionCapabilities?.currentModel,
  ]);

  // Auto-probe when OpenCode model changes or session becomes ready.
  useEffect(() => {
    if (!currentSessionId) return;
    const active = availableSessions.find((s) => s.id === currentSessionId);
    const agent = availableAgents.find((a) => a.id === active?.agentId);
    if (agent?.id !== "opencode") return;
    if (!active || (active.status !== "running" && active.status !== "waiting" && active.status !== "starting")) {
      return;
    }
    const model = activeModelId ?? sessionCapabilities?.currentModel ?? null;
    if (!model) return;
    const key = `${currentSessionId}|${model}`;
    if (lastProviderProbeKey.current === key) return;
    lastProviderProbeKey.current = key;
    void refreshProviderBalance(currentSessionId, model);
  }, [
    activeModelId,
    availableAgents,
    availableSessions,
    currentSessionId,
    refreshProviderBalance,
    sessionCapabilities?.currentModel,
  ]);

  // Auto-probe Grok weekly credits once the ACP session is live.
  useEffect(() => {
    if (!currentSessionId) return;
    const active = availableSessions.find((s) => s.id === currentSessionId);
    const agent = availableAgents.find((a) => a.id === active?.agentId);
    if (!agent || (agent.id !== "grok-build" && agent.id !== "grok")) return;
    if (!active || (active.status !== "running" && active.status !== "waiting" && active.status !== "starting")) {
      return;
    }
    const key = `${currentSessionId}|grok-billing`;
    if (lastProviderProbeKey.current === key) return;
    lastProviderProbeKey.current = key;
    void (async () => {
      const billing = await probeAcpBilling(currentSessionId);
      if (!billing) return;
      setSessionUsageById((current) => {
        const merged = mergeGrokBilling(current[currentSessionId], billing);
        if (!merged) return current;
        return { ...current, [currentSessionId]: merged };
      });
    })();
  }, [availableAgents, availableSessions, currentSessionId]);

  // Manual refresh should re-hit the network even if model unchanged.
  const handleUsageRefreshForce = useCallback(() => {
    lastProviderProbeKey.current = "";
    handleUsageRefresh();
  }, [handleUsageRefresh]);

  /**
   * After an AI turn ends (`turn/complete`), refresh the Usage panel once.
   * Throttled so multi-chunk finalization / rapid turns don't spam probes.
   * ChatGPT subscription usage is intentionally excluded: it is a private
   * endpoint and refreshes only on model change or explicit user action.
   */
  const refreshUsageAfterTurn = useCallback(
    (sessionId: string) => {
      if (!sessionId || sessionId !== currentSessionIdRef.current) return;
      const session = sessionsRef.current.find((candidate) => candidate.id === sessionId);
      const model = activeModelId ?? sessionCapabilities?.currentModel ?? null;
      if (session?.agentId === "opencode" && isChatgptUsageModel(model)) return;
      const now = Date.now();
      const prev = lastUsageRefreshAt.current[sessionId] ?? 0;
      if (now - prev < 4000) return;
      lastUsageRefreshAt.current[sessionId] = now;
      // Let status settle to waiting before probing.
      window.setTimeout(() => {
        if (sessionId !== currentSessionIdRef.current) return;
        lastProviderProbeKey.current = "";
        handleUsageRefresh();
      }, 350);
    },
    [activeModelId, handleUsageRefresh, sessionCapabilities?.currentModel],
  );
  const refreshUsageAfterTurnRef = useRef(refreshUsageAfterTurn);
  refreshUsageAfterTurnRef.current = refreshUsageAfterTurn;

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    document.documentElement.style.colorScheme = themeColorScheme(theme);
    window.localStorage.setItem("marionette-theme", theme);
  }, [theme]);

  // Global scrollbar auto-hide: `.is-scrolling` class on scroll + idle timeout.
  useEffect(() => initScrollbarAutoHide(), []);

  // Clear taskbar flash / yellow bar when the user comes back to the window.
  useEffect(() => bindDesktopNotifyFocusHandlers(), []);

  /**
   * Stuck / stalled turns: if a session stays `running` with no stream for
   * long enough, ping once so the user can interrupt without staring at the UI.
   */
  useEffect(() => {
    if (!desktopNotifyOn) return;
    /** sessionId → last health we already notified for */
    const notified = new Map<string, "stalled" | "stuck">();
    const tick = () => {
      const now = Date.now();
      for (const session of sessionsRef.current) {
        if (session.status !== "running" && session.status !== "starting") {
          notified.delete(session.id);
          continue;
        }
        const last = lastActivityByIdRef.current[session.id] ?? null;
        // Nested OpenCode/Claude task tools emit no parent stream — use the
        // longer subagent fuse so we don't desktop-notify at 1–2m of silence.
        let openSubagent = false;
        const evs = liveEventsRef.current;
        for (let i = evs.length - 1; i >= 0; i -= 1) {
          const e = evs[i];
          if (e.sessionId !== session.id) continue;
          if (
            e.type === "tool_call" &&
            isToolInProgress(e.status) &&
            isSubagentTool(e.toolName, e.title)
          ) {
            openSubagent = true;
            break;
          }
        }
        const health = activityHealth(
          session.status,
          last,
          now,
          openSubagent ? { openSubagent: true } : undefined,
        );
        if (health !== "stalled" && health !== "stuck") {
          // Reset so a later stall can notify again after recovery.
          if (health === "live" || health === "quiet") notified.delete(session.id);
          continue;
        }
        const prev = notified.get(session.id);
        // Notify on first stalled, and upgrade once to stuck.
        if (prev === health) continue;
        if (prev === "stuck") continue;
        if (prev === "stalled" && health === "stalled") continue;
        notified.set(session.id, health);
        const label = session.label?.trim() || "Session";
        void raiseDesktopNotify(
          "stuck",
          health === "stuck" ? `${label} · appears stuck` : `${label} · no updates`,
        );
      }
    };
    const id = window.setInterval(tick, 4000);
    return () => clearInterval(id);
  }, [desktopNotifyOn]);

  /**
   * The Usage meter's ceiling follows the model.
   *
   * Grok publishes one ceiling per model in its session/new catalog (32K…1M) and
   * never sends usage_update, so a value seeded once at connect pinned the meter
   * to whatever model happened to be current then. Agents that do send
   * usage_update keep their own number — applyModelContextSize refuses those.
   */
  useEffect(() => {
    const sid = currentSessionId;
    const sizes = sessionCapabilities?.modelContextSizes;
    if (!sid || !sizes) return;
    const model = activeModelId ?? sessionCapabilities?.currentModel ?? null;
    if (!model) return;
    const size = sizes[model];
    if (typeof size !== "number") return;
    setSessionUsageById((current) => {
      const next = applyModelContextSize(current[sid], size);
      if (!next) return current;
      return { ...current, [sid]: next };
    });
  }, [currentSessionId, sessionCapabilities, activeModelId]);

  // Drag-resize: mutate CSS vars on the grid during move (no React re-render).
  // Commit width to state + localStorage only on mouseup.
  useEffect(() => {
    if (!resizingSide) return;

    let raf = 0;
    const side = resizingSide;

    const applyCss = (left: number, right: number) => {
      const el = workspaceGridRef.current;
      if (!el) return;
      el.style.setProperty("--left-panel-width", `${left}px`);
      el.style.setProperty("--right-panel-width", `${right}px`);
    };

    const onMove = (event: MouseEvent) => {
      if (raf) cancelAnimationFrame(raf);
      raf = requestAnimationFrame(() => {
        raf = 0;
        if (side === "left") {
          leftWidthRef.current = clamp(event.clientX, LEFT_PANEL_MIN, LEFT_PANEL_MAX);
        } else {
          rightWidthRef.current = clamp(
            window.innerWidth - event.clientX,
            RIGHT_PANEL_MIN,
            RIGHT_PANEL_MAX
          );
        }
        applyCss(leftWidthRef.current, rightWidthRef.current);
      });
    };

    const onUp = () => {
      if (raf) cancelAnimationFrame(raf);
      const nextLeft = leftWidthRef.current;
      const nextRight = rightWidthRef.current;
      setLeftWidth(nextLeft);
      setRightWidth(nextRight);
      try {
        window.localStorage.setItem(
          LAYOUT_STORAGE_KEY,
          JSON.stringify({ leftWidth: nextLeft, rightWidth: nextRight })
        );
      } catch {
        // ignore quota / private mode
      }
      setResizingSide(null);
    };

    document.body.classList.add("is-panel-resizing");
    window.addEventListener("mousemove", onMove, { passive: true });
    window.addEventListener("mouseup", onUp);
    return () => {
      if (raf) cancelAnimationFrame(raf);
      document.body.classList.remove("is-panel-resizing");
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };
  }, [resizingSide]);

  const currentProject = useMemo(
    () => {
      const selected = availableProjects.find((project) => project.id === currentProjectId);
      if (selected || currentProjectId === CHAT_PROJECT_ID) return selected;
      return availableProjects[0] ?? (isTauriRuntime() ? undefined : projects[0]);
    },
    [availableProjects, currentProjectId]
  );

  const projectSessions = useMemo(
    () => availableSessions.filter((session) => session.projectId === currentProject?.id),
    [availableSessions, currentProject]
  );

  const chatSessions = useMemo(
    () => availableSessions.filter((session) => session.projectId === CHAT_PROJECT_ID),
    [availableSessions]
  );

  const currentSession = useMemo(
    () =>
      availableSessions.find((session) => session.id === currentSessionId) ??
      projectSessions[0],
    [availableSessions, currentSessionId, projectSessions]
  );

  const displaySession = useMemo((): Session => {
    return (
      currentSession ?? {
        id: `session-empty-${currentProjectId || currentProject?.id || "none"}`,
        projectId: currentProject?.id ?? currentProjectId,
        agentId: availableAgents[0]?.id ?? agents[0].id,
        label: "New session",
        cwd: currentProject?.rootPath ?? (currentProjectId === CHAT_PROJECT_ID ? defaultFolderPath : ""),
        status: "exited" as const,
        processId: null,
        startedAt: "",
        lastActiveAt: "",
        transcriptPath: "",
        handoffPath: "",
        viewMode: "clean" as const,
      }
    );
  }, [availableAgents, currentProject, currentProjectId, currentSession, defaultFolderPath]);

  const currentAgent = useMemo(
    () => availableAgents.find((agent) => agent.id === displaySession.agentId) ?? availableAgents[0] ?? agents[0],
    [availableAgents, displaySession]
  );

  const currentEvents = useMemo(() => {
    return liveEvents.filter((event) => event.sessionId === displaySession.id);
  }, [liveEvents, displaySession.id]);

  const sessionRunning =
    displaySession.status === "running" || displaySession.status === "starting";

  // Whole-dialog statistics for the Usage card, folded from the same events the
  // transcript renders — no separate live state to drift out of sync. A running
  // turn is folded against a clock: its elapsed windows and the rate move every
  // second, including while a tool runs silently. An idle dialog settles its
  // last turn instead, so its final step counts even when the card carries no
  // end stamp (older transcripts).
  const [statsTick, setStatsTick] = useState(0);
  useEffect(() => {
    if (!sessionRunning) return;
    const id = window.setInterval(() => setStatsTick((tick) => tick + 1), 1000);
    return () => window.clearInterval(id);
  }, [sessionRunning]);

  const sessionStats = useMemo(
    () =>
      foldSessionStats(currentEvents, displaySession.id, {
        settled: !sessionRunning,
        now: sessionRunning ? Date.now() : undefined,
        // Grok reports usage once per model call, so a turn that loops through
        // tools has real numbers before it ends. Every other agent reports only
        // when the turn ends, and its running turn is estimated from text.
        liveTokens: sessionRunning
          ? grokTurnUsageToTurnTokens(grokTurnUsageRef.current[displaySession.id])
          : null,
        agentId: displaySession.agentId,
      }),
    // statsTick is a clock and sessionUsageById carries the per-call reports:
    // both force the re-fold without being read here.
    [
      currentEvents,
      displaySession.id,
      displaySession.agentId,
      sessionRunning,
      statsTick,
      sessionUsageById,
    ]
  );

  const openSessions = useMemo(
    () => openSessionIds
      .map((sessionId) => availableSessions.find((session) => session.id === sessionId))
      .filter((session): session is Session => Boolean(session)),
    [availableSessions, openSessionIds]
  );

  /**
   * Open a new dialog. When the caller does not pick an agent/prefs, inherit
   * the active dialog's agent + model/mode/effort so a new tab does not snap
   * back to the global default (OpenCode / first model).
   */
  const createSessionForProject = async (
    projectId: string,
    agentId?: string,
    /** Use when React state has not yet committed a brand-new project. */
    projectHint?: Project,
    prefs?: SessionComposerPrefs | null,
  ) => {
    const project =
      projectHint?.id === projectId
        ? projectHint
        : availableProjects.find((item) => item.id === projectId);
    if (!project) return;

    // Prefer the open dialog when it belongs to this project; otherwise the
    // most recent session for the project. Falls back to the first agent.
    const source =
      (currentSession && currentSession.projectId === projectId ? currentSession : null) ??
      availableSessions.find((s) => s.projectId === projectId) ??
      null;
    const resolvedAgentId =
      agentId ??
      source?.agentId ??
      (availableAgents[0] ?? agents[0]).id;
    // Model/mode/effort only transfer when the agent is the same (or the
    // caller passed prefs explicitly). A different agent has a different catalog.
    const sameAgentAsSource = !source || source.agentId === resolvedAgentId;
    const resolvedPrefs: SessionComposerPrefs | null =
      prefs !== undefined
        ? prefs
        : source && sameAgentAsSource
          ? {
              preferredModel: source.preferredModel ?? null,
              preferredMode: source.preferredMode ?? null,
              preferredEffort: source.preferredEffort ?? null,
              preferredEffortId: source.preferredEffortId ?? null,
              preferredAlwaysApprove: source.preferredAlwaysApprove ?? null,
            }
          : (() => {
              // No dialog to inherit from — fall back to the agent's last-used
              // chips so a fresh session opens with the model/档位 we left on.
              const last = getLastUsedDefaults(resolvedAgentId);
              return last
                ? {
                    preferredModel: last.modelId,
                    preferredMode: last.modeId,
                    preferredEffort: last.effort,
                    preferredEffortId: last.effortId,
                    preferredAlwaysApprove: last.alwaysApprove,
                  }
                : null;
            })();

    const newSession = await createSessionApi(projectId, resolvedAgentId);
    if (!newSession) return;

    let sessionWithPrefs = newSession;
    if (resolvedPrefs) {
      const next = {
        ...newSession,
        preferredModel: resolvedPrefs.preferredModel ?? null,
        preferredMode: resolvedPrefs.preferredMode ?? null,
        preferredEffort: resolvedPrefs.preferredEffort ?? null,
        preferredEffortId: resolvedPrefs.preferredEffortId ?? null,
        preferredAlwaysApprove: resolvedPrefs.preferredAlwaysApprove ?? null,
      };
      sessionWithPrefs = next;
      void updateSessionPrefs(newSession.id, {
        preferredModel: next.preferredModel,
        preferredMode: next.preferredMode,
        preferredEffort: next.preferredEffort,
        preferredEffortId: next.preferredEffortId,
        preferredAlwaysApprove: next.preferredAlwaysApprove,
      }).catch((error) => {
        pushDebug({
          sessionId: newSession.id,
          level: "warn",
          source: "session",
          summary: "persist inherited composer prefs failed",
          detail: error instanceof Error ? error.message : String(error),
        });
      });
    }

    // Newest dialog sits at the top of the project shelf (and left of tabs).
    setAvailableSessions((current) => [
      sessionWithPrefs,
      ...current.filter((s) => s.id !== sessionWithPrefs.id),
    ]);
    setOpenSessionIds((current) => [
      sessionWithPrefs.id,
      ...current.filter((id) => id !== sessionWithPrefs.id),
    ]);
    setCurrentProjectId(projectId);
    setCurrentSessionId(sessionWithPrefs.id);
    // Product default is always Clean View — transport is an implementation detail.
    setViewMode("clean");
    setSessionCapabilities(null);
    setActiveModelId(null);
  };

  const openSession = (nextSession: Session) => {
    setOpenSessionIds((current) => current.includes(nextSession.id) ? current : [...current, nextSession.id]);
    setCurrentProjectId(nextSession.projectId);
    setCurrentSessionId(nextSession.id);
    // Product primary is always Clean when opening a dialog (Raw is one click away).
    setViewMode("clean");
    // HARD RULE: caps belong to (sessionId, agentId). Never leak previous dialog's models.
    setSessionCapabilities(null);
    setActiveModelId(null);
    // Pending Ask / Plan / Permission cards stay: every one of them is gated by
    // session id at render time, so they cannot leak into another dialog — and
    // dropping them here lost asks the agent was still blocked on (switch away
    // and back, and the card never returned until its timeout declined it).
    lastProviderProbeKey.current = "";
    void loadSessionTranscript(nextSession.id);
  };
  openSessionRef.current = openSession;

  /** Open a chat session from the Chat section. */
  const openChatSession = useCallback((session: Session) => {
    openSessionRef.current(session);
  }, []);

  /** Create a new Chat session in the current/default working folder. */
  const handleCreateChatSession = useCallback(async () => {
    try {
      const agentId = availableAgents[0]?.id ?? agents[0].id;
      const cwd = defaultFolderPath || (await getDefaultFolder());
      const session = await createChatSession(agentId, "新对话", cwd || null);
      if (!session) return;
      if (!defaultFolderPath && session.cwd) {
        setDefaultFolderPath(session.cwd);
      }
      const last = getLastUsedDefaults(agentId);
      let sessionWithPrefs = session;
      if (last) {
        sessionWithPrefs = {
          ...session,
          preferredModel: last.modelId,
          preferredMode: last.modeId,
          preferredEffort: last.effort,
          preferredEffortId: last.effortId,
          preferredAlwaysApprove: last.alwaysApprove,
        };
        void updateSessionPrefs(session.id, {
          preferredModel: sessionWithPrefs.preferredModel,
          preferredMode: sessionWithPrefs.preferredMode,
          preferredEffort: sessionWithPrefs.preferredEffort,
          preferredEffortId: sessionWithPrefs.preferredEffortId,
          preferredAlwaysApprove: sessionWithPrefs.preferredAlwaysApprove,
        }).catch(() => undefined);
      }
      startupCreatedChatsRef.current = [
        sessionWithPrefs,
        ...startupCreatedChatsRef.current.filter((item) => item.id !== sessionWithPrefs.id),
      ];
      setAvailableSessions((current) => [
        sessionWithPrefs,
        ...current.filter((item) => item.id !== sessionWithPrefs.id),
      ]);
      openSessionRef.current(sessionWithPrefs);
    } catch (error) {
      pushDebug({
        level: "error",
        source: "chat",
        summary: "create chat failed",
        detail: error instanceof Error ? error.message : String(error),
      });
    }
  }, [availableAgents, defaultFolderPath, pushDebug]);

  const setSessionStatusById = useCallback((sessionId: string, status: Session["status"]) => {
    setAvailableSessions((current) =>
      current.map((session) => {
        if (session.id !== sessionId) return session;
        if (status === "exited" || status === "error") {
          return { ...session, status, processId: null };
        }
        return { ...session, status };
      })
    );
    // Cross-window (detached) + disk SSOT — list_sessions must match the live turn
    // so a torn-off window shows Interrupt instead of Send mid-reply.
    void broadcastSessionPatch({ sessionId, status });
    if (ownsAcpSession(sessionId)) {
      void updateSessionStatus(sessionId, status).catch(() => undefined);
    }
  }, []);

  /**
   * Lazy ACP: start only when the user is about to talk (type/focus/send).
   * Dedupes concurrent warm+send. Never blocks the main UI thread beyond await
   * at the call site (composer stays interactive while this runs).
   */
  const ensureAcpReady = useCallback(
    async (sessionId: string): Promise<CapabilitySnapshot | null> => {
      if (!isTauriRuntime()) return null;

      const session = sessionsRef.current.find((s) => s.id === sessionId);
      if (!session) return null;
      const agent =
        agentsRef.current.find((a) => a.id === session.agentId) ??
        agentsRef.current[0] ??
        agents[0];
      if (agent.transport !== "acp") return null;

      // Already bootstrapped?
      const existing = await getSessionCapabilities(sessionId);
      if (existing) {
        const merged = mergeAcpCapabilities(agent.id, existing) ?? existing;
        if (sessionId === currentSessionIdRef.current) {
          setSessionCapabilities(merged);
        }
        if (session.status === "exited" || session.status === "error" || session.status === "starting") {
          setSessionStatusById(sessionId, "waiting");
        }
        return merged;
      }

      const inflight = acpBootstrapRef.current.get(sessionId);
      if (inflight) return inflight;

      const boot = (async () => {
        // Let the current paint/IME frame finish before status-driven re-renders.
        await new Promise<void>((resolve) => window.setTimeout(resolve, 0));
        setSessionStatusById(sessionId, "starting");
        touchActivity(sessionId);
        pushDebug({
          sessionId,
          level: "info",
          source: "acp",
          summary: `lazy warm: ${agent.command} ${agent.args.join(" ")}`.trim(),
        });
        try {
          // Handshake is async + spawn_blocking in Rust — must not stall webview/IME.
          const caps = await startAcpSession(
            sessionId,
            agent.command,
            agent.args,
            session.cwd
          );
          const merged = mergeAcpCapabilities(agent.id, caps) ?? caps;
          if (sessionId === currentSessionIdRef.current) {
            setSessionCapabilities(merged);
          }
          setSessionStatusById(sessionId, "waiting");
          // New ACP session/new → agent has empty memory; reinject local history once.
          acpNeedsHistoryRef.current.add(sessionId);
          pushDebug({
            sessionId,
            level: "info",
            source: "acp",
            summary: "lazy warm ready (history inject armed)",
            detail: merged
              ? `models=${merged.models.length} modes=${merged.modes.length}`
              : undefined,
          });
          return merged;
        } catch (error) {
          setSessionStatusById(sessionId, "error");
          pushDebug({
            sessionId,
            level: "error",
            source: "acp",
            summary: "lazy warm failed",
            detail: error instanceof Error ? error.message : String(error),
          });
          throw error;
        } finally {
          acpBootstrapRef.current.delete(sessionId);
        }
      })();

      acpBootstrapRef.current.set(sessionId, boot);
      return boot;
    },
    [pushDebug, setSessionStatusById, touchActivity]
  );

  const warmActiveAcp = useCallback(() => {
    const sid = currentSessionIdRef.current;
    if (!sid) return;
    const session = sessionsRef.current.find((s) => s.id === sid);
    const agent = agentsRef.current.find((a) => a.id === session?.agentId);
    if (agent?.transport !== "acp") return;
    void ensureAcpReady(sid).catch(() => undefined);
  }, [ensureAcpReady]);

  const closeSessionTab = (sessionId: string) => {
    lastClosedTabsRef.current = [
      sessionId,
      ...lastClosedTabsRef.current.filter((id) => id !== sessionId),
    ].slice(0, 20);
    saveClosedTabs(lastClosedTabsRef.current);
    const nextOpenIds = openSessionIds.filter((id) => id !== sessionId);
    if (nextOpenIds.length === 0) {
      const closing = availableSessions.find((session) => session.id === sessionId);
      if (closing?.projectId === CHAT_PROJECT_ID || currentProjectId === CHAT_PROJECT_ID) {
        void handleCreateChatSession();
      } else {
        void createSessionForProject(currentProject?.id ?? availableProjects[0]?.id ?? "");
      }
      return;
    }

    setOpenSessionIds(nextOpenIds);
    openSessionIdsRef.current = nextOpenIds;
    if (sessionId === currentSessionId) {
      const nextSession = availableSessions.find((session) => session.id === nextOpenIds[nextOpenIds.length - 1]);
      if (nextSession) {
        // Keep refs current so the prune below doesn't treat the new active as cold.
        currentSessionIdRef.current = nextSession.id;
        openSession(nextSession);
      }
    }
    // Closed tab is no longer hot — free its transcript after flush (disk kept).
    void pruneColdSessionMemory();
  };

  /** Drag project A to a line before/after B — persist array order in projects.json. */
  const handleReorderProjects = useCallback(
    (fromProjectId: string, toProjectId: string, place: "before" | "after") => {
      setAvailableProjects((current) => {
        const from = current.findIndex((p) => p.id === fromProjectId);
        const to = current.findIndex((p) => p.id === toProjectId);
        if (from < 0 || to < 0) return current;
        // Desired index in the original list, then shift left if we remove an earlier item.
        let insertAt = place === "before" ? to : to + 1;
        if (from < insertAt) insertAt -= 1;
        if (insertAt === from) return current; // line sits on an edge that keeps order
        const next = [...current];
        const [item] = next.splice(from, 1);
        next.splice(insertAt, 0, item);
        void reorderProjectsApi(next.map((p) => p.id)).catch(() => {
          void listProjects().then((projects) => {
            if (projects.length > 0) setAvailableProjects(projects);
          });
        });
        return next;
      });
    },
    []
  );

  /**
   * Detach a dialog into its own OS window.
   * - Click ↗: create/show + focus, then drop the tab.
   * - Drag tear-off: ghost chip already followed; create WebView at cursor on drop.
   */
  const handleTabPopOut = async (session: Session, viaDrag = false) => {
    if (!isTauriRuntime() || IS_DETACHED_WINDOW) return;
    // Flush before the detached SPA loads from disk, so tokens already in
    // this window are not missing for the gap until it claims ownership.
    const pending = persistableEventsForSession(liveEventsRef.current, session.id);
    if (pending.length > 0) {
      void writeTranscript(session.id, pending).catch(() => undefined);
    }
    const ok = await openDetachedSessionWindow(session, { atCursor: viaDrag });
    if (!ok) return;
    // Detached SPA boots async — re-push live status/label so Interrupt matches
    // the main window even if list_sessions races or the shell was about:blank.
    const live = sessionsRef.current.find((s) => s.id === session.id) ?? session;
    void broadcastSessionPatch({
      sessionId: live.id,
      status: live.status,
      label: live.label,
    });
    // A second pulse after the secondary window has time to subscribe.
    window.setTimeout(() => {
      const again = sessionsRef.current.find((s) => s.id === session.id) ?? live;
      void broadcastSessionPatch({
        sessionId: again.id,
        status: again.status,
        label: again.label,
      });
    }, 400);
    const nextOpenIds = openSessionIds.filter((id) => id !== session.id);
    if (nextOpenIds.length === 0) {
      setOpenSessionIds([]);
      if (session.projectId === CHAT_PROJECT_ID) {
        void handleCreateChatSession();
      } else {
        void createSessionForProject(
          session.projectId || currentProject?.id || availableProjects[0]?.id || ""
        );
      }
      return;
    }
    setOpenSessionIds(nextOpenIds);
    if (session.id === currentSessionId) {
      const nextSession =
        availableSessions.find((s) => s.id === nextOpenIds[nextOpenIds.length - 1]) ??
        availableSessions.find((s) => nextOpenIds.includes(s.id));
      if (nextSession) openSession(nextSession);
    }
  };

  /**
   * Detached window: hand the dialog back to the main window.
   * - Click ⇥: ask main to re-adopt the tab.
   * - Drag over main + drop: same request (viaDrag).
   * Main re-adds the tab, focuses it, then hides this shell.
   */
  const handleMergeBack = async (session: Session, viaDrag = false) => {
    if (!isTauriRuntime() || !IS_DETACHED_WINDOW) return;
    void viaDrag;
    const events = persistableEventsForSession(liveEventsRef.current, session.id);
    if (events.length > 0) {
      await writeTranscript(session.id, events).catch(() => undefined);
    }
    await requestMergeBack(session.id);
  };

  const deleteSession = (sessionId: string) => {
    const deletedSession = availableSessions.find((session) => session.id === sessionId);
    if (!deletedSession) return;
    if (deletedSession && (deletedSession.status === "starting" || deletedSession.status === "running" || deletedSession.status === "waiting")) {
      void stopAcpSession(sessionId).catch(() => undefined);
    }
    void deleteSessionApi(deletedSession.projectId, sessionId).catch((error) => {
      pushDebug({
        sessionId,
        level: "warn",
        source: "session",
        summary: "delete session failed",
        detail: error instanceof Error ? error.message : String(error),
      });
    });
    startupCreatedChatsRef.current = startupCreatedChatsRef.current.filter(
      (session) => session.id !== sessionId,
    );
    manuallyRenamedSessionIdsRef.current.delete(sessionId);
    const remainingSessions = availableSessions.filter((session) => session.id !== sessionId);
    setAvailableSessions(remainingSessions);
    setOpenSessionIds((current) => current.filter((id) => id !== sessionId));
    setLiveEvents((current) => dropEventsForSessions(current, new Set([sessionId])));
    transcriptLoadedRef.current.delete(sessionId);
    acpNeedsHistoryRef.current.delete(sessionId);
    pendingSendsRef.current.delete(sessionId);
    flushingSendRef.current.delete(sessionId);
    const saveTimer = transcriptSaveTimers.current.get(sessionId);
    if (saveTimer) {
      clearTimeout(saveTimer);
      transcriptSaveTimers.current.delete(sessionId);
    }
    setSessionUsageById((current) => {
      if (!(sessionId in current)) return current;
      const next = { ...current };
      delete next[sessionId];
      return next;
    });
    if (sessionId === currentSessionId) {
      const targetProjectId = deletedSession.projectId || currentProject?.id || "";
      const nextSession = remainingSessions.find(
        (session) => session.id !== sessionId && session.projectId === targetProjectId,
      );
      if (nextSession) openSession(nextSession);
      else if (targetProjectId === CHAT_PROJECT_ID) {
        // Deleting the last Chat should leave Chat empty; recreating a row here
        // made the delete action appear to do nothing. The + button creates the
        // next conversation explicitly.
        setCurrentProjectId(CHAT_PROJECT_ID);
        setCurrentSessionId("");
        setOpenSessionIds([]);
        setViewMode("clean");
        setSessionCapabilities(null);
      }
      else void createSessionForProject(currentProject?.id ?? availableProjects[0]?.id ?? "");
    }
  };

  const refreshChangedFiles = useCallback(async () => {
    if (!currentProjectId || currentProjectId === CHAT_PROJECT_ID) {
      setChangedFiles([]);
      setChangedFilesNote(null);
      setGitBranch(null);
      return;
    }
    try {
      const [files, branch] = await Promise.all([
        getChangedFiles(currentProjectId),
        getCurrentBranch(currentProjectId),
      ]);
      setChangedFiles(files);
      setChangedFilesNote(files.length === 0 ? "No local changes (or not a git repo)." : null);
      setGitBranch(branch);
    } catch (error) {
      setChangedFiles([]);
      setChangedFilesNote(error instanceof Error ? error.message : String(error));
      setGitBranch(null);
    }
  }, [currentProjectId]);

  useEffect(() => {
    void refreshChangedFiles();
    const timer = window.setInterval(() => {
      void refreshChangedFiles();
    }, 12_000);
    return () => window.clearInterval(timer);
  }, [refreshChangedFiles]);

  /**
   * Scan timing: on project switch / add, and on demand — never at startup.
   * It is a handful of file reads, but the launch path stays untouched, and a
   * stale inventory would be worse than no inventory: the user installs a skill
   * in another tool and expects the panel to notice next time they look.
   */
  const refreshProjectContext = useCallback(
    async (projectId: string) => {
      if (!projectId) {
        setProjectContext(null);
        return;
      }
      if (projectId === CHAT_PROJECT_ID) {
        setProjectContext(null);
        setProjectContextScanning(false);
        return;
      }
      setProjectContextScanning(true);
      try {
        const scanned = await scanProjectContext(projectId);
        setProjectContext(scanned);
        if (scanned) {
          pushDebug({
            level: "info",
            source: "context",
            summary: `scan: ${scanned.inventory.mcpServers.length} mcp · ${scanned.inventory.skills.length} skills`,
          });
        }
      } finally {
        setProjectContextScanning(false);
      }
    },
    [pushDebug]
  );

  useEffect(() => {
    if (rightCollapsed) return; // nothing on screen to feed
    void refreshProjectContext(currentProjectId);
  }, [currentProjectId, rightCollapsed, refreshProjectContext]);

  // Project todos — load when project changes.
  useEffect(() => {
    if (!currentProjectId || currentProjectId === CHAT_PROJECT_ID) {
      setTodoItems([]);
      return;
    }
    let cancelled = false;
    void listTodos(currentProjectId).then((items) => {
      if (!cancelled) setTodoItems(items);
    });
    return () => {
      cancelled = true;
    };
  }, [currentProjectId]);

  const handleTodosChange = useCallback(
    (items: TodoItem[]) => {
      setTodoItems(items);
      if (!currentProjectId || currentProjectId === CHAT_PROJECT_ID) return;
      void saveTodos(currentProjectId, items).catch((error) => {
        pushDebug({
          sessionId: currentSessionIdRef.current || "",
          level: "warn",
          source: "todos",
          summary: "save todos failed",
          detail: error instanceof Error ? error.message : String(error),
        });
      });
    },
    [currentProjectId, pushDebug],
  );

  const prefillComposer = useCallback((text: string) => {
    setComposerPrefill({ text, token: Date.now() });
  }, []);

  const handleAbsorbPlan = useCallback(() => {
    const sid = currentSessionIdRef.current;
    const plan = sid ? planBySessionId[sid] : undefined;
    if (!plan?.length) return;
    handleTodosChange(absorbPlanIntoTodos(todoItems, plan, sid));
  }, [planBySessionId, todoItems, handleTodosChange]);

  const handlePrepareAiTodoMerge = useCallback(() => {
    const sid = currentSessionIdRef.current;
    const plan = sid ? planBySessionId[sid] : undefined;
    if (plan && plan.length > 0) {
      return previewMergeFromAi(todoItems, planToProposed(plan));
    }
    // Prefer fenced block from last assistant message in this session.
    const events = liveEventsRef.current.filter((e) => e.sessionId === sid);
    for (let i = events.length - 1; i >= 0; i--) {
      const e = events[i];
      if (e.type !== "assistant_message") continue;
      const proposed = parseMarionetteTodoFence(e.text);
      if (proposed) return previewMergeFromAi(todoItems, proposed);
      break;
    }
    return null;
  }, [planBySessionId, todoItems]);

  const handleToggleProjectContext = useCallback(
    async (kind: "mcp" | "skill", id: string, enabled: boolean) => {
      if (!currentProjectId || currentProjectId === CHAT_PROJECT_ID) return;
      // Optimistic: the checkbox must not lag behind a disk write.
      setProjectContext((current) => {
        if (!current) return current;
        const selection = { ...current.selection };
        if (kind === "skill") {
          selection.skills = { ...selection.skills, [id]: enabled };
        } else {
          selection.mcpServers = { ...selection.mcpServers, [id]: enabled };
        }
        return { ...current, selection };
      });
      try {
        await setProjectContextEnabled(currentProjectId, kind, id, enabled);
      } catch (error) {
        pushDebug({
          level: "warn",
          source: "context",
          summary: "persist context selection failed",
          detail: error instanceof Error ? error.message : String(error),
        });
        void refreshProjectContext(currentProjectId);
      }
    },
    [currentProjectId, pushDebug, refreshProjectContext]
  );

  const handleOpenDiff = useCallback(
    async (path: string) => {
      if (!currentProjectId || currentProjectId === CHAT_PROJECT_ID) return;
      const text = await getFileDiff(currentProjectId, path);
      setDiffPreview({ path, text: text || "(empty diff)" });
    },
    [currentProjectId]
  );

  const handlePermissionChoose = useCallback(async (optionId: string) => {
    if (!permissionPrompt) return;
    setPermissionBusy(true);
    try {
      await respondAcpPermission(permissionPrompt.requestId, optionId);
      pushDebug({
        sessionId: permissionPrompt.sessionId,
        level: "info",
        source: "permission",
        summary: `permission ${optionId}`,
        detail: permissionPrompt.title,
      });
    } catch (error) {
      pushDebug({
        sessionId: permissionPrompt.sessionId,
        level: "error",
        source: "permission",
        summary: "respond permission failed",
        detail: error instanceof Error ? error.message : String(error),
      });
    } finally {
      setPermissionBusy(false);
      setPermissionPrompt(null);
    }
  }, [permissionPrompt, pushDebug]);

  const handleAskSubmit = useCallback(
    async (answers: { question: string; selected: string[] }[]) => {
      if (!askPrompt) return;
      setAskBusy(true);
      try {
        await respondAcpQuestion(askPrompt.requestId, answers, false);
        pushDebug({
          sessionId: askPrompt.sessionId,
          level: "info",
          source: "question",
          summary: `answered ${answers.length} question(s)`,
        });
      } catch (error) {
        pushDebug({
          sessionId: askPrompt.sessionId,
          level: "error",
          source: "question",
          summary: "respond question failed",
          detail: error instanceof Error ? error.message : String(error),
        });
      } finally {
        setAskBusy(false);
        setAskPrompt(null);
      }
    },
    [askPrompt, pushDebug],
  );

  const handleAskDecline = useCallback(async () => {
    if (!askPrompt) return;
    setAskBusy(true);
    try {
      await respondAcpQuestion(askPrompt.requestId, [], true);
    } catch (error) {
      pushDebug({
        sessionId: askPrompt.sessionId,
        level: "error",
        source: "question",
        summary: "decline question failed",
        detail: error instanceof Error ? error.message : String(error),
      });
    } finally {
      setAskBusy(false);
      setAskPrompt(null);
    }
  }, [askPrompt, pushDebug]);

  /**
   * Resolve Grok exit_plan_mode. For `request_changes`, Grok keeps plan mode
   * but discards reply feedback — mirror Codeg/TUI by also sending notes as a
   * follow-up user prompt so the agent revises the plan.
   */
  const handlePlanApproval = useCallback(
    async (decision: PlanApprovalDecision, feedback?: string) => {
      if (!planApproval) return;
      setPlanApprovalBusy(true);
      try {
        await respondAcpPlanApproval(
          planApproval.requestId,
          decision,
          feedback ?? null,
        );
        pushDebug({
          sessionId: planApproval.sessionId,
          level: "info",
          source: "plan",
          summary: `plan approval: ${decision}`,
          detail: feedback?.trim() || undefined,
        });
        setPlanApproval(null);

        if (decision === "request_changes" && feedback?.trim()) {
          const sid = planApproval.sessionId;
          const notes = feedback.trim();
          // Small delay so the keep_planning turn can settle before the next prompt.
          window.setTimeout(() => {
            void sendAcpPrompt(sid, notes, [])
              .then(() => {
                pushDebug({
                  sessionId: sid,
                  level: "info",
                  source: "plan",
                  summary: "sent plan revision notes as follow-up prompt",
                });
              })
              .catch((error) => {
                pushDebug({
                  sessionId: sid,
                  level: "warn",
                  source: "plan",
                  summary: "follow-up revision prompt failed — paste notes manually",
                  detail: error instanceof Error ? error.message : String(error),
                });
              });
          }, 400);
        }
      } catch (error) {
        pushDebug({
          sessionId: planApproval.sessionId,
          level: "error",
          source: "plan",
          summary: "respond plan approval failed",
          detail: error instanceof Error ? error.message : String(error),
        });
        throw error;
      } finally {
        setPlanApprovalBusy(false);
      }
    },
    [planApproval, pushDebug],
  );

  const handleAgentChange = async (agentId: string) => {
    if (!currentProject && !currentSession) return;

    // If no current session, create one bound to this agent
    if (!currentSession || !currentSessionId) {
      if (currentProject) {
        void createSessionForProject(currentProject.id, agentId);
      } else {
        void handleCreateChatSession();
      }
      return;
    }

    if (currentSession.agentId === agentId) return;

    // HARD RULE: session.agentId is the only source of truth for which agent
    // owns this dialog. Persist it so reopening the tab cannot show OpenCode
    // while still listing Claude models.
    const sid = currentSessionId;
    const sourceAgentId = currentSession.agentId;
    const oldAgent = availableAgents.find((a) => a.id === sourceAgentId);

    // Mid-turn switch: cancel the open prompt first so the old process does
    // not keep running after we tear it down (and so turn/complete frees UI).
    const midTurn =
      currentSession.status === "running" || currentSession.status === "starting";
    if (midTurn) {
      try {
        await cancelAcpSession(sid);
      } catch {
        // stop below still kills the process
      }
      setSessionStatusById(sid, "waiting");
    }

    // Seal the previous agent's last Reply so the next harness's tools / CoT
    // stream cannot demote it to Thinking (same dialog, shared transcript).
    setLiveEvents((current) => sealOpenAssistantReplies(current, sid));
    // liveEventsRef may lag one frame behind setState — seal the snapshot we
    // are about to flush too.
    const sealedLive = sealOpenAssistantReplies(liveEventsRef.current, sid);
    liveEventsRef.current = sealedLive;

    // Flush transcript so handoff can read the latest Clean history.
    try {
      const events = persistableEventsForSession(sealedLive, sid);
      await writeTranscript(sid, events);
    } catch {
      // still attempt handoff from whatever is on disk
    }

    // P1-B: handoff.md + composer prefill (never auto-send).
    try {
      const handoff = await generateHandoff({
        projectId: currentSession.projectId,
        sessionId: sid,
        targetAgentId: agentId,
        sourceAgentId,
      });
      if (handoff) {
        setLastHandoff(handoff);
        // Never prefill the composer: the notes ride along with the next message
        // the user actually sends (see pendingHandoffPrompt in handleSend).
        setLiveEvents((current) => [
          ...sealOpenAssistantReplies(current, sid),
          {
            type: "handoff_prepared" as const,
            sessionId: sid,
            targetAgentId: agentId,
            handoffPath: handoff.handoffPath,
            prompt: handoff.prompt,
            createdAt: new Date().toISOString(),
          },
        ]);
        pushDebug({
          sessionId: sid,
          level: "info",
          source: "handoff",
          summary: `handoff → ${agentId}`,
          detail: handoff.handoffPath,
        });
      } else {
        // Fresh dialog (no messages yet) or generation failed: nothing to
        // attach — drop the previous dialog's handoff card.
        setLastHandoff(null);
      }
    } catch (error) {
      pushDebug({
        sessionId: sid,
        level: "warn",
        source: "handoff",
        summary: "generate handoff failed",
        detail: error instanceof Error ? error.message : String(error),
      });
    }

    setSessionCapabilities(null);
    setActiveModelId(null);
    lastProviderProbeKey.current = "";
    // Pending cards belonged to the old process — do not answer them after switch.
    setAskPrompt((cur) => (cur?.sessionId === sid ? null : cur));
    setPlanApproval((cur) => (cur?.sessionId === sid ? null : cur));
    setPlanApprovalBusy(false);
    setPermissionPrompt((cur) => (cur?.sessionId === sid ? null : cur));
    // Keep Clean history for this dialog; only the agent process is replaced.
    setSessionUsageById((current) => ({ ...current, [sid]: emptySessionUsage() }));
    acpBootstrapRef.current.delete(sid);
    setViewMode("clean");
    // New agent, new catalog — but seed it with that agent's last-used chips so
    // switching agents does not snap back to the harness default.
    const agentDefaults = getLastUsedDefaults(agentId);
    setAvailableSessions((current) =>
      current.map((s) =>
        s.id === sid
          ? {
              ...s,
              agentId,
              status: "exited" as const,
              processId: null,
              preferredModel: agentDefaults?.modelId ?? null,
              preferredMode: agentDefaults?.modeId ?? null,
              preferredEffort: agentDefaults?.effort ?? null,
              preferredEffortId: agentDefaults?.effortId ?? null,
              preferredAlwaysApprove: null,
            }
          : s
      )
    );
    void updateSessionAgent(sid, agentId).catch((error) => {
      pushDebug({
        sessionId: sid,
        level: "warn",
        source: "session",
        summary: "persist agentId failed",
        detail: error instanceof Error ? error.message : String(error),
      });
    });

    // Tear down previous transport (also unparks Ask/Plan/Permission with cancel).
    if (oldAgent) {
      void stopAcpSession(sid).catch(() => undefined);
    }
  };

  const handleSessionStatusChange = useCallback((status: Session["status"]) => {
    if (!currentSessionId) return;
    setSessionStatusById(currentSessionId, status);
  }, [currentSessionId, setSessionStatusById]);

  const disarmCancelWatchdog = useCallback((sid: string) => {
    const timer = cancelWatchdogsRef.current.get(sid);
    if (timer) clearTimeout(timer);
    cancelWatchdogsRef.current.delete(sid);
  }, []);

  /**
   * Replace a wedged ACP process and make the next prompt carry the transcript.
   * Same dance as the path-grant restart below: a live agent cannot pick up new
   * state, so `session/new` is the only way back.
   */
  const restartAcpProcess = useCallback(async (sid: string) => {
    disarmCancelWatchdog(sid);
    cancelIgnoredRef.current.delete(sid);
    await stopAcpSession(sid).catch(() => undefined);
    acpBootstrapRef.current.delete(sid);
    setSessionCapabilities(null);
    setSessionStatusById(sid, "exited");
    acpNeedsHistoryRef.current.add(sid);
    pushDebug({
      sessionId: sid,
      level: "info",
      source: "interrupt",
      summary: "replaced wedged ACP process",
    });
  }, [disarmCancelWatchdog, pushDebug, setSessionStatusById]);

  /**
   * Re-negotiate the agent connection so newly-reachable MCP servers attach.
   *
   * MCP servers are handed to the agent exactly once, in `session/new`. One that
   * was not listening at that moment — Unity or UE started after Marionette — is
   * recorded as unavailable for the life of that session and never retried, so
   * rescanning the inventory changes nothing on its own: it takes a fresh
   * `session/new`. Conversation context survives, because the replacement
   * session gets the local transcript replayed with the next message.
   */
  const handleReconnectAgent = useCallback(async () => {
    const sid = currentSessionIdRef.current;
    if (!sid) return;
    const session = sessionsRef.current.find((s) => s.id === sid);
    const agent = agentsRef.current.find((a) => a.id === session?.agentId);
    if (agent?.transport !== "acp") return;

    setReconnecting(true);
    try {
      await refreshProjectContext(currentProjectId);
      await restartAcpProcess(sid);
      await ensureAcpReady(sid);
      setLiveEvents((current) => [
        ...current,
        {
          type: "assistant_message" as const,
          sessionId: sid,
          text:
            "**Reconnected.**\n\nMCP servers were re-negotiated for this session, so anything that started after the agent (Unity, UE, …) should be attached now. Your conversation is kept — it is replayed to the new session with your next message.",
          createdAt: new Date().toISOString(),
        },
      ]);
      pushDebug({
        sessionId: sid,
        level: "info",
        source: "context",
        summary: "reconnected agent to pick up MCP servers",
      });
    } catch (error) {
      pushDebug({
        sessionId: sid,
        level: "warn",
        source: "context",
        summary: "reconnect failed",
        detail: error instanceof Error ? error.message : String(error),
      });
    } finally {
      setReconnecting(false);
    }
  }, [currentProjectId, ensureAcpReady, pushDebug, refreshProjectContext, restartAcpProcess]);

  /**
   * Replace the agent process after provider keys were edited, so it re-reads
   * OpenCode's `auth.json`.
   *
   * A key saved from the composer dialog is invisible to a running agent:
   * OpenCode loads auth.json once at startup, so the live session keeps
   * reporting the model list it negotiated before the key existed — the model
   * selector stays empty and the user is back where they started. Same
   * constraint as `handleReconnectAgent` above: only a fresh `session/new`
   * picks up new state.
   */
  const handleProviderKeysChanged = useCallback(async () => {
    const sid = currentSessionIdRef.current;
    if (!sid || sid.startsWith("session-empty-")) return;
    const session = sessionsRef.current.find((s) => s.id === sid);
    const agent = agentsRef.current.find((a) => a.id === session?.agentId);
    if (agent?.transport !== "acp") return;
    try {
      await restartAcpProcess(sid);
      await ensureAcpReady(sid);
      pushDebug({
        sessionId: sid,
        level: "info",
        source: "session",
        summary: "restarted agent to load new provider key",
      });
    } catch (error) {
      pushDebug({
        sessionId: sid,
        level: "warn",
        source: "session",
        summary: "restart after provider key change failed",
        detail: error instanceof Error ? error.message : String(error),
      });
    }
  }, [ensureAcpReady, pushDebug, restartAcpProcess]);

  /**
   * Persist the proxy config, then restart the live agent so the new env vars
   * (HTTPS_PROXY / NO_PROXY) actually apply to its next spawned processes.
   */
  const handleSaveProxy = useCallback(
    async (config: ProxyConfig) => {
      // Nothing changed: keep the live agent untouched (no pointless reconnect).
      const sameAsCurrent =
        config.enabled === (proxyConfig?.enabled ?? false) &&
        config.url.trim() === (proxyConfig?.url ?? "").trim();
      if (sameAsCurrent) return;
      await setProxyConfig(config);
      setProxyConfigState(config);
      const sid = currentSessionIdRef.current;
      if (!sid || sid.startsWith("session-empty-")) return;
      const session = sessionsRef.current.find((s) => s.id === sid);
      const agent = agentsRef.current.find((a) => a.id === session?.agentId);
      if (agent?.transport !== "acp") return;
      try {
        await restartAcpProcess(sid);
        await ensureAcpReady(sid);
        pushDebug({
          sessionId: sid,
          level: "info",
          source: "session",
          summary: config.enabled
            ? `restarted agent with proxy ${config.url}`
            : "restarted agent (proxy off)",
        });
      } catch (error) {
        pushDebug({
          sessionId: sid,
          level: "warn",
          source: "session",
          summary: "restart after proxy change failed",
          detail: error instanceof Error ? error.message : String(error),
        });
      }
    },
    [ensureAcpReady, proxyConfig, pushDebug, restartAcpProcess]
  );

  /** Verify the proxy path by round-tripping to OpenAI's API. */
  const handleTestProxy = useCallback(
    async (url: string): Promise<ProxyTestResult | null> => {
      try {
        return await testProxy(url);
      } catch (error) {
        return {
          ok: false,
          message: error instanceof Error ? error.message : String(error),
          latencyMs: null,
        };
      }
    },
    []
  );

  /**
   * Composer install/upgrade path: stop the live process so the binary can be
   * replaced, then start a fresh ACP session so the dialog uses the new build
   * without quitting the app.
   */
  const handleAgentBinaryUpdated = useCallback(
    async (agentId: string, phase: "stop" | "restart") => {
      const sid = currentSessionIdRef.current;
      if (!sid || sid.startsWith("session-empty-")) return;
      const session = sessionsRef.current.find((s) => s.id === sid);
      if (session?.agentId !== agentId) return;
      const agent = agentsRef.current.find((a) => a.id === agentId);
      if (agent?.transport !== "acp") return;

      if (phase === "stop") {
        await restartAcpProcess(sid);
        pushDebug({
          sessionId: sid,
          level: "info",
          source: "session",
          summary: "stopped agent before CLI upgrade",
        });
        return;
      }

      try {
        await restartAcpProcess(sid);
        await ensureAcpReady(sid);
        pushDebug({
          sessionId: sid,
          level: "info",
          source: "session",
          summary: "restarted agent after CLI upgrade",
        });
      } catch (error) {
        pushDebug({
          sessionId: sid,
          level: "warn",
          source: "session",
          summary: "restart after CLI upgrade failed",
          detail: error instanceof Error ? error.message : String(error),
        });
      }
    },
    [ensureAcpReady, pushDebug, restartAcpProcess],
  );

  /**
   * `session/cancel` is a notification the agent never replies to, so the only
   * evidence it landed is the turn actually ending. If the turn is still open
   * after the grace period, it is stuck inside the agent process where nothing
   * on this side can reach it — flag the session so the next send starts over.
   */
  const armCancelWatchdog = useCallback((sid: string, cancelAt: number) => {
    const previous = cancelWatchdogsRef.current.get(sid);
    if (previous) clearTimeout(previous);
    const timer = setTimeout(() => {
      cancelWatchdogsRef.current.delete(sid);
      if ((turnEndedAtRef.current[sid] ?? 0) > cancelAt) return; // cancel honoured
      cancelIgnoredRef.current.add(sid);
      pushDebug({
        sessionId: sid,
        level: "warn",
        source: "interrupt",
        summary: "cancel unacknowledged — agent silent, will restart on next send",
      });
      setLiveEvents((current) => [
        ...current,
        {
          type: "assistant_message" as const,
          sessionId: sid,
          text:
            "**The agent did not answer the cancel.**\n\nIt has stayed silent since the interrupt, so the turn is stuck inside the agent process — a hung tool or model call, which nothing on this side can reach.\n\nSend your next message as usual: Marionette will replace the agent process first and replay this conversation to the new one.",
          createdAt: new Date().toISOString(),
        },
      ]);
    }, CANCEL_ACK_GRACE_MS);
    cancelWatchdogsRef.current.set(sid, timer);
  }, [pushDebug]);

  /**
   * Stop the live turn. `announce` paints the Interrupted card (Stop / Esc×2).
   * Mid-turn follow-up sends pass `{ announce: false }` so the user's next
   * You card is the continuation, not a system notice.
   *
   * Seal + suppress *before* `session/cancel`: cancel emits `turn/complete`
   * immediately, which may drain a queued follow-up before this function
   * returns — an unsealed Reply would then absorb the next turn's stream.
   */
  const cancelLiveTurn = useCallback(
    async (sid: string, opts?: { announce?: boolean }) => {
      const announce = opts?.announce === true;
      const session = sessionsRef.current.find((s) => s.id === sid);
      const midTurn = session?.status === "running";
      const cancelAt = Date.now();
      let cancelNote = "Cancel request sent to the agent.";

      streamSuppressedRef.current.add(sid);
      setLiveEvents((current) =>
        sealOpenAssistantReplies(markOpenTools(current, sid, "cancelled"), sid),
      );

      try {
        await cancelAcpSession(sid);
        if (midTurn && announce) armCancelWatchdog(sid, cancelAt);
      } catch (error) {
        if (midTurn) cancelIgnoredRef.current.add(sid);
        cancelNote = `Cancel request failed (${error instanceof Error ? error.message : String(error)}).${
          midTurn ? " The agent will be restarted on your next message." : ""
        }`;
        pushDebug({
          sessionId: sid,
          level: "warn",
          source: "interrupt",
          summary: "cancel failed",
          detail: error instanceof Error ? error.message : String(error),
        });
      }

      setSessionStatusById(sid, "waiting");
      touchActivity(sid);
      if (announce) {
        setLiveEvents((current) => [
          ...current,
          {
            type: "assistant_message" as const,
            sessionId: sid,
            text: `**Interrupted.**\n\n${cancelNote}\n\nYou can send a new message now.`,
            createdAt: new Date().toISOString(),
            durationMs: 0,
          },
        ]);
      }
      pushDebug({
        sessionId: sid,
        level: "info",
        source: "interrupt",
        summary: announce ? "ACP cancel (interrupt)" : "ACP cancel (mid-turn follow-up)",
      });
      queueMicrotask(() => drainQueuedSendRef.current(sid));
    },
    [armCancelWatchdog, pushDebug, setSessionStatusById, touchActivity],
  );

  const handleInterrupt = useCallback(async () => {
    if (!currentSessionId) return;
    const session = sessionsRef.current.find((s) => s.id === currentSessionId);
    const agent =
      agentsRef.current.find((a) => a.id === session?.agentId) ??
      agentsRef.current[0];
    if (!agent) return;
    await cancelLiveTurn(currentSessionId, { announce: true });
  }, [cancelLiveTurn, currentSessionId]);

  useEffect(() => {
    const stored = loadQueuedSends();
    let restored = 0;
    for (const [id, items] of Object.entries(stored)) {
      if (!items.length) continue;
      pendingSendsRef.current.set(
        id,
        items.map((item) => ({
          composed: item.composed,
          imageAttachments: (item.imageAttachments ?? []) as ImageAttachment[],
          composerSnap: item.composerSnap,
        })),
      );
      restored += 1;
    }
    if (restored > 0) setQueuedStripTick((t) => t + 1);
  }, []);

  useEffect(() => {
    const map: Record<string, ReturnType<typeof loadQueuedSends>[string]> = {};
    pendingSendsRef.current.forEach((items, id) => {
      map[id] = items;
    });
    saveQueuedSends(map);
  }, [queuedStripTick]);

  // P2-UX-3: double Esc → interrupt (after closing overlays).
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      if (event.defaultPrevented) return;

      // Layered dismiss first — single Esc should not interrupt.
      if (permissionPrompt) {
        // Prefer a reject option if present; otherwise clear UI only.
        const reject =
          permissionPrompt.options.find((o) =>
            /reject|deny|cancel/i.test(`${o.kind} ${o.name} ${o.optionId}`)
          ) ?? permissionPrompt.options[permissionPrompt.options.length - 1];
        if (reject) {
          void handlePermissionChoose(reject.optionId);
        } else {
          setPermissionPrompt(null);
        }
        lastEscAtRef.current = 0;
        return;
      }
      // Ask / Plan occupy the composer — Esc = skip / abandon (do not leave zombie cards).
      if (askPrompt) {
        void handleAskDecline();
        lastEscAtRef.current = 0;
        return;
      }
      if (planApproval) {
        void handlePlanApproval("abandon");
        lastEscAtRef.current = 0;
        return;
      }
      if (diffPreview) {
        setDiffPreview(null);
        lastEscAtRef.current = 0;
        return;
      }
      if (projectDialogOpen) {
        if (!projectAdding) setProjectDialogOpen(false);
        lastEscAtRef.current = 0;
        return;
      }

      const now = Date.now();
      if (now - lastEscAtRef.current <= 400) {
        lastEscAtRef.current = 0;
        event.preventDefault();
        void handleInterrupt();
      } else {
        lastEscAtRef.current = now;
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [
    permissionPrompt,
    askPrompt,
    planApproval,
    diffPreview,
    projectDialogOpen,
    projectAdding,
    handleInterrupt,
    handlePermissionChoose,
    handleAskDecline,
    handlePlanApproval,
  ]);

  // Ctrl/Cmd+1..9 jumps to a tab; Ctrl/Cmd+Shift+T reopens the last closed one.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (!(event.ctrlKey || event.metaKey) || event.altKey) return;
      if (event.key >= "1" && event.key <= "9" && !event.shiftKey) {
        const idx = Number(event.key) - 1;
        const tabs = openSessionIds
          .map((id) => availableSessions.find((s) => s.id === id))
          .filter((s): s is Session => Boolean(s));
        const target = tabs[idx];
        if (!target) return;
        event.preventDefault();
        openSession(target);
        return;
      }
      if (event.shiftKey && (event.key === "T" || event.key === "t")) {
        const id = lastClosedTabsRef.current[0];
        if (!id) return;
        const session = availableSessions.find((s) => s.id === id);
        if (!session) return;
        event.preventDefault();
        lastClosedTabsRef.current = lastClosedTabsRef.current.slice(1);
        saveClosedTabs(lastClosedTabsRef.current);
        openSession(session);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [availableSessions, openSessionIds]);

  /** P2-UX-4: edit You → truncate following events for this session + resend. */
  const handleEditResend = useCallback(
    async (anchor: UserMessageAnchor, newText: string) => {
      const sid = currentSessionIdRef.current;
      if (!sid || !newText.trim()) return;
      const session = sessionsRef.current.find((s) => s.id === sid);
      const agent = agentsRef.current.find((a) => a.id === session?.agentId);
      if (!session || !agent) return;

      // Cancel in-flight turn first.
      if (session.status === "running") {
        try {
          await cancelAcpSession(sid);
        } catch {
          // continue with local truncate
        }
      }

      const findCutIndex = (events: SessionEvent[]) => {
        if (anchor.messageId) {
          return events.findIndex(
            (e) =>
              e.sessionId === sid &&
              e.type === "user_message" &&
              e.messageId === anchor.messageId
          );
        }
        return events.findIndex(
          (e) =>
            e.sessionId === sid &&
            e.type === "user_message" &&
            e.createdAt === anchor.createdAt &&
            e.text === anchor.text
        );
      };

      const current = liveEventsRef.current;
      const cut = findCutIndex(current);
      if (cut < 0) {
        pushDebug({
          sessionId: sid,
          level: "warn",
          source: "edit-resend",
          summary: "could not find message to edit",
        });
        return;
      }
      const kept = current.filter((e, i) => {
        if (e.sessionId !== sid) return true;
        return i < cut;
      });
      const nextEvents = [...kept, userMessageEvent(sid, newText.trim(), sendMetaRef.current ?? undefined)];
      setLiveEvents(nextEvents);
      liveEventsRef.current = nextEvents;

      // Persist truncated transcript immediately.
      try {
        const forSession = persistableEventsForSession(nextEvents, sid);
        await writeTranscript(sid, forSession);
      } catch (error) {
        pushDebug({
          sessionId: sid,
          level: "warn",
          source: "edit-resend",
          summary: "write truncated transcript failed",
          detail: error instanceof Error ? error.message : String(error),
        });
      }

      renameSessionFromText(sid, newText.trim());
      pushDebug({
        sessionId: sid,
        level: "info",
        source: "edit-resend",
        summary: `edit&resend: ${newText.trim().slice(0, 80)}`,
      });

      try {
        disarmCancelWatchdog(sid);
        if (cancelIgnoredRef.current.has(sid)) {
          await restartAcpProcess(sid);
        }
        await ensureAcpReady(sid);
        let promptText = newText.trim();
        if (acpNeedsHistoryRef.current.has(sid)) {
          acpNeedsHistoryRef.current.delete(sid);
          // History is everything kept before the resend user message.
          const prefix = buildHistoryInjection(kept, sid);
          promptText = withHistoryInjection(prefix, promptText);
        }
        setSessionStatusById(sid, "running");
        await sendAcpPrompt(sid, promptText);
      } catch (error) {
        setSessionStatusById(sid, "error");
        const sess = sessionsRef.current.find((s) => s.id === sid);
        const agent = sess
          ? agentsRef.current.find((a) => a.id === sess.agentId)
          : undefined;
        const classified = classifyAgentError(error, {
          agentId: sess?.agentId,
          agentLabel: agent?.label,
        });
        setLiveEvents((events) => [
          ...events,
          {
            type: "assistant_message" as const,
            sessionId: sid,
            text: formatClassifiedError(classified),
            createdAt: new Date().toISOString(),
          },
        ]);
      }
    },
    [ensureAcpReady, pushDebug, renameSessionFromText, setSessionStatusById]
  );

  const countRunningDelegates = useCallback((parentId: string) => {
    let n = 0;
    for (const meta of delegateMetaRef.current.values()) {
      if (meta.parentId === parentId && !meta.finished) n += 1;
    }
    return n;
  }, []);

  const drainDelegateQueueRef = useRef<(parentId: string) => void>(() => undefined);

  const finalizeDelegateChild = useCallback(
    (
      childId: string,
      status: "done" | "failed" | "cancelled" | "timeout",
      error?: string,
    ) => {
      const meta = delegateMetaRef.current.get(childId);
      if (!meta || meta.finished) return;
      meta.finished = true;
      if (meta.idleTimer) clearTimeout(meta.idleTimer);

      const events = liveEventsRef.current.filter((e) => e.sessionId === childId);
      let summary = "";
      for (let i = events.length - 1; i >= 0; i--) {
        const e = events[i];
        if (e.type === "assistant_message" && e.text.trim()) {
          summary = e.text.trim();
          break;
        }
      }
      if (summary.length > 2000) summary = `${summary.slice(0, 2000)}…`;
      if (!summary && status === "done") summary = "(no assistant reply)";
      if (!summary && error) summary = error;

      const durationMs = Date.now() - meta.startedAt;
      const result: SessionEvent = {
        type: "subtask_result",
        sessionId: meta.parentId,
        childSessionId: childId,
        agentId: meta.agentId,
        status,
        summary,
        durationMs,
        error,
        createdAt: new Date().toISOString(),
      };
      setLiveEvents((current) => {
        const next = [...current, result];
        liveEventsRef.current = next;
        return next;
      });

      const subagentLabel = `${meta.agentLabel || "Subagent"} subagent`;
      if (status === "done") {
        scheduleDesktopNotify(childId, "subagent_complete", subagentLabel);
      } else if (status === "failed" || status === "timeout") {
        const failureDetail = compactNotifyDetail(error);
        void raiseDesktopNotify(
          "error",
          failureDetail
            ? `${subagentLabel} · ${failureDetail}`
            : `${subagentLabel} · ${status}`,
        );
      }

      void stopAcpSession(childId).catch(() => undefined);

      // Drain queue for this parent.
      const parentId = meta.parentId;
      window.setTimeout(() => {
        drainDelegateQueueRef.current(parentId);
      }, 0);
    },
    [],
  );

  const armDelegateIdleTimer = useCallback(
    (childId: string) => {
      const meta = delegateMetaRef.current.get(childId);
      if (!meta || meta.finished) return;
      if (meta.idleTimer) clearTimeout(meta.idleTimer);
      meta.idleTimer = setTimeout(() => {
        finalizeDelegateChild(childId, "timeout", "600s 无事件");
      }, DELEGATE_IDLE_TIMEOUT_MS);
    },
    [finalizeDelegateChild],
  );

  const startDelegateChild = useCallback(
    async (job: {
      parentId: string;
      projectId: string;
      agentId: string;
      agentLabel: string;
      modelId?: string;
      prompt: string;
      queuedCard?: boolean;
    }) => {
      const { parentId, projectId, agentId, agentLabel, modelId, prompt } = job;
      const agent =
        availableAgents.find((a) => a.id === agentId) ??
        agents.find((a) => a.id === agentId);
      if (!agent || agent.transport !== "acp") {
        setLiveEvents((current) => [
          ...current,
          {
            type: "subtask_result" as const,
            sessionId: parentId,
            childSessionId: `failed-${Date.now()}`,
            agentId,
            status: "failed" as const,
            summary: "",
            error: `Agent ${agentId} 不可用或非 ACP`,
            createdAt: new Date().toISOString(),
          },
        ]);
        return;
      }

      // Install check
      try {
        const statuses = await listAgentCommands();
        const st = statuses.find((s) => s.id === agentId);
        if (st && st.status !== "installed") {
          setLiveEvents((current) => [
            ...current,
            {
              type: "subtask_started" as const,
              sessionId: parentId,
              childSessionId: `fail-${Date.now()}`,
              agentId,
              agentLabel,
              modelId,
              prompt,
              createdAt: new Date().toISOString(),
            },
            {
              type: "subtask_result" as const,
              sessionId: parentId,
              childSessionId: `fail-${Date.now()}`,
              agentId,
              status: "failed" as const,
              summary: "",
              error: "agent 未安装",
              createdAt: new Date().toISOString(),
            },
          ]);
          return;
        }
      } catch {
        /* proceed; start will fail loudly */
      }

      const label = `→ ${agentLabel}${modelId ? `/${modelId}` : ""}: ${prompt.slice(0, 32)}`;
      let child: Session | null = null;
      try {
        child = await createChildSession(projectId, parentId, agentId, label);
      } catch (error) {
        setLiveEvents((current) => [
          ...current,
          {
            type: "subtask_result" as const,
            sessionId: parentId,
            childSessionId: `fail-${Date.now()}`,
            agentId,
            status: "failed" as const,
            summary: "",
            error: error instanceof Error ? error.message : String(error),
            createdAt: new Date().toISOString(),
          },
        ]);
        return;
      }
      if (!child) return;

      const startedAt = Date.now();
      delegateMetaRef.current.set(child.id, {
        parentId,
        agentId,
        agentLabel,
        modelId,
        prompt,
        startedAt,
        finished: false,
      });

      const started: SessionEvent = {
        type: "subtask_started",
        sessionId: parentId,
        childSessionId: child.id,
        agentId,
        agentLabel,
        modelId,
        prompt,
        createdAt: new Date().toISOString(),
      };
      setLiveEvents((current) => {
        const next = [...current, started];
        liveEventsRef.current = next;
        return next;
      });

      armDelegateIdleTimer(child.id);

      try {
        const caps = await startAcpSession(child.id, agent.command, agent.args, child.cwd);
        if (modelId && caps) {
          const attempts = expandAcpConfigAttempts(agentId, { model: modelId }, caps);
          for (const attempt of attempts) {
            try {
              await updateAcpSession(child.id, attempt);
              break;
            } catch {
              /* try next shape */
            }
          }
        }
        // Child has no parent history by design.
        await sendAcpPrompt(child.id, prompt);
      } catch (error) {
        finalizeDelegateChild(
          child.id,
          "failed",
          error instanceof Error ? error.message : String(error),
        );
      }
    },
    [availableAgents, armDelegateIdleTimer, finalizeDelegateChild],
  );

  const drainDelegateQueue = useCallback(
    async (parentId: string) => {
      while (countRunningDelegates(parentId) < MAX_DELEGATE_CONCURRENT) {
        const q = delegateQueueRef.current.get(parentId);
        if (!q?.length) break;
        const job = q.shift()!;
        await startDelegateChild({
          parentId,
          projectId: job.projectId,
          agentId: job.agentId,
          agentLabel: job.agentLabel,
          modelId: job.modelId,
          prompt: job.prompt,
        });
      }
    },
    [countRunningDelegates, startDelegateChild],
  );
  drainDelegateQueueRef.current = (parentId: string) => {
    void drainDelegateQueue(parentId);
  };
  finalizeDelegateChildRef.current = finalizeDelegateChild;

  const handleDelegate = useCallback(
    async (
      parentId: string,
      projectId: string,
      parsed: { agentId: string; modelId?: string; prompt: string },
    ) => {
      const agent =
        availableAgents.find((a) => a.id === parsed.agentId) ??
        agents.find((a) => a.id === parsed.agentId);
      const agentLabel = agent?.label ?? parsed.agentId;

      if (countRunningDelegates(parentId) >= MAX_DELEGATE_CONCURRENT) {
        const q = delegateQueueRef.current.get(parentId) ?? [];
        q.push({
          agentId: parsed.agentId,
          agentLabel,
          modelId: parsed.modelId,
          prompt: parsed.prompt,
          projectId,
        });
        delegateQueueRef.current.set(parentId, q);
        // Placeholder card so the user sees "排队中"
        setLiveEvents((current) => [
          ...current,
          {
            type: "subtask_started" as const,
            sessionId: parentId,
            childSessionId: `queued-${Date.now()}-${q.length}`,
            agentId: parsed.agentId,
            agentLabel,
            modelId: parsed.modelId,
            prompt: `（排队中）${parsed.prompt}`,
            createdAt: new Date().toISOString(),
          },
        ]);
        return;
      }

      await startDelegateChild({
        parentId,
        projectId,
        agentId: parsed.agentId,
        agentLabel,
        modelId: parsed.modelId,
        prompt: parsed.prompt,
      });
    },
    [availableAgents, countRunningDelegates, startDelegateChild],
  );

  /**
   * Ask about paths outside the project *before* sending.
   *
   * A subagent's approval prompt never reaches us (its runtime handles it
   * in-process and does not forward it over ACP), so it would sit silent
   * forever. The only moment we can widen the scope is `session/new`, which
   * means the answer has to be collected before the turn starts.
   */
  const handleSend = async (
    text: string,
    droppedPaths: string[] = [],
    imageAttachments: ImageAttachment[] = [],
    opts?: {
      modeId?: string | null;
      modeLabel?: string | null;
      modelId?: string | null;
      modelLabel?: string | null;
      effortLabel?: string | null;
    },
  ) => {
    if (!currentSessionId) return;
    const sid = currentSessionId;

    // @-delegate: line-start @agent task — does not block the parent dialog.
    // Images on a delegate line are ignored for now (depth=1, simple task text).
    const knownIds = availableAgents.map((a) => a.id);
    const delegated = parseDelegateLine(text, knownIds);
    if (delegated) {
      const projectId = displaySession.projectId || currentProjectId;
      setQuotePins([]);
      await handleDelegate(sid, projectId, delegated);
      return;
    }

    // Merge quote pins + image mark text + free Composer text.
    const pins = quotePins;
    let composed = pins.length > 0 ? formatPinsForSend(pins, text) : text;
    const markBlock = formatImageMarksForSend(imageAttachments);
    if (markBlock) {
      composed = composed.trim()
        ? `${markBlock}\n\n${composed.trim()}`
        : markBlock;
    }
    if (!composed.trim() && imageAttachments.length === 0) return;

    const projectId = displaySession.projectId || currentProjectId;
    const candidates = [
      ...new Set([
        ...droppedPaths,
        ...imageAttachments.map((a) => a.path),
        ...findLinkTargets(composed)
          .filter((target) => target.kind === "path")
          .map((target) => target.raw),
      ]),
    ];
    // Chat has a real cwd but no project-level `.marionette` scope. Its agent
    // is already started in that cwd, so do not ask to persist a project grant.
    const outside = displaySession.projectId === CHAT_PROJECT_ID
      ? []
      : await checkOutsideProjectPaths(projectId, candidates).catch(() => []);
    if (outside.length > 0) {
      setPathGrantPrompt({
        paths: outside,
        text: composed,
        sessionId: sid,
        imageAttachments,
        composerSnap: opts
          ? {
              modeId: opts.modeId,
              modeLabel: opts.modeLabel,
              modelId: opts.modelId,
              modelLabel: opts.modelLabel,
              effortLabel: opts.effortLabel,
            }
          : undefined,
      });
      return;
    }

    setQuotePins([]);
    await performSend(sid, composed, imageAttachments, opts);
  };

  /** One-click: ask the active agent to commit local changes and push. */
  const handleCommitAndPush = () => {
    if (changedFiles.length === 0) return;
    const badge = (t: ChangedFile["changeType"]) =>
      t === "added" ? "A" : t === "deleted" ? "D" : t === "untracked" ? "U" : "M";
    const list = changedFiles
      .map((f) => `- ${badge(f.changeType)} ${f.path}`)
      .join("\n");
    const parts = [
      "请整理当前工作区并推送到远端：只提交与本项目相关的改动。",
      "先审一遍改动列表：构建产物、缓存、密钥、本地配置、日志、依赖目录、临时文件等与项目无关或不该进仓库的内容，写入或更新 `.gitignore`，不要把它们 commit 进去。",
      "只 stage 真正属于本项目的源码/配置/文档等改动；写清晰的 commit message，完成 commit，再 push。",
      "不要改业务逻辑代码；若 push 需要设置 upstream，使用合理的 `-u`。拿不准是否该忽略时先问我，不要瞎加。",
    ];
    if (gitBranch) parts.push(`当前分支：\`${gitBranch}\``);
    parts.push("", "当前改动文件：", list);
    void handleSend(parts.join("\n"));
  };

  const performSend = async (
    sid: string,
    composed: string,
    imageAttachments: ImageAttachment[] = [],
    composerSnap?: {
      modeId?: string | null;
      modeLabel?: string | null;
      modelId?: string | null;
      modelLabel?: string | null;
      effortLabel?: string | null;
    },
    options?: {
      /** Flushing a queued follow-up: no card exists yet — create it now,
       *  right after the reply that just freed the slot. */
      flushQueued?: boolean;
    },
  ) => {
    try {
      // An agent switch leaves handoff notes waiting — attach them to this send
      // (the composer stays clean; only the wire payload carries them).
      const handoff = pendingHandoff(liveEventsRef.current, sid);

      // Prefer Composer chip snapshot (what the user saw at send). Caps.currentMode
      // lags on purpose after a mode switch — agent often still echoes the old mode.
      const caps = sessionCapabilities;
      const modeIdRaw =
        composerSnap?.modeId?.trim() ||
        displaySession.preferredMode?.trim() ||
        caps?.currentMode ||
        null;
      const modeId = modeIdRaw
        ? normalizeAgentModeId(currentAgent.id, modeIdRaw)
        : null;
      const modeLabel =
        composerSnap?.modeLabel?.trim() ||
        (modeId ? caps?.modes.find((m) => m.id === modeId)?.label ?? modeId : undefined);
      const modelId =
        composerSnap?.modelId?.trim() ||
        activeModelId ||
        displaySession.preferredModel?.trim() ||
        caps?.currentModel ||
        null;
      const modelLabel =
        composerSnap?.modelLabel?.trim() ||
        (modelId ? caps?.models.find((m) => m.id === modelId)?.label ?? modelId : undefined);
      const effortId = displaySession.preferredEffortId;
      const effortLabelVal =
        composerSnap?.effortLabel?.trim() ||
        (effortId
          ? prettyEffortLabel(
              caps?.effortOptions?.find((o) => o.id === effortId)?.label,
              effortId,
            )
          : displaySession.preferredEffort != null
            ? effortLabel(displaySession.preferredEffort)
            : undefined);
      sendMetaRef.current = {
        agentId: currentAgent.id,
        agentLabel: currentAgent.label,
        modelId: modelId ?? undefined,
        modelLabel: modelLabel ?? undefined,
        modeLabel: modeLabel ?? undefined,
        effortLabel: effortLabelVal || undefined,
      };
      delete turnStartedAtRef.current[sid];

      // Show the user message immediately; wait for ACP only after that.
      // History injection uses events *before* this message.
      const priorForInject = liveEventsRef.current.filter((e) => e.sessionId === sid);
      const liveStatus = sessionsRef.current.find((s) => s.id === sid)?.status;
      const pendingQueue = pendingSendsRef.current.get(sid) ?? [];
      const flushing = flushingSendRef.current.has(sid);
      // One ACP prompt at a time. A send while a turn is live (or while a
      // follow-up is already flushing) queues; we then cancel the live turn
      // so the follow-up is wired as soon as the slot frees — not after the
      // agent finishes the current reply on its own.
      const shouldQueue =
        !options?.flushQueued &&
        (liveStatus === "running" || flushing || pendingQueue.length > 0);

      // Follow-ups wait in a strip above the composer — a timeline card here
      // would sit mid-stream and split the running reply into two bubbles.
      // The card is created when the flush happens, right after cancel.
      if (shouldQueue) {
        const queue = pendingSendsRef.current.get(sid) ?? [];
        queue.push({ composed, imageAttachments, composerSnap });
        pendingSendsRef.current.set(sid, queue);
        // Shelf order is recency-only: bump lastActiveAt only when the user sends.
        const activeAt = new Date().toISOString();
        setAvailableSessions((current) =>
          current.map((s) => (s.id === sid ? { ...s, lastActiveAt: activeAt } : s))
        );
        renameSessionFromText(sid, composed || imageAttachments[0]?.name || "Image");
        touchActivity(sid);
        setQueuedStripTick((t) => t + 1);
        pushDebug({
          sessionId: sid,
          level: "info",
          source: "composer",
          summary:
            liveStatus === "running"
              ? `mid-turn follow-up (interrupt then send): ${composed.length > 80 ? `${composed.slice(0, 80)}…` : composed}`
              : `queued (agent busy): ${composed.length > 80 ? `${composed.slice(0, 80)}…` : composed}`,
        });
        if (liveStatus === "running") {
          void cancelLiveTurn(sid, { announce: false });
        }
        return;
      }

      // TTFT anchor: the moment this prompt is about to go over the wire
      // (queued sends only reach here at flush time, so the anchor is honest).
      turnSentAtRef.current[sid] = Date.now();
      grokTurnUsageRef.current[sid] = emptyGrokTurnUsage();
      lastSendBySessionRef.current.set(sid, {
        composed,
        imageAttachments,
        composerSnap,
      });
      setComposerFailure((cur) => (cur?.sessionId === sid ? null : cur));

      // Starting a new turn supersedes a delayed completion cue for the
      // previous turn (the same reset OpenCode performs when it sees busy).
      cancelScheduledDesktopNotify(sid);
      const um = userMessageEvent(sid, composed, {
        ...(sendMetaRef.current ?? {}),
        attachments: imageAttachments.length > 0 ? imageAttachments : undefined,
        sentAt: new Date(turnSentAtRef.current[sid] ?? Date.now()).toISOString(),
      });
      setLiveEvents((current) => [...current, um]);
      // Shelf order is recency-only: bump lastActiveAt only when the user sends
      // (selecting a dialog must not jump it to the top).
      const activeAt = new Date().toISOString();
      setAvailableSessions((current) =>
        current.map((s) => (s.id === sid ? { ...s, lastActiveAt: activeAt } : s))
      );
      renameSessionFromText(sid, composed || imageAttachments[0]?.name || "Image");
      touchActivity(sid);
      setQueuedStripTick((t) => t + 1);
      pushDebug({
        sessionId: sid,
        level: "info",
        source: "composer",
        summary: options?.flushQueued
          ? `flush queued: ${composed.length > 80 ? `${composed.slice(0, 80)}…` : composed}`
          : `send: ${composed.length > 80 ? `${composed.slice(0, 80)}…` : composed}`,
      });
      // Sending supersedes any pending verdict on the last interrupt — do not
      // let a stale watchdog fire mid-turn and mark this session for restart.
      disarmCancelWatchdog(sid);
      // Allow a new turn's Thinking/Reply stream after a local cancel.
      streamSuppressedRef.current.delete(sid);
      // A cancel the agent never acknowledged leaves the process wedged;
      // prompting it again would just hang. Replace it before sending.
      if (cancelIgnoredRef.current.has(sid)) {
        await restartAcpProcess(sid);
      }
      // Ensure agent is up (may already be warming from keystrokes).
      await ensureAcpReady(sid);

      // Codex (and friends) take approval+sandbox from the session mode on
      // each turn. The chip is optimistic: Composer paints preferredMode /
      // a menu pick before set_config finishes, and a fresh session/new
      // always starts at the harness default ("agent" = workspace sandbox).
      // If we prompt without re-pushing the chip, full access shows in the UI
      // while Codex still asks for every shell/edit. Same pattern as child
      // delegates: start → set config → prompt.
      {
        const agentId = currentAgent.id;
        const live = await getSessionCapabilities(sid).catch(() => null);
        const liveCaps = mergeAcpCapabilities(agentId, live) ?? live;
        const pushConfig = async (patch: Record<string, unknown>) => {
          const attempts = expandAcpConfigAttempts(agentId, patch, liveCaps);
          for (const attempt of attempts) {
            try {
              await updateAcpSession(sid, attempt);
              return true;
            } catch {
              /* try next config shape */
            }
          }
          return false;
        };
        // Codex: always re-assert mode before the turn. Local currentMode can
        // claim full access after an optimistic chip paint or a failed restore
        // while the harness is still on default "agent" (workspace sandbox).
        const codexMode = agentId === "codex" || agentId === "codex-acp";
        if (modeId && (codexMode || modeId !== liveCaps?.currentMode)) {
          const ok = await pushConfig({ mode: modeId });
          if (codexMode || modeId !== liveCaps?.currentMode) {
            pushDebug({
              sessionId: sid,
              level: ok ? "info" : "warn",
              source: "composer",
              summary: ok
                ? `pre-prompt mode sync: ${modeId}`
                : `pre-prompt mode sync failed: ${modeId}`,
            });
          }
        }
        if (modelId && modelId !== liveCaps?.currentModel) {
          await pushConfig({ model: modelId });
        }
        if (
          effortId &&
          effortId !== liveCaps?.currentEffortId &&
          liveCaps?.effortConfigId
        ) {
          await pushConfig({ effortId });
        } else if (
          typeof displaySession.preferredEffort === "number" &&
          liveCaps?.effortConfigId &&
          displaySession.preferredEffort !== liveCaps.currentEffort
        ) {
          await pushConfig({ thinkingEffort: displaySession.preferredEffort });
        }
      }

      // The reconnect send already carries the whole transcript — then the
      // handoff shrinks to a pointer instead of repeating the same context.
      const willInjectHistory = acpNeedsHistoryRef.current.has(sid);
      let promptText = withHandoffAttachment(handoff, composed, {
        compact: willInjectHistory,
      });
      if (handoff) {
        pushDebug({
          sessionId: sid,
          level: "info",
          source: "handoff",
          summary: `attached pending handoff to this send${willInjectHistory ? " (compact)" : ""}`,
          detail: handoff.handoffPath,
        });
      }
      if (acpNeedsHistoryRef.current.has(sid)) {
        acpNeedsHistoryRef.current.delete(sid);
        // Skills this agent does not ship with — a pointer list, once per
        // connection, so it can read the SKILL.md itself when relevant.
        const skillsPrefix = displaySession.projectId === CHAT_PROJECT_ID
          ? null
          : await projectContextPrompt(
              displaySession.projectId || currentProjectId,
              currentAgent.id,
            ).catch(() => null);
        if (skillsPrefix) {
          promptText = `${skillsPrefix}${promptText}`;
          pushDebug({
            sessionId: sid,
            level: "info",
            source: "context",
            summary: "injected project skills list",
            detail: `chars=${skillsPrefix.length}`,
          });
        }
        const prefix = buildHistoryInjection(priorForInject, sid);
        promptText = withHistoryInjection(prefix, promptText);
        if (prefix) {
          pushDebug({
            sessionId: sid,
            level: "info",
            source: "acp",
            summary: "injected local transcript into first prompt after reconnect",
            detail: `prefixChars=${prefix.length}`,
          });
        }
      }

      setSessionStatusById(sid, "running");
      touchActivity(sid);
      const imagePaths = imageAttachments.map((a) => a.path);
      const wireText = promptText;
      const projectId = displaySession.projectId || currentProjectId;
      const fileSnapshot = displaySession.projectId === CHAT_PROJECT_ID
        ? null
        : await captureProjectFileSnapshot(projectId);
      if (fileSnapshot) fileChangeSnapshotsRef.current[sid] = fileSnapshot;
      await sendAcpPrompt(sid, wireText, imagePaths);
      touchActivity(sid);
      pushDebug({
        sessionId: sid,
        level: "info",
        source: "composer",
        summary: `session/prompt accepted (streaming…)${imagePaths.length ? ` images=${imagePaths.length}` : ""}`,
      });
    } catch (error) {
      flushingSendRef.current.delete(sid);
      delete fileChangeSnapshotsRef.current[sid];
      delete fileChangePublishedRef.current[sid];
      fileChangeFinalizeRef.current.delete(sid);
      setSessionStatusById(sid, "error");
      const sess = sessionsRef.current.find((s) => s.id === sid);
      const agent = sess
        ? agentsRef.current.find((a) => a.id === sess.agentId)
        : undefined;
      const classified = classifyAgentError(error, {
        agentId: sess?.agentId,
        agentLabel: agent?.label,
      });
      const label = sess?.label?.trim() || "Session";
      const sendError = compactNotifyDetail(classified.message);
      void raiseDesktopNotify(
        "error",
        sendError ? `${label} · ${sendError}` : `${label} · Send failed`,
      );
      if (classified.kind === "auth" && sess?.agentId) {
        const spec = agentAuthSpec(sess.agentId);
        if (spec) {
          setAuthHintFor(
            sess.agentId,
            spec.login
              ? `需要登录 — 点 Sign in，或终端执行 \`${spec.loginCommand}\``
              : `需要登录 — 终端执行 \`${spec.loginCommand}\``
          );
        }
      }
      setComposerFailure({ sessionId: sid, error: classified });
      setLiveEvents((current) => [
        ...current,
        {
          type: "assistant_message" as const,
          sessionId: sid,
          text: formatClassifiedError(classified),
          createdAt: new Date().toISOString(),
        },
      ]);
      pushDebug({
        sessionId: sid,
        level: "error",
        source: "composer",
        summary: `send failed (${classified.kind})`,
        detail: classified.message,
      });
    }
  };

  // Drain follow-ups when a turn frees the session (natural end or cancel).
  drainQueuedSendRef.current = (sessionId: string) => {
    void (async () => {
      if (flushingSendRef.current.has(sessionId)) return;
      const queue = pendingSendsRef.current.get(sessionId);
      if (!queue?.length) return;
      const status = sessionsRef.current.find((s) => s.id === sessionId)?.status;
      // Wait until the live turn slot is free.
      if (status === "running" || status === "starting") return;

      flushingSendRef.current.add(sessionId);
      try {
        const next = queue.shift()!;
        if (queue.length === 0) pendingSendsRef.current.delete(sessionId);
        else pendingSendsRef.current.set(sessionId, queue);
        await performSend(
          sessionId,
          next.composed,
          next.imageAttachments,
          next.composerSnap,
          { flushQueued: true },
        );
      } finally {
        flushingSendRef.current.delete(sessionId);
      }
    })();
  };

  // Drain only after the idle status is committed. Reading sessionsRef from a
  // microtask right after setSessionStatusById can still see the pre-render
  // "running" value (React commits async updates on a later macrotask), which
  // made the drain above early-return and leave the queue stuck forever.
  // Watching committed transitions is deterministic — it also catches queue
  // items added while a flush was already in flight.
  const lastStatusByIdRef = useRef<Record<string, Session["status"]>>({});
  useEffect(() => {
    const seen = lastStatusByIdRef.current;
    for (const session of availableSessions) {
      const prev = seen[session.id];
      const now = session.status;
      if (prev === undefined) {
        seen[session.id] = now;
        continue;
      }
      if (prev === now) continue;
      seen[session.id] = now;
      const freesSlot =
        (prev === "running" || prev === "starting") && now !== "running" && now !== "starting";
      if (freesSlot) queueMicrotask(() => drainQueuedSendRef.current(session.id));
    }
  }, [availableSessions]);

  /** Dismiss outside-project prompt without sending; restore draft to Composer. */
  const cancelPathGrant = useCallback(() => {
    const prompt = pathGrantPrompt;
    if (!prompt || pathGrantBusy) return;
    setPathGrantPrompt(null);
    // Composer already cleared the draft on submit — put the held text back.
    setComposerPrefill({ text: prompt.text, token: Date.now() });
  }, [pathGrantPrompt, pathGrantBusy]);

  /** Grant the folders, restart the agent if it is live, then send the held draft. */
  const resolvePathGrant = useCallback(
    async () => {
      const prompt = pathGrantPrompt;
      if (!prompt || pathGrantBusy) return;
      setPathGrantBusy(true);
      try {
        let mustRestart = false;
        const projectId = displaySession.projectId || currentProjectId;
        for (const item of prompt.paths) {
          try {
            const result = await grantWorkspaceRoot(projectId, item.dir, prompt.sessionId);
            mustRestart = mustRestart || result.restartNeeded;
          } catch (error) {
            pushDebug({
              sessionId: prompt.sessionId,
              level: "warn",
              source: "context",
              summary: "grant workspace root failed",
              detail: error instanceof Error ? error.message : String(error),
            });
          }
        }
        if (mustRestart) {
          // Scope is fixed at session/new — the live process cannot learn it.
          await stopAcpSession(prompt.sessionId).catch(() => undefined);
          acpBootstrapRef.current.delete(prompt.sessionId);
          setSessionCapabilities(null);
          setSessionStatusById(prompt.sessionId, "exited");
          acpNeedsHistoryRef.current.add(prompt.sessionId);
          pushDebug({
            sessionId: prompt.sessionId,
            level: "info",
            source: "context",
            summary: "reconnecting to apply new workspace roots",
          });
        }
        void refreshProjectContext(projectId);
        setQuotePins([]);
        setPathGrantPrompt(null);
        await performSend(
          prompt.sessionId,
          prompt.text,
          prompt.imageAttachments ?? [],
          prompt.composerSnap,
        );
      } finally {
        setPathGrantBusy(false);
      }
    },
    // performSend is defined below in the same component scope
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [pathGrantPrompt, pathGrantBusy, displaySession.projectId, currentProjectId, pushDebug, refreshProjectContext, setSessionStatusById]
  );

  const handleAddProject = async () => {
    const path = projectPath.trim();
    if (!path) {
      setProjectError("Enter a project folder path.");
      return;
    }
    setProjectAdding(true);
    setProjectError("");
    try {
      const project = await addProject(path);
      setAvailableProjects((current) => [...current.filter((item) => item.id !== project.id), project]);
      setCurrentProjectId(project.id);
      setProjectDialogOpen(false);
      setProjectPath("");
      // Auto-open a conversation so ACP/composer are usable immediately.
      await createSessionForProject(project.id, availableAgents[0]?.id ?? agents[0].id, project);
      // First look at a new folder: show what this machine can lend it.
      void refreshProjectContext(project.id);
    } catch (error) {
      setProjectError(String(error));
    } finally {
      setProjectAdding(false);
    }
  };

  const handleDeleteProject = (projectId: string) => {
    const project = availableProjects.find((item) => item.id === projectId);
    if (!project) return;

    const projectSessions = availableSessions.filter((session) => session.projectId === projectId);
    for (const session of projectSessions) {
      if (session.status === "starting" || session.status === "running" || session.status === "waiting") {
        void stopAcpSession(session.id).catch(() => undefined);
      }
      void deleteSessionApi(projectId, session.id).catch(() => undefined);
    }

    void deleteProjectApi(projectId).catch(() => undefined);

    const remainingProjects = availableProjects.filter((item) => item.id !== projectId);
    const removedSessionIds = new Set(projectSessions.map((session) => session.id));

    setAvailableProjects(remainingProjects);
    setAvailableSessions((current) => current.filter((session) => session.projectId !== projectId));
    setOpenSessionIds((current) => current.filter((id) => !removedSessionIds.has(id)));
    setSessionUsageById((current) => {
      let changed = false;
      const next = { ...current };
      for (const id of removedSessionIds) {
        if (id in next) {
          delete next[id];
          changed = true;
        }
      }
      return changed ? next : current;
    });

    if (currentProjectId === projectId) {
      const nextProject = remainingProjects[0];
      if (nextProject) {
        setCurrentProjectId(nextProject.id);
        const nextSession = availableSessions.find(
          (session) => session.projectId === nextProject.id && !removedSessionIds.has(session.id)
        );
        if (nextSession) {
          openSession(nextSession);
        } else {
          void createSessionForProject(nextProject.id);
        }
      } else {
        setCurrentProjectId("");
        setCurrentSessionId("");
        setOpenSessionIds([]);
        setViewMode("clean");
        setSessionCapabilities(null);
      }
    }
  };

  // ── Center-workspace edge hover — detects mouse near chat area edges ──
  const handleCenterMove = useCallback((e: React.MouseEvent) => {
    // Selecting / dragging: never pop rail toggles under the pointer — that
    // steals hit-testing and jumps the text selection (esp. right→left drags).
    if (e.buttons !== 0) {
      wasNearLeft.current = false;
      wasNearRight.current = false;
      if (edgeHoverTimers.current.left) clearTimeout(edgeHoverTimers.current.left);
      if (edgeHoverTimers.current.right) clearTimeout(edgeHoverTimers.current.right);
      setLeftEdgeHover(false);
      setRightEdgeHover(false);
      return;
    }

    const rect = e.currentTarget.getBoundingClientRect();
    const distLeft = e.clientX - rect.left;
    const distRight = rect.right - e.clientX;
    const nearLeft = distLeft < 32;
    const nearRight = distRight < 32;

    if (nearLeft) {
      if (edgeHoverTimers.current.left) clearTimeout(edgeHoverTimers.current.left);
      setLeftEdgeHover(true);
      wasNearLeft.current = true;
    } else if (wasNearLeft.current) {
      wasNearLeft.current = false;
      edgeHoverTimers.current.left = setTimeout(() => setLeftEdgeHover(false), 600);
    }

    if (nearRight) {
      if (edgeHoverTimers.current.right) clearTimeout(edgeHoverTimers.current.right);
      setRightEdgeHover(true);
      wasNearRight.current = true;
    } else if (wasNearRight.current) {
      wasNearRight.current = false;
      edgeHoverTimers.current.right = setTimeout(() => setRightEdgeHover(false), 600);
    }
  }, []);

  const handleCenterLeave = useCallback(() => {
    wasNearLeft.current = false;
    wasNearRight.current = false;
    if (edgeHoverTimers.current.left) clearTimeout(edgeHoverTimers.current.left);
    if (edgeHoverTimers.current.right) clearTimeout(edgeHoverTimers.current.right);
    edgeHoverTimers.current.left = setTimeout(() => setLeftEdgeHover(false), 600);
    edgeHoverTimers.current.right = setTimeout(() => setRightEdgeHover(false), 600);
  }, []);

  return (
    <main className="app-shell">
      <div
        ref={workspaceGridRef}
        className={`workspace-grid${leftCollapsed || IS_DETACHED_WINDOW ? " is-left-collapsed" : ""}${rightCollapsed ? " is-right-collapsed" : ""}${IS_DETACHED_WINDOW ? " is-detached" : ""}`}
        style={
          {
            "--left-panel-width": `${leftWidth}px`,
            "--right-panel-width": `${rightWidth}px`,
          } as CSSProperties
        }
      >
        {/* Left sidebar: full height (spans both rows) */}
        <aside className={leftCollapsed || IS_DETACHED_WINDOW ? "left-rail is-collapsed" : "left-rail"} aria-label="Projects and sessions">
          {!IS_DETACHED_WINDOW && (
          <ProjectShelf
            agents={availableAgents}
            projects={availableProjects.filter((p) => p.id !== CHAT_PROJECT_ID)}
            sessions={availableSessions}
            currentProjectId={currentProjectId}
            currentSessionId={displaySession.id}
            pinnedProjectId={openHereProjectId}
            collapsed={leftCollapsed}
            theme={theme}
            desktopNotifyEnabled={desktopNotifyOn}
            proxyConfig={proxyConfig}
            onSaveProxy={handleSaveProxy}
            onTestProxy={handleTestProxy}
            onToggleDesktopNotify={() => {
              setDesktopNotifyOn((current) => {
                const next = !current;
                setDesktopNotifyEnabled(next);
                return next;
              });
            }}
            searchHitIds={searchHitIds}
            onSearchQueryChange={(q) => {
              void handleShelfSearch(q);
            }}
            onCollapse={() => setLeftCollapsed(true)}
            onExpand={() => setLeftCollapsed(false)}
            onToggleTheme={() => setTheme((current) => nextTheme(current))}
            onAddProject={() => { setProjectDialogOpen(true); setProjectError(""); }}
            onNewSession={(projectId) => {
              createSessionForProject(projectId);
            }}
            onProjectSelect={(projectId) => {
              setCurrentProjectId(projectId);
              const nextSession = availableSessions.find((session) => session.projectId === projectId);
              if (nextSession) {
                openSession(nextSession);
              } else {
                // Selecting a project with no sessions should still get a usable dialog.
                void createSessionForProject(projectId);
              }
            }}
            onSessionSelect={openSession}
            onDeleteSession={deleteSession}
            onDeleteProject={handleDeleteProject}
            onRenameSession={handleRenameSession}
            onToggleSessionPin={handleToggleSessionPin}
            onReorderProjects={handleReorderProjects}
            onRevealProject={(project) => {
              void revealInFileManager(project.rootPath).catch(() => undefined);
            }}
            defaultFolderPath={defaultFolderPath}
            chatSessions={chatSessions}
            onChatSessionSelect={openChatSession}
            onNewChat={handleCreateChatSession}
          />
          )}
          {!leftCollapsed && !IS_DETACHED_WINDOW && (
            <button
              type="button"
              className={resizingSide === "left" ? "panel-resizer panel-resizer--left is-dragging" : "panel-resizer panel-resizer--left"}
              aria-label="Resize projects panel"
              title="Drag to resize"
              onMouseDown={(event) => {
                event.preventDefault();
                setResizingSide("left");
              }}
            />
          )}
        </aside>

        {/* Shared titlebar across center + right columns */}
        <div className="workspace-titlebar">
          <SessionTabs
            openSessions={openSessions}
            session={displaySession}
            onTabSelect={openSession}
            onTabClose={closeSessionTab}
            onNewTab={() => {
              if (displaySession.projectId === CHAT_PROJECT_ID) {
                void handleCreateChatSession();
              } else {
                void createSessionForProject(currentProject?.id ?? "");
              }
            }}
            onRenameSession={handleRenameSession}
            onTabPopOut={
              IS_DETACHED_WINDOW
                ? undefined
                : (s, viaDrag) => void handleTabPopOut(s, viaDrag)
            }
            onTabMergeBack={
              IS_DETACHED_WINDOW
                ? (s, viaDrag) => void handleMergeBack(s, viaDrag)
                : undefined
            }
            mergeDropActive={IS_DETACHED_WINDOW ? false : mergeTargetActive}
            detachedMode={IS_DETACHED_WINDOW}
          />
          <div className="workspace-titlebar__spacer" data-tauri-drag-region />
          <WindowControls />
        </div>

        {/* Center workspace — hover edges to reveal sidebar toggle buttons */}
        <section
          className="center-workspace"
          aria-label="Active workspace"
          onMouseMove={handleCenterMove}
          onMouseLeave={handleCenterLeave}
        >
          {/* Absolute inside center: always inset from the seam, never overlaps the rail border */}
          {(leftCollapsed || leftEdgeHover) && (
            <button
              type="button"
              className={`floating-trigger floating-trigger--center-left${leftEdgeHover ? " is-visible" : ""}`}
              onClick={() => setLeftCollapsed(!leftCollapsed)}
              aria-label={leftCollapsed ? "Show projects and sessions" : "Hide projects and sessions"}
              title={leftCollapsed ? "Show projects and sessions" : "Hide projects and sessions"}
            >
              {leftCollapsed ? <ChevronRight size={12} /> : <ChevronLeft size={12} />}
            </button>
          )}
          {(rightCollapsed || rightEdgeHover) && (
            <button
              type="button"
              className={`floating-trigger floating-trigger--center-right${rightEdgeHover ? " is-visible" : ""}`}
              onClick={() => setRightCollapsed(!rightCollapsed)}
              aria-label={rightCollapsed ? "Show information panel" : "Hide information panel"}
              title={rightCollapsed ? "Show information panel" : "Hide information panel"}
            >
              {rightCollapsed ? <ChevronLeft size={12} /> : <ChevronRight size={12} />}
            </button>
          )}
          <SessionView
            agent={currentAgent}
            events={currentEvents}
            session={displaySession}
            viewMode={viewMode}
            openSessions={openSessions}
            lastActivityAt={lastActivityById[displaySession.id] ?? null}
            authBanner={agentAuthHint[currentAgent.id] ?? null}
            onSignIn={
              agentAuthSpec(currentAgent.id)?.login
                ? () => void handleAgentSignIn(currentAgent.id)
                : undefined
            }
            signInBusy={signInBusy}
            onTabSelect={openSession}
            onTabClose={closeSessionTab}
            onNewTab={() => {
              if (displaySession.projectId === CHAT_PROJECT_ID) {
                void handleCreateChatSession();
              } else {
                void createSessionForProject(currentProject?.id ?? "");
              }
            }}
            onSessionStatusChange={handleSessionStatusChange}
            onCapabilities={setSessionCapabilities}
            onEditResend={(anchor, text) => void handleEditResend(anchor, text)}
            quotePins={quotePins}
            onQuotePinsChange={setQuotePins}
            onInterrupt={() => void handleInterrupt()}
            projectId={currentProjectId || displaySession.projectId}
            allEvents={liveEvents}
            onSubtaskStop={(childId) => {
              void cancelAcpSession(childId).catch(() => undefined);
              finalizeDelegateChildRef.current(childId, "cancelled", "用户停止");
            }}
            onSubtaskQuote={(summary) => {
              const block = `> ${summary.replace(/\n/g, "\n> ").slice(0, 2000)}\n\n`;
              setComposerPrefill({ text: block, token: Date.now() });
            }}
            onSubtaskRetry={(childId) => {
              const meta = delegateMetaRef.current.get(childId);
              const startEv = liveEventsRef.current.find(
                (e) =>
                  e.type === "subtask_started" &&
                  e.childSessionId === childId &&
                  e.sessionId === displaySession.id,
              );
              if (!startEv || startEv.type !== "subtask_started") return;
              const projectId = displaySession.projectId || currentProjectId;
              void handleDelegate(displaySession.id, projectId, {
                agentId: startEv.agentId,
                modelId: startEv.modelId,
                prompt: startEv.prompt.replace(/^（排队中）/, ""),
              });
              void meta;
            }}
          />
          {/* Only the active dialog's pending cards — never leak from another session. */}
          {askPrompt && askPrompt.sessionId === displaySession.id && (
            <AskQuestionCard
              key={askPrompt.requestId}
              prompt={askPrompt}
              busy={askBusy}
              onSubmit={(answers) => void handleAskSubmit(answers)}
              onDecline={() => void handleAskDecline()}
            />
          )}
          {planApproval &&
            planApproval.sessionId === displaySession.id &&
            !(askPrompt && askPrompt.sessionId === displaySession.id) && (
            <PlanApprovalCard
              key={planApproval.requestId}
              prompt={{
                ...planApproval,
                // Grok often omits planContent; fall back to live plan entries.
                planMarkdown:
                  planApproval.planMarkdown.trim() ||
                  (planBySessionId[planApproval.sessionId] ?? [])
                    .map((e, i) => {
                      const status = e.status ? ` [${e.status}]` : "";
                      return `${i + 1}. ${e.content}${status}`;
                    })
                    .join("\n"),
              }}
              busy={planApprovalBusy}
              onAnswer={(decision, feedback) => handlePlanApproval(decision, feedback)}
            />
          )}
          <div className="composer-slot">
          <div className="composer-slot__notices">
          {composerFailure && composerFailure.sessionId === displaySession.id && (
            <ComposerErrorStrip
              error={composerFailure.error}
              canSignIn={Boolean(agentAuthSpec(currentAgent.id)?.login)}
              onRetry={() => {
                const last = lastSendBySessionRef.current.get(displaySession.id);
                if (!last) return;
                setComposerFailure(null);
                void performSend(
                  displaySession.id,
                  last.composed,
                  last.imageAttachments,
                  last.composerSnap,
                );
              }}
              onSignIn={() => void handleAgentSignIn(currentAgent.id)}
              onNewSession={() => {
                setComposerFailure(null);
                void createSessionForProject(displaySession.projectId, displaySession.agentId);
              }}
              onDismiss={() => setComposerFailure(null)}
            />
          )}
          {/* Mid-turn follow-ups wait here until cancel frees the prompt slot.
              Never a timeline card — that would split the running reply. */}
          {(() => {
            const queuedSends = pendingSendsRef.current.get(displaySession.id) ?? [];
            if (queuedSends.length === 0) return null;
            const preview = queuedSends
              .map((q) => (q.composed.length > 36 ? `${q.composed.slice(0, 36)}…` : q.composed))
              .join("、");
            return (
              <div className="queued-strip" title="插话 — 打断当前回合后立即发送">
                <span className="queued-strip__badge">插话 ×{queuedSends.length}</span>
                <span className="queued-strip__text">{preview}</span>
                <button
                  type="button"
                  className="queued-strip__dismiss"
                  title="取消插话"
                  onClick={() => {
                    pendingSendsRef.current.delete(displaySession.id);
                    setQueuedStripTick((t) => t + 1);
                  }}
                >
                  取消
                </button>
              </div>
            );
          })()}
          </div>
          {/* Ask fully occupies the composer slot so all options can show. */}
          {!(askPrompt && askPrompt.sessionId === displaySession.id) && (
          <Composer
            // Remount when dialog identity changes so model/mode state cannot leak.
            key={`${displaySession.id}:${displaySession.agentId}`}
            agent={currentAgent}
            agents={availableAgents}
            currentAgentId={displaySession.agentId}
            sessionId={displaySession.id}
            sessionStatus={displaySession.status}
            lastActivityAt={lastActivityById[displaySession.id] ?? null}
            onProviderKeysChanged={handleProviderKeysChanged}
            onAgentBinaryUpdated={handleAgentBinaryUpdated}
            onAgentsReload={async () => {
              try {
                const next = await listAgents();
                if (next.length > 0) setAvailableAgents(next);
              } catch {
                /* ignore */
              }
            }}
            capabilities={sessionCapabilities}
            prefillText={composerPrefill?.text ?? null}
            prefillToken={composerPrefill?.token ?? 0}
            sessionPrefs={{
              preferredModel: displaySession.preferredModel,
              preferredMode: displaySession.preferredMode,
              preferredEffort: displaySession.preferredEffort,
              preferredEffortId: displaySession.preferredEffortId,
              preferredAlwaysApprove: displaySession.preferredAlwaysApprove,
            }}
            onSessionPrefsChange={(patch: SessionComposerPrefs) => {
              const sid = displaySession.id;
              if (!sid || sid.startsWith("session-empty-")) return;
              // Merge patch onto existing session so a model-only write cannot null mode/effort.
              let merged: SessionComposerPrefs | null = null;
              setAvailableSessions((current) =>
                current.map((s) => {
                  if (s.id !== sid) return s;
                  const next = {
                    ...s,
                    preferredModel:
                      patch.preferredModel !== undefined ? patch.preferredModel : s.preferredModel,
                    preferredMode:
                      patch.preferredMode !== undefined ? patch.preferredMode : s.preferredMode,
                    preferredEffort:
                      patch.preferredEffort !== undefined ? patch.preferredEffort : s.preferredEffort,
                    preferredEffortId:
                      patch.preferredEffortId !== undefined
                        ? patch.preferredEffortId
                        : s.preferredEffortId,
                    preferredAlwaysApprove:
                      patch.preferredAlwaysApprove !== undefined
                        ? patch.preferredAlwaysApprove
                        : s.preferredAlwaysApprove,
                  };
                  merged = {
                    preferredModel: next.preferredModel,
                    preferredMode: next.preferredMode,
                    preferredEffort: next.preferredEffort,
                    preferredEffortId: next.preferredEffortId,
                    preferredAlwaysApprove: next.preferredAlwaysApprove,
                  };
                  return next;
                })
              );
              if (!merged) return;
              void updateSessionPrefs(sid, merged).catch((error) => {
                pushDebug({
                  sessionId: sid,
                  level: "warn",
                  source: "session",
                  summary: "persist composer prefs failed",
                  detail: error instanceof Error ? error.message : String(error),
                });
              });
            }}
            onAgentChange={(id) => void handleAgentChange(id)}
            onInterrupt={() => void handleInterrupt()}
            onSend={(text, droppedPaths, imageAttachments, opts) =>
              void handleSend(text, droppedPaths, imageAttachments, opts)
            }
            onActiveModelChange={setActiveModelId}
            sessionEvents={currentEvents}
            availableCommands={slashCommandsById[displaySession.id] ?? null}
            onWarmAgent={warmActiveAcp}
            onEnsureAgentReady={async () => {
              if (!currentSessionId) return false;
              if (currentAgent.transport !== "acp") return true;
              try {
                await ensureAcpReady(currentSessionId);
                return true;
              } catch {
                return false;
              }
            }}
          />
          )}
          </div>
        </section>

        <ContextPanel
          collapsed={rightCollapsed}
          onCollapse={() => setRightCollapsed(true)}
          onExpand={() => setRightCollapsed(false)}
          usage={usage}
          onUsageRefresh={handleUsageRefreshForce}
          sessionStats={sessionStats}
          changedFiles={changedFiles}
          changedFilesNote={changedFilesNote}
          onRefreshChangedFiles={() => void refreshChangedFiles()}
          gitBranch={gitBranch}
          onCommitAndPush={handleCommitAndPush}
          onOpenDiff={(path) => void handleOpenDiff(path)}
          handoff={lastHandoff}
          projectContext={projectContext}
          projectContextScanning={projectContextScanning}
          onRescanProjectContext={() => void refreshProjectContext(currentProjectId)}
          onReconnectAgent={() => void handleReconnectAgent()}
          reconnecting={reconnecting}
          onToggleProjectContext={(kind, id, enabled) =>
            void handleToggleProjectContext(kind, id, enabled)
          }
          activeAgentId={currentAgent.id}
          activeAgentLabel={currentAgent.label}
          planEntries={currentSessionId ? planBySessionId[currentSessionId] : undefined}
          planModeActive={
            (sessionCapabilities?.currentMode ?? "").toLowerCase() === "plan"
          }
          todoItems={todoItems}
          onTodosChange={handleTodosChange}
          onAbsorbPlan={handleAbsorbPlan}
          onSendTodosToAi={() => prefillComposer(formatTodosForPrompt(todoItems))}
          onRequestAiTodoUpdate={() => prefillComposer(formatAiUpdatePrompt(todoItems))}
          onPrepareAiTodoMerge={handlePrepareAiTodoMerge}
          onCheckAppUpdate={handleCheckAppUpdate}
          checkAppUpdateBusy={appUpdateBusy}
          appUpdateAvailable={Boolean(appUpdate?.updateAvailable)}
          resizeDragging={resizingSide === "right"}
          onResizeStart={() => setResizingSide("right")}
        />
      </div>
      {permissionPrompt &&
        currentSessionId &&
        permissionPrompt.sessionId === currentSessionId && (
        <PermissionDialog
          prompt={permissionPrompt}
          busy={permissionBusy}
          onChoose={(optionId) => void handlePermissionChoose(optionId)}
        />
      )}
      {appUpdate && (
        <div className="update-banner" role="status">
          <span>
            {appUpdate.updateAvailable
              ? `新版本 ${appUpdate.latestVersion}${
                  appUpdate.currentVersion ? `（当前 ${appUpdate.currentVersion}）` : ""
                }`
              : appUpdate.note ||
                `已是最新版本 ${appUpdate.currentVersion || ""}`.trim()}
          </span>
          <div className="update-banner__actions">
            {appUpdate.updateAvailable && appUpdate.releaseUrl && (
              <a
                href={appUpdate.releaseUrl}
                target="_blank"
                rel="noreferrer"
                onClick={(event) => {
                  event.preventDefault();
                  void openExternal(appUpdate.releaseUrl!);
                }}
              >
                说明
              </a>
            )}
            {appUpdate.updateAvailable && (
              <button
                type="button"
                className="update-banner__btn"
                disabled={appUpdateBusy}
                onClick={() => {
                  void (async () => {
                    setAppUpdateBusy(true);
                    try {
                      await downloadAppUpdate();
                      await applyAppUpdateAndRelaunch();
                    } catch (error) {
                      pushDebug({
                        sessionId: currentSessionId ?? "",
                        level: "error",
                        source: "update",
                        summary: "app update failed",
                        detail: error instanceof Error ? error.message : String(error),
                      });
                      setAppUpdateBusy(false);
                    }
                  })();
                }}
              >
                {appUpdateBusy ? "下载中…" : "下载并重启"}
              </button>
            )}
            <button
              type="button"
              className="update-banner__btn update-banner__btn--ghost"
              disabled={appUpdateBusy}
              onClick={() => setAppUpdate(null)}
            >
              {appUpdate.updateAvailable ? "稍后" : "关闭"}
            </button>
          </div>
        </div>
      )}
      {pathGrantPrompt && (
        <div className="project-dialog-backdrop" role="presentation">
          <div className="project-dialog" role="dialog" aria-modal="true" aria-labelledby="path-grant-title">
            <div className="project-dialog__header">
              <div>
                <strong id="path-grant-title">项目外路径</strong>
                <span>需要先授权，agent 才能访问这些文件夹</span>
              </div>
            </div>
            <ul className="path-grant__list">
              {pathGrantPrompt.paths.map((item) => (
                <li key={item.dir} title={item.path}>
                  <code>{item.dir}</code>
                  {!item.isDirectory && <span className="path-grant__from">from {item.path.split(/[\\/]/).pop()}</span>}
                </li>
              ))}
            </ul>
            <p className="path-grant__hint">
              访问范围在会话启动时就定死了；子 agent 的系统权限弹窗
              <strong>不会传回 Marionette</strong>，不授权直接发只会卡住。
              点「授权并发送」会把文件夹记入本项目，并在需要时重连 agent。
            </p>
            <div className="project-dialog__actions">
              <button
                type="button"
                className="project-dialog__cancel"
                disabled={pathGrantBusy}
                onClick={cancelPathGrant}
              >
                取消发送
              </button>
              <button
                type="button"
                className="project-dialog__submit"
                disabled={pathGrantBusy}
                onClick={() => void resolvePathGrant()}
              >
                {pathGrantBusy ? "授权中…" : "授权并发送"}
              </button>
            </div>
          </div>
        </div>
      )}
      {diffPreview && (
        <div
          className="project-dialog-backdrop"
          role="presentation"
          onMouseDown={(event) => {
            if (event.target === event.currentTarget) setDiffPreview(null);
          }}
        >
          <div className="diff-dialog" role="dialog" aria-modal="true" aria-labelledby="diff-dialog-title">
            <div className="project-dialog__header">
              <div>
                <strong id="diff-dialog-title">Diff</strong>
                <span>{diffPreview.path}</span>
              </div>
              <button className="project-dialog__close" type="button" title="Close" aria-label="Close" onClick={() => setDiffPreview(null)}>
                <X size={14} />
              </button>
            </div>
            <UnifiedDiffView className="diff-dialog__body" text={diffPreview.text} />
          </div>
        </div>
      )}
      {projectDialogOpen && (
        <div className="project-dialog-backdrop" role="presentation" onMouseDown={(event) => { if (event.target === event.currentTarget && !projectAdding) setProjectDialogOpen(false); }}>
          <form className="project-dialog" role="dialog" aria-modal="true" aria-labelledby="project-dialog-title" onSubmit={(event) => { event.preventDefault(); void handleAddProject(); }}>
            <div className="project-dialog__header">
              <div>
                <strong id="project-dialog-title">Add project</strong>
                <span>Open a local folder as a project</span>
              </div>
              <button className="project-dialog__close" type="button" title="Close" aria-label="Close" onClick={() => setProjectDialogOpen(false)} disabled={projectAdding}><X size={14} /></button>
            </div>
            <label className="project-dialog__field">
              <span>Folder path</span>
              <div className="project-dialog__path-row">
                <input
                  autoFocus
                  value={projectPath}
                  onChange={(event) => setProjectPath(event.target.value)}
                  placeholder="D:\\Work\\MyProject"
                  spellCheck={false}
                />
                <button
                  type="button"
                  className="project-dialog__browse"
                  title="Browse…"
                  aria-label="Browse for folder"
                  disabled={projectAdding}
                  onClick={() => {
                    void (async () => {
                      const picked = await pickFolder();
                      if (picked) {
                        setProjectPath(picked);
                        setProjectError("");
                      }
                    })();
                  }}
                >
                  <FolderOpen size={14} />
                  <span>Browse</span>
                </button>
              </div>
            </label>
            {projectError && <p className="project-dialog__error">{projectError}</p>}
            <div className="project-dialog__actions">
              <button type="button" className="project-dialog__cancel" onClick={() => setProjectDialogOpen(false)} disabled={projectAdding}>Cancel</button>
              <button type="submit" className="project-dialog__submit" disabled={projectAdding}>{projectAdding ? "Adding..." : "Add project"}</button>
            </div>
          </form>
        </div>
      )}
    </main>
  );
}
