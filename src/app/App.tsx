import { emit, listen } from "@tauri-apps/api/event";
import { ChevronLeft, ChevronRight, FolderOpen, Inbox, X } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState, memo, type CSSProperties } from "react";
import { agents, projects, sessions } from "../lib/mockData";
import {
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
import { useSessionRails } from "./useSessionRails";
import {
  REENTRY_IDLE_MS,
  sealAgentSegmentForIdle,
  shouldArmAgentReentry,
} from "../lib/agentReentry";
import {
  chunkKindFromRole,
  flushShadowsIntoPaintSet,
  paintSet,
  shouldPublishChunk,
  unfinishedDelegateChildrenFromMeta,
} from "../lib/acpRenderBudget";
import { engineQuota as fetchEngineQuota, type EngineQuotaEntry } from "../lib/api";
import { TELEGRAM_COMMAND_EVENT, telegramRemoteResult, type TelegramCommand } from "../lib/api";
import { invalidateBridgeModels } from "../lib/bridge";
import { useLatestCallback } from "../lib/useLatestCallback";
import { addProject, appendDebugLog, applyAppUpdateAndRelaunch, beginWorkspaceSnapshot, cancelAcpSession, checkAppUpdate, checkOutsideProjectPaths, createChildSession, createSession as createSessionApi, createChatSession, deleteProject as deleteProjectApi, deleteSession as deleteSessionApi, discardWorkspaceSnapshot, downloadAppUpdate, endWorkspaceSnapshot, getDefaultFolder, generateHandoff, getProxyConfig, getChangedFiles, getCurrentBranch, getFileDiff, getSessionCapabilities, grantWorkspaceRoot, isTauriRuntime, killAcpTerminal, listAcpTerminals, listAgentCommands, listAgents, listProjects, listSessions, listTodos, loadTranscript, listChatSessions, OPEN_PATH_EVENT, openHere, pickFolder, probeAcpBilling, probeAgentAuth, probeProviderUsage, projectContextPrompt, quitApp, refreshExternalSessions, reorderProjects as reorderProjectsApi, respondAcpPermission, respondAcpPlanApproval, respondAcpQuestion, revealInFileManager, openExternal, saveTodos, scanProjectContext, sendAcpPrompt, setProjectContextEnabled, setProxyConfig, setSessionPinned, startAcpSession, startAgentLogin, TELEGRAM_INBOUND_EVENT, telegramAck, type TelegramInbound, startGrokUpdate, stopAcpSession, takeLaunchOpenPath, testProxy, updateAcpSession, updateSessionAgent, updateSessionLabel, updateSessionPrefs, updateSessionStatus, writeTranscript, type AppUpdateInfo, type OutsidePath, type PlanApprovalDecision, type TurnSnapshotEnd, type TurnSnapshotStart, CHAT_PROJECT_ID } from "../lib/api";
import {
  bindDetachedWindowReaper,
  DETACHED_HIDDEN_EVENT,
  DETACHED_READY_EVENT,
  focusDetachedWindow,
  hideDetachedWindowForSession,
  listenMergeBackRequests,
  listenMergeHighlights,
  listVisibleDetachedSessionIds,
  openDetachedSessionWindow,
  readDetachedSessionId,
  readMainDropClient,
  requestMergeBack,
  sessionIdFromDetachedLabel,
  setDetachedSessionOwner,
  setStreamInterests,
  shouldHandleAcpSession,
  type DetachedReadyPayload,
} from "../lib/detachedWindow";
import { broadcastSessionPatch, listenSessionPatches } from "../lib/sessionBus";
import {
  coldSessionIdsFromMap,
  collectHotSessionIds,
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
import { enterDock, getDockState, initDockMode, readDockPrefs, subscribeDockPrefs, subscribeDockState, syncBackgroundMode, toggleAutoHideOnClose, undock } from "../lib/dockMode";
import { DockChrome, DockResizeGrip, type DockPage } from "../components/DockChrome";
import { DockDropRouter } from "../components/DockDropRouter";
import { MemoBox } from "../components/MemoBox";
import { memoList, memoPrompt } from "../lib/memos";
import { installPageDragSourceTracker } from "../lib/dragText";
import { splitShelfChatSessions, withoutGhostProjects } from "../lib/sessionGroups";
import { parseAvailableCommandsUpdate } from "../lib/slashCommands";
import type { PlanEntry } from "../lib/acpPlan";
import { parseAcpPlanUpdate } from "../lib/acpPlan";
import { extractPermissionPlanMarkdown } from "../lib/permissionPlan";
import {
  askPromptEvent,
  expirePendingPrompts,
  patchPromptEvent,
  planDecisionLabel,
  planPromptEvent,
  promptCardOnRail,
  sessionHidesComposer,
  upsertPromptEvent,
} from "../lib/interactionEvents";
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
  isInjectedPromptText,
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
import { SPEC_ACTION_EVENT } from "../lib/specSchema";
import {
  classifyAgentError,
  formatAcpRpcError,
  formatClassifiedError,
  isTurnInFlightError,
} from "../lib/errors";
import { prettyEffortLabel } from "../lib/modelLabel";
import { classifyToolCall } from "../lib/turnActivity";
import { selectTurnWorkspaceChanges } from "../lib/turnDelivery";
import { getLastUsedDefaults } from "../lib/recentModels";
import { pickRestoredSession, saveUiRestore, saveQueuedSends, loadQueuedSends, saveClosedTabs, loadClosedTabs, loadDetailsVisible, saveDetailsVisible } from "../lib/uiRestore";
import {
  MAX_PANES,
  activateTab,
  addTabToPane,
  adoptExternalSession,
  closeTab,
  collectLeaves,
  emptyWorkspace,
  findPane,
  focusPane,
  focusSession,
  focusedSessionId,
  hitTestDropZone,
  leafCount,
  migrateFromFlat,
  moveTab,
  openSessionIds as tabIdsFromTree,
  paneOfSession,
  popOutTab,
  resizeSplit,
  serialize,
  splitPane,
  visibleSessionIds as visibleIdsFromTree,
  type DropEdge,
  type DropKind,
  type HitTestInput,
  type SplitWorkspaceState,
} from "../lib/splitWorkspace";
import type { QuotePin } from "../lib/quoteComment";
import { canPark, lastActiveMs, shouldAutoSuspend } from "../lib/sessionSuspend";
import { forkCopyEvents, forkCutIndex } from "../lib/sessionFork";
import { type AskQuestionPrompt } from "../components/AskQuestionCard";
import { buildBackgroundTaskRows } from "../lib/backgroundTasks";
import type { AcpTerminalRow } from "../lib/api";
import { Composer, transferEmptyDraft } from "../components/Composer";
import { ComposerErrorStrip, SessionNoticeStrip } from "../components/ComposerErrorStrip";
import { isAcpTranscriptMethod, parseLiveNotice } from "../lib/acpMeta";
import { ContextPanel } from "../components/ContextPanel";
import { PermissionDialog, type PermissionPrompt } from "../components/PermissionDialog";
import { type PlanApprovalPrompt } from "../components/PlanApprovalCard";
import { UnifiedDiffView } from "../components/UnifiedDiffView";
import { ProjectShelf } from "../components/ProjectShelf";
import { CommandPalette } from "../components/CommandPalette";
import { withViewTransition } from "../lib/viewTransition";
import { ToastStack } from "../components/ToastStack";
import { SessionTabs, SessionView, type ForkAnchor, type UserMessageAnchor } from "../components/SessionView";
// 侧栏/标签栏是流式期间的重渲大户：memo 化 + useLatestCallback 稳定回调，
// eventsTick（20-50Hz）bump 时不再波及整棵侧栏树（审计 2026-10-24 P1-A）。
const MemoProjectShelf = memo(ProjectShelf);
const MemoSessionTabs = memo(SessionTabs);
import {
  SplitWorkspace,
  collectDropGeometry,
  dropKindsEqual,
  ghostHintForDrop,
} from "../components/SplitWorkspace";
import { SplitDropOverlay } from "../components/SplitDropOverlay";
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
  } catch (err) {
    console.warn("[layout] persisted layout read failed — using fallback", err);
    return fallback;
  }
}

type AgentBannerKind = "auth" | "upgrade";

/**
 * Composer chip snapshot taken at send time. The parent must not re-derive
 * mode/model from stale sessionCapabilities — a chip may already show a pick
 * whose `set_config` echo has not landed. Effort rides along so a dialog whose
 * effort was inherited (never explicitly picked here) can persist it.
 */
type ComposerChipSnapshot = {
  /** Explicitly picked/dropped files; preserve through grants and queued sends. */
  filePaths?: string[];
  modeId?: string | null;
  modeLabel?: string | null;
  modelId?: string | null;
  modelLabel?: string | null;
  effortId?: string | null;
  effort?: number | null;
  effortLabel?: string | null;
};

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
 * How long a `session/prompt` refused by the ACP concurrency gate waits for the
 * occupying turn to end before retrying once. Generous on purpose: the turn in
 * flight may be one the UI never saw start (a hidden slash replay), and giving
 * up early is what produced the bogus "Agent 错误: a turn is already in flight".
 */
const TURN_SLOT_WAIT_MS = 120_000;

/** Wake every send parked on `sessionId`'s turn slot (see `waitForTurnSlot`). */
function releaseTurnSlotWaiters(
  waiters: Map<string, Set<() => void>>,
  sessionId: string
) {
  const list = waiters.get(sessionId);
  if (!list?.size) return;
  waiters.delete(sessionId);
  for (const wake of list) wake();
}

/**
 * Resolve once `sessionId` reports a turn boundary (`turn/complete` or process
 * end), or `false` when `timeoutMs` passes first — a turn that never ends must
 * not park the send forever.
 */
function waitForTurnSlot(
  waiters: Map<string, Set<() => void>>,
  sessionId: string,
  timeoutMs: number
): Promise<boolean> {
  return new Promise((resolve) => {
    let settled = false;
    const list = waiters.get(sessionId) ?? new Set<() => void>();
    function finish(freed: boolean) {
      if (settled) return;
      settled = true;
      list.delete(wake);
      if (list.size === 0 && waiters.get(sessionId) === list) {
        waiters.delete(sessionId);
      }
      resolve(freed);
    }
    function wake() {
      finish(true);
    }
    list.add(wake);
    waiters.set(sessionId, list);
    setTimeout(() => finish(false), timeoutMs);
  });
}

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
const EMPTY_EVENTS: SessionEvent[] = [];

/** 后台任务面板：终端快照轮询 3s 一次，浅比较避免无变化时击穿下游 memo。 */
function sameTerminals(prev: AcpTerminalRow[], next: AcpTerminalRow[]): boolean {
  if (prev.length !== next.length) return false;
  for (let i = 0; i < prev.length; i++) {
    const a = prev[i]!;
    const b = next[i]!;
    if (
      a.terminalId !== b.terminalId ||
      a.running !== b.running ||
      a.command !== b.command ||
      a.sessionId !== b.sessionId ||
      a.exitStatus?.exitCode !== b.exitStatus?.exitCode ||
      a.exitStatus?.signal !== b.exitStatus?.signal
    ) {
      return false;
    }
  }
  return true;
}

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

/**
 * One prompt's workspace baseline: a content tree of the whole worktree taken
 * through a private git index (see `begin_workspace_snapshot`). The turn's file
 * delta is that tree vs the worktree at turn end.
 */
type TurnBaseline = {
  projectId: string;
  /** Project root, so tool paths can be compared with git's relative paths. */
  root: string;
  /** Distinguishes this prompt from a later one on the same session id. */
  token: string;
  /** Resolves to null when the project is not a git work tree. */
  snapshot: Promise<TurnSnapshotStart | null>;
};

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
  const [currentSessionId, setCurrentSessionId] = useState("");
  const [externalScanning, setExternalScanning] = useState(false);
  const [openSessionIds, setOpenSessionIds] = useState<string[]>([]);
  const [splitState, setSplitState] = useState<SplitWorkspaceState>(() => emptyWorkspace());
  const splitStateRef = useRef(splitState);
  splitStateRef.current = splitState;
  const [tabDropPreview, setTabDropPreview] = useState<DropKind | null>(null);
  const tabDropPreviewRef = useRef<DropKind | null>(null);
  const dropPanesRef = useRef<HitTestInput["panes"]>([]);
  const [dropPanes, setDropPanes] = useState<HitTestInput["panes"]>([]);
  const [capsById, setCapsById] = useState<Record<string, CapabilitySnapshot | null>>({});
  const capsByIdRef = useRef(capsById);
  capsByIdRef.current = capsById;
  const [detailsVisible, setDetailsVisible] = useState(() => loadDetailsVisible());
  /** Project opened via Explorer "在此处打开" / --open-path — pinned at shelf top. */
  const [openHereProjectId, setOpenHereProjectId] = useState<string | null>(null);
  /** Default folder for the Chat section (from --open-path or CWD). */
  const [defaultFolderPath, setDefaultFolderPath] = useState<string>("");
  /** Chat rows created before the initial async storage restore completed. */
  const startupCreatedChatsRef = useRef<Session[]>([]);
  /** Dedup Chat creates from empty-composer click / send / agent chip. */
  const createChatInFlightRef = useRef<Promise<Session | undefined> | null>(null);
  /** Focus the composer after creating a Chat from the empty workspace. */
  const [composerAutoFocus, setComposerAutoFocus] = useState(false);
  /** Manual rename wins over any later title emitted by an agent. */
  const manuallyRenamedSessionIdsRef = useRef<Set<string>>(new Set());
  // View-mode state is write-only now: SessionView dropped the prop, and every
  // call site only ever resets to "clean" (kept for the reset semantics).
  const [, setViewMode] = useState<SessionViewMode>("clean");
  const [leftCollapsed, setLeftCollapsed] = useState(IS_DETACHED_WINDOW);
  /** Ctrl+K palette: jump to any dialog / project, or run a shell action. */
  const [paletteOpen, setPaletteOpen] = useState(false);
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
  const [webReplyWidthPercent, setWebReplyWidthPercent] = useState(() => {
    const savedPercent = Number(window.localStorage.getItem("marionette-web-reply-width-percent"));
    if (Number.isFinite(savedPercent) && savedPercent >= 25 && savedPercent <= 100) return savedPercent;
    const legacyPx = Number(window.localStorage.getItem("marionette-web-reply-width"));
    return Number.isFinite(legacyPx) && legacyPx >= 320 && legacyPx <= 980
      ? Math.round((legacyPx / 980) * 100)
      : 85;
  });
  /** Taskbar flash + chime when AI replies / may be stuck. Completion also chimes while focused. */
  const [desktopNotifyOn, setDesktopNotifyOn] = useState(() => isDesktopNotifyEnabled());
  /** Edge dock stage 0: close = retract to background instead of quitting. */
  const [autoHideOnClose, setAutoHideOnClose] = useState(() => readDockPrefs().autoHideOnClose);
  /** Edge dock stage 1: main window is parked at a screen edge. */
  const [dockActive, setDockActive] = useState(false);
  const [dockExpanded, setDockExpanded] = useState(false);
  /** Edge dock: the page covering the strip below its title bar (all dialogs / usage / memo box). */
  const [dockPage, setDockPage] = useState<DockPage | null>(null);
  /** Edge dock stage 3: memo inbox overlay + its count badge. */
  const [memoCount, setMemoCount] = useState(0);
  /** Floating-window 收纳盒 page (docked mode uses dockPage === "memo"). */
  const [memoOpen, setMemoOpen] = useState(false);
  /**
   * 收纳盒 lives in the tab strip beside conversations: `memoOpen` = the tab
   * exists, `memoActive` = it is the selected page (switching to a
   * conversation tab keeps the tab, like any other open dialog).
   */
  const [memoActive, setMemoActive] = useState(false);
  /** Sessions that finished a turn while this window was looking at another dialog. */
  const [doneUnreadIds, setDoneUnreadIds] = useState<Set<string>>(() => new Set());
  /** Agent proxy config (single exit address), loaded on mount. */
  const [proxyConfig, setProxyConfigState] = useState<ProxyConfig | null>(null);
  /** Main window only: a detached tab drag is hovering the tab strip. */
  const [mergeTargetActive, setMergeTargetActive] = useState(false);
  // TEMP-VERIFY: fake caps so the Composer model menu renders in the plain
  // browser harness. Revert before commit.
  const [sessionCapabilities, setSessionCapabilities] = useState<CapabilitySnapshot | null>(() => {
    if (isTauriRuntime()) return null;
    const providers = ["DeepSeek", "Local", "OpenCode", "xAI", "Z.AI", "ChatGPT", "GLM"];
    const models = [] as import("../lib/types").ModelDef[];
    for (const p of providers) {
      for (let i = 1; i <= 6; i++) models.push({ id: `${p.toLowerCase()}-m${i}`, label: `${p} Model ${i}` });
    }
    return {
      modes: [],
      models,
      thinkingEffort: null,
      effortOptions: [
        { id: "low", label: "Low" },
        { id: "high", label: "High" },
        { id: "max", label: "Max" },
      ],
      supportsCancel: true,
      currentMode: null,
      currentModel: "deepseek-m1",
      currentEffort: null,
      currentEffortId: "high",
      modelConfigId: "model",
      modeConfigId: null,
      effortConfigId: "effort",
    } as unknown as CapabilitySnapshot;
  });
  const [eventsTick, setEventsTick] = useState<Record<string, number>>({});
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
  const activeModelIdRef = useRef(activeModelId);
  activeModelIdRef.current = activeModelId;
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
   * Re-entry segments（再次进入对话）: the agent resumed on its own after
   * `turn/complete` (a background command came back). Keyed by session;
   * the timer ends the segment on quiet because several adapters never send
   * a boundary for it (see lib/agentReentry.ts).
   */
  const agentReentryRef = useRef<
    Record<string, { startedAt: number; lastAt: number; timer: ReturnType<typeof setTimeout> }>
  >({});
  /** Drop a re-entry segment's bookkeeping (timer included) — turn boundary, send, or teardown. */
  const disarmAgentReentry = useCallback((sessionId: string) => {
    const armed = agentReentryRef.current[sessionId];
    if (!armed) return;
    if (armed.timer) clearTimeout(armed.timer);
    delete agentReentryRef.current[sessionId];
  }, []);
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
        composerSnap?: ComposerChipSnapshot;
        /** true = Telegram 入站（无 Composer 快照）：flush 时继续走会话自身偏好，不读主窗口 Composer 态。 */
        remote?: boolean;
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
        composerSnap?: ComposerChipSnapshot;
      }
    >
  >(new Map());
  const [composerFailure, setComposerFailure] = useState<{
    sessionId: string;
    error: import("../lib/errors").ClassifiedError;
  } | null>(null);
  /** Live advisory (retry / crash / protocol notice). Not written into chat. */
  const [sessionNotices, setSessionNotices] = useState<
    Record<string, import("../components/ComposerErrorStrip").SessionNoticeView>
  >({});
  const flushingSendRef = useRef<Set<string>>(new Set());
  /**
   * Sends parked on a session's single ACP turn slot — see
   * `sendPromptWithTurnWait`. Woken by the acp-event listener on turn boundary.
   */
  const turnSlotWaitersRef = useRef<Map<string, Set<() => void>>>(new Map());
  /**
   * 2026-10-18 B/C 实例「发消息石沉大海」:prompt 已写入、agent 一字不回,
   * 既无错误也无状态变化,用户只能重启。发送后 60 秒内该会话没有任何
   * acp-event 到达,就在 composer 上方放一条可见告警(不自动取消,停止
   * 按钮仍可用)。任何事件到达即解除。
   */
  const sendSilenceTimersRef = useRef<Map<string, number>>(new Map());
  const clearSendSilenceWatchdog = (sessionId: string) => {
    const timer = sendSilenceTimersRef.current.get(sessionId);
    if (timer !== undefined) {
      clearTimeout(timer);
      sendSilenceTimersRef.current.delete(sessionId);
    }
  };
  const armSendSilenceWatchdog = (sessionId: string) => {
    clearSendSilenceWatchdog(sessionId);
    const timer = window.setTimeout(() => {
      sendSilenceTimersRef.current.delete(sessionId);
      pushDebug({
        sessionId,
        level: "warn",
        source: "composer",
        summary: "prompt sent but no acp events for 60s",
      });
      setSessionNotices((current) => ({
        ...current,
        [sessionId]: {
          sessionId,
          severity: "warning",
          title: "Agent 已 60 秒无任何输出",
          description:
            "消息已发出,但没有收到任何回传事件。可先点停止再重发;若反复出现,重启该会话或 agent。",
        },
      }));
    }, 60_000);
    sendSilenceTimersRef.current.set(sessionId, timer);
  };
  /**
   * Sessions whose live turn is a silent prompt (`sendSilentPrompt`). Consumed
   * on turn/complete so that turn's invisible end stays quiet.
   */
  const silentTurnRef = useRef<Set<string>>(new Set());
  /**
   * Sessions whose live turn is a usage probe (`/status` / `/usage` sent by the
   * usage panel refresh). Consumed on turn/complete: no chime / desktop notice,
   * and no after-turn usage re-probe (the probe itself just delivered fresh
   * data — re-probing made every refresh click fire twice).
   */
  const probeTurnRef = useRef<Set<string>>(new Set());
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
  /** agentId → sticky banner. Probe-driven (logged out) or error-driven (auth / outdated CLI). */
  const [agentBanner, setAgentBanner] = useState<
    Record<string, { kind: AgentBannerKind; message: string }>
  >({});
  const [signInBusy, setSignInBusy] = useState(false);
  const [updateBusy, setUpdateBusy] = useState(false);
  const authPollRef = useRef<ReturnType<typeof setInterval> | null>(null);
  /**
   * Stable per-agent setter so the ACP listener can raise/clear banners.
   * Clearing `auth` leaves an `upgrade` notice in place — a logged-in probe
   * must not wipe “your CLI is too old”.
   */
  const setAuthHintFor = useCallback(
    (agentId: string, hint: string | null, kind: AgentBannerKind = "auth") => {
      setAgentBanner((current) => {
        const existing = current[agentId];
        if (hint === null) {
          if (!existing || existing.kind !== kind) return current;
          const next = { ...current };
          delete next[agentId];
          return next;
        }
        if (existing?.kind === kind && existing.message === hint) return current;
        return { ...current, [agentId]: { kind, message: hint } };
      });
    },
    []
  );
  const clearAgentBanner = useCallback((agentId: string) => {
    setAgentBanner((current) => {
      if (!current[agentId]) return current;
      const next = { ...current };
      delete next[agentId];
      return next;
    });
  }, []);
  const raiseClassifiedBanner = useCallback(
    (agentId: string | undefined, classified: { kind: string; actionHint?: string }) => {
      if (!agentId) return;
      if (classified.kind === "auth") {
        const spec = agentAuthSpec(agentId);
        if (!spec) return;
        setAuthHintFor(
          agentId,
          spec.login
            ? `需要登录 — 点 Sign in，或终端执行 \`${spec.loginCommand}\``
            : `需要登录 — 终端执行 \`${spec.loginCommand}\``
        );
        return;
      }
      if (classified.kind === "upgrade") {
        const grok = agentId === "grok-build" || agentId === "grok";
        setAuthHintFor(
          agentId,
          grok
            ? "版本过低 — 点「更新」运行 `grok update`，完成后新建会话。"
            : classified.actionHint || "版本过低 — 按提示更新 CLI 后新建会话。",
          "upgrade"
        );
      }
    },
    [setAuthHintFor]
  );
  const [composerPrefill, setComposerPrefill] = useState<{
    sessionId?: string;
    text: string;
    token: number;
  } | null>(null);
  /** Inline Clean quote-comments (numbered pins) per dialog. */
  const [quotePinsById, setQuotePinsById] = useState<Record<string, QuotePin[]>>({});
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
    composerSnap?: ComposerChipSnapshot;
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
  const openSessionRef = useRef<(s: Session) => void>(() => undefined);
  const loadSessionTranscriptRef = useRef<(id: string) => Promise<void>>(async () => undefined);
  const pushDebugRef = useRef<(entry: {
    sessionId?: string;
    level?: "info" | "warn" | "error";
    source: string;
    summary: string;
    detail?: string;
  }) => void>(() => undefined);
  sessionsRef.current = availableSessions;
  agentsRef.current = availableAgents;
  projectsRef.current = availableProjects;
  currentSessionIdRef.current = currentSessionId;
  currentProjectIdRef.current = currentProjectId;
  openSessionIdsRef.current = openSessionIds;

  const workspaceSplit = !IS_DETACHED_WINDOW && !dockActive && leafCount(splitState) > 1;
  /** Split panes would be unreadable at strip width — paused while docked. */
  const dockSplitPaused = dockActive && leafCount(splitState) > 1;

  const commitSplit = (next: SplitWorkspaceState) => {
    splitStateRef.current = next;
    setSplitState(next);
    const ids = tabIdsFromTree(next);
    setOpenSessionIds(ids);
    openSessionIdsRef.current = ids;
    const focused = focusedSessionId(next);
    if (focused && focused !== currentSessionIdRef.current) {
      currentSessionIdRef.current = focused;
      setCurrentSessionId(focused);
      const sess = sessionsRef.current.find((s) => s.id === focused);
      if (sess) setCurrentProjectId(sess.projectId);
      setSessionCapabilities(capsByIdRef.current[focused] ?? null);
    }
  };

  /** Permission / ask / plan steal pane focus; never wait for that to paint the dialog. */
  const stealFocusForPrompt = (sessionId: string) => {
    if (!sessionId || IS_DETACHED_WINDOW) return;
    if (detachedOwnedIdsRef.current.has(sessionId)) return;
    const sess = sessionsRef.current.find((s) => s.id === sessionId);
    if (!sess) return;
    openSessionRef.current(sess);
  };

  const hitTestTabDrop = (session: Session, x: number, y: number): DropKind => {
    const tree =
      leafCount(splitStateRef.current) > 1
        ? splitStateRef.current
        : migrateFromFlat(openSessionIdsRef.current, currentSessionIdRef.current);
    const leaves = collectLeaves(tree.root);
    const geometry: HitTestInput["geometry"] =
      leafCount(tree) > 1 ? "per-pane" : "titlebar-plus-center";
    const panes = collectDropGeometry(geometry, leaves);
    dropPanesRef.current = panes;
    const sourcePane = paneOfSession(tree, session.id);
    return hitTestDropZone({
      x,
      y,
      windowW: window.innerWidth,
      windowH: window.innerHeight,
      geometry,
      panes,
      root: tree.root,
      source: {
        sessionId: session.id,
        paneId: sourcePane?.id ?? null,
        tabCountInSource: sourcePane?.tabIds.length ?? 1,
      },
      leafCount: leafCount(tree),
      allowHorizontal: true,
      maxLeaves: MAX_PANES,
    });
  };

  // Opening a dialog means the user has seen it — drop the "just finished" mark.
  useEffect(() => {
    if (!currentSessionId) return;
    setDoneUnreadIds((current) => {
      if (!current.has(currentSessionId)) return current;
      const next = new Set(current);
      next.delete(currentSessionId);
      return next;
    });
  }, [currentSessionId]);

  // Edge dock stage 0: mirror the persisted autoHideOnClose into the Rust
  // close-path branch at launch (the flag resets every process start), and
  // keep the toggle in sync with pref writes from any window.
  useEffect(() => {
    void syncBackgroundMode();
    return subscribeDockPrefs((prefs) => setAutoHideOnClose(prefs.autoHideOnClose));
  }, []);

  // Edge dock stage 3: memo count for the strip's badge (fetch once; the
  // overlay publishes the live count while open).
  useEffect(() => {
    void memoList().then((rows) => setMemoCount(rows.length));
  }, []);

  // Text-drag channel (vendor wry patch): mark drags that start inside this
  // page so the OLE-side listeners leave them on the pure HTML5 path.
  useEffect(() => installPageDragSourceTracker(), []);

  // Edge dock stage 1: dock engine bindings (close handoff, sliver hover,
  // focus tracking) + footer button state. Stage 2: overlays only live while
  // the strip is expanded — closing them on retract keeps the hidden state
  // free of stale drawers.
  useEffect(() => {
    const disposeEngine = initDockMode();
    const unsubState = subscribeDockState((state) => {
      setDockActive(state !== "floating");
      setDockExpanded(state === "docked-expanded");
      // A floating page cannot stay open across the mode switch.
      setMemoOpen(false);
      setMemoActive(false);
      if (state !== "docked-expanded") setDockPage(null);
    });
    return () => {
      disposeEngine();
      unsubState();
    };
  }, []);

  // Edge dock: Esc is 返回 (the memo box handles its own Esc: select mode
  // first), and picking a dialog from the full list goes back to it.
  useEffect(() => {
    if (!dockPage || dockPage === "memo") return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || event.defaultPrevented) return;
      setDockPage(null);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [dockPage]);

  useEffect(() => {
    setDockPage((page) => (page === "shelf" ? null : page));
  }, [currentSessionId]);

  // Drop marks for sessions that no longer exist.
  useEffect(() => {
    setDoneUnreadIds((current) => {
      if (current.size === 0) return current;
      const live = new Set(availableSessions.map((session) => session.id));
      let changed = false;
      const next = new Set<string>();
      for (const id of current) {
        if (live.has(id)) next.add(id);
        else changed = true;
      }
      return changed ? next : current;
    });
  }, [availableSessions]);

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

  const currentPaintSet = (): Set<string> => {
    const tree = splitStateRef.current;
    const visible =
      !IS_DETACHED_WINDOW && leafCount(tree) > 1
        ? visibleIdsFromTree(tree)
        : currentSessionIdRef.current
          ? [currentSessionIdRef.current]
          : [];
    return paintSet({
      visibleSessionIds: visible,
      unfinishedDelegateChildren: unfinishedDelegateChildrenFromMeta(delegateMetaRef.current),
    });
  };

  // ── Stream interest reporting ──────────────────────────────────────────
  // Rust defers+coalesces text chunks for low-interest sessions (hidden tab
  // 1s, visible-unfocused 250ms), so one saturating stream can no longer take
  // the shared main webview down with it. Levels: 0 hidden / 1 visible /
  // 2 focused. Change-detected so it is safe to call from every flush.
  const lastReportedInterestsRef = useRef<Record<string, number> | null>(null);
  const reportStreamInterestsRef = useRef<() => void>(() => undefined);
  const reportStreamInterests = () => {
    if (!isTauriRuntime()) return;
    let next: Record<string, number>;
    if (IS_DETACHED_WINDOW) {
      if (!DETACHED_SESSION_ID) return;
      next = {
        [DETACHED_SESSION_ID]:
          document.visibilityState === "visible" ? 2 : 0,
      };
    } else {
      const paint = currentPaintSet();
      const focused = currentSessionIdRef.current ?? "";
      const docVisible = document.visibilityState === "visible";
      next = {};
      for (const s of sessionsRef.current) {
        if (!s?.id) continue;
        // Detached windows report their own session's level; writing 0 from
        // here would fight the detached SPA's reports.
        if (detachedOwnedIdsRef.current.has(s.id)) continue;
        next[s.id] = s.id === focused && docVisible ? 2 : paint.has(s.id) ? 1 : 0;
      }
      if (focused && !(focused in next) && !detachedOwnedIdsRef.current.has(focused)) {
        next[focused] = docVisible ? 2 : 1;
      }
    }
    // ~20Hz from every flush: compare shallowly against the cached map instead
    // of re-stringifying the whole object every call (audit P1-A).
    const last = lastReportedInterestsRef.current;
    let changed = true;
    if (typeof last === "object" && last !== null) {
      changed = false;
      for (const id in next) {
        if (next[id] !== (last as Record<string, number>)[id]) { changed = true; break; }
      }
      if (!changed) {
        for (const id in last as Record<string, number>) {
          if (!(id in next)) { changed = true; break; }
        }
      }
    }
    if (!changed) return;
    lastReportedInterestsRef.current = next;
    void setStreamInterests(next);
  };
  reportStreamInterestsRef.current = reportStreamInterests;

  const bumpEventsTick = (ids: string[]) => {
    if (ids.length === 0) return;
    setEventsTick((t) => {
      const next = { ...t };
      for (const id of ids) next[id] = (next[id] ?? 0) + 1;
      return next;
    });
  };

  // 每会话事件 rail 库（审计「App.tsx 事件管线」抽出）：rails 本体、落盘
  // 定时、流式合并与视图拼装都在 useSessionRails 里；这里只解构同名符号，
  // 组件其余部分零改动。
  const {
    eventsBySessionRef,
    shadowBySessionRef,
    livePaintSetRef,
    updateSessionEvents,
    dropSessionRails,
    applyStreamPart,
    publishedEventsForView,
  } = useSessionRails({
    transcriptSaveTimers,
    ownsAcpSession,
    reloadingSessionsRef,
    pushDebugRef,
    bumpEventsTick,
    delegateMetaRef,
    turnStartedAtRef,
    sendMetaRef,
  });

  /**
   * One workspace baseline per prompt. The delivery card's git truth is
   * "worktree at turn start vs worktree at turn end" — never `git status` of the
   * whole repo. `begin_workspace_snapshot` (Rust) explains why that baseline is
   * a content tree taken through a private git index instead of per-file diff
   * hashes.
   */
  const turnBaselineRef = useRef<Record<string, TurnBaseline>>({});
  const turnBaselineSeqRef = useRef(0);
  /** sessionId → the baseline token whose delta was already published. */
  const turnBaselinePublishedRef = useRef<Record<string, string>>({});

  /** Forget a baseline (send failed / abandoned turn) without publishing. */
  const dropTurnBaseline = useCallback((sessionId: string) => {
    const baseline = turnBaselineRef.current[sessionId];
    delete turnBaselineRef.current[sessionId];
    delete turnBaselinePublishedRef.current[sessionId];
    if (!baseline) return;
    void baseline.snapshot
      .then((started) => (started ? discardWorkspaceSnapshot(started.snapshotId) : undefined))
      .catch(() => undefined);
  }, []);

  const startTurnBaseline = useCallback((sessionId: string, projectId: string) => {
    if (!isTauriRuntime() || !projectId || projectId === CHAT_PROJECT_ID) return;
    // An abandoned turn (the agent died before its turn ended) leaves a baseline
    // behind; it belongs to that turn, never to this prompt's delta.
    dropTurnBaseline(sessionId);
    turnBaselineSeqRef.current += 1;
    const token = `${Date.now()}-${turnBaselineSeqRef.current}`;
    const root = projectsRef.current.find((project) => project.id === projectId)?.rootPath ?? "";
    // A baseline that fails must fail closed: publishing nothing leaves the card
    // on the turn's own tool calls, whereas treating the failure as "the
    // workspace was clean" published every dirty file as this turn's work.
    const snapshot = beginWorkspaceSnapshot(projectId).catch((error: unknown) => {
      pushDebugRef.current({
        sessionId,
        level: "warn",
        source: "git",
        summary: "turn baseline failed — delivery card falls back to tool calls",
        detail: error instanceof Error ? error.message : String(error),
      });
      return null;
    });
    turnBaselineRef.current[sessionId] = { projectId, root, token, snapshot };
  }, [dropTurnBaseline]);

  /** Changed-Files panel refresh only — the turn delta waits for the turn end. */
  const refreshProjectChangedFiles = useCallback(async (sessionId: string) => {
    if (!isTauriRuntime()) return;
    const projectId = sessionsRef.current.find((session) => session.id === sessionId)?.projectId;
    if (!projectId || projectId === CHAT_PROJECT_ID) return;
    const files = await getChangedFiles(projectId);
    if (currentProjectIdRef.current !== projectId) return;
    setChangedFiles(files);
    setChangedFilesNote(files.length === 0 ? "No local changes (or not a git repo)." : null);
  }, []);

  /**
   * Files this turn's own edit/write tools touched, child sessions included.
   * Only this turn's calls count: a previous turn's edit must not vouch for a
   * file that was already dirty before this turn started.
   */
  const collectTurnTouchedPaths = useCallback((sessionId: string) => {
    const ids = new Set<string>([sessionId]);
    for (const [childId, meta] of delegateMetaRef.current) {
      if (meta.parentId === sessionId) ids.add(childId);
    }
    const paths: string[] = [];
    for (const id of ids) {
      const rail = eventsBySessionRef.current.get(id);
      if (!rail?.length) continue;
      let startIndex = 0;
      for (let i = rail.length - 1; i >= 0; i -= 1) {
        const event = rail[i]!;
        if (event.type === "user_message" || event.type === "handoff_prepared") {
          startIndex = i;
          break;
        }
      }
      for (let i = startIndex; i < rail.length; i += 1) {
        const event = rail[i]!;
        if (event.type !== "tool_call" || !event.path) continue;
        const kind = classifyToolCall(event);
        if (kind === "edit" || kind === "write") paths.push(event.path);
      }
    }
    return paths;
  }, []);

  /**
   * Turn end: diff the worktree against the turn's baseline and publish one
   * `file_change` per file. Files that were already dirty before the turn are
   * only kept when the turn's own tools touched them, so a second conversation
   * (or the user's editor) on the same project cannot land in this card.
   */
  const finalizeTurnFileChanges = useCallback(async (sessionId: string) => {
    const baseline = turnBaselineRef.current[sessionId];
    if (!baseline) return;
    if (turnBaselinePublishedRef.current[sessionId] === baseline.token) return;
    // Claim the baseline synchronously: the queued-next-prompt flush can start a
    // new baseline before this async work finishes, and that one must not be
    // diffed against this turn's start.
    delete turnBaselineRef.current[sessionId];
    const started = await baseline.snapshot.catch(() => null);
    // Not a git work tree, or the baseline itself failed: no git truth at all.
    if (!started) return;
    if (turnBaselinePublishedRef.current[sessionId] === baseline.token) return;
    let delta: TurnSnapshotEnd;
    try {
      delta = await endWorkspaceSnapshot(started.snapshotId);
    } catch (error) {
      pushDebugRef.current({
        sessionId,
        level: "warn",
        source: "git",
        summary: "turn delta failed — no file chips for this turn",
        detail: error instanceof Error ? error.message : String(error),
      });
      return;
    }
    turnBaselinePublishedRef.current[sessionId] = baseline.token;
    const changes = selectTurnWorkspaceChanges(
      delta.files.map((file) => ({
        path: file.path,
        changeType: file.changeType,
        add: file.binary ? null : file.add,
        del: file.binary ? null : file.del,
        binary: file.binary,
      })),
      {
        dirtyAtStart: started.dirtyPaths,
        touched: collectTurnTouchedPaths(sessionId),
        root: baseline.root,
      },
    );
    if (changes.length > 0) {
      const publishedAt = new Date().toISOString();
      updateSessionEvents(sessionId, (current) => {
        let next = [...current];
        for (const change of changes) {
          const index = next.findIndex(
            (event) =>
              event.type === "file_change" &&
              event.sessionId === sessionId &&
              event.path === change.path,
          );
          const previous = index >= 0 ? next[index] : null;
          const prior = previous?.type === "file_change" ? previous : null;
          const published: SessionEvent = {
            type: "file_change",
            sessionId,
            path: change.path,
            changeType: change.changeType,
            createdAt: prior?.createdAt ?? publishedAt,
            revision: (prior?.revision ?? 0) + 1,
            ...(change.add == null ? {} : { add: change.add }),
            ...(change.del == null ? {} : { del: change.del }),
            ...(change.binary ? { binary: true } : {}),
          };
          if (index >= 0) next[index] = published;
          else next = [...next, published];
        }
        return next;
      }, { publish: true });
    }
    await refreshProjectChangedFiles(sessionId);
  }, [collectTurnTouchedPaths, refreshProjectChangedFiles]);

  const finalizeTurnFileChangesRef = useRef(finalizeTurnFileChanges);
  finalizeTurnFileChangesRef.current = finalizeTurnFileChanges;
  const refreshProjectChangedFilesRef = useRef(refreshProjectChangedFiles);
  refreshProjectChangedFilesRef.current = refreshProjectChangedFiles;

  // State-publish cadence for lastActivityById. The ref above stays exact on
  // every event; the state copy re-renders the App root, so a 60–80 events/sec
  // stream (2026-10-16 freeze) must not publish it per event.
  const lastActivityStateAtRef = useRef<Record<string, number>>({});
  const touchActivity = useCallback((sessionId: string) => {
    if (!sessionId) return;
    const now = Date.now();
    lastActivityByIdRef.current = { ...lastActivityByIdRef.current, [sessionId]: now };
    if (now - (lastActivityStateAtRef.current[sessionId] ?? 0) < 1000) return;
    lastActivityStateAtRef.current[sessionId] = now;
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
  pushDebugRef.current = pushDebug;

  /** Apply an ACP-provided conversation title without overriding a manual rename. */
  const applyAgentSessionTitle = useCallback((sessionId: string, rawTitle: string) => {
    // An agent titles the session from its first prompt. After a reconnect that
    // prompt is Marionette's own history/skills preamble — refuse it, or every
    // resumed dialog ends up called "[Marionette — prior conversation…".
    if (isInjectedPromptText(rawTitle)) return;
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

  /** Engine subscription quotas (`magpie quota`) merged into the Usage panel. */
  const [engineQuotaRows, setEngineQuotaRows] = useState<EngineQuotaEntry[]>([]);
  const refreshEngineQuota = useCallback((force = false) => {
    const apply = (rows: EngineQuotaEntry[] | null | undefined, replaceEmpty: boolean) => {
      if (rows && rows.length > 0) setEngineQuotaRows(rows);
      else if (replaceEmpty) setEngineQuotaRows([]);
    };
    // kept=1 几乎立刻从网关缓存刷出 Grok / Codex / Go 等行；点刷新再跟一发现场查询。
    void fetchEngineQuota(false)
      .then((rows) => apply(rows, !force))
      .catch(() => undefined)
      .then(() => {
        if (!force) return;
        return fetchEngineQuota(true).then((rows) => apply(rows, true));
      })
      .catch(() => undefined);
  }, []);
  useEffect(() => {
    refreshEngineQuota();
  }, [refreshEngineQuota]);

  /**
   * Engine state change → drop every cached cross-use catalog. The bridge
   * cache is otherwise forever, so a menu queried while the engine slept
   * (before startup / after idle reclaim) would keep serving the stale
   * engine-less result even after the user woke the gateway (2026-10-17).
   * Reaching running also pulls subscription quotas into Usage (the first
   * fetch on mount often races the engine still starting).
   */
  useEffect(() => {
    if (!isTauriRuntime()) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen<{ state?: string }>("engine-state-changed", (event) => {
      invalidateBridgeModels();
      const state = event.payload?.state;
      if (state === "running") refreshEngineQuota();
      else if (state === "absent" || state === "failed") setEngineQuotaRows([]);
    }).then((fn) => {
      if (disposed) fn();
      else unlisten = fn;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [refreshEngineQuota]);

  const usage = useMemo<UsageSnapshot>(() => {
    const activeSession = availableSessions.find((session) => session.id === currentSessionId);
    const agent =
      availableAgents.find((candidate) => candidate.id === activeSession?.agentId) ??
      availableAgents[0] ??
      agents[0]!;
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
      engineQuota: engineQuotaRows,
    });
  }, [availableAgents, availableSessions, currentSessionId, sessionUsageById, engineQuotaRows]);

  // Portable app update check — delayed so it never stalls first paint.
  useEffect(() => {
    if (!isTauriRuntime()) return;
    const t = window.setTimeout(() => {
      void checkAppUpdate().then((info) => {
        if (info?.updateAvailable || info?.note) setAppUpdate(info);
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

  /** Download the announced update and relaunch (footer 升级 / update banner). */
  const handleUpdateAndRelaunch = useCallback(() => {
    if (appUpdateBusy) return;
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
  }, [appUpdateBusy, currentSessionId, pushDebug]);

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
        agents[0]!.id;
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
      let resolvedProjects = withoutGhostProjects(
        nextProjects.length > 0 || isTauriRuntime() ? nextProjects : projects
      );
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
        void repairInjectedLabels(merged);
        return merged;
      };

      // Cold start from Explorer / CLI. A folder that already owns
      // `.marionette` is a project; a plain folder becomes one Chat session.
      if (launchPath) {
        try {
          const agentId = resolvedAgents[0]?.id ?? agents[0]!.id;
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
            : resolvedProjects[0]!.id,
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
        const excludeSessionIds = new Set<string>();
        if (!IS_DETACHED_WINDOW && isTauriRuntime()) {
          const visibleDetached = await listVisibleDetachedSessionIds();
          for (const id of visibleDetached) {
            excludeSessionIds.add(id);
            detachedOwnedIdsRef.current.add(id);
          }
        }
        const restored = startupChat
          ? {
              session: startupChat,
              openSessionIds: [startupChat.id],
              splitState: migrateFromFlat([startupChat.id], startupChat.id),
            }
          : pickRestoredSession(effectiveSessions, {
              detachedSessionId: DETACHED_SESSION_ID,
              excludeSessionIds,
              maxLeaves: MAX_PANES,
            });
        if (restored) {
          const preferred = restored.session;
          setCurrentProjectId(preferred.projectId);
          currentSessionIdRef.current = preferred.id;
          setCurrentSessionId(preferred.id);
          openSessionIdsRef.current = restored.openSessionIds;
          setOpenSessionIds(restored.openSessionIds);
          if (!IS_DETACHED_WINDOW) {
            splitStateRef.current = restored.splitState;
            setSplitState(restored.splitState);
          }
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
          setCurrentProjectId(CHAT_PROJECT_ID);
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
      splitLayout: serialize(splitState),
    });
  }, [currentSessionId, currentProjectId, openSessionIds, splitState]);

  useEffect(() => {
    if (!composerAutoFocus) return;
    if (!currentSessionId || currentSessionId.startsWith("session-empty-")) return;
    const id = window.requestAnimationFrame(() => setComposerAutoFocus(false));
    return () => window.cancelAnimationFrame(id);
  }, [composerAutoFocus, currentSessionId]);

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
      dropSessionRails([sessionId]);
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
      dropSessionRails([sessionId]);
      void loadSessionTranscriptRef.current(sessionId).finally(() => {
        reloadingSessionsRef.current.delete(sessionId);
      });
      const session = sessionsRef.current.find((s) => s.id === sessionId);
      const alreadyOpen =
        Boolean(paneOfSession(splitStateRef.current, sessionId)) ||
        openSessionIdsRef.current.includes(sessionId);
      if (session && !alreadyOpen) {
        openSessionRef.current(session);
      }
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
    void listenMergeBackRequests(async (payload) => {
      const sessionId = payload.sessionId;
      detachedOwnedIdsRef.current.delete(sessionId);
      const session = sessionsRef.current.find((s) => s.id === sessionId);
      const dropClient = payload.dropClient ?? (await readMainDropClient());
      let drop: DropKind = {
        kind: "activate",
        paneId: splitStateRef.current.focusedPaneId,
      };
      if (dropClient && session) {
        const hit = hitTestTabDrop(session, dropClient.x, dropClient.y);
        if (hit.kind === "split" || hit.kind === "activate" || hit.kind === "insert-tab") {
          drop = hit;
        }
      }
      if (session) {
        const tree =
          leafCount(splitStateRef.current) > 1
            ? splitStateRef.current
            : migrateFromFlat(openSessionIdsRef.current, currentSessionIdRef.current);
        commitSplit(adoptExternalSession(tree, session.id, drop, { maxLeaves: MAX_PANES }));
        openSessionRef.current(session);
        void broadcastSessionPatch({
          sessionId: session.id,
          status: session.status,
          label: session.label,
        });
        pushDebugRef.current({
          sessionId: session.id,
          level: "info",
          source: "split",
          summary: `merge-back drop kind=${drop.kind}${drop.kind === "split" ? ` edge=${drop.edge}` : ""} pane=${"paneId" in drop ? drop.paneId : ""}`,
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
    const cold = coldSessionIdsFromMap(shadowBySessionRef.current, hot);
    if (cold.length === 0) return;

    const flushed: string[] = [];
    for (const sessionId of cold) {
      // Don't drop while a debounced save is still pending — flush first.
      const pending = transcriptSaveTimers.current.get(sessionId);
      if (pending) {
        clearTimeout(pending);
        transcriptSaveTimers.current.delete(sessionId);
      }
      const events = persistableEventsForSession(
        shadowBySessionRef.current.get(sessionId) ?? EMPTY_EVENTS,
        sessionId,
      );
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
    dropSessionRails(drop);
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

    // Full per-event handler. Runs from flushAcpInbox in batches — never
    // straight from the Tauri listener (see the batching block after the body).
    const handleAcpEvent = (payload: AcpEvent) => {
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
        // A send parked because the connection's turn slot was busy may go now.
        releaseTurnSlotWaiters(turnSlotWaitersRef.current, payload.sessionId);
        turnEndedAtRef.current[payload.sessionId] = Date.now();
        // A re-entry segment ends here too — adapters that do send a boundary
        // for the continuation let the normal turn/complete path below seal
        // and notify (it already sees status "running" from the arm).
        disarmAgentReentry(payload.sessionId);
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
        void finalizeTurnFileChangesRef.current(payload.sessionId);
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
        setSessionNotices((current) => {
          const held = current[payload.sessionId];
          if (!held || held.title !== "Agent 已退出") return current;
          const next = { ...current };
          delete next[payload.sessionId];
          return next;
        });
      }

      // Live transcript: thinking / tool / assistant stream as events arrive.
      // OpenCode v2 child sessions arrive as `opencode/session/child_update`
      // when we advertised child-session-updates; otherwise they are projected
      // onto session/update with `_meta["opencode/child-session"]`.
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

        if (update) {
          const notice = parseLiveNotice(update);
          if (notice) {
            const sid = payload.sessionId;
            setSessionNotices((current) => ({
              ...current,
              [sid]: { sessionId: sid, ...notice },
            }));
            if (notice.severity === "info") {
              window.setTimeout(() => {
                setSessionNotices((current) => {
                  const held = current[sid];
                  if (!held || held.title !== notice.title) return current;
                  const next = { ...current };
                  delete next[sid];
                  return next;
                });
              }, 8000);
            }
          }
        }

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
      }

      if (isAcpTranscriptMethod(payload.method)) {
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
            // 再次进入对话: the agent is producing again while Marionette
            // considers the session idle (post-turn background continuation).
            // Arm on assistant/thought text; once armed, any transcript chunk
            // (tool status included) refreshes the quiet timer.
            const reentryArmed = agentReentryRef.current[payload.sessionId];
            if (reentryArmed) {
              if (
                extracted.role === "assistant" ||
                extracted.role === "thought" ||
                extracted.role === "tool"
              ) {
                armAgentReentryRef.current(payload.sessionId);
              }
            } else if (extracted.role === "assistant" || extracted.role === "thought") {
              const sess = sessionsRef.current.find((s) => s.id === payload.sessionId);
              if (
                shouldArmAgentReentry(
                  sess?.status,
                  turnEndedAtRef.current[payload.sessionId] ?? null,
                )
              ) {
                armAgentReentryRef.current(payload.sessionId);
              }
            }
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
              void refreshProjectChangedFilesRef.current(payload.sessionId);
            }
            if (extracted.text) {
              // Status-only blocks — usage already merged above; keep the chat
              // rail free of Codex `/status` and Claude `/usage` dumps.
              const isRuntimeStatusOnly =
                isRuntimeMetadataOnly(extracted.text) ||
                (/^\s*\*{0,2}Model\*{0,2}:/i.test(extracted.text) &&
                  /\n\s*\*{0,2}Directory\*{0,2}:/i.test(extracted.text));
              if (!isRuntimeStatusOnly) {
                const sid = payload.sessionId;
                const kind = chunkKindFromRole(extracted.role);
                const publish =
                  shouldPublishChunk(sid, kind, livePaintSetRef.current) === "now";
                updateSessionEvents(
                  sid,
                  (rail) => applyStreamPart(rail, sid, extracted),
                  { publish },
                );
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
          updateSessionEvents(payload.sessionId, (current) => {
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
            return next;
          }, { publish: true });
        } else {
          updateSessionEvents(payload.sessionId, (current) => {
            return collapseIntermediateAssistantAsThought(
              toolClose
                ? markOpenTools(current, payload.sessionId, toolClose)
                : current,
              payload.sessionId,
            );
          }, { publish: true });
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

        // A clean turn proves login and CLI version are fine — drop the banner.
        if (payload.kind !== "error" && stopReason !== "error" && stopReason !== "refusal") {
          const sessAgentId = sessionsRef.current.find((s) => s.id === payload.sessionId)?.agentId;
          if (sessAgentId) clearAgentBanner(sessAgentId);
        }

        // Usage panel: refresh once after each completed Reply turn.
        refreshUsageAfterTurnRef.current(payload.sessionId);
        // Desktop notify only for a real completed turn (was running, not cancel).
        // Clean completion follows OpenCode's 350ms idle-settle window so a
        // queued follow-up can cancel the intermediate completion alert.
        // A silent turn (hidden `/always-approve` replay) completes with nothing
        // on screen — chiming for it would be noise the user cannot explain.
        // Same for a usage probe (`/status` card): refresh ≠ a reply.
        const silentTurnEnded = silentTurnRef.current.delete(payload.sessionId);
        const probeTurnEnded = probeTurnRef.current.delete(payload.sessionId);
        if (wasRunning && stopReason !== "cancelled" && !silentTurnEnded && !probeTurnEnded) {
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
            shadowBySessionRef.current.get(payload.sessionId) ?? EMPTY_EVENTS,
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
              scheduleDesktopNotify(
                payload.sessionId,
                "reply",
                label,
                DESKTOP_NOTIFY_SETTLE_MS,
                { audibleWhenFocused: true },
              );
            }
            if (!turnFailed && payload.sessionId !== currentSessionIdRef.current) {
              setDoneUnreadIds((current) => {
                if (current.has(payload.sessionId)) return current;
                const next = new Set(current);
                next.add(payload.sessionId);
                return next;
              });
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
        setSessionNotices((current) => {
          const held = current[payload.sessionId];
          if (!held || !held.title.startsWith("正在重试")) return current;
          const next = { ...current };
          delete next[payload.sessionId];
          return next;
        });
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
        silentTurnRef.current.delete(payload.sessionId);
        probeTurnRef.current.delete(payload.sessionId);
        setQueuedStripTick((t) => t + 1);
        // Zombie Ask / Plan / Permission cards block the composer — clear for this session.
        setAskPrompt((cur) => (cur?.sessionId === payload.sessionId ? null : cur));
        setPlanApproval((cur) => (cur?.sessionId === payload.sessionId ? null : cur));
        setPlanApprovalBusy(false);
        setPermissionPrompt((cur) => (cur?.sessionId === payload.sessionId ? null : cur));
        updateSessionEvents(
          payload.sessionId,
          (current) => expirePendingPrompts(current, payload.sessionId, "timeout"),
          { publish: true },
        );
        if (endedHard) {
          const sid = payload.sessionId;
          updateSessionEvents(sid, (current) => markOpenTools(current, sid, "failed"), {
            publish: true,
          });
          setSessionNotices((current) => ({
            ...current,
            [sid]: {
              sessionId: sid,
              severity: "error",
              title: "Agent 已退出",
              description: `${detail}。点输入框或发一条消息会重新拉起，也可以开新会话。`,
            },
          }));
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
          const sess = sessionsRef.current.find((s) => s.id === payload.sessionId);
          const sessAgentId = sess?.agentId;
          const sessAgent = sessAgentId
            ? agentsRef.current.find((a) => a.id === sessAgentId)
            : undefined;
          const caps = capsByIdRef.current[payload.sessionId];
          const currentModel =
            (payload.sessionId === currentSessionIdRef.current
              ? activeModelIdRef.current
              : null) ||
            sess?.preferredModel ||
            caps?.currentModel ||
            undefined;
          const classified = classifyAgentError(errText, {
            agentId: sessAgentId,
            agentLabel: sessAgent?.label,
            currentModel,
          });
          let body = formatClassifiedError(classified);
          setComposerFailure({ sessionId: payload.sessionId, error: classified });
          // Auth and “CLI too old” both arrive as Internal error + data.message.
          // The banner is the Claude/Codex-style prompt; the strip is the button.
          raiseClassifiedBanner(sessAgentId, classified);
          updateSessionEvents(payload.sessionId, (current) => {
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
          }, { publish: true });
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
          const planMarkdown = extractPermissionPlanMarkdown({
            title,
            detail,
            options,
            params: data.params,
            planMarkdown: data.planMarkdown ?? data.plan_markdown,
          });
          if (planMarkdown) {
            setPlanApproval({
              requestId,
              sessionId: payload.sessionId,
              planMarkdown,
              title,
              permissionOptions: options,
            });
            updateSessionEvents(
              payload.sessionId,
              (current) =>
                upsertPromptEvent(
                  current,
                  planPromptEvent({
                    sessionId: payload.sessionId,
                    requestId,
                    source: "permission",
                    title,
                    planMarkdown,
                    options,
                  }),
                ),
              { publish: true },
            );
          } else {
            setPermissionPrompt({
              requestId,
              sessionId: payload.sessionId,
              title,
              detail,
              planMarkdown,
              options,
            });
          }
          stealFocusForPrompt(payload.sessionId);
          cancelScheduledDesktopNotify(payload.sessionId);
          void raiseDesktopNotify(
            planMarkdown ? "plan" : "permission",
            planMarkdown
              ? "Plan ready for review"
              : compactNotifyDetail(detail ? `${title}: ${detail}` : title),
          );
        }
      }
      // resolved = 另一个面（Telegram 按钮）已解答；与本窗口的点击、超时一样收卡片。
      if (payload.method === "permission/timeout" || payload.method === "permission/resolved") {
        const data = payload.data as { requestId?: string } | null;
        const requestId = data?.requestId;
        const timedOut = payload.method === "permission/timeout";
        setPermissionPrompt((current) => {
          if (!current) return current;
          if (requestId && requestId === current.requestId) return null;
          return current;
        });
        setPlanApproval((current) => {
          if (!current) return current;
          if (requestId && requestId === current.requestId) return null;
          return current;
        });
        if (requestId) {
          updateSessionEvents(
            payload.sessionId,
            (current) =>
              patchPromptEvent(current, requestId, {
                status: timedOut ? "timeout" : "answered",
                decisionLabel: timedOut ? "已超时" : "已在其他窗口回复",
              }),
            { publish: true },
          );
        }
      }

      // Grok `_x.ai/ask_user_question` → interactive choice card
      if (payload.method === "question/prompt" && payload.data) {
        const parsed = parseAskQuestionPrompt(payload.sessionId, payload.data);
        if (parsed) {
          setAskPrompt(parsed);
          updateSessionEvents(
            payload.sessionId,
            (current) => upsertPromptEvent(current, askPromptEvent(parsed)),
            { publish: true },
          );
          stealFocusForPrompt(payload.sessionId);
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
      if (payload.method === "question/timeout" || payload.method === "question/resolved") {
        const data = payload.data as { requestId?: string } | null;
        const requestId = data?.requestId;
        const timedOut = payload.method === "question/timeout";
        setAskPrompt((current) => {
          if (!current) return current;
          if (requestId && requestId === current.requestId) return null;
          return current;
        });
        if (requestId) {
          updateSessionEvents(
            payload.sessionId,
            (current) =>
              patchPromptEvent(current, requestId, {
                status: timedOut ? "timeout" : "answered",
                decisionLabel: timedOut ? "已超时" : "已在其他窗口回复",
              }),
            { publish: true },
          );
        }
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
          updateSessionEvents(
            payload.sessionId,
            (current) =>
              upsertPromptEvent(
                current,
                planPromptEvent({
                  sessionId: payload.sessionId,
                  requestId,
                  source: "plan_approval",
                  planMarkdown,
                }),
              ),
            { publish: true },
          );
          stealFocusForPrompt(payload.sessionId);
          cancelScheduledDesktopNotify(payload.sessionId);
          void raiseDesktopNotify("plan", "Plan ready for review");
        }
      }
      if (payload.method === "plan/timeout" || payload.method === "plan/resolved") {
        const data = payload.data as { requestId?: string } | null;
        const requestId = data?.requestId;
        const timedOut = payload.method === "plan/timeout";
        setPlanApproval((current) => {
          if (!current) return current;
          if (requestId && requestId === current.requestId) return null;
          return current;
        });
        if (requestId) {
          updateSessionEvents(
            payload.sessionId,
            (current) =>
              patchPromptEvent(current, requestId, {
                status: timedOut ? "timeout" : "answered",
                decisionLabel: timedOut ? "已超时" : "已在其他窗口回复",
              }),
            { publish: true },
          );
        }
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
    };

    // ── acp-event stream batching ──────────────────────────────────────────
    // 2026-10-16 假死根因:两个 agent 并发流式输出时 acp-event 以 60–80 条/秒
    // 到达,每条事件各自触发根组件 setState(整条聊天栏重建),渲染主线程被
    // 打满,暂停/关闭按钮再也无法响应,只能杀进程。改为先收进缓冲、每
    // ACP_EVENT_FLUSH_MS 统一应用一次:同一次 flush 里的全部 setState 会被
    // React 18 合并成一个渲染批次,而文本到达延迟仍远低于感知阈值。
    const ACP_EVENT_FLUSH_MS = 50;
    // 每片最多同步消化的事件数：重放/追赶可能一次塞进几千条，单循环照旧
    // 打满主线程（假死根因的变体）——每片之间让出事件循环。
    const ACP_FLUSH_CHUNK = 200;
    const acpInbox: AcpEvent[] = [];
    let acpFlushTimer: number | undefined;
    const flushAcpInbox = () => {
      acpFlushTimer = undefined;
      if (disposed) {
        acpInbox.length = 0;
        return;
      }
      if (acpInbox.length === 0) return;
      const batch = acpInbox.splice(0, ACP_FLUSH_CHUNK);
      livePaintSetRef.current = currentPaintSet();
      // One bad event must not drop the rest of the batch.
      for (const payload of batch) {
        // Any event for a session proves its agent is alive — retire the
        // send-silence watchdog before guards can skip the event.
        if (payload.sessionId) clearSendSilenceWatchdog(payload.sessionId);
        try {
          // Guards run at processing time, exactly as they did pre-batching —
          // ownership / reload state may flip while events sit in the inbox.
          if (payload.sessionId && !ownsAcpSession(payload.sessionId)) continue;
          if (payload.sessionId && reloadingSessionsRef.current.has(payload.sessionId)) continue;
          handleAcpEvent(payload);
        } catch (err) {
          pushDebug({
            sessionId: payload.sessionId,
            level: "error",
            source: "acp",
            summary: `event handler failed: ${String(err)}`,
          });
        }
      }
      livePaintSetRef.current = currentPaintSet();
      const flushed = flushShadowsIntoPaintSet(
        livePaintSetRef.current,
        shadowBySessionRef.current,
        eventsBySessionRef.current,
      );
      if (flushed.length > 0) bumpEventsTick(flushed);
      // Delegate children finishing / pane structure changed mid-stream both
      // show up here; the reporter itself is change-detected.
      reportStreamInterestsRef.current();
      // 还有积压：下一片在事件循环里继续（0ms 让出主线程）。
      if (acpInbox.length > 0) {
        acpFlushTimer = window.setTimeout(flushAcpInbox, 0);
      }
    };
    const scheduleAcpFlush = () => {
      if (acpFlushTimer === undefined) {
        acpFlushTimer = window.setTimeout(flushAcpInbox, ACP_EVENT_FLUSH_MS);
      }
    };

    void listen<AcpEvent>("acp-event", (event) => {
      if (disposed) return;
      acpInbox.push(event.payload);
      scheduleAcpFlush();
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
      // Drain whatever is still in the inbox so teardown (unmount / detached
      // hide) cannot silently drop up to one flush window of stream. Also
      // clears the pending timer slot; a late callback on an empty inbox is a
      // no-op.
      flushAcpInbox();
      disposed = true;
      if (IS_DETACHED_WINDOW && DETACHED_SESSION_ID) {
        void setDetachedSessionOwner(DETACHED_SESSION_ID, false);
      }
      unlistenAcp?.();
      for (const t of cancelWatchdogsRef.current.values()) clearTimeout(t);
      cancelWatchdogsRef.current.clear();
    };
  }, [applyAgentSessionTitle, pushDebug, touchActivity, clearAgentBanner, raiseClassifiedBanner, disarmAgentReentry]);
  // note: pushDebug is stable via useCallback

  // Stream interest follows pane structure, the focused tab, the session list,
  // and window visibility (detached hide/show, minimize/restore). Event-driven
  // drift (delegate children finishing) is caught by the flush piggyback.
  useEffect(() => {
    reportStreamInterests();
    const onVisibility = () => reportStreamInterests();
    document.addEventListener("visibilitychange", onVisibility);
    return () => document.removeEventListener("visibilitychange", onVisibility);
  }, [splitState, currentSessionId, availableSessions]);

  // Detached close: flush JSONL then give the ACP stream back to main.
  useEffect(() => {
    if (!isTauriRuntime() || !IS_DETACHED_WINDOW || !DETACHED_SESSION_ID) return;
    let unlisten: (() => void) | undefined;
    void listen<string>(DETACHED_HIDDEN_EVENT, (event) => {
      const label = typeof event.payload === "string" ? event.payload : "";
      if (sessionIdFromDetachedLabel(label) !== DETACHED_SESSION_ID) return;
      const events = persistableEventsForSession(
        shadowBySessionRef.current.get(DETACHED_SESSION_ID) ?? EMPTY_EVENTS,
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
   * Park a dialog: stop its agent process and arm the history injection, so the
   * next send reconnects with the local transcript on top — the same path a
   * restart takes.
   *
   * Parking works mid-turn too — the user parking a working dialog is asking it
   * to stop. Seal the open Reply and its tools locally *before* the kill, so
   * chunks already in flight cannot reopen the card this call just closed, and
   * so the next turn on this dialog starts from a closed card rather than
   * swallowing the new process's stream. `process/stopped` then takes care of
   * the rest: queued sends, Ask/Plan/Permission cards, capabilities.
   */
  /** 结束会话的 agent 进程并标记「下一条消息带历史自动拉起」的共享实现。 */
  const parkSessionProcess = useCallback(
    (sessionId: string, opts: { midTurnBanner?: string; debugSummary: string }) => {
      const midTurn = opts.midTurnBanner != null;
      void stopAcpSession(sessionId)
        .catch(() => undefined)
        .then(() => {
          acpBootstrapRef.current.delete(sessionId);
          acpNeedsHistoryRef.current.add(sessionId);
          setSessionStatusById(sessionId, "exited");
          if (sessionId === currentSessionIdRef.current) setSessionCapabilities(null);
          if (midTurn) {
            updateSessionEvents(sessionId, (current) => [
              ...current,
              {
                type: "assistant_message" as const,
                sessionId,
                text: opts.midTurnBanner!,
                createdAt: new Date().toISOString(),
                durationMs: 0,
              },
            ], { publish: true });
          }
          pushDebug({
            sessionId,
            level: "info",
            source: "shelf",
            summary: opts.debugSummary,
          });
        });
    },
    // `setSessionStatusById` is a stable `useCallback` declared further down; it
    // cannot be named in this dep array without a TDZ read at definition time.
    [pushDebug],
  );

  const handleSuspendSession = useCallback(
    (sessionId: string) => {
      const session = sessionsRef.current.find((s) => s.id === sessionId);
      if (!session || !canPark(session)) return;
      const midTurn = session.status === "running" || session.status === "starting";
      const label = session.label?.trim() || "会话";

      if (midTurn) {
        streamSuppressedRef.current.add(sessionId);
        updateSessionEvents(sessionId, (current) =>
          sealOpenAssistantReplies(markOpenTools(current, sessionId, "cancelled"), sessionId),
        { publish: true });
      }

      parkSessionProcess(sessionId, {
        midTurnBanner: midTurn
          ? `**已挂起。**\n\n${label} 当时正在跑，我中断了它并结束了 agent 进程。说出口的部分留在上面。对话记录都在 —— 下次发消息会自动重新拉起进程，并把这段历史接上去。`
          : undefined,
        debugSummary: midTurn ? "suspended a working dialog" : "suspended idle dialog",
      });
    },
    [parkSessionProcess],
  );

  /**
   * 跨用模型切换的「当场生效」：注入是 spawn 级的（env / grok 配置表），活
   * 进程必须换血。复用挂起机制结束进程（历史保留、下一条消息自动拉起），
   * banner 换成切换说明。midTurn 时同样先封口再杀。
   */
  const handleRestartAgentSession = useCallback(
    (sessionId: string, banner: string) => {
      const session = sessionsRef.current.find((s) => s.id === sessionId);
      if (!session || !canPark(session)) return;
      const midTurn = session.status === "running" || session.status === "starting";
      if (midTurn) {
        streamSuppressedRef.current.add(sessionId);
        updateSessionEvents(sessionId, (current) =>
          sealOpenAssistantReplies(markOpenTools(current, sessionId, "cancelled"), sessionId),
        { publish: true });
      }
      parkSessionProcess(sessionId, {
        midTurnBanner: midTurn ? banner : undefined,
        debugSummary: "restarted agent process for bridge model switch",
      });
    },
    [parkSessionProcess],
  );

  /**
   * Suspend warm dialogs nobody is using. A warm session holds an agent process
   * per dialog, and a shelf of untouched dialogs is memory the user is not
   * spending on anything.
   */
  // 提示卡状态走 ref：挂起巡检的 60s 窗口不随提示出现/消失被重置
  // （键盘监听同理——见下方 Esc effect）。
  const escPromptRef = useRef({
    permissionPrompt,
    askPrompt,
    planApproval,
    diffPreview,
    projectDialogOpen,
    projectAdding,
  });
  const layoutPersistWarnedRef = useRef(false);
  useEffect(() => {
    escPromptRef.current = {
      permissionPrompt,
      askPrompt,
      planApproval,
      diffPreview,
      projectDialogOpen,
      projectAdding,
    };
  });
  useEffect(() => {
    if (!isTauriRuntime()) return;
    const timer = setInterval(() => {
      const { permissionPrompt, askPrompt, planApproval } = escPromptRef.current;
      const now = Date.now();
      const pendingSessionIds = new Set(
        [permissionPrompt, askPrompt, planApproval]
          .map((prompt) => prompt?.sessionId)
          .filter((id): id is string => Boolean(id)),
      );
      const queuedSessionIds = new Set(pendingSendsRef.current.keys());
      // 「当前对话不挂起」只保护看得见的窗口：窗口藏进后台（background
      // mode/最小化，visibilityState=hidden）时用户根本不在看，30 分钟到了
      // 连当前对话一起挂起——否则它会永远停在「等你」并养着一个活进程
      // （2026-10-25 反馈）。贴边停靠的窄条窗口并没有 hide，仍是 visible，
      // 照旧豁免。
      const exemptCurrent =
        typeof document !== "undefined" && document.visibilityState === "visible"
          ? currentSessionIdRef.current
          : null;
      for (const session of sessionsRef.current) {
        if (
          !shouldAutoSuspend(session, now, {
            currentSessionId: exemptCurrent,
            pendingSessionIds,
            queuedSessionIds,
          })
        ) {
          continue;
        }
        handleSuspendSession(session.id);
      }
    }, 60_000);
    return () => clearInterval(timer);
  }, [handleSuspendSession]);

  /**
   * Fork: copy the dialog up to one reply into a new session and open it.
   *
   * The copy becomes the new dialog's transcript, and its first send arms the
   * history injection — so the agent starts from exactly that context while the
   * original dialog stays untouched.
   */
  const handleForkFromReply = useCallback(
    async (sessionId: string, anchor: ForkAnchor) => {
      const source = sessionsRef.current.find((s) => s.id === sessionId);
      if (!source) return;
      const events = shadowBySessionRef.current.get(sessionId) ?? EMPTY_EVENTS;
      // Nothing to copy means the anchor is gone (the reply was truncated or
      // the transcript reloaded) — bail before creating a dialog.
      if (forkCutIndex(events, anchor) < 0) return;

      const label = `${source.label} · 分叉`;
      const created =
        source.projectId === CHAT_PROJECT_ID
          ? await createChatSession(source.agentId, label, source.cwd || null)
          : await createSessionApi(source.projectId, source.agentId);
      if (!created) return;

      const forked: Session = {
        ...created,
        label,
        labelSource: "manual",
        preferredModel: source.preferredModel ?? null,
        preferredMode: source.preferredMode ?? null,
        preferredEffort: source.preferredEffort ?? null,
        preferredEffortId: source.preferredEffortId ?? null,
        preferredAlwaysApprove: source.preferredAlwaysApprove ?? null,
      };
      setAvailableSessions((current) => [
        forked,
        ...current.filter((s) => s.id !== forked.id),
      ]);
      const tree = splitStateRef.current;
      if (!IS_DETACHED_WINDOW && leafCount(tree) > 1) {
        const sourcePane = paneOfSession(tree, sessionId);
        commitSplit(addTabToPane(tree, sourcePane?.id ?? tree.focusedPaneId, forked.id));
      } else {
        setOpenSessionIds((current) => [
          forked.id,
          ...current.filter((id) => id !== forked.id),
        ]);
        setCurrentSessionId(forked.id);
      }
      setCurrentProjectId(forked.projectId);
      setViewMode("clean");
      setSessionCapabilities(capsByIdRef.current[forked.id] ?? null);
      setActiveModelId(null);

      // Subtask cards and handoffs point at the old dialog's children and
      // paths, so the copy drops them.
      const copied = forkCopyEvents(events, anchor, forked.id);
      if (!copied) return;
      updateSessionEvents(forked.id, () => copied, { publish: true });
      // The agent starts empty; the first send carries this transcript.
      acpNeedsHistoryRef.current.add(forked.id);

      try {
        await writeTranscript(forked.id, persistableEventsForSession(copied, forked.id));
        await updateSessionLabel(forked.id, label, "manual");
        await updateSessionPrefs(forked.id, {
          preferredModel: forked.preferredModel ?? null,
          preferredMode: forked.preferredMode ?? null,
          preferredEffort: forked.preferredEffort ?? null,
          preferredEffortId: forked.preferredEffortId ?? null,
          preferredAlwaysApprove: forked.preferredAlwaysApprove ?? null,
        });
      } catch (error) {
        pushDebug({
          sessionId: forked.id,
          level: "warn",
          source: "fork",
          summary: "fork copy failed",
          detail: error instanceof Error ? error.message : String(error),
        });
      }
      pushDebug({
        sessionId: forked.id,
        level: "info",
        source: "fork",
        summary: `forked ${sessionId} (${copied.length} rows)`,
        detail: label,
      });
    },
    [pushDebug],
  );

  /**
   * Repair labels already stored from a Marionette prompt preamble: re-derive
   * the title from the first message the user actually typed, out of the
   * session's own transcript. Runs once per startup, on the affected rows only.
   */
  const repairInjectedLabels = useCallback(async (sessions: Session[]) => {
    if (!isTauriRuntime()) return;
    for (const session of sessions) {
      if (!isInjectedPromptText(session.label)) continue;
      const raw = await loadTranscript(session.id).catch(() => []);
      const first = parseTranscriptEvents(raw).find(
        (event) => event.type === "user_message" && !isInjectedPromptText(event.text),
      );
      if (!first || first.type !== "user_message") continue;
      const label = titleFromUserText(first.text);
      if (!label || isInjectedPromptText(label)) continue;
      setAvailableSessions((current) =>
        current.map((s) => (s.id === session.id ? { ...s, label, labelSource: "user" } : s))
      );
      void updateSessionLabel(session.id, label, "user").catch(() => undefined);
      void broadcastSessionPatch({ sessionId: session.id, label, labelSource: "user" });
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
    const existing = shadowBySessionRef.current.get(sessionId);
    if (!existing || existing.length === 0) {
      updateSessionEvents(sessionId, () => parsed, { publish: true });
    }
    // Seed lastTurnStats from persisted transcript so a restored dialog
    // still has per-turn token splits in memory (the Usage panel no longer
    // renders those rows).
    let lastStats: TurnStats | null = null;
    for (let i = parsed.length - 1; i >= 0; i -= 1) {
      const e = parsed[i]!;
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

  // Restore Clean history for every visible pane (and the focused tab when unsplit).
  useEffect(() => {
    const nextPaint = currentPaintSet();
    livePaintSetRef.current = nextPaint;
    const flushed = flushShadowsIntoPaintSet(
      nextPaint,
      shadowBySessionRef.current,
      eventsBySessionRef.current,
    );
    if (flushed.length > 0) bumpEventsTick(flushed);
    const visibles =
      workspaceSplit
        ? visibleIdsFromTree(splitState)
        : currentSessionId
          ? [currentSessionId]
          : [];
    for (const id of visibles) {
      if (id && !id.startsWith("session-empty-")) void loadSessionTranscript(id);
    }
  }, [currentSessionId, loadSessionTranscript, splitState, workspaceSplit]);

  useEffect(() => {
    saveDetailsVisible(detailsVisible);
  }, [detailsVisible]);

  // Keep the one-leaf tree in sync with the flat tab list. Once split, the tree is source.
  useEffect(() => {
    if (IS_DETACHED_WINDOW) return;
    setSplitState((cur) => {
      if (leafCount(cur) > 1) return cur;
      const next = migrateFromFlat(openSessionIds, currentSessionId);
      if (cur.root.type === "leaf" && next.root.type === "leaf") {
        const curTabs = cur.root.tabIds;
        const nextTabs = next.root.tabIds;
        if (
          cur.root.id === next.root.id &&
          cur.root.activeTabId === next.root.activeTabId &&
          cur.focusedPaneId === next.focusedPaneId &&
          curTabs.length === nextTabs.length &&
          curTabs.every((id, i) => id === nextTabs[i])
        ) {
          return cur;
        }
      }
      splitStateRef.current = next;
      return next;
    });
  }, [openSessionIds, currentSessionId]);

  const modelIdForAgentProbe = useCallback((agentId: string): string | undefined => {
    const sid = currentSessionIdRef.current;
    const sess = sid ? sessionsRef.current.find((s) => s.id === sid) : undefined;
    if (sess?.agentId === agentId) {
      const model = (
        activeModelIdRef.current ||
        sess.preferredModel ||
        capsByIdRef.current[sess.id]?.currentModel ||
        ""
      ).trim();
      return model || undefined;
    }
    const other = sessionsRef.current.find((s) => s.agentId === agentId);
    if (!other) return undefined;
    const model = (
      other.preferredModel ||
      capsByIdRef.current[other.id]?.currentModel ||
      ""
    ).trim();
    return model || undefined;
  }, []);

  const refreshAgentAuth = useCallback(
    async (agentId: string) => {
      const spec = agentAuthSpec(agentId);
      if (!spec?.probe) return null;
      const probe = await probeAgentAuth(agentId, modelIdForAgentProbe(agentId));
      if (!probe) return null;
      if (probe.status === "logged_in") {
        setAuthHintFor(agentId, null);
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
      return probe;
    },
    [modelIdForAgentProbe, setAuthHintFor]
  );

  const probedAgentId =
    availableSessions.find((s) => s.id === currentSessionId)?.agentId ??
    availableAgents[0]?.id ??
    "";

  // Probe when the active agent changes — not on every status tick.
  // A status update used to re-probe and wipe an error-driven banner
  // (expired Grok token, CLI too old) a moment after it appeared.
  useEffect(() => {
    const agentId = probedAgentId;
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
  }, [probedAgentId, refreshAgentAuth, setAuthHintFor]);

  const handleGrokUpdate = useCallback(
    async (agentId: string) => {
      setUpdateBusy(true);
      pushDebug({
        level: "info",
        source: "auth",
        summary: "start grok update",
      });
      const result = await startGrokUpdate();
      setUpdateBusy(false);
      setAuthHintFor(
        agentId,
        result?.started
          ? "已打开 `grok update`。完成后新建会话，新版本才会生效。"
          : result?.message || "无法启动更新。请在终端执行 `grok update`。",
        "upgrade"
      );
    },
    [pushDebug, setAuthHintFor]
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
      probeTurnRef.current.add(sid);
      void sendAcpPrompt(sid, limitCommand, [], true).catch((error) => {
        probeTurnRef.current.delete(sid);
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
    refreshEngineQuota(true);
    handleUsageRefresh();
  }, [handleUsageRefresh, refreshEngineQuota]);

  /**
   * After an AI turn ends (`turn/complete`), refresh the Usage panel once.
   * Throttled so multi-chunk finalization / rapid turns don't spam probes.
   * ChatGPT subscription usage is intentionally excluded: it is a private
   * endpoint and refreshes only on model change or explicit user action.
   */
  const refreshUsageAfterTurn = useCallback(
    (sessionId: string) => {
      if (!sessionId || sessionId !== currentSessionIdRef.current) return;
      // The turn that just ended was itself the usage probe — the panel already
      // has the freshest data; probing again doubles the transcript noise.
      if (probeTurnRef.current.has(sessionId)) return;
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
        const evs = shadowBySessionRef.current.get(session.id) ?? EMPTY_EVENTS;
        for (let i = evs.length - 1; i >= 0; i -= 1) {
          const e = evs[i]!;
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
      } catch (err) {
        // ignore quota / private mode — but say it once, not per pointermove.
        if (!layoutPersistWarnedRef.current) {
          layoutPersistWarnedRef.current = true;
          console.warn("[layout] persist failed (quota / private mode)", err);
        }
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

  const shelfChatSplit = useMemo(
    () => splitShelfChatSessions(availableSessions),
    [availableSessions]
  );
  const chatSessions = shelfChatSplit.chat;
  /** 收纳盒-grown dialogs (origin "memo") — their own shelf group. */
  const memoSessions = shelfChatSplit.memo;

  /**
   * Edge dock strip (instead of tabs): every dialog still holding a process,
   * plus the one in view, newest first. A finished (waiting) dialog stays
   * listed until its process is really gone — the 30-min auto-suspend, a
   * manual park, or an agent exit flips it to `exited` and only then does the
   * row leave (2026-10-26; the 60s-after-quiet eviction read as "finished
   * dialogs vanish the moment they complete"). 全部对话 keeps the unread-done
   * badge either way.
   */
  const [dockListTick, setDockListTick] = useState(0);
  useEffect(() => {
    if (!dockActive) return;
    // No time-based eviction anymore — the tick only keeps the rows' "5m"
    // recency labels honest while the strip sits idle.
    const id = window.setInterval(() => setDockListTick((tick) => tick + 1), 15_000);
    return () => window.clearInterval(id);
  }, [dockActive]);

  const dockActiveSessions = useMemo(() => {
    if (!dockActive) return [];
    void dockListTick;
    return availableSessions
      .filter((session) => session.id === currentSessionId || canPark(session))
      .sort((a, b) => lastActiveMs(b) - lastActiveMs(a));
  }, [availableSessions, currentSessionId, dockActive, dockListTick]);

  // Strip sliver: the parked handle breathes while any dialog is working.
  const dockAnyBusy = dockActive && availableSessions.some((s) => s.status === "running" || s.status === "starting");
  useEffect(() => {
    document.documentElement.classList.toggle("dock-live", dockAnyBusy);
  }, [dockAnyBusy]);

  // ─── 后台任务面板 ──────────────────────────────────────────────────────────
  // 聚合 @-delegate 子代理 / 排队委托 / ACP terminal/* 后台命令 / 排队发送 /
  // 待审批（需要你）。子代理与排队发送的真相在 ref 里，靠 delegateTick /
  // queuedStripTick / 会话状态变化触发重算；终端快照 3s 拉一次（幂等 invoke）。
  const [acpTerminals, setAcpTerminals] = useState<AcpTerminalRow[]>([]);
  const [backgroundTick, setBackgroundTick] = useState(0);
  useEffect(() => {
    let alive = true;
    const pull = () => {
      void listAcpTerminals().then((rows) => {
        if (alive) setAcpTerminals((prev) => (sameTerminals(prev, rows) ? prev : rows));
      });
    };
    pull();
    const id = window.setInterval(pull, 3000);
    return () => {
      alive = false;
      window.clearInterval(id);
    };
  }, []);
  useEffect(() => {
    // delegateMeta/delegateQueue/pendingSends 都是 ref——子代理启动/结束、
    // 排队入列都会 bump queuedStripTick，会话状态翻转走这里重算。
    setBackgroundTick((t) => t + 1);
  }, [queuedStripTick, availableSessions, permissionPrompt, askPrompt, planApproval]);

  const backgroundTasks = useMemo(() => {
    void backgroundTick;
    const queuedSends = new Map<string, number>();
    const queuedSendLabels = new Map<string, string>();
    for (const [sessionId, items] of pendingSendsRef.current) {
      if (items.length > 0) queuedSends.set(sessionId, items.length);
    }
    const sessionLabels = new Map<string, string>();
    for (const session of availableSessions) {
      sessionLabels.set(session.id, session.label?.trim() || "Session");
      if (queuedSends.has(session.id)) {
        queuedSendLabels.set(session.id, session.label?.trim() || "排队发送");
      }
    }
    return buildBackgroundTaskRows({
      delegateMeta: delegateMetaRef.current,
      delegateQueue: delegateQueueRef.current,
      terminals: acpTerminals,
      queuedSends,
      queuedSendLabels,
      currentSessionId,
      permissionSessionId: permissionPrompt?.sessionId,
      permissionTitle: permissionPrompt?.title ?? null,
      askSessionId: askPrompt?.sessionId,
      planSessionId: planApproval?.sessionId,
      sessionLabels,
    });
  }, [backgroundTick, acpTerminals, availableSessions, currentSessionId, permissionPrompt, askPrompt, planApproval]);

  const handleKillBackgroundTerminal = useCallback((terminalId: string) => {
    void killAcpTerminal(terminalId).catch(() => undefined);
  }, []);

  const sessionLabelsForBackground = useMemo(() => {
    const labels = new Map<string, string>();
    for (const session of availableSessions) {
      labels.set(session.id, session.label?.trim() || "Session");
    }
    return labels;
  }, [availableSessions]);

  /** 面板行点击：切到发起会话（子代理是父会话，行内已有归属说明）。 */
  const handleOpenBackgroundTask = useCallback(
    (row: { sessionId: string }) => {
      const session = availableSessions.find((s) => s.id === row.sessionId);
      if (session) {
        openSession(session);
        return;
      }
      // 会话表里没有（如刚结束的 delegate 子会话）——退而打开父会话。
      const parentId = delegateMetaRef.current.get(row.sessionId)?.parentId;
      const parent = parentId ? availableSessions.find((s) => s.id === parentId) : undefined;
      if (parent) openSession(parent);
    },
    [availableSessions],
  );

  // ───────────────────────────────────────────────────────────────────────────

  // ProjectShelf 的项目列表（memo 化：内联 filter 会让引用每渲染都变，击穿 memo）。
  const shelfProjects = useMemo(
    () => availableProjects.filter((p) => p.id !== CHAT_PROJECT_ID),
    [availableProjects]
  );

  const currentSession = useMemo(
    () =>
      availableSessions.find((session) => session.id === currentSessionId) ??
      (currentSessionId ? projectSessions[0] : undefined),
    [availableSessions, currentSessionId, projectSessions]
  );

  const displaySession = useMemo((): Session => {
    return (
      currentSession ?? {
        id: `session-empty-${currentProjectId || currentProject?.id || "none"}`,
        projectId: currentProject?.id ?? currentProjectId,
        agentId: availableAgents[0]?.id ?? agents[0]!.id,
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
    () => availableAgents.find((agent) => agent.id === displaySession.agentId) ?? availableAgents[0] ?? agents[0]!,
    [availableAgents, displaySession]
  );

  // 只订阅本会话的 tick 值：别的会话在流式输出时不再重算本会话的事件表。
  const displaySessionTick = eventsTick[displaySession.id];
  const currentEvents = useMemo(() => {
    return eventsBySessionRef.current.get(displaySession.id) ?? EMPTY_EVENTS;
  }, [displaySessionTick, displaySession.id]);

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
      (availableAgents[0] ?? agents[0])!.id;
    // Model/mode/effort only transfer when the agent is the same (or the
    // caller passed prefs explicitly). A different agent has a different catalog.
    const sameAgentAsSource = !source || source.agentId === resolvedAgentId;
    const sourcePrefs: SessionComposerPrefs | null =
      source && sameAgentAsSource
        ? {
            preferredModel: source.preferredModel ?? null,
            preferredMode: source.preferredMode ?? null,
            preferredEffort: source.preferredEffort ?? null,
            preferredEffortId: source.preferredEffortId ?? null,
            preferredAlwaysApprove: source.preferredAlwaysApprove ?? null,
          }
        : null;
    // All-null source prefs mean "this dialog remembers nothing" — inherit from
    // the agent's last-used chips instead of treating the empty row as a choice.
    const hasSourcePrefs = sourcePrefs
      ? Object.values(sourcePrefs).some((value) => value != null)
      : false;
    const resolvedPrefs: SessionComposerPrefs | null =
      prefs !== undefined
        ? prefs
        : hasSourcePrefs
          ? sourcePrefs
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
    const tree = splitStateRef.current;
    if (!IS_DETACHED_WINDOW && leafCount(tree) > 1) {
      commitSplit(addTabToPane(tree, tree.focusedPaneId, sessionWithPrefs.id));
    } else {
      setOpenSessionIds((current) => [
        sessionWithPrefs.id,
        ...current.filter((id) => id !== sessionWithPrefs.id),
      ]);
      setCurrentSessionId(sessionWithPrefs.id);
    }
    setCurrentProjectId(projectId);
    setViewMode("clean");
    setSessionCapabilities(capsByIdRef.current[sessionWithPrefs.id] ?? null);
    setActiveModelId(null);
    return sessionWithPrefs;
  };

  const handleRefreshExternal = useCallback(async (projectId: string) => {
    if (!isTauriRuntime() || !projectId) return;
    setExternalScanning(true);
    try {
      const result = await refreshExternalSessions(projectId);
      const importedIds = new Set(
        result.sessions.filter((s) => s.id.startsWith("ext-")).map((s) => s.id),
      );
      const redirectTo = new Map(result.redirected.map((item) => [item.from, item.to]));
      for (const id of importedIds) transcriptLoadedRef.current.delete(id);
      for (const from of redirectTo.keys()) transcriptLoadedRef.current.delete(from);
      dropSessionRails([...importedIds, ...redirectTo.keys()]);
      setAvailableSessions((current) => [
        ...current.filter((s) => s.projectId !== projectId),
        ...result.sessions,
      ]);
      setOpenSessionIds((current) => {
        const next: string[] = [];
        for (const id of current) {
          const mapped = redirectTo.get(id) ?? id;
          if (!next.includes(mapped)) next.push(mapped);
        }
        return next;
      });
      const activeId = currentSessionIdRef.current;
      const mappedActive = (activeId && redirectTo.get(activeId)) || activeId;
      if (mappedActive && mappedActive !== activeId) {
        setCurrentSessionId(mappedActive);
        void loadSessionTranscriptRef.current(mappedActive);
      } else if (activeId && importedIds.has(activeId)) {
        void loadSessionTranscriptRef.current(activeId);
      }
    } catch (error) {
      pushDebug({
        sessionId: "",
        level: "warn",
        source: "shelf",
        summary: "refresh CLI sessions failed",
        detail: error instanceof Error ? error.message : String(error),
      });
    } finally {
      setExternalScanning(false);
    }
  }, [pushDebug]);

  const applyFocusedSession = (nextSession: Session) => {
    setCurrentProjectId(nextSession.projectId);
    setCurrentSessionId(nextSession.id);
    // Selecting any conversation switches the center back to its flow — the
    // 收纳盒 tab itself stays open in the strip.
    setMemoActive(false);
    setViewMode("clean");
    setSessionCapabilities(capsByIdRef.current[nextSession.id] ?? null);
    setActiveModelId(null);
    lastProviderProbeKey.current = "";
    void loadSessionTranscript(nextSession.id);
  };

  const openSession = (nextSession: Session) => {
    if (!IS_DETACHED_WINDOW && detachedOwnedIdsRef.current.has(nextSession.id)) {
      void focusDetachedWindow(nextSession.id);
      return;
    }
    const tree = splitStateRef.current;
    if (!IS_DETACHED_WINDOW && leafCount(tree) > 1) {
      commitSplit(focusSession(tree, nextSession.id));
      applyFocusedSession(nextSession);
      return;
    }
    setOpenSessionIds((current) => current.includes(nextSession.id) ? current : [...current, nextSession.id]);
    applyFocusedSession(nextSession);
  };
  openSessionRef.current = openSession;

  /** Open a chat session from the Chat section. */
  const openChatSession = useCallback((session: Session) => {
    openSessionRef.current(session);
  }, []);

  /** Tab / shelf 「在右侧打开」/「在下方打开」. Only-tab of the target pane is a no-op. */
  const handleOpenBeside = (session: Session, edge: DropEdge) => {
    if (IS_DETACHED_WINDOW) return;
    if (detachedOwnedIdsRef.current.has(session.id)) {
      void focusDetachedWindow(session.id);
      return;
    }
    const tree =
      leafCount(splitStateRef.current) > 1
        ? splitStateRef.current
        : migrateFromFlat(openSessionIdsRef.current, currentSessionIdRef.current);
    const source = paneOfSession(tree, session.id);
    const targetId = source?.id ?? tree.focusedPaneId;
    const next = splitPane(tree, targetId, edge, session.id, { maxLeaves: MAX_PANES });
    if (next === tree) return;
    commitSplit(next);
    pushDebug({
      sessionId: session.id,
      level: "info",
      source: "split",
      summary: `open-beside edge=${edge} pane=${targetId}`,
    });
  };

  /** Create a new Chat session in the current/default working folder. `origin:
   *  "memo"` files it under the shelf's 收纳盒 group. */
  const handleCreateChatSession = useCallback(async (agentId?: string, opts?: { origin?: "memo" }) => {
    if (createChatInFlightRef.current) return createChatInFlightRef.current;
    const work = (async (): Promise<Session | undefined> => {
      try {
        const resolvedAgentId =
          typeof agentId === "string" && agentId
            ? agentId
            : (availableAgents[0]?.id ?? agents[0]!.id);
        const cwd = defaultFolderPath || (await getDefaultFolder());
        const session = await createChatSession(resolvedAgentId, "新对话", cwd || null, opts?.origin);
        if (!session) return undefined;
        if (!defaultFolderPath && session.cwd) {
          setDefaultFolderPath(session.cwd);
        }
        const last = getLastUsedDefaults(resolvedAgentId);
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
        transferEmptyDraft(sessionWithPrefs.id);
        startupCreatedChatsRef.current = [
          sessionWithPrefs,
          ...startupCreatedChatsRef.current.filter((item) => item.id !== sessionWithPrefs.id),
        ];
        setAvailableSessions((current) => [
          sessionWithPrefs,
          ...current.filter((item) => item.id !== sessionWithPrefs.id),
        ]);
        openSessionRef.current(sessionWithPrefs);
        return sessionWithPrefs;
      } catch (error) {
        pushDebug({
          level: "error",
          source: "chat",
          summary: "create chat failed",
          detail: error instanceof Error ? error.message : String(error),
        });
        return undefined;
      }
    })();
    createChatInFlightRef.current = work;
    try {
      return await work;
    } finally {
      createChatInFlightRef.current = null;
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

  // ── 再次进入对话（agent re-entry, lib/agentReentry.ts）──────────────────
  // The agent resumed on its own after its turn ended (a background command
  // came back). Marionette re-arms the live-turn state so the Working bar,
  // shelf pulse and live timeline come back; the segment ends on turn/complete
  // (adapters that do send a boundary) or on quiet (the rest).
  const finalizeAgentReentryRef = useRef<(sessionId: string, reason: "idle") => void>(
    () => undefined,
  );

  const armAgentReentry = useCallback((sessionId: string) => {
    const now = Date.now();
    const existing = agentReentryRef.current[sessionId];
    if (existing?.timer) clearTimeout(existing.timer);
    if (!existing) {
      setSessionStatusById(sessionId, "running");
      pushDebug({
        sessionId,
        level: "info",
        source: "acp",
        summary: "agent re-entry armed (post-turn continuation)",
      });
    }
    const timer = setTimeout(() => {
      finalizeAgentReentryRef.current(sessionId, "idle");
    }, REENTRY_IDLE_MS);
    agentReentryRef.current[sessionId] = {
      startedAt: existing?.startedAt ?? now,
      lastAt: now,
      timer,
    };
  }, [pushDebug, setSessionStatusById]);
  const armAgentReentryRef = useRef(armAgentReentry);
  armAgentReentryRef.current = armAgentReentry;

  /**
   * Quiet end of a re-entry segment (adapter never sent a boundary): seal the
   * tail card like turn/complete does, free the composer and alert the same
   * way a completed turn does. A later chunk simply arms a new segment.
   */
  const finalizeAgentReentry = (sessionId: string, reason: "idle") => {
    const armed = agentReentryRef.current[sessionId];
    if (!armed) return;
    if (armed.timer) clearTimeout(armed.timer);
    delete agentReentryRef.current[sessionId];
    const prev = sessionsRef.current.find((s) => s.id === sessionId);
    if (!prev) return; // deleted while armed — nothing to seal or announce
    const endedAtMs = Date.now();
    updateSessionEvents(
      sessionId,
      (current) =>
        collapseIntermediateAssistantAsThought(
          sealAgentSegmentForIdle(current, sessionId, endedAtMs),
          sessionId,
        ),
      { publish: true },
    );
    // The re-entry still owns the running state only if no prompt took over
    // since (a send disarms first; its own lifecycle owns the status then).
    if (prev.status !== "running") return;
    setSessionStatusById(sessionId, "waiting");
    const label = prev.label?.trim() || "Session";
    const stoppedSilently = turnEndedSilently(
      shadowBySessionRef.current.get(sessionId) ?? EMPTY_EVENTS,
      sessionId,
    );
    scheduleDesktopNotify(
      sessionId,
      stoppedSilently ? "idle" : "reply",
      label,
      DESKTOP_NOTIFY_SETTLE_MS,
      { audibleWhenFocused: true },
    );
    if (sessionId !== currentSessionIdRef.current) {
      setDoneUnreadIds((current) => {
        if (current.has(sessionId)) return current;
        const next = new Set(current);
        next.add(sessionId);
        return next;
      });
    }
    pushDebug({
      sessionId,
      level: "info",
      source: "acp",
      summary: `agent re-entry ended (${reason})`,
    });
  };
  finalizeAgentReentryRef.current = finalizeAgentReentry;

  /**
   * Merge a Composer prefs patch onto the dialog row, publish it to the UI and
   * write it to disk.
   *
   * The row is read from `sessionsRef` (latest render), not from inside a
   * `setAvailableSessions` updater: React may call an updater after this
   * function returns, and capturing the merged value from within it silently
   * dropped the disk write whenever the update was batched.
   */
  const persistComposerPrefs = useCallback(
    (sid: string, patch: SessionComposerPrefs) => {
      if (!sid || sid.startsWith("session-empty-")) return;
      const row = sessionsRef.current.find((session) => session.id === sid);
      if (!row) return;
      const merged: SessionComposerPrefs = {
        preferredModel:
          patch.preferredModel !== undefined ? patch.preferredModel : row.preferredModel,
        preferredMode:
          patch.preferredMode !== undefined ? patch.preferredMode : row.preferredMode,
        preferredEffort:
          patch.preferredEffort !== undefined ? patch.preferredEffort : row.preferredEffort,
        preferredEffortId:
          patch.preferredEffortId !== undefined ? patch.preferredEffortId : row.preferredEffortId,
        preferredAlwaysApprove:
          patch.preferredAlwaysApprove !== undefined
            ? patch.preferredAlwaysApprove
            : row.preferredAlwaysApprove,
      };
      setAvailableSessions((current) =>
        current.map((session) => (session.id === sid ? { ...session, ...merged } : session))
      );
      void updateSessionPrefs(sid, merged).catch((error) => {
        pushDebug({
          sessionId: sid,
          level: "warn",
          source: "session",
          summary: "persist composer prefs failed",
          detail: error instanceof Error ? error.message : String(error),
        });
      });
    },
    [pushDebug],
  );

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
        agents[0]!;
      if (agent.transport !== "acp") return null;

      // Already bootstrapped?
      const existing = await getSessionCapabilities(sessionId);
      if (existing) {
        const merged = mergeAcpCapabilities(agent.id, existing) ?? existing;
        setCapsById((current) => ({ ...current, [sessionId]: merged }));
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
          setCapsById((current) => ({ ...current, [sessionId]: merged }));
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

  const warmAcp = useCallback((sessionId?: string) => {
    const sid = sessionId ?? currentSessionIdRef.current;
    if (!sid) return;
    const session = sessionsRef.current.find((s) => s.id === sid);
    const agent = agentsRef.current.find((a) => a.id === session?.agentId);
    if (agent?.transport !== "acp") return;
    void ensureAcpReady(sid).catch(() => undefined);
  }, [ensureAcpReady]);

  /**
   * Hidden turn: an agent-side slash replay (Grok `/always-approve`) still needs
   * the connection's one turn slot. Marking the session `running` is the whole
   * point — the composer's queue guard, the Working bar and the turn/complete
   * bookkeeping all key off status. Without it the replay was invisible while
   * `acp.rs` refused the user's next send as "already in flight".
   *
   * No user card / history injection / file snapshot: this is a preference sync,
   * not something the user said.
   */
  const sendSilentPrompt = useCallback(
    async (sessionId: string, text: string) => {
      await ensureAcpReady(sessionId);
      setSessionStatusById(sessionId, "running");
      touchActivity(sessionId);
      pushDebug({
        sessionId,
        level: "info",
        source: "acp",
        summary: `silent prompt: ${text}`,
      });
      try {
        silentTurnRef.current.add(sessionId);
        return await sendAcpPrompt(sessionId, text);
      } catch (error) {
        // Hand the slot back — nothing is running, so "Working" would lie.
        silentTurnRef.current.delete(sessionId);
        setSessionStatusById(sessionId, "waiting");
        throw error;
      }
    },
    [ensureAcpReady, pushDebug, setSessionStatusById, touchActivity]
  );

  const closeSessionTab = (sessionId: string) => {
    lastClosedTabsRef.current = [
      sessionId,
      ...lastClosedTabsRef.current.filter((id) => id !== sessionId),
    ].slice(0, 20);
    saveClosedTabs(lastClosedTabsRef.current);
    const tree = splitStateRef.current;
    if (!IS_DETACHED_WINDOW && leafCount(tree) > 1) {
      const result = closeTab(tree, sessionId);
      if (result.needPlaceholderChat) {
        commitSplit(result.state);
        const closing = availableSessions.find((session) => session.id === sessionId);
        if (closing?.projectId === CHAT_PROJECT_ID || currentProjectId === CHAT_PROJECT_ID) {
          void handleCreateChatSession();
        } else {
          void createSessionForProject(currentProject?.id ?? availableProjects[0]?.id ?? "");
        }
        return;
      }
      commitSplit(result.state);
      void pruneColdSessionMemory();
      return;
    }
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
        next.splice(insertAt, 0, item!);
        void reorderProjectsApi(next.map((p) => p.id)).catch(() => {
          void listProjects().then((projects) => {
            const visible = withoutGhostProjects(projects);
            if (visible.length > 0) setAvailableProjects(visible);
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
    const pending = persistableEventsForSession(
      shadowBySessionRef.current.get(session.id) ?? EMPTY_EVENTS,
      session.id,
    );
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
    const tree = splitStateRef.current;
    if (!IS_DETACHED_WINDOW && leafCount(tree) > 1) {
      const result = popOutTab(tree, session.id);
      if (result.needPlaceholderChat) {
        commitSplit(result.state);
        if (session.projectId === CHAT_PROJECT_ID) {
          void handleCreateChatSession();
        } else {
          void createSessionForProject(
            session.projectId || currentProject?.id || availableProjects[0]?.id || "",
          );
        }
        return;
      }
      commitSplit(result.state);
      return;
    }
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
    const events = persistableEventsForSession(
      shadowBySessionRef.current.get(session.id) ?? EMPTY_EVENTS,
      session.id,
    );
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
    dropSessionRails([sessionId]);
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

  // Spec-card action buttons (```spec fences, SpecCard.tsx) only dispatch this
  // event; routing the prompt into the composer happens here. Prefill, never
  // auto-send — the user stays in control of what goes to the agent.
  useEffect(() => {
    const onSpecAction = (event: Event) => {
      const prompt = (event as CustomEvent<string>).detail;
      if (typeof prompt === "string" && prompt.trim()) prefillComposer(prompt);
    };
    window.addEventListener(SPEC_ACTION_EVENT, onSpecAction);
    return () => window.removeEventListener(SPEC_ACTION_EVENT, onSpecAction);
  }, [prefillComposer]);

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
    const events = sid
      ? (shadowBySessionRef.current.get(sid) ?? EMPTY_EVENTS)
      : EMPTY_EVENTS;
    for (let i = events.length - 1; i >= 0; i--) {
      const e = events[i]!;
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
    const requestId = permissionPrompt.requestId;
    const sessionId = permissionPrompt.sessionId;
    try {
      await respondAcpPermission(requestId, optionId);
      pushDebug({
        sessionId,
        level: "info",
        source: "permission",
        summary: `permission ${optionId}`,
        detail: permissionPrompt.title,
      });
      setPermissionPrompt(null);
    } catch (error) {
      pushDebug({
        sessionId,
        level: "error",
        source: "permission",
        summary: "respond permission failed",
        detail: error instanceof Error ? error.message : String(error),
      });
    } finally {
      setPermissionBusy(false);
    }
  }, [permissionPrompt, pushDebug]);

  const handleAskSubmit = useCallback(
    async (answers: { question: string; selected: string[] }[]) => {
      if (!askPrompt) return;
      setAskBusy(true);
      const requestId = askPrompt.requestId;
      const sessionId = askPrompt.sessionId;
      try {
        await respondAcpQuestion(requestId, answers, false);
        updateSessionEvents(
          sessionId,
          (current) =>
            patchPromptEvent(current, requestId, {
              status: "answered",
              answers,
            }),
          { publish: true },
        );
        setAskPrompt(null);
        pushDebug({
          sessionId,
          level: "info",
          source: "question",
          summary: `answered ${answers.length} question(s)`,
        });
      } catch (error) {
        pushDebug({
          sessionId,
          level: "error",
          source: "question",
          summary: "respond question failed",
          detail: error instanceof Error ? error.message : String(error),
        });
      } finally {
        setAskBusy(false);
      }
    },
    [askPrompt, pushDebug],
  );

  const handleAskDecline = useCallback(async () => {
    if (!askPrompt) return;
    setAskBusy(true);
    const requestId = askPrompt.requestId;
    const sessionId = askPrompt.sessionId;
    try {
      await respondAcpQuestion(requestId, [], true);
      updateSessionEvents(
        sessionId,
        (current) =>
          patchPromptEvent(current, requestId, {
            status: "declined",
          }),
        { publish: true },
      );
      setAskPrompt(null);
    } catch (error) {
      pushDebug({
        sessionId,
        level: "error",
        source: "question",
        summary: "decline question failed",
        detail: error instanceof Error ? error.message : String(error),
      });
    } finally {
      setAskBusy(false);
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
      const requestId = planApproval.requestId;
      const sessionId = planApproval.sessionId;
      try {
        await respondAcpPlanApproval(
          requestId,
          decision,
          feedback ?? null,
        );
        updateSessionEvents(
          sessionId,
          (current) =>
            patchPromptEvent(current, requestId, {
              status: decision === "abandon" ? "declined" : "answered",
              decision,
              decisionLabel: planDecisionLabel(decision),
              ...(feedback?.trim() ? { feedback: feedback.trim() } : {}),
            }),
          { publish: true },
        );
        pushDebug({
          sessionId,
          level: "info",
          source: "plan",
          summary: `plan approval: ${decision}`,
          detail: feedback?.trim() || undefined,
        });
        setPlanApproval(null);

        if (decision === "request_changes" && feedback?.trim()) {
          const sid = sessionId;
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

  const handlePlanPermissionChoose = useCallback(
    async (optionId: string) => {
      if (!planApproval) return;
      setPlanApprovalBusy(true);
      const requestId = planApproval.requestId;
      const sessionId = planApproval.sessionId;
      const option = (planApproval.permissionOptions ?? []).find((o) => o.optionId === optionId);
      try {
        await respondAcpPermission(requestId, optionId);
        updateSessionEvents(
          sessionId,
          (current) =>
            patchPromptEvent(current, requestId, {
              status: "answered",
              decision: optionId,
              decisionLabel: planDecisionLabel(optionId, planApproval.permissionOptions),
            }),
          { publish: true },
        );
        setPlanApproval(null);
        pushDebug({
          sessionId,
          level: "info",
          source: "permission",
          summary: `plan permission ${optionId}`,
          detail: option?.name || planApproval.title,
        });
      } catch (error) {
        pushDebug({
          sessionId,
          level: "error",
          source: "permission",
          summary: "respond plan permission failed",
          detail: error instanceof Error ? error.message : String(error),
        });
      } finally {
        setPlanApprovalBusy(false);
      }
    },
    [planApproval, pushDebug],
  );

  const handleAgentChange = async (agentId: string, sessionId?: string) => {
    const sid = sessionId ?? currentSessionId;
    const target = sessionsRef.current.find((s) => s.id === sid);
    // If no current session, create one bound to this agent
    if (!target || !sid || sid.startsWith("session-empty-")) {
      if (currentProject) {
        void createSessionForProject(currentProject.id, agentId);
      } else {
        setComposerAutoFocus(true);
        void handleCreateChatSession(agentId);
      }
      return;
    }

    if (target.agentId === agentId) return;

    // HARD RULE: session.agentId is the only source of truth for which agent
    // owns this dialog. Persist it so reopening the tab cannot show OpenCode
    // while still listing Claude models.
    const sourceAgentId = target.agentId;
    const oldAgent = availableAgents.find((a) => a.id === sourceAgentId);

    // Mid-turn switch: cancel the open prompt first so the old process does
    // not keep running after we tear it down (and so turn/complete frees UI).
    const midTurn =
      target.status === "running" || target.status === "starting";
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
    const sealedLive = updateSessionEvents(
      sid,
      (current) => sealOpenAssistantReplies(current, sid),
      { publish: true },
    );

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
        projectId: target.projectId,
        sessionId: sid,
        targetAgentId: agentId,
        sourceAgentId,
      });
      if (handoff) {
        setLastHandoff(handoff);
        // Never prefill the composer: the notes ride along with the next message
        // the user actually sends (see pendingHandoffPrompt in handleSend).
        updateSessionEvents(sid, (current) => [
          ...sealOpenAssistantReplies(current, sid),
          {
            type: "handoff_prepared" as const,
            sessionId: sid,
            targetAgentId: agentId,
            handoffPath: handoff.handoffPath,
            prompt: handoff.prompt,
            createdAt: new Date().toISOString(),
          },
        ], { publish: true });
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
    updateSessionEvents(
      sid,
      (current) => expirePendingPrompts(current, sid, "timeout"),
      { publish: true },
    );
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
      updateSessionEvents(sid, (current) => [
        ...current,
        {
          type: "assistant_message" as const,
          sessionId: sid,
          text:
            "**Reconnected.**\n\nMCP servers were re-negotiated for this session, so anything that started after the agent (Unity, UE, …) should be attached now. Your conversation is kept — it is replayed to the new session with your next message.",
          createdAt: new Date().toISOString(),
        },
      ], { publish: true });
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
   * After native login, the live ACP child still holds the old auth. Current
   * session gets a fresh `session/new`; other same-agent sessions stop so the
   * next send respawns. Do not clear current-session caps for those others.
   */
  const reconnectSessionsAfterLogin = useCallback(
    async (agentId: string) => {
      const currentSid = currentSessionIdRef.current;
      const rows = sessionsRef.current.filter(
        (s) => s.agentId === agentId && !s.id.startsWith("session-empty-")
      );
      const current = rows.find((s) => s.id === currentSid);
      const others = rows.filter((s) => s.id !== currentSid);
      for (const s of others) {
        disarmCancelWatchdog(s.id);
        cancelIgnoredRef.current.delete(s.id);
        await stopAcpSession(s.id).catch(() => undefined);
        acpBootstrapRef.current.delete(s.id);
        setSessionStatusById(s.id, "exited");
        acpNeedsHistoryRef.current.add(s.id);
      }
      if (current) {
        await restartAcpProcess(current.id);
        await ensureAcpReady(current.id);
      }
      pushDebug({
        sessionId: current?.id,
        level: "info",
        source: "auth",
        summary: `reconnected ${agentId} after login`,
      });
    },
    [disarmCancelWatchdog, ensureAcpReady, pushDebug, restartAcpProcess, setSessionStatusById]
  );

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
      const baseline = await probeAgentAuth(agentId, modelIdForAgentProbe(agentId));
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
        result.message || "请在浏览器/终端完成登录，然后回到这里（会自动检测并重连）。"
      );
      if (authPollRef.current) {
        clearInterval(authPollRef.current);
        authPollRef.current = null;
      }
      let ticks = 0;
      let inFlight = false;
      let settled = false;
      const loginSucceeded = (probe: { status: string; stamp?: number }) => {
        const becameIn =
          probe.status === "logged_in" && baseline?.status !== "logged_in";
        const stampChanged =
          probe.stamp != null && probe.stamp !== baseline?.stamp;
        return becameIn || stampChanged;
      };
      const finish = async (reconnect: boolean) => {
        if (settled) return;
        settled = true;
        if (authPollRef.current) {
          clearInterval(authPollRef.current);
          authPollRef.current = null;
        }
        if (reconnect) {
          try {
            await reconnectSessionsAfterLogin(agentId);
          } catch (error) {
            pushDebug({
              level: "warn",
              source: "auth",
              summary: `reconnect after ${agentId} login failed`,
              detail: error instanceof Error ? error.message : String(error),
            });
          }
        }
        await refreshAgentAuth(agentId);
        setSignInBusy(false);
      };
      const tick = () => {
        if (inFlight || settled) return;
        inFlight = true;
        void (async () => {
          try {
            const probe = await probeAgentAuth(
              agentId,
              modelIdForAgentProbe(agentId)
            );
            if (settled) return;
            if (!probe) {
              if (ticks >= 40) await finish(false);
              return;
            }
            if (loginSucceeded(probe)) {
              await finish(true);
              return;
            }
            if (ticks >= 40) {
              await finish(probe.status === "logged_in");
            }
          } finally {
            inFlight = false;
          }
        })();
      };
      tick();
      authPollRef.current = setInterval(() => {
        ticks += 1;
        tick();
      }, 3000);
    },
    [
      modelIdForAgentProbe,
      pushDebug,
      reconnectSessionsAfterLogin,
      refreshAgentAuth,
      setAuthHintFor,
    ]
  );

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
      updateSessionEvents(sid, (current) => [
        ...current,
        {
          type: "assistant_message" as const,
          sessionId: sid,
          text:
            "**The agent did not answer the cancel.**\n\nIt has stayed silent since the interrupt, so the turn is stuck inside the agent process — a hung tool or model call, which nothing on this side can reach.\n\nSend your next message as usual: Marionette will replace the agent process first and replay this conversation to the new one.",
          createdAt: new Date().toISOString(),
        },
      ], { publish: true });
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
      updateSessionEvents(sid, (current) =>
        sealOpenAssistantReplies(markOpenTools(current, sid, "cancelled"), sid),
      { publish: true });

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
        updateSessionEvents(sid, (current) => [
          ...current,
          {
            type: "assistant_message" as const,
            sessionId: sid,
            text: `**Interrupted.**\n\n${cancelNote}\n\nYou can send a new message now.`,
            createdAt: new Date().toISOString(),
            durationMs: 0,
          },
        ], { publish: true });
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
      // 经 ref 读最新提示卡状态：监听器挂一次，不再随提示出现/消失反复卸载重挂。
      const {
        permissionPrompt,
        askPrompt,
        planApproval,
        diffPreview,
        projectDialogOpen,
        projectAdding,
      } = escPromptRef.current;

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
        const reject = (planApproval.permissionOptions ?? []).find((o) =>
          /reject|deny|cancel|no[, ]|keep planning/i.test(`${o.kind} ${o.name} ${o.optionId}`),
        );
        if (reject) {
          void handlePlanPermissionChoose(reject.optionId);
        } else if (planApproval.permissionOptions?.length) {
          const last = planApproval.permissionOptions[planApproval.permissionOptions.length - 1]!;
          void handlePlanPermissionChoose(last.optionId);
        } else {
          void handlePlanApproval("abandon");
        }
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
    handleInterrupt,
    handlePermissionChoose,
    handleAskDecline,
    handlePlanApproval,
    handlePlanPermissionChoose,
  ]);

  // Ctrl/Cmd+K toggles the palette; Ctrl/Cmd+1..9 jumps to a tab; Ctrl/Cmd+Shift+T
  // reopens the last closed one.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (!(event.ctrlKey || event.metaKey) || event.altKey) return;
      if (!event.shiftKey && (event.key === "k" || event.key === "K") && !IS_DETACHED_WINDOW) {
        event.preventDefault();
        setPaletteOpen((open) => !open);
        return;
      }
      if (event.key >= "1" && event.key <= "9" && !event.shiftKey) {
        const idx = Number(event.key) - 1;
        const tree = splitStateRef.current;
        const paneTabs = findPane(tree, tree.focusedPaneId)?.tabIds ?? [];
        const tabIds = paneTabs.length > 0 ? paneTabs : openSessionIds;
        const tabs = tabIds
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

      // Cancel in-flight turn first. Suppress late chunks until the new prompt
      // goes out, or they would land after the resent You card.
      if (session.status === "running") {
        streamSuppressedRef.current.add(sid);
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

      const current = shadowBySessionRef.current.get(sid) ?? EMPTY_EVENTS;
      const cut = findCutIndex(current);
      if (cut < 0) {
        streamSuppressedRef.current.delete(sid);
        pushDebug({
          sessionId: sid,
          level: "warn",
          source: "edit-resend",
          summary: "could not find message to edit",
        });
        return;
      }
      const kept = current.slice(0, cut);
      const original = current[cut];
      const fromOriginal =
        original?.type === "user_message"
          ? {
              agentId: original.agentId,
              agentLabel: original.agentLabel,
              modelId: original.modelId,
              modelLabel: original.modelLabel,
              modeLabel: original.modeLabel,
              effortLabel: original.effortLabel,
            }
          : {};
      const sendMeta = {
        agentId: fromOriginal.agentId || session.agentId,
        agentLabel: fromOriginal.agentLabel || agent.label,
        modelId: fromOriginal.modelId || session.preferredModel || undefined,
        modelLabel: fromOriginal.modelLabel,
        modeLabel: fromOriginal.modeLabel,
        effortLabel: fromOriginal.effortLabel,
      };
      // sendMetaRef is cleared on the first assistant chunk of a turn, so the
      // resend path must not read it — that is how later Replies lost agentId
      // and rendered as Notification.
      sendMetaRef.current = sendMeta;
      delete turnStartedAtRef.current[sid];
      disarmAgentReentry(sid);
      turnSentAtRef.current[sid] = Date.now();
      grokTurnUsageRef.current[sid] = emptyGrokTurnUsage();
      cancelScheduledDesktopNotify(sid);
      const nextEvents = [
        ...kept,
        userMessageEvent(sid, newText.trim(), {
          ...sendMeta,
          sentAt: new Date(turnSentAtRef.current[sid]).toISOString(),
        }),
      ];
      updateSessionEvents(sid, () => nextEvents, { publish: true });

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
        streamSuppressedRef.current.delete(sid);
        await sendAcpPrompt(sid, promptText);
      } catch (error) {
        streamSuppressedRef.current.delete(sid);
        setSessionStatusById(sid, "error");
        const sess = sessionsRef.current.find((s) => s.id === sid);
        const agent = sess
          ? agentsRef.current.find((a) => a.id === sess.agentId)
          : undefined;
        const classified = classifyAgentError(error, {
          agentId: sess?.agentId,
          agentLabel: agent?.label,
          currentModel:
            (sid === currentSessionIdRef.current
              ? activeModelIdRef.current
              : null) ||
            sess?.preferredModel ||
            capsByIdRef.current[sid]?.currentModel ||
            undefined,
        });
        raiseClassifiedBanner(sess?.agentId, classified);
        setComposerFailure({ sessionId: sid, error: classified });
        updateSessionEvents(sid, (events) => [
          ...events,
          {
            type: "assistant_message" as const,
            sessionId: sid,
            text: formatClassifiedError(classified),
            createdAt: new Date().toISOString(),
          },
        ], { publish: true });
      }
    },
    [ensureAcpReady, pushDebug, raiseClassifiedBanner, renameSessionFromText, setSessionStatusById]
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

      const events = shadowBySessionRef.current.get(childId) ?? EMPTY_EVENTS;
      let summary = "";
      for (let i = events.length - 1; i >= 0; i--) {
        const e = events[i]!;
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
      updateSessionEvents(meta.parentId, (current) => [...current, result], { publish: true });

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
        updateSessionEvents(parentId, (current) => [
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
        ], { publish: true });
        return;
      }

      // Install check
      try {
        const statuses = await listAgentCommands();
        const st = statuses.find((s) => s.id === agentId);
        if (st && st.status !== "installed") {
          updateSessionEvents(parentId, (current) => [
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
          ], { publish: true });
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
        updateSessionEvents(parentId, (current) => [
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
        ], { publish: true });
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
      updateSessionEvents(parentId, (current) => [...current, started], { publish: true });

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
        updateSessionEvents(parentId, (current) => [
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
        ], { publish: true });
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
    opts?: ComposerChipSnapshot,
    sessionId?: string,
  ) => {
    opts = { ...opts, filePaths: [...new Set(droppedPaths)] };
    let sid = sessionId ?? currentSessionId;
    const row = sessionsRef.current.find((s) => s.id === sid);
    if (!sid || sid.startsWith("session-empty-")) {
      setComposerAutoFocus(true);
      const created = await handleCreateChatSession(row?.agentId ?? displaySession.agentId);
      if (!created) return;
      sid = created.id;
    }

    const sess = sessionsRef.current.find((s) => s.id === sid) ?? row;
    const projectId = sess?.projectId || currentProjectId;

    // @-delegate: line-start @agent task — does not block the parent dialog.
    // Images on a delegate line are ignored for now (depth=1, simple task text).
    const knownIds = availableAgents.map((a) => a.id);
    const delegated = parseDelegateLine(text, knownIds);
    if (delegated) {
      setQuotePinsById((current) => {
        if (!(sid in current)) return current;
        const next = { ...current };
        delete next[sid];
        return next;
      });
      await handleDelegate(sid, projectId, delegated);
      return;
    }

    // Merge quote pins + image mark text + free Composer text.
    const pins = quotePinsById[sid] ?? [];
    let composed = pins.length > 0 ? formatPinsForSend(pins, text) : text;
    const markBlock = formatImageMarksForSend(imageAttachments);
    if (markBlock) {
      composed = composed.trim()
        ? `${markBlock}\n\n${composed.trim()}`
        : markBlock;
    }
    if (!composed.trim() && imageAttachments.length === 0) return;
    // Image attachments are inlined as ACP `ContentBlock::Image` (base64);
    // the agent never reads those files from disk, so their folders must
    // not trigger a workspace grant / session/new. DSH rejects a non-empty
    // `additionalDirectories` list.
    const candidates = [
      ...new Set([
        ...droppedPaths,
        ...findLinkTargets(composed)
          .filter((target) => target.kind === "path")
          .map((target) => target.raw),
      ]),
    ];
    // Chat has a real cwd but no project-level `.marionette` scope. Its agent
    // is already started in that cwd, so do not ask to persist a project grant.
    const outside = (sess?.projectId ?? displaySession.projectId) === CHAT_PROJECT_ID
      ? []
      : await checkOutsideProjectPaths(projectId, candidates).catch(() => []);
    if (outside.length > 0) {
      setPathGrantPrompt({
        paths: outside,
        text: composed,
        sessionId: sid,
        imageAttachments,
        composerSnap: opts ? { ...opts } : undefined,
      });
      return;
    }

    setQuotePinsById((current) => {
      if (!(sid in current)) return current;
      const next = { ...current };
      delete next[sid];
      return next;
    });
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

  /**
   * `session/prompt` can be refused by the ACP concurrency gate ("a turn is
   * already in flight", `acp.rs::send_prompt`) when the connection's single turn
   * slot is held by a turn this side never saw start — a hidden slash replay, or
   * one whose `turn/complete` was missed. The user's card is already on screen,
   * so wait for the boundary and retry the wire write once rather than painting
   * an "Agent 错误" for something that is merely busy.
   */
  const sendPromptWithTurnWait = async (
    sid: string,
    text: string,
    imagePaths: string[],
    filePaths: string[] = [],
  ) => {
    try {
      return await sendAcpPrompt(sid, text, imagePaths, false, filePaths);
    } catch (error) {
      if (!isTurnInFlightError(error)) throw error;
      pushDebug({
        sessionId: sid,
        level: "warn",
        source: "composer",
        summary: "session/prompt refused — another turn holds the slot",
        detail: "waiting for the turn boundary, then retrying once",
      });
      const freed = await waitForTurnSlot(
        turnSlotWaitersRef.current,
        sid,
        TURN_SLOT_WAIT_MS
      );
      pushDebug({
        sessionId: sid,
        level: freed ? "info" : "warn",
        source: "composer",
        summary: freed
          ? "turn slot freed — retrying session/prompt"
          : `turn still in flight after ${Math.round(TURN_SLOT_WAIT_MS / 1000)}s — retrying anyway`,
      });
      return await sendAcpPrompt(sid, text, imagePaths, false, filePaths);
    }
  };

  const performSend = async (
    sid: string,
    composed: string,
    imageAttachments: ImageAttachment[] = [],
    composerSnap?: ComposerChipSnapshot,
    options?: {
      /** Flushing a queued follow-up: no card exists yet — create it now,
       *  right after the reply that just freed the slot. */
      flushQueued?: boolean;
      /** 远程发送（Telegram 入站）：没有 Composer 快照，也绝不能读主窗口
       *  Composer 的瞬态模型——分屏下 openSession 同步改写 currentSessionIdRef
       *  而 activeModelId 还是上一个 tab 的值，会把所有会话的模型串成一个
       *  并经 persistComposerPrefs 固化。模型只从会话自身偏好解析。 */
      remote?: boolean;
    },
  ) => {
    try {
      // An agent switch leaves handoff notes waiting — attach them to this send
      // (the composer stays clean; only the wire payload carries them).
      const handoff = pendingHandoff(
        shadowBySessionRef.current.get(sid) ?? EMPTY_EVENTS,
        sid,
      );

      const sendRow = sessionsRef.current.find((s) => s.id === sid);
      const sendAgent =
        agentsRef.current.find((a) => a.id === sendRow?.agentId) ??
        agentsRef.current[0] ??
        agents[0]!;
      // Prefer Composer chip snapshot (what the user saw at send). Caps.currentMode
      // lags on purpose after a mode switch — agent often still echoes the old mode.
      const caps = capsByIdRef.current[sid] ?? (sid === currentSessionIdRef.current ? sessionCapabilities : null);
      const modeIdRaw =
        composerSnap?.modeId?.trim() ||
        sendRow?.preferredMode?.trim() ||
        caps?.currentMode ||
        null;
      const modeId = modeIdRaw
        ? normalizeAgentModeId(sendAgent.id, modeIdRaw)
        : null;
      const modeLabel =
        composerSnap?.modeLabel?.trim() ||
        (modeId ? caps?.modes.find((m) => m.id === modeId)?.label ?? modeId : undefined);
      const modelId =
        composerSnap?.modelId?.trim() ||
        (!options?.remote && sid === currentSessionIdRef.current ? activeModelId : null) ||
        sendRow?.preferredModel?.trim() ||
        caps?.currentModel ||
        null;
      const modelLabel =
        composerSnap?.modelLabel?.trim() ||
        (modelId ? caps?.models.find((m) => m.id === modelId)?.label ?? modelId : undefined);
      // The chip's effort, not just the disk pref: a dialog that inherited its
      // effort never had a pref written, and the send would silently go out on
      // the agent default while the chip showed something else.
      const effortId =
        composerSnap?.effortId?.trim() ||
        sendRow?.preferredEffortId?.trim() ||
        null;
      const sendEffort =
        typeof composerSnap?.effort === "number" && Number.isFinite(composerSnap.effort)
          ? composerSnap.effort
          : typeof sendRow?.preferredEffort === "number"
            ? sendRow.preferredEffort
            : null;
      const effortLabelVal =
        composerSnap?.effortLabel?.trim() ||
        (effortId
          ? prettyEffortLabel(
              caps?.effortOptions?.find((o) => o.id === effortId)?.label,
              effortId,
            )
          : sendEffort != null
            ? effortLabel(sendEffort)
            : undefined);
      sendMetaRef.current = {
        agentId: sendAgent.id,
        agentLabel: sendAgent.label,
        modelId: modelId ?? undefined,
        modelLabel: modelLabel ?? undefined,
        modeLabel: modeLabel ?? undefined,
        effortLabel: effortLabelVal || undefined,
      };
      delete turnStartedAtRef.current[sid];
      // A user prompt takes over the turn lifecycle — any armed re-entry
      // segment must stop owning the status (its quiet timer must not fire
      // mid-prompt and free the composer under a live turn).
      disarmAgentReentry(sid);

      // Show the user message immediately; wait for ACP only after that.
      // History injection uses events *before* this message.
      const priorForInject = shadowBySessionRef.current.get(sid) ?? EMPTY_EVENTS;
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
        queue.push({ composed, imageAttachments, composerSnap, remote: options?.remote });
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

      // The chips are this dialog's own choice even when they were inherited
      // (offline catalog / last-used defaults) and never explicitly picked here.
      // Persist what is actually being sent, or the next launch restores the
      // harness defaults and the selection looks forgotten.
      {
        const prefsPatch: SessionComposerPrefs = {};
        if (modelId) prefsPatch.preferredModel = modelId;
        if (modeId) prefsPatch.preferredMode = modeId;
        if (effortId) prefsPatch.preferredEffortId = effortId;
        if (sendEffort != null) prefsPatch.preferredEffort = sendEffort;
        if (Object.keys(prefsPatch).length > 0) persistComposerPrefs(sid, prefsPatch);
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
      updateSessionEvents(sid, (current) => [...current, um], { publish: true });
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
        const agentId = sendAgent.id;
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
          sendEffort != null &&
          liveCaps?.effortConfigId &&
          sendEffort !== liveCaps.currentEffort
        ) {
          await pushConfig({ thinkingEffort: sendEffort });
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
        const skillsPrefix = sendRow?.projectId === CHAT_PROJECT_ID
          ? null
          : await projectContextPrompt(
              sendRow?.projectId || currentProjectId,
              sendAgent.id,
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
      armSendSilenceWatchdog(sid);
      const imagePaths = imageAttachments.map((a) => a.path);
      const wireText = promptText;
      const projectId = sendRow?.projectId || currentProjectId;
      // Turn baseline for the end-of-turn delivery card: a worktree tree taken
      // now, so the card can show what *this* turn changed. It never delays the
      // prompt (the tree walk runs off the UI thread and is seeded from the
      // user's index, ~0.1-0.6 s), and a failed baseline publishes nothing
      // instead of the whole dirty workspace (2026-10-23).
      if (sendRow?.projectId !== CHAT_PROJECT_ID && projectId) {
        startTurnBaseline(sid, projectId);
      }
      await sendPromptWithTurnWait(sid, wireText, imagePaths, composerSnap?.filePaths);
      clearSendSilenceWatchdog(sid);
      touchActivity(sid);
      pushDebug({
        sessionId: sid,
        level: "info",
        source: "composer",
        summary: `session/prompt accepted (streaming…)${imagePaths.length ? ` images=${imagePaths.length}` : ""}`,
      });
    } catch (error) {
      clearSendSilenceWatchdog(sid);
      flushingSendRef.current.delete(sid);
      dropTurnBaseline(sid);
      const sess = sessionsRef.current.find((s) => s.id === sid);
      const agent = sess
        ? agentsRef.current.find((a) => a.id === sess.agentId)
        : undefined;
      const classified = classifyAgentError(error, {
        agentId: sess?.agentId,
        agentLabel: agent?.label,
        currentModel:
          composerSnap?.modelId?.trim() ||
          (!options?.remote && sid === currentSessionIdRef.current
            ? activeModelIdRef.current
            : null) ||
          sess?.preferredModel ||
          capsByIdRef.current[sid]?.currentModel ||
          undefined,
      });
      // The one-turn gate refused us: a live turn this side failed to bookkeep
      // as "running" (missed boundary, reconnect window) still owns the
      // connection. That is the queue condition, not a failure — route into
      // the standard pending queue (the flush after the turn boundary pushes
      // the You card itself, so drop the one this attempt painted) instead of
      // an error card + toast + "error" status, which historically dropped the
      // message AND un-gated every following send into the same refusal.
      if (classified.kind === "busy") {
        updateSessionEvents(sid, (current) => {
          for (let i = current.length - 1; i >= 0; i -= 1) {
            const e = current[i]!;
            if (e.type === "user_message" && e.text === composed) {
              return [...current.slice(0, i), ...current.slice(i + 1)];
            }
          }
          return current;
        }, { publish: true });
        const queue = pendingSendsRef.current.get(sid) ?? [];
        queue.push({ composed, imageAttachments, composerSnap, remote: options?.remote });
        pendingSendsRef.current.set(sid, queue);
        setQueuedStripTick((t) => t + 1);
        pushDebug({
          sessionId: sid,
          level: "info",
          source: "composer",
          summary: `queued after busy gate: ${composed.length > 80 ? `${composed.slice(0, 80)}…` : composed}`,
        });
        // Same contract as any mid-turn send: interrupt then send, so the
        // follow-up is wired as soon as the slot actually frees.
        void cancelLiveTurn(sid, { announce: false });
        return;
      }
      setSessionStatusById(sid, "error");
      const label = sess?.label?.trim() || "Session";
      const sendError = compactNotifyDetail(classified.message);
      void raiseDesktopNotify(
        "error",
        sendError ? `${label} · ${sendError}` : `${label} · Send failed`,
      );
      raiseClassifiedBanner(sess?.agentId, classified);
      setComposerFailure({ sessionId: sid, error: classified });
      updateSessionEvents(sid, (current) => [
        ...current,
        {
          type: "assistant_message" as const,
          sessionId: sid,
          text: formatClassifiedError(classified),
          createdAt: new Date().toISOString(),
        },
      ], { publish: true });
      pushDebug({
        sessionId: sid,
        level: "error",
        source: "composer",
        summary: `send failed (${classified.kind})`,
        detail: classified.message,
      });
    }
  };

  // Telegram 入站（手机发来的文字）：先把目标会话切到前台（回来电脑时看得见
  // 刚才在手机上驱动的对话），再走与本地 Composer 相同的 performSend 管线
  // （挂起重连、历史注入、排队、配置回推），完成后回执给 bot。
  // 注意传 remote: true——远程发送没有 Composer 快照，模型必须取会话自身
  // 偏好，不能读主窗口 Composer 的瞬态选择（否则会把别的 tab 的模型串进
  // 本会话并固化）。performSend 内部吞错，所以按会话状态判定成败。
  const telegramPerformSendRef = useRef(performSend);
  telegramPerformSendRef.current = performSend;
  useEffect(() => {
    if (!isTauriRuntime() || IS_DETACHED_WINDOW) return;
    let unlisten: (() => void) | undefined;
    void listen<TelegramInbound>(TELEGRAM_INBOUND_EVENT, (event) => {
      const payload = event.payload;
      if (!payload?.sessionId) return;
      const text = typeof payload.text === "string" ? payload.text.trim() : "";
      if (!text) return;
      const sid = payload.sessionId;
      const messageId = typeof payload.messageId === "number" ? payload.messageId : null;
      const session = sessionsRef.current.find((s) => s.id === sid);
      if (!session) {
        void telegramAck(false, "会话不存在（可能已被删除）", messageId, sid);
        return;
      }
      // 先把目标切到前台是给用户看的（电脑端能看到手机驱动的对话）；
      // 模型/模式解析走 remote 标记，与会话自身偏好走，不依赖这次切换。
      openSessionRef.current(session);
      pushDebug({
        sessionId: sid,
        level: "info",
        source: "telegram",
        summary: `inbound text from telegram · ${text.length} chars`,
      });
      void telegramPerformSendRef
        .current(sid, text, [], undefined, { remote: true })
        .then(() => {
          const failed =
            sessionsRef.current.find((s) => s.id === sid)?.status === "error";
          void telegramAck(
            !failed,
            failed ? "发送失败，详情在电脑上看" : null,
            messageId,
            sid,
          );
        })
        .catch((error) => {
          const message = error instanceof Error ? error.message : String(error);
          void telegramAck(false, message, messageId, sid);
        });
    }).then((fn) => {
      unlisten = fn;
    });
    return () => unlisten?.();
    // 只订阅一次；回调和会话查找全部走 ref。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Telegram 远程命令（/new 建会话、/model 切模型）：Rust 侧向导/菜单选完后
  // 发事件过来，前端走与本地完全相同的创建/偏好管线执行，再回执给 bot。
  const telegramCreateProjectSessionRef = useRef(createSessionForProject);
  telegramCreateProjectSessionRef.current = createSessionForProject;
  const telegramCreateChatSessionRef = useRef(handleCreateChatSession);
  telegramCreateChatSessionRef.current = handleCreateChatSession;
  const telegramPersistPrefsRef = useRef(persistComposerPrefs);
  telegramPersistPrefsRef.current = persistComposerPrefs;
  useEffect(() => {
    if (!isTauriRuntime() || IS_DETACHED_WINDOW) return;
    let unlisten: (() => void) | undefined;
    void listen<TelegramCommand>(TELEGRAM_COMMAND_EVENT, (event) => {
      const payload = event.payload;
      if (!payload?.kind) return;
      void (async () => {
        if (payload.kind === "newSession") {
          const modelId = typeof payload.modelId === "string" && payload.modelId ? payload.modelId : null;
          const modeId = typeof payload.modeId === "string" && payload.modeId ? payload.modeId : null;
          try {
            if (payload.chat) {
              // 聊天区：createChatSession 建行+打开（自动继承该 agent 最近
              // 的模型/档位），远程选中的项在返回的会话上覆盖落盘。
              const session = await telegramCreateChatSessionRef.current(payload.agentId);
              if (!session) {
                await telegramRemoteResult("没建成——聊天会话创建失败，详情在电脑上看");
                return;
              }
              if (modelId || modeId) {
                const merged = {
                  preferredModel: modelId ?? session.preferredModel ?? null,
                  preferredMode: modeId ?? session.preferredMode ?? null,
                  preferredEffort: session.preferredEffort ?? null,
                  preferredEffortId: session.preferredEffortId ?? null,
                  preferredAlwaysApprove: session.preferredAlwaysApprove ?? null,
                };
                setAvailableSessions((current) =>
                  current.map((s) =>
                    s.id === session.id
                      ? {
                          ...s,
                          preferredModel: merged.preferredModel,
                          preferredMode: merged.preferredMode,
                        }
                      : s,
                  ),
                );
                void updateSessionPrefs(session.id, merged).catch(() => undefined);
              }
              await telegramRemoteResult(`✅ 已新建 · ${session.label}`, session.id);
            } else {
              const session = await telegramCreateProjectSessionRef.current(
                payload.projectId || "",
                payload.agentId,
                undefined,
                modelId || modeId
                  ? {
                      preferredModel: modelId,
                      preferredMode: modeId,
                      preferredEffort: null,
                      preferredEffortId: null,
                      preferredAlwaysApprove: null,
                    }
                  : null,
              );
              if (!session) {
                await telegramRemoteResult("没建成——会话创建失败，详情在电脑上看");
                return;
              }
              await telegramRemoteResult(`✅ 已新建 · ${session.label}`, session.id);
            }
          } catch (error) {
            const message = error instanceof Error ? error.message : String(error);
            await telegramRemoteResult(`没建成：${message}`);
          }
          return;
        }
        if (payload.kind === "setModel") {
          const sid = payload.sessionId;
          const row = sessionsRef.current.find((s) => s.id === sid);
          if (!row) {
            await telegramRemoteResult("会话不存在（可能已被删除）");
            return;
          }
          // 偏好先落盘：下一次 performSend 的配置回推会带上它。
          telegramPersistPrefsRef.current(sid, { preferredModel: payload.modelId });
          // 会话活着且空闲就直接推给 agent，不等下一条消息。
          const caps = capsByIdRef.current[sid];
          const status = sessionsRef.current.find((s) => s.id === sid)?.status;
          if (caps && status !== "running" && status !== "starting") {
            try {
              await updateAcpSession(sid, { model: payload.modelId });
            } catch {
              // 推不动就留给发送前的配置回推。
            }
          }
          await telegramRemoteResult(
            `✅ [${row.label}] 模型已切换 · ${payload.modelLabel || payload.modelId}`,
          );
        }
      })();
    }).then((fn) => {
      unlisten = fn;
    });
    return () => unlisten?.();
    // 只订阅一次；回调和会话查找全部走 ref。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

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
          { flushQueued: true, remote: next.remote },
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
        const projectId =
          sessionsRef.current.find((s) => s.id === prompt.sessionId)?.projectId ||
          currentProjectId;
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
        setQuotePinsById((current) => {
          const sid = prompt.sessionId;
          if (!(sid in current)) return current;
          const next = { ...current };
          delete next[sid];
          return next;
        });
        setPathGrantPrompt(null);
        // 经 ref 走最新 performSend（闭包不随渲染过期；telegram 管线同款做法）。
        await telegramPerformSendRef.current(
          prompt.sessionId,
          prompt.text,
          prompt.imageAttachments ?? [],
          prompt.composerSnap,
        );
      } finally {
        setPathGrantBusy(false);
      }
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [pathGrantPrompt, pathGrantBusy, currentProjectId, pushDebug, refreshProjectContext, setSessionStatusById]
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
      await createSessionForProject(project.id, availableAgents[0]?.id ?? agents[0]!.id, project);
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

  const tabGhostHint = useMemo(
    () => ghostHintForDrop(tabDropPreview, { atMax: leafCount(splitState) >= MAX_PANES }),
    [tabDropPreview, splitState],
  );

  const handleTabDragPreview = (session: Session, clientX: number, clientY: number) => {
    const drop = hitTestTabDrop(session, clientX, clientY);
    if (dropKindsEqual(tabDropPreviewRef.current, drop)) return;
    tabDropPreviewRef.current = drop;
    setTabDropPreview(drop);
    setDropPanes(dropPanesRef.current);
  };

  const handleTabDragEnd = (session: Session, clientX: number, clientY: number, cancelled: boolean) => {
    const drop = cancelled
      ? ({ kind: "cancel", reason: "miss" } as const)
      : hitTestTabDrop(session, clientX, clientY);
    tabDropPreviewRef.current = null;
    setTabDropPreview(null);
    setDropPanes([]);
    if (cancelled || drop.kind === "cancel") return;
    if (drop.kind === "detach") {
      void handleTabPopOut(session, true);
      return;
    }
    const tree =
      leafCount(splitStateRef.current) > 1
        ? splitStateRef.current
        : migrateFromFlat(openSessionIdsRef.current, currentSessionIdRef.current);
    if (drop.kind === "split") {
      commitSplit(splitPane(tree, drop.paneId, drop.edge, session.id, { maxLeaves: MAX_PANES }));
      return;
    }
    if (drop.kind === "activate") {
      commitSplit(moveTab(tree, session.id, { kind: "activate", paneId: drop.paneId }));
      return;
    }
    if (drop.kind === "insert-tab") {
      commitSplit(moveTab(tree, session.id, { kind: "insert-tab", paneId: drop.paneId, index: drop.index }));
    }
  };

  const newTabForSession = (session: Session) => {
    if (!currentSession || session.projectId === CHAT_PROJECT_ID) {
      void handleCreateChatSession();
    } else {
      void createSessionForProject(session.projectId || currentProject?.id || "");
    }
  };

  // ── 侧栏/标签栏稳定回调（审计 2026-10-24 P1-A）─────────────────────────
  // MemoProjectShelf / MemoSessionTabs 靠这些引用稳定的包装器在流式
  // eventsTick（20-50Hz）重渲时跳过整棵侧栏树。useLatestCallback 每次渲染
  // 把最新闭包写进 ref——引用不变、内容不过期，不引入过期闭包风险。
  const shelfStable = {
    onToggleDesktopNotify: useLatestCallback(() => {
      setDesktopNotifyOn((current) => {
        const next = !current;
        setDesktopNotifyEnabled(next);
        return next;
      });
    }),
    onToggleAutoHideClose: useLatestCallback(() => {
      setAutoHideOnClose(toggleAutoHideOnClose());
    }),
    onQuitApp: useLatestCallback(() => {
      void quitApp();
    }),
    onDockToEdge: useLatestCallback(() => {
      void (getDockState() === "floating" ? enterDock() : undock());
    }),
    onCollapse: useLatestCallback(() => setLeftCollapsed(true)),
    onExpand: useLatestCallback(() => setLeftCollapsed(false)),
    onToggleTheme: useLatestCallback(() => withViewTransition(() => {
      const next = nextTheme(theme);
      // Paint the new palette inside the transition, not a frame later in the effect.
      document.documentElement.dataset.theme = next;
      setTheme(next);
    })),
    onAddProject: useLatestCallback(() => {
      setProjectDialogOpen(true);
      setProjectError("");
    }),
    onNewSession: useLatestCallback((projectId: string) => {
      createSessionForProject(projectId);
    }),
    onProjectSelect: useLatestCallback((projectId: string) => {
      setCurrentProjectId(projectId);
      const nextSession = availableSessions.find((session) => session.projectId === projectId);
      if (nextSession) {
        openSession(nextSession);
      } else {
        // Selecting a project with no sessions should still get a usable dialog.
        void createSessionForProject(projectId);
      }
    }),
    onSessionSelect: useLatestCallback(openSession),
    onDeleteSession: useLatestCallback(deleteSession),
    onDeleteProject: useLatestCallback(handleDeleteProject),
    onOpenBeside: useLatestCallback(handleOpenBeside),
    onRevealProject: useLatestCallback((project: Project) => {
      void revealInFileManager(project.rootPath).catch(() => undefined);
    }),
    onNewChat: useLatestCallback(() => {
      void handleCreateChatSession();
    }),
  };

  const tabStable = {
    onTabSelect: useLatestCallback(openSession),
    onTabClose: useLatestCallback(closeSessionTab),
    onNewTab: useLatestCallback(() => newTabForSession(displaySession)),
    onTabPopOut: useLatestCallback((s: Session, viaDrag?: boolean) => void handleTabPopOut(s, viaDrag === true)),
    onTabMergeBack: useLatestCallback((s: Session, viaDrag?: boolean) => void handleMergeBack(s, viaDrag === true)),
    onTabDragPreview: useLatestCallback(handleTabDragPreview),
    onTabDragEnd: useLatestCallback(handleTabDragEnd),
  };

  const renderConversation = (session: Session) => {
    void eventsTick[session.id];
    const events = eventsBySessionRef.current.get(session.id) ?? EMPTY_EVENTS;
    // A prompt only removes the composer once its card is actually on the rail;
    // hiding it for a card that never rendered leaves no way to answer.
    const askOnRail = promptCardOnRail(events, askPrompt?.requestId);
    const planOnRail = promptCardOnRail(events, planApproval?.requestId);
    const hidesComposer = sessionHidesComposer(
      session.id,
      askOnRail ? askPrompt : null,
      planOnRail ? planApproval : null,
    );
    const allEvents = publishedEventsForView(session.id);
    const agent =
      availableAgents.find((item) => item.id === session.agentId) ??
      availableAgents[0] ??
      agents[0]!;
    const caps = capsById[session.id] ?? (session.id === currentSessionId ? sessionCapabilities : null);
    const queuedSends = pendingSendsRef.current.get(session.id) ?? [];
    const prefill =
      composerPrefill &&
      (composerPrefill.sessionId
        ? composerPrefill.sessionId === session.id
        : session.id === currentSessionId)
        ? composerPrefill
        : null;
    return (
      <>
        <SessionView
          agent={agent}
          events={events}
          session={session}
          webReplyWidthPercent={webReplyWidthPercent}
          onWebReplyWidthChange={(value) => {
            setWebReplyWidthPercent(value);
            window.localStorage.setItem("marionette-web-reply-width-percent", String(value));
          }}
          lastActivityAt={lastActivityById[session.id] ?? null}
          authBanner={agentBanner[agent.id]?.message ?? null}
          bannerKind={agentBanner[agent.id]?.kind ?? "auth"}
          onSignIn={
            agentAuthSpec(agent.id)?.login
              ? () => void handleAgentSignIn(agent.id)
              : undefined
          }
          signInBusy={signInBusy}
          onUpdate={
            agent.id === "grok-build" || agent.id === "grok"
              ? () => void handleGrokUpdate(agent.id)
              : undefined
          }
          updateBusy={updateBusy}
          onSessionStatusChange={handleSessionStatusChange}
          onEditResend={(anchor, text) => void handleEditResend(anchor, text)}
          onForkReply={(anchor) => void handleForkFromReply(session.id, anchor)}
          quotePins={quotePinsById[session.id] ?? []}
          onQuotePinsChange={(pins) => {
            setQuotePinsById((current) => ({ ...current, [session.id]: pins }));
          }}
          onInterrupt={() => void cancelLiveTurn(session.id, { announce: true })}
          projectId={session.projectId || currentProjectId}
          onFilesRestored={() => void refreshChangedFiles()}
          allEvents={allEvents}
          detailsVisible={detailsVisible}
          onDetailsVisibleChange={setDetailsVisible}
          onSubtaskStop={(childId) => {
            void cancelAcpSession(childId).catch(() => undefined);
            finalizeDelegateChildRef.current(childId, "cancelled", "用户停止");
          }}
          onSubtaskQuote={(summary) => {
            const block = `> ${summary.replace(/\n/g, "\n> ").slice(0, 2000)}\n\n`;
            setComposerPrefill({ sessionId: session.id, text: block, token: Date.now() });
          }}
          onSubtaskRetry={(childId) => {
            const startEv = (
              eventsBySessionRef.current.get(session.id) ?? EMPTY_EVENTS
            ).find(
              (e) =>
                e.type === "subtask_started" &&
                e.childSessionId === childId &&
                e.sessionId === session.id,
            );
            if (!startEv || startEv.type !== "subtask_started") return;
            const projectId = session.projectId || currentProjectId;
            void handleDelegate(session.id, projectId, {
              agentId: startEv.agentId,
              modelId: startEv.modelId,
              prompt: startEv.prompt.replace(/^（排队中）/, ""),
            });
          }}
          streamPrompt={{
            liveAsk: askPrompt?.sessionId === session.id ? askPrompt : null,
            livePlan:
              planApproval?.sessionId === session.id
                ? {
                    ...planApproval,
                    planMarkdown:
                      planApproval.planMarkdown.trim() ||
                      (planBySessionId[planApproval.sessionId] ?? [])
                        .map((e, i) => {
                          const status = e.status ? ` [${e.status}]` : "";
                          return `${i + 1}. ${e.content}${status}`;
                        })
                        .join("\n"),
                  }
                : null,
            askBusy,
            planBusy: planApprovalBusy,
            onAskSubmit: (answers) => void handleAskSubmit(answers),
            onAskDecline: () => void handleAskDecline(),
            onPlanAnswer: (decision, feedback) => handlePlanApproval(decision, feedback),
            onPlanPermissionChoose: (optionId) => void handlePlanPermissionChoose(optionId),
          }}
        />
        <div className="composer-slot">
        <div className="composer-slot__notices">
        {sessionNotices[session.id] && (
          <SessionNoticeStrip
            notice={sessionNotices[session.id]!}
            onDismiss={() => {
              setSessionNotices((current) => {
                const next = { ...current };
                delete next[session.id];
                return next;
              });
            }}
          />
        )}
        {composerFailure && composerFailure.sessionId === session.id && (
          <ComposerErrorStrip
            error={composerFailure.error}
            canSignIn={Boolean(agentAuthSpec(agent.id)?.login)}
            onUpdate={
              agent.id === "grok-build" || agent.id === "grok"
                ? () => void handleGrokUpdate(agent.id)
                : undefined
            }
            onRetry={() => {
              const last = lastSendBySessionRef.current.get(session.id);
              if (!last) return;
              setComposerFailure(null);
              void performSend(
                session.id,
                last.composed,
                last.imageAttachments,
                last.composerSnap,
              );
            }}
            onSignIn={() => void handleAgentSignIn(agent.id)}
            onNewSession={() => {
              setComposerFailure(null);
              void createSessionForProject(session.projectId, session.agentId);
            }}
            onDismiss={() => setComposerFailure(null)}
          />
        )}
        {queuedSends.length > 0 && (() => {
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
                  pendingSendsRef.current.delete(session.id);
                  setQueuedStripTick((t) => t + 1);
                }}
              >
                取消
              </button>
            </div>
          );
        })()}
        </div>
        <div
          className={
            hidesComposer
              ? "composer-slot__input is-hidden"
              : "composer-slot__input"
          }
          aria-hidden={hidesComposer}
        >
        <Composer
          key={`${session.id}:${session.agentId}`}
          agent={agent}
          agents={availableAgents}
          sessionId={session.id}
          sessionStatus={session.status}
          docked={dockActive}
          onRestartAgentSession={handleRestartAgentSession}
          autoFocus={
            composerAutoFocus &&
            session.id === currentSessionId &&
            !hidesComposer
          }
          isFocused={session.id === currentSessionId}
          onEmptyActivate={() => {
            if (session.id.startsWith("session-empty-") || !session.id) {
              setComposerAutoFocus(true);
              void handleCreateChatSession(session.agentId);
            }
          }}
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
          capabilities={caps}
          prefillText={prefill?.text ?? null}
          prefillToken={prefill?.token ?? 0}
          sessionPrefs={{
            preferredModel: session.preferredModel,
            preferredMode: session.preferredMode,
            preferredEffort: session.preferredEffort,
            preferredEffortId: session.preferredEffortId,
            preferredAlwaysApprove: session.preferredAlwaysApprove,
          }}
          onSessionPrefsChange={(patch: SessionComposerPrefs) => {
            persistComposerPrefs(session.id, patch);
          }}
          onAgentChange={(id) => void handleAgentChange(id, session.id)}
          onInterrupt={() => void cancelLiveTurn(session.id, { announce: true })}
          onSend={(text, droppedPaths, imageAttachments, opts) =>
            void handleSend(text, droppedPaths, imageAttachments, opts, session.id)
          }
          onActiveModelChange={session.id === currentSessionId ? setActiveModelId : undefined}
          sessionEvents={events}
          availableCommands={slashCommandsById[session.id] ?? null}
          onWarmAgent={() => warmAcp(session.id)}
          onEnsureAgentReady={async () => {
            if (agent.transport !== "acp") return true;
            try {
              await ensureAcpReady(session.id);
              return true;
            } catch {
              return false;
            }
          }}
          onSilentPrompt={(text) => sendSilentPrompt(session.id, text)}
        />
        </div>
        </div>
      </>
    );
  };

  const renderPaneTabs = (leaf: import("../lib/splitWorkspace").PaneLeaf, session: Session) => {
    const tabs = leaf.tabIds
      .map((id) => availableSessions.find((item) => item.id === id))
      .filter((item): item is Session => Boolean(item));
    return (
      <SessionTabs
        openSessions={tabs}
        session={session}
        doneUnreadIds={doneUnreadIds}
        onTabSelect={(s) => {
          commitSplit(activateTab(splitStateRef.current, leaf.id, s.id));
          applyFocusedSession(s);
        }}
        onTabClose={closeSessionTab}
        onNewTab={() => {
          commitSplit(focusPane(splitStateRef.current, leaf.id));
          newTabForSession(session);
        }}
        onRenameSession={handleRenameSession}
        onTabPopOut={
          IS_DETACHED_WINDOW ? undefined : (s, viaDrag) => void handleTabPopOut(s, viaDrag)
        }
        mergeDropActive={mergeTargetActive}
        onTabDragPreview={IS_DETACHED_WINDOW ? undefined : handleTabDragPreview}
        onTabDragEnd={IS_DETACHED_WINDOW ? undefined : handleTabDragEnd}
        ghostHint={tabGhostHint}
        onOpenBeside={IS_DETACHED_WINDOW ? undefined : handleOpenBeside}
      />
    );
  };

  /**
   * 收纳盒 page — shared by the docked strip (dockPage === "memo") and the
   * floating window's tab form (memoActive). The mount's chrome owns title
   * and close (strip 返回 / titlebar tab). Using a note opens a dialog in the
   * project in view; plain context can go to a chat when no project is open.
   */
  const renderMemoBox = (close: () => void) => (
    <MemoBox
      onClose={close}
      onChanged={setMemoCount}
      onUse={(picked, intent) => {
        // 开新对话 always lands in the shelf's 收纳盒 group; 整理成文档 keeps
        // targeting the project in view (falls back to the group when none is).
        const projectId = currentProjectId && currentProjectId !== CHAT_PROJECT_ID ? currentProjectId : null;
        const create =
          intent === "dialog" || !projectId
            ? handleCreateChatSession(undefined, { origin: "memo" })
            : createSessionForProject(projectId);
        void create.then((session) => {
          if (!session) return;
          setComposerPrefill({ sessionId: session.id, text: memoPrompt(picked, intent), token: Date.now() });
        });
      }}
    />
  );

  return (
    <main className="app-shell">
      <div
        ref={workspaceGridRef}
        className={`workspace-grid${leftCollapsed || IS_DETACHED_WINDOW ? " is-left-collapsed" : ""}${rightCollapsed ? " is-right-collapsed" : ""}${IS_DETACHED_WINDOW ? " is-detached" : ""}${dockActive ? " is-docked" : ""}${dockActive && dockPage === "shelf" ? " dock-drawer-open" : ""}${dockActive && dockPage === "context" ? " dock-context-open" : ""}`}
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
          <MemoProjectShelf
            agents={availableAgents}
            projects={shelfProjects}
            sessions={availableSessions}
            currentProjectId={currentProjectId}
            currentSessionId={displaySession.id}
            doneUnreadIds={doneUnreadIds}
            pinnedProjectId={openHereProjectId}
            collapsed={leftCollapsed}
            theme={theme}
            desktopNotifyEnabled={desktopNotifyOn}
            proxyConfig={proxyConfig}
            onSaveProxy={handleSaveProxy}
            onTestProxy={handleTestProxy}
            onToggleDesktopNotify={shelfStable.onToggleDesktopNotify}
            onCollapse={shelfStable.onCollapse}
            onExpand={shelfStable.onExpand}
            onToggleTheme={shelfStable.onToggleTheme}
            onAddProject={shelfStable.onAddProject}
            onNewSession={shelfStable.onNewSession}
            onProjectSelect={shelfStable.onProjectSelect}
            onSessionSelect={shelfStable.onSessionSelect}
            onOpenBeside={shelfStable.onOpenBeside}
            externalScanning={externalScanning}
            onRefreshExternal={handleRefreshExternal}
            onDeleteSession={shelfStable.onDeleteSession}
            onDeleteProject={shelfStable.onDeleteProject}
            onRenameSession={handleRenameSession}
            onSuspendSession={handleSuspendSession}
            onToggleSessionPin={handleToggleSessionPin}
            onReorderProjects={handleReorderProjects}
            onRevealProject={shelfStable.onRevealProject}
            defaultFolderPath={defaultFolderPath}
            chatSessions={chatSessions}
            memoSessions={memoSessions}
            onChatSessionSelect={openChatSession}
            onNewChat={shelfStable.onNewChat}
            onOpenMemo={dockActive ? undefined : () => {
              setMemoOpen(true);
              setMemoActive(true);
            }}
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

        <ToastStack />
        {paletteOpen && (
          <CommandPalette
            sessions={availableSessions}
            chatSessions={[...chatSessions, ...memoSessions]}
            projects={shelfProjects}
            currentSessionId={displaySession.id}
            doneUnreadIds={doneUnreadIds}
            agentLabel={(agentId) => availableAgents.find((a) => a.id === agentId)?.label ?? agentId}
            onOpenSession={openSession}
            onOpenChat={openChatSession}
            onSelectProject={(projectId) => {
              setCurrentProjectId(projectId);
              const nextSession = availableSessions.find((session) => session.projectId === projectId);
              if (nextSession) openSession(nextSession);
              else void createSessionForProject(projectId);
            }}
            onClose={() => setPaletteOpen(false)}
            actions={[
              ...(currentProjectId && currentProjectId !== CHAT_PROJECT_ID
                ? [
                    {
                      id: "new-session",
                      label: "在当前项目新建对话",
                      hint: availableProjects.find((p) => p.id === currentProjectId)?.name,
                      run: () => void createSessionForProject(currentProjectId),
                    },
                  ]
                : []),
              { id: "new-chat", label: "新建聊天", run: () => void handleCreateChatSession() },
              {
                id: "add-project",
                label: "添加项目…",
                run: () => {
                  setProjectDialogOpen(true);
                  setProjectError("");
                },
              },
              ...(lastClosedTabsRef.current.length > 0
                ? [
                    {
                      id: "reopen-tab",
                      label: "重新打开刚关闭的标签",
                      hint: "Ctrl Shift T",
                      run: () => {
                        const id = lastClosedTabsRef.current[0];
                        const session = availableSessions.find((s) => s.id === id);
                        if (!session) return;
                        lastClosedTabsRef.current = lastClosedTabsRef.current.slice(1);
                        saveClosedTabs(lastClosedTabsRef.current);
                        openSession(session);
                      },
                    },
                  ]
                : []),
              {
                id: "toggle-left",
                label: leftCollapsed ? "展开左侧栏" : "收起左侧栏",
                run: () => setLeftCollapsed((value) => !value),
              },
              {
                id: "theme",
                label: "切换主题",
                run: () =>
                  withViewTransition(() => {
                    const next = nextTheme(theme);
                    document.documentElement.dataset.theme = next;
                    setTheme(next);
                  }),
              },
              {
                id: "dock-edge",
                label: dockActive ? "解除贴边" : "贴边收起",
                hint: dockActive ? "恢复普通窗口" : "也可以把窗口拖到屏幕左右边缘",
                run: shelfStable.onDockToEdge,
              },
              {
                id: "auto-hide-close",
                label: autoHideOnClose
                  ? "关闭窗口时：收起到边缘（点此改为退出）"
                  : "关闭窗口时：退出（点此改为收起到边缘）",
                run: shelfStable.onToggleAutoHideClose,
              },
              {
                id: "quit-app",
                label: "退出 Marionette",
                hint: "停止引擎与全部会话",
                run: shelfStable.onQuitApp,
              },
            ]}
          />
        )}

        {/* Shared titlebar across center + right columns */}
        <div className="workspace-titlebar">
          {!workspaceSplit ? (
          <MemoSessionTabs
            openSessions={openSessions}
            session={displaySession}
            doneUnreadIds={doneUnreadIds}
            onTabSelect={tabStable.onTabSelect}
            onTabClose={tabStable.onTabClose}
            onNewTab={tabStable.onNewTab}
            onRenameSession={handleRenameSession}
            onTabPopOut={
              IS_DETACHED_WINDOW
                ? undefined
                : tabStable.onTabPopOut
            }
            onTabMergeBack={
              IS_DETACHED_WINDOW
                ? tabStable.onTabMergeBack
                : undefined
            }
            mergeDropActive={IS_DETACHED_WINDOW ? false : mergeTargetActive}
            detachedMode={IS_DETACHED_WINDOW}
            onTabDragPreview={IS_DETACHED_WINDOW ? undefined : tabStable.onTabDragPreview}
            onTabDragEnd={IS_DETACHED_WINDOW ? undefined : tabStable.onTabDragEnd}
            ghostHint={tabGhostHint}
            onOpenBeside={IS_DETACHED_WINDOW ? undefined : shelfStable.onOpenBeside}
            titlebarDragRegion
            memoTab={
              IS_DETACHED_WINDOW || !memoOpen
                ? undefined
                : {
                    active: memoActive,
                    count: memoCount,
                    onSelect: () => setMemoActive(true),
                    onClose: () => {
                      setMemoOpen(false);
                      setMemoActive(false);
                    },
                  }
            }
          />
          ) : (
            <>
              {memoOpen && !IS_DETACHED_WINDOW && (
                <div
                  className={`editor-tab is-memo${memoActive ? " is-active" : ""}`}
                  role="tab"
                  aria-selected={memoActive}
                  data-tauri-drag-region="false"
                >
                  <button
                    className="editor-tab__select"
                    type="button"
                    title="收纳盒"
                    onClick={() => setMemoActive(true)}
                  >
                    <Inbox size={12} />
                    <span>收纳盒</span>
                    {memoCount > 0 && <b className="editor-tab__memo-count">{memoCount}</b>}
                  </button>
                  <button
                    className="pill-action pill-action--icon pill-action--sm editor-tab__close"
                    type="button"
                    title="Close 收纳盒"
                    aria-label="Close 收纳盒"
                    onClick={() => {
                      setMemoOpen(false);
                      setMemoActive(false);
                    }}
                  >
                    <X size={12} />
                  </button>
                </div>
              )}
              <div className="workspace-titlebar__fill" data-tauri-drag-region />
            </>
          )}
          <div className="workspace-titlebar__spacer" data-tauri-drag-region />
          <WindowControls />
        </div>

        {/* Center workspace — hover edges to reveal sidebar toggle buttons */}
        <section
          className={`center-workspace${workspaceSplit ? " is-split" : ""}${mergeTargetActive ? " is-merge-target" : ""}`}
          aria-label="Active workspace"
          onMouseMove={handleCenterMove}
          onMouseLeave={handleCenterLeave}
        >
          {/* Absolute inside center: always inset from the seam, never overlaps the rail border */}
          {(leftCollapsed || leftEdgeHover) && (
            <button
              type="button"
              className={`floating-trigger floating-trigger--center-left${leftEdgeHover ? " is-visible" : ""}${leftCollapsed && doneUnreadIds.size > 0 ? " is-done" : ""}`}
              onClick={() => setLeftCollapsed(!leftCollapsed)}
              aria-label={leftCollapsed ? "Show projects and sessions" : "Hide projects and sessions"}
              title={
                leftCollapsed && doneUnreadIds.size > 0
                  ? "有对话已完成任务 · 打开侧边栏查看"
                  : leftCollapsed
                    ? "Show projects and sessions"
                    : "Hide projects and sessions"
              }
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
          {memoActive && !dockActive && !IS_DETACHED_WINDOW ? (
            // 收纳盒 as a tab page: only the conversation-flow column switches
            // (shelf and context panel stay); no composer for this page.
            <div className="memo-tab-page" aria-label="收纳盒">
              {renderMemoBox(() => {
                setMemoOpen(false);
                setMemoActive(false);
              })}
            </div>
          ) : workspaceSplit ? (
            <SplitWorkspace
              state={splitState}
              dropPreview={tabDropPreview}
              dropPanes={dropPanes}
              geometry="per-pane"
              onFocusPane={(paneId) => {
                const next = focusPane(splitStateRef.current, paneId);
                commitSplit(next);
                const sess = sessionsRef.current.find((s) => s.id === focusedSessionId(next));
                if (sess) applyFocusedSession(sess);
              }}
              onResize={(splitId, ratio) => {
                commitSplit(resizeSplit(splitStateRef.current, splitId, ratio));
              }}
              renderLeaf={(leaf) => {
                const session =
                  availableSessions.find((s) => s.id === leaf.activeTabId) ?? displaySession;
                return {
                  tabs: renderPaneTabs(leaf, session),
                  body: renderConversation(session),
                };
              }}
            />
          ) : (
            <>
              {renderConversation(displaySession)}
              <SplitDropOverlay
                preview={tabDropPreview}
                panes={dropPanes}
                geometry="titlebar-plus-center"
                root={splitState.root}
              />
            </>
          )}
        </section>

        <ContextPanel
          collapsed={rightCollapsed && !dockActive}
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
          projectRoot={currentProject?.rootPath ?? null}
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
          activeSessionId={currentSessionId}
          resizeDragging={resizingSide === "right"}
          onResizeStart={() => setResizingSide("right")}
          backgroundTasks={backgroundTasks}
          sessionLabels={sessionLabelsForBackground}
          onOpenBackgroundTask={handleOpenBackgroundTask}
          onKillBackgroundTerminal={handleKillBackgroundTerminal}
        />
      </div>
      {/* Window-level: draw as soon as the prompt exists. Do not wait for pane focus. */}
      {/* Edge dock stage 2: strip chrome bands + overlay backdrops. */}
      {dockActive && (
        <>
          <DockChrome
            page={dockPage}
            onPage={setDockPage}
            onNewChat={() => void handleCreateChatSession()}
            onNewChatIn={(projectId) => void createSessionForProject(projectId)}
            projects={shelfProjects}
            memoCount={memoCount}
            quotaRows={engineQuotaRows}
            active={dockActiveSessions}
            totalSessions={availableSessions.length}
            currentSessionId={currentSessionId}
            doneUnreadIds={doneUnreadIds}
            onOpenSession={openSession}
            onSuspendSession={handleSuspendSession}
            onOpenPalette={() => setPaletteOpen(true)}
            onUndock={() => void undock()}
            onQuit={() => void quitApp()}
            updateAvailable={Boolean(appUpdate?.updateAvailable)}
            updateLabel={appUpdate?.latestVersion ?? null}
            updateBusy={appUpdateBusy}
            onCheckUpdate={handleCheckAppUpdate}
            onUpdateAndRelaunch={handleUpdateAndRelaunch}
            splitPaused={dockSplitPaused}
          />
          {/* Native drop routing for the whole strip: top band → 收纳盒,
              conversation → composer (2026-10-26). */}
          <DockDropRouter
            page={dockPage}
            hasConversation={currentSessionId != null}
            onMemoCount={setMemoCount}
          />
          {/* Width grip only on the home view: the full pages are overlays and
              keep their own width until 返回. */}
          {dockExpanded && dockPage === null && <DockResizeGrip />}
          {dockPage === "memo" && renderMemoBox(() => setDockPage(null))}
        </>
      )}
      {permissionPrompt && (
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
                onClick={handleUpdateAndRelaunch}
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
