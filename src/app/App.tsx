import { listen } from "@tauri-apps/api/event";
import { ChevronLeft, ChevronRight, FolderOpen, X } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties } from "react";
import { agents, projects, sessions } from "../lib/mockData";
import {
  applyAcpPartToEvents,
  coalesceAdjacentThoughts,
  collapseIntermediateAssistantAsThought,
  extractAcpUpdateText,
  getSessionUpdate,
  getSessionUpdateKind,
  sealOpenAssistantReplies,
  userMessageEvent,
} from "../lib/acpTranscript";
import { addProject, appendDebugLog, cancelAcpSession, checkOutsideProjectPaths, createChildSession, createSession as createSessionApi, deleteProject as deleteProjectApi, deleteSession as deleteSessionApi, generateHandoff, getChangedFiles, getFileDiff, getSessionCapabilities, grantWorkspaceRoot, isTauriRuntime, listAgentCommands, listAgents, listProjects, listSessions, listTodos, loadTranscript, pickFolder, probeAcpBilling, probeAgentAuth, probeProviderUsage, projectContextPrompt, respondAcpPermission, saveTodos, scanProjectContext, searchSessions, sendAcpPrompt, setProjectContextEnabled, startAcpSession, startAgentLogin, stopAcpSession, updateAcpSession, updateSessionAgent, updateSessionLabel, updateSessionPrefs, writeTranscript, type OutsidePath } from "../lib/api";
import type { AcpEvent, AvailableCommand, CapabilitySnapshot, ChangedFile, HandoffResult, Project, ProjectContext, Session, SessionComposerPrefs, SessionEvent, SessionViewMode, UsageSnapshot } from "../lib/types";
import {
  buildUsageSnapshot,
  emptySessionUsage,
  mergeGrokBilling,
  mergeProviderProbe,
  mergeUsageFromAcp,
  mergeUsageFromPromptResult,
  mergeUsageFromText,
  seedContextSize,
  type SessionUsageState,
} from "../lib/usage";
import { mergeAcpCapabilities } from "../lib/acpSupplements";
import {
  bindDesktopNotifyFocusHandlers,
  isDesktopNotifyEnabled,
  raiseDesktopNotify,
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
import { expandAcpConfigAttempts } from "../lib/acpSupplements";
import {
  formatImageMarksForSend,
  type ImageAttachment,
} from "../lib/imageAttachments";
import {
  parseTranscriptEvents,
  persistableEventsForSession,
  shouldAutoRenameLabel,
  titleFromUserText,
} from "../lib/transcript";
import { activityHealth } from "../lib/activityHealth";
import {
  buildHistoryInjection,
  pendingHandoff,
  withHandoffAttachment,
  withHistoryInjection,
} from "../lib/sessionHistory";
import { formatPinsForSend } from "../lib/quoteComment";
import { findLinkTargets } from "../lib/linkTargets";
import { isToolInProgress } from "../lib/activityHealth";
import { classifyAgentError, formatClassifiedError } from "../lib/errors";
import { Composer } from "../components/Composer";
import { ContextPanel } from "../components/ContextPanel";
import { PermissionDialog, type PermissionPrompt } from "../components/PermissionDialog";
import { ProjectShelf } from "../components/ProjectShelf";
import { SessionTabs, SessionView, type UserMessageAnchor } from "../components/SessionView";
import { WindowControls } from "../components/WindowControls";
import { initScrollbarAutoHide } from "../lib/scrollbarAutoHide";

type ThemeMode = "dark" | "light";

const LEFT_PANEL_MIN = 180;
const LEFT_PANEL_MAX = 420;
const LEFT_PANEL_DEFAULT = 224;
const RIGHT_PANEL_MIN = 220;
const RIGHT_PANEL_MAX = 480;
const RIGHT_PANEL_DEFAULT = 270;
const LAYOUT_STORAGE_KEY = "marionette-layout";

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

/**
 * Status is runtime state, not history. A freshly started app owns no agent
 * process, whatever the previous run left on disk — restoring a dialog as
 * "running" would show a Working bar and a locked composer for a turn that
 * ended when the app closed. (Rust heals the file too; this keeps the browser
 * mock and any future read path honest.)
 */
function asIdleOnLoad(session: Session): Session {
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

/** Close out tools that never received a terminal status (stuck in_progress). */
function markOpenTools(
  events: SessionEvent[],
  sessionId: string,
  status: "cancelled" | "failed"
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

export function App() {
  const [availableProjects, setAvailableProjects] = useState<Project[]>(projects);
  const [availableAgents, setAvailableAgents] = useState(agents);
  const [availableSessions, setAvailableSessions] = useState<Session[]>(sessions);
  const [currentProjectId, setCurrentProjectId] = useState(projects[0]?.id ?? "");
  const [currentSessionId, setCurrentSessionId] = useState(sessions[0]?.id ?? "");
  const [openSessionIds, setOpenSessionIds] = useState<string[]>([sessions[0]?.id ?? ""]);
  const [viewMode, setViewMode] = useState<SessionViewMode>("clean");
  const [leftCollapsed, setLeftCollapsed] = useState(false);
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
    return stored === "light" ? "light" : "dark";
  });
  /** Taskbar flash + chime when AI replies / may be stuck (off while focused). */
  const [desktopNotifyOn, setDesktopNotifyOn] = useState(() => isDesktopNotifyEnabled());
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
  /** Fresh ACP process needs local transcript injected once (no session/load yet). */
  const acpNeedsHistoryRef = useRef<Set<string>>(new Set());
  /** When each session's turn last actually finished (`rpc/response` / process end). */
  const turnEndedAtRef = useRef<Record<string, number>>({});
  /**
   * Sessions whose `session/cancel` went unanswered. Cancel is a fire-and-forget
   * notification, so an agent wedged inside a tool or a model call never reads
   * it — the process is unrecoverable from that point and the next send has to
   * replace it rather than write into a pipe nobody drains.
   */
  const cancelIgnoredRef = useRef<Set<string>>(new Set());
  const cancelWatchdogsRef = useRef<Map<string, ReturnType<typeof setTimeout>>>(new Map());
  const transcriptSaveTimers = useRef<Map<string, ReturnType<typeof setTimeout>>>(new Map());
  const transcriptLoadedRef = useRef<Set<string>>(new Set());
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
  const [claudeAuthHint, setClaudeAuthHint] = useState<string | null>(null);
  const [signInBusy, setSignInBusy] = useState(false);
  const authPollRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const [composerPrefill, setComposerPrefill] = useState<{ text: string; token: number } | null>(null);
  /** Inline Clean quote-comments (numbered pins) for the active dialog. */
  const [quotePins, setQuotePins] = useState<import("../lib/quoteComment").QuotePin[]>([]);
  const [lastHandoff, setLastHandoff] = useState<HandoffResult | null>(null);
  const [changedFiles, setChangedFiles] = useState<ChangedFile[]>([]);
  const [changedFilesNote, setChangedFilesNote] = useState<string | null>(null);
  const [diffPreview, setDiffPreview] = useState<{ path: string; text: string } | null>(null);
  const [permissionPrompt, setPermissionPrompt] = useState<PermissionPrompt | null>(null);
  const [permissionBusy, setPermissionBusy] = useState(false);
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
  } | null>(null);
  const [pathGrantBusy, setPathGrantBusy] = useState(false);
  const lastEscAtRef = useRef(0);
  /** Last ACP activity timestamp per session — for heartbeat / stale-working UI. */
  const [lastActivityById, setLastActivityById] = useState<Record<string, number>>({});
  const lastActivityByIdRef = useRef(lastActivityById);
  lastActivityByIdRef.current = lastActivityById;
  const sessionsRef = useRef(availableSessions);
  const agentsRef = useRef(availableAgents);
  const currentSessionIdRef = useRef(currentSessionId);
  const liveEventsRef = useRef(liveEvents);
  sessionsRef.current = availableSessions;
  agentsRef.current = availableAgents;
  currentSessionIdRef.current = currentSessionId;
  liveEventsRef.current = liveEvents;

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

  useEffect(() => {
    void Promise.all([listProjects(), listAgents()]).then(async ([nextProjects, nextAgents]) => {
      const resolvedProjects = nextProjects.length > 0 || isTauriRuntime() ? nextProjects : projects;
      setAvailableProjects(resolvedProjects);
      if (resolvedProjects.length > 0) {
        setCurrentProjectId((current) => resolvedProjects.some((project) => project.id === current) ? current : resolvedProjects[0].id);
      } else {
        setCurrentProjectId("");
      }
      setAvailableAgents(nextAgents.length > 0 ? nextAgents : agents);

      const loadedSessions = (await Promise.all(resolvedProjects.map((project) => listSessions(project.id))))
        .flat()
        .map(asIdleOnLoad);
      if (loadedSessions.length > 0 || isTauriRuntime()) {
        setAvailableSessions(loadedSessions);
        if (loadedSessions[0]) {
          setCurrentProjectId(loadedSessions[0].projectId);
          setCurrentSessionId(loadedSessions[0].id);
          setOpenSessionIds([loadedSessions[0].id]);
          setViewMode("clean");
        } else {
          setCurrentSessionId("");
          setOpenSessionIds([]);
        }
      }
    });
  }, []);

  useEffect(() => {
    if (!isTauriRuntime()) return;
    let disposed = false;
    let unlistenAcp: (() => void) | undefined;

    void listen<AcpEvent>("acp-event", (event) => {
      if (disposed) return;
      const payload = event.payload;
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

      // Did the *turn* end? That — not mere output — is what proves a cancel
      // landed. An agent can keep streaming tokens while its stdin reader is
      // wedged, which is exactly the runaway case people hit pause for, so
      // "the agent made a sound" would be the wrong signal here.
      if (payload.sessionId && (payload.method === "rpc/response" || payload.method === "process/ended")) {
        turnEndedAtRef.current[payload.sessionId] = Date.now();
        cancelIgnoredRef.current.delete(payload.sessionId);
        // Finalize @-delegate child when its turn ends.
        if (delegateMetaRef.current.has(payload.sessionId)) {
          if (payload.method === "process/ended") {
            finalizeDelegateChildRef.current(
              payload.sessionId,
              "cancelled",
              "应用关闭时中断或进程退出",
            );
          } else if (payload.kind === "error") {
            finalizeDelegateChildRef.current(
              payload.sessionId,
              "failed",
              formatAcpRpcError(payload.data) || "rpc error",
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
          const seeded = seedContextSize(
            current[payload.sessionId],
            typeof size === "number" ? size : null
          );
          if (!seeded) return current;
          return { ...current, [payload.sessionId]: seeded };
        });
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
        if (update && getSessionUpdateKind(update) === "session_info_update") {
          const title = typeof update.title === "string" ? update.title.trim() : "";
          if (title) {
            setAvailableSessions((current) =>
              current.map((s) =>
                s.id === payload.sessionId && shouldAutoRenameLabel(s.label)
                  ? { ...s, label: titleFromUserText(title) }
                  : s
              )
            );
            void updateSessionLabel(payload.sessionId, titleFromUserText(title)).catch(() => undefined);
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

        const extracted = extractAcpUpdateText(payload.data);
        if (extracted && (extracted.role === "assistant" || extracted.role === "thought" || extracted.role === "tool")) {
          // Codex `/status` (and similar) embed rate-limit lines in assistant text.
          if (extracted.role === "assistant" && extracted.text) {
            setSessionUsageById((current) => {
              const merged = mergeUsageFromText(current[payload.sessionId], extracted.text);
              if (!merged) return current;
              return { ...current, [payload.sessionId]: merged };
            });
          }
          setLiveEvents((current) => {
            const prevLast = current[current.length - 1];
            let next = applyAcpPartToEvents(current, payload.sessionId, extracted);
            // Track turn start for duration: first new assistant_message card
            if (extracted.role === "assistant") {
              const newLast = next[next.length - 1];
              if (
                newLast?.type === "assistant_message" &&
                newLast.sessionId === payload.sessionId &&
                (prevLast?.type !== "assistant_message" || prevLast.sessionId !== payload.sessionId) &&
                !turnStartedAtRef.current[payload.sessionId]
              ) {
                turnStartedAtRef.current[payload.sessionId] = Date.now();
                // Clear sendMetaRef — snapshot consumed by the first assistant chunk
                sendMetaRef.current = null;
              }
            }
            // Glue fragment thoughts that landed next to each other mid-stream.
            if (extracted.role === "thought" || extracted.role === "assistant") {
              next = coalesceAdjacentThoughts(next, payload.sessionId);
            }
            return next;
          });
        }
      }
      // Grok also puts the full turn usage on `_x.ai/session_notification`
      // (turn_completed.usage) — same numbers as the prompt result, but this
      // notification is easier to spot in logs and arrives even if the RPC
      // response path is filtered.
      if (payload.method === "_x.ai/session_notification") {
        setSessionUsageById((current) => {
          const merged = mergeUsageFromPromptResult(current[payload.sessionId], payload.data);
          if (!merged) return current;
          return { ...current, [payload.sessionId]: merged };
        });
      }

      if (payload.kind === "response" && payload.method === "rpc/response") {
        // End-of-turn token split. This is the only usage Grok ever reports, and
        // it carries the in/out/cached breakdown that usage_update omits.
        setSessionUsageById((current) => {
          const merged = mergeUsageFromPromptResult(current[payload.sessionId], payload.data);
          if (!merged) return current;
          return { ...current, [payload.sessionId]: merged };
        });

        // Compute duration and stamp on the last assistant_message
        const startedAt = turnStartedAtRef.current[payload.sessionId];
        if (startedAt) {
          const durationMs = Date.now() - startedAt;
          delete turnStartedAtRef.current[payload.sessionId];
          setLiveEvents((current) => {
            const last = current[current.length - 1];
            if (last?.type === "assistant_message" && last.sessionId === payload.sessionId && last.durationMs == null) {
              const next = [...current];
              next[next.length - 1] = { ...last, durationMs };
              return collapseIntermediateAssistantAsThought(next, payload.sessionId);
            }
            return collapseIntermediateAssistantAsThought(current, payload.sessionId);
          });
        } else {
          setLiveEvents((current) => collapseIntermediateAssistantAsThought(current, payload.sessionId));
        }
        setAvailableSessions((current) =>
          current.map((session) =>
            session.id === payload.sessionId && session.status === "running"
              ? { ...session, status: "waiting" }
              : session
          )
        );
        // Usage panel: refresh once after each completed Reply turn.
        refreshUsageAfterTurnRef.current(payload.sessionId);
        // Desktop notify only for a real turn end (was running), not set_config echoes.
        {
          const sess = sessionsRef.current.find((s) => s.id === payload.sessionId);
          if (sess?.status === "running") {
            const label = sess.label?.trim() || "Session";
            void raiseDesktopNotify("reply", label);
          }
        }
      }
      // Agent process stdout closed (crash / exit) — never leave the UI "Working" forever.
      if (payload.method === "process/ended") {
        const detail =
          payload.data && typeof payload.data === "object"
            ? String((payload.data as { message?: unknown }).message ?? "Agent process ended")
            : "Agent process ended";
        setLiveEvents((current) => {
          const last = current[current.length - 1];
          if (
            last?.type === "assistant_message" &&
            last.sessionId === payload.sessionId &&
            last.text.includes("Agent process ended")
          ) {
            return markOpenTools(current, payload.sessionId, "failed");
          }
          return [
            ...markOpenTools(current, payload.sessionId, "failed"),
            {
              type: "assistant_message" as const,
              sessionId: payload.sessionId,
              text: `**Agent process ended.**\n\n${detail}\n\nThe turn is no longer live. Warm the agent again (focus composer / send) or start a new session.`,
              createdAt: new Date().toISOString(),
            },
          ];
        });
        setAvailableSessions((current) =>
          current.map((session) =>
            session.id === payload.sessionId ? { ...session, status: "exited" } : session
          )
        );
        if (payload.sessionId === currentSessionIdRef.current) {
          setSessionCapabilities(null);
        }
        // Drop dead process bookkeeping so the next warm can respawn cleanly.
        void stopAcpSession(payload.sessionId).catch(() => undefined);
        pushDebug({
          sessionId: payload.sessionId,
          level: "error",
          source: "acp",
          summary: "process/ended",
          detail,
        });
      }

      if (payload.kind === "error" && (payload.method === "rpc/response" || payload.method == null)) {
        // Surface auth/turn failures in Clean (Claude often returns
        // { error: { message: "Authentication required" } } with no message chunks).
        const errText = formatAcpRpcError(payload.data);
        if (errText) {
          const classified = classifyAgentError(errText);
          let body = formatClassifiedError(classified);
          if (classified.kind === "auth") {
            body += "\n\nClaude 未登录时可点 Clean 横幅 **Sign in**，或终端执行 `claude auth login` 后新建会话。";
          }
          setLiveEvents((current) => {
            // Avoid spamming the same auth error on every retry.
            const last = current[current.length - 1];
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
        setAvailableSessions((current) =>
          current.map((session) =>
            session.id === payload.sessionId ? { ...session, status: "error" } : session
          )
        );
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
          setPermissionPrompt({
            requestId,
            sessionId: payload.sessionId,
            title: typeof data.title === "string" ? data.title : "Permission required",
            detail: typeof data.detail === "string" ? data.detail : null,
            options,
          });
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
    }).then((dispose) => {
      if (disposed) {
        dispose();
        return;
      }
      unlistenAcp = dispose;
    });

    return () => {
      disposed = true;
      unlistenAcp?.();
      for (const t of cancelWatchdogsRef.current.values()) clearTimeout(t);
      cancelWatchdogsRef.current.clear();
    };
  }, [pushDebug, touchActivity]);
  // note: pushDebug is stable via useCallback

  /** Debounced rewrite of Clean transcript JSONL per session. */
  useEffect(() => {
    if (!isTauriRuntime()) return;
    const sessionIds = new Set(liveEvents.map((e) => e.sessionId));
    for (const sessionId of sessionIds) {
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
    if (!session || !shouldAutoRenameLabel(session.label)) return;
    const label = titleFromUserText(text);
    setAvailableSessions((current) =>
      current.map((s) => (s.id === sessionId ? { ...s, label } : s))
    );
    void updateSessionLabel(sessionId, label).catch(() => undefined);
  }, []);

  /** Manual rename (shelf / tab) — always persists, stops future auto-title. */
  const handleRenameSession = useCallback((sessionId: string, label: string) => {
    const next = label.trim() || "New session";
    setAvailableSessions((current) =>
      current.map((s) => (s.id === sessionId ? { ...s, label: next } : s))
    );
    void updateSessionLabel(sessionId, next).catch(() => undefined);
  }, []);

  const loadSessionTranscript = useCallback(async (sessionId: string) => {
    if (!isTauriRuntime()) return;
    if (transcriptLoadedRef.current.has(sessionId)) return;
    transcriptLoadedRef.current.add(sessionId);
    const raw = await loadTranscript(sessionId);
    const parsed = parseTranscriptEvents(raw);
    if (parsed.length === 0) return;
    setLiveEvents((current) => {
      const hasLive = current.some((e) => e.sessionId === sessionId);
      if (hasLive) return current;
      return [...current.filter((e) => e.sessionId !== sessionId), ...parsed];
    });
  }, []);

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
    if (!q) {
      setSearchHitIds(null);
      return;
    }
    const hits = await searchSessions(q);
    setSearchHitIds(hits);
  }, []);

  const refreshClaudeAuth = useCallback(async () => {
    const probe = await probeAgentAuth("claude-code");
    if (!probe) return;
    if (probe.loggedIn === false || probe.status === "logged_out") {
      setClaudeAuthHint(probe.message || "Claude is not logged in.");
    } else {
      setClaudeAuthHint(null);
      if (authPollRef.current) {
        clearInterval(authPollRef.current);
        authPollRef.current = null;
      }
      setSignInBusy(false);
    }
  }, []);

  // Claude auth probe when Claude session is active.
  useEffect(() => {
    const agent =
      availableAgents.find((a) => a.id === (availableSessions.find((s) => s.id === currentSessionId)?.agentId)) ??
      availableAgents[0];
    if (agent?.id !== "claude-code") {
      setClaudeAuthHint(null);
      return;
    }
    let cancelled = false;
    void refreshClaudeAuth().then(() => {
      if (cancelled) return;
    });
    return () => {
      cancelled = true;
      if (authPollRef.current) {
        clearInterval(authPollRef.current);
        authPollRef.current = null;
      }
    };
  }, [availableAgents, availableSessions, currentSessionId, refreshClaudeAuth]);

  const handleClaudeSignIn = useCallback(async () => {
    setSignInBusy(true);
    pushDebug({
      level: "info",
      source: "auth",
      summary: "start Claude login",
    });
    const result = await startAgentLogin("claude-code");
    if (!result?.started) {
      setSignInBusy(false);
      setClaudeAuthHint(
        result?.message ||
          "Could not start Claude login. Install the Claude CLI and try again."
      );
      return;
    }
    setClaudeAuthHint(
      result.message ||
        "Complete sign-in in the browser window, then return here."
    );
    // Poll until logged in or timeout (~2 min).
    if (authPollRef.current) clearInterval(authPollRef.current);
    let ticks = 0;
    authPollRef.current = setInterval(() => {
      ticks += 1;
      void refreshClaudeAuth();
      if (ticks >= 40) {
        if (authPollRef.current) {
          clearInterval(authPollRef.current);
          authPollRef.current = null;
        }
        setSignInBusy(false);
      }
    }, 3000);
  }, [pushDebug, refreshClaudeAuth]);

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

    // OpenCode: probe the selected model’s provider balance API.
    if (agent?.id === "opencode") {
      const model =
        activeModelId ??
        sessionCapabilities?.currentModel ??
        null;
      void refreshProviderBalance(sid, model);
      return;
    }

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
   * After an AI turn ends (`rpc/response`), refresh the Usage panel once.
   * Throttled so multi-chunk finalization / rapid turns don't spam probes.
   */
  const refreshUsageAfterTurn = useCallback(
    (sessionId: string) => {
      if (!sessionId || sessionId !== currentSessionIdRef.current) return;
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
    [handleUsageRefresh],
  );
  const refreshUsageAfterTurnRef = useRef(refreshUsageAfterTurn);
  refreshUsageAfterTurnRef.current = refreshUsageAfterTurn;

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    document.documentElement.style.colorScheme = theme;
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
        const health = activityHealth(session.status, last, now);
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
    () => availableProjects.find((project) => project.id === currentProjectId) ?? availableProjects[0] ?? (isTauriRuntime() ? undefined : projects[0]),
    [availableProjects, currentProjectId]
  );

  const projectSessions = useMemo(
    () => availableSessions.filter((session) => session.projectId === currentProject?.id),
    [availableSessions, currentProject]
  );

  const currentSession = useMemo(
    () =>
      projectSessions.find((session) => session.id === currentSessionId) ??
      projectSessions[0],
    [currentSessionId, projectSessions]
  );

  const displaySession = useMemo((): Session => {
    return (
      currentSession ?? {
        id: `session-empty-${currentProject?.id ?? "none"}`,
        projectId: currentProject?.id ?? "",
        agentId: availableAgents[0]?.id ?? agents[0].id,
        label: "New session",
        cwd: currentProject?.rootPath ?? "",
        status: "exited" as const,
        processId: null,
        startedAt: "",
        lastActiveAt: "",
        transcriptPath: "",
        handoffPath: "",
        viewMode: "clean" as const,
      }
    );
  }, [availableAgents, currentProject, currentSession]);

  const currentAgent = useMemo(
    () => availableAgents.find((agent) => agent.id === displaySession.agentId) ?? availableAgents[0] ?? agents[0],
    [availableAgents, displaySession]
  );

  const currentEvents = useMemo(() => {
    return liveEvents.filter((event) => event.sessionId === displaySession.id);
  }, [liveEvents, displaySession.id]);

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
          : null;

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
    lastProviderProbeKey.current = "";
    void loadSessionTranscript(nextSession.id);
  };

  const setSessionStatusById = useCallback((sessionId: string, status: Session["status"]) => {
    setAvailableSessions((current) =>
      current.map((session) => (session.id === sessionId ? { ...session, status } : session))
    );
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
    const nextOpenIds = openSessionIds.filter((id) => id !== sessionId);
    if (nextOpenIds.length === 0) {
      void createSessionForProject(currentProject?.id ?? availableProjects[0]?.id ?? "");
      return;
    }

    setOpenSessionIds(nextOpenIds);
    if (sessionId === currentSessionId) {
      const nextSession = availableSessions.find((session) => session.id === nextOpenIds[nextOpenIds.length - 1]);
      if (nextSession) openSession(nextSession);
    }
  };

  const deleteSession = (sessionId: string) => {
    const deletedSession = availableSessions.find((session) => session.id === sessionId);
    if (deletedSession && (deletedSession.status === "starting" || deletedSession.status === "running" || deletedSession.status === "waiting")) {
      void stopAcpSession(sessionId).catch(() => undefined);
    }
    if (deletedSession) void deleteSessionApi(deletedSession.projectId, sessionId).catch(() => undefined);
    setAvailableSessions((current) => current.filter((session) => session.id !== sessionId));
    setOpenSessionIds((current) => current.filter((id) => id !== sessionId));
    setSessionUsageById((current) => {
      if (!(sessionId in current)) return current;
      const next = { ...current };
      delete next[sessionId];
      return next;
    });
    if (sessionId === currentSessionId) {
      const nextSession = availableSessions.find((session) => session.id !== sessionId && session.projectId === currentProject?.id);
      if (nextSession) openSession(nextSession);
      else void createSessionForProject(currentProject?.id ?? availableProjects[0]?.id ?? "");
    }
  };

  const refreshChangedFiles = useCallback(async () => {
    if (!currentProjectId) {
      setChangedFiles([]);
      setChangedFilesNote(null);
      return;
    }
    try {
      const files = await getChangedFiles(currentProjectId);
      setChangedFiles(files);
      setChangedFilesNote(files.length === 0 ? "No local changes (or not a git repo)." : null);
    } catch (error) {
      setChangedFiles([]);
      setChangedFilesNote(error instanceof Error ? error.message : String(error));
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
    if (!currentProjectId) {
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
      if (!currentProjectId) return;
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
      if (!currentProjectId) return;
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
      if (!currentProjectId) return;
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

  const handleAgentChange = async (agentId: string) => {
    if (!currentProject) return;

    // If no current session, create one bound to this agent
    if (!currentSession || !currentSessionId) {
      void createSessionForProject(currentProject.id, agentId);
      return;
    }

    if (currentSession.agentId === agentId) return;

    // HARD RULE: session.agentId is the only source of truth for which agent
    // owns this dialog. Persist it so reopening the tab cannot show OpenCode
    // while still listing Claude models.
    const sid = currentSessionId;
    const sourceAgentId = currentSession.agentId;
    const oldAgent = availableAgents.find((a) => a.id === sourceAgentId);

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
        projectId: currentProject.id,
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
    // Keep Clean history for this dialog; only the agent process is replaced.
    setSessionUsageById((current) => ({ ...current, [sid]: emptySessionUsage() }));
    acpBootstrapRef.current.delete(sid);
    setViewMode("clean");
    setAvailableSessions((current) =>
      current.map((s) =>
        s.id === sid
          ? {
              ...s,
              agentId,
              status: "exited" as const,
              processId: null,
              preferredModel: null,
              preferredMode: null,
              preferredEffort: null,
              preferredEffortId: null,
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

    // Tear down previous transport in the background.
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

  const handleInterrupt = useCallback(async () => {
    if (!currentSessionId) return;
    const session = sessionsRef.current.find((s) => s.id === currentSessionId);
    const agent =
      agentsRef.current.find((a) => a.id === session?.agentId) ??
      agentsRef.current[0];
    if (!agent) return;

    const sid = currentSessionId;
    let cancelNote = "";

    // Esc×2 fires regardless of state, and an idle agent has nothing to say —
    // silence only means "ignored the cancel" when a turn was actually live.
    const midTurn = session?.status === "running";
    // True turn cancel — do not kill the session process.
    const cancelAt = Date.now();
    try {
      await cancelAcpSession(sid);
      cancelNote = "Cancel request sent to the agent.";
      if (midTurn) armCancelWatchdog(sid, cancelAt);
    } catch (error) {
      // The pipe or the process is already gone — only a restart recovers.
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
    // Always free the composer — even if the agent ignored cancel.
    setSessionStatusById(sid, "waiting");
    touchActivity(sid);
    setLiveEvents((current) => [
      ...markOpenTools(current, sid, "cancelled"),
      {
        type: "assistant_message" as const,
        sessionId: sid,
        text: `**Interrupted.**\n\n${cancelNote}\n\nYou can send a new message now.`,
        createdAt: new Date().toISOString(),
      },
    ]);
    pushDebug({
      sessionId: sid,
      level: "info",
      source: "interrupt",
      summary: "ACP cancel (interrupt)",
    });
  }, [armCancelWatchdog, currentSessionId, pushDebug, setSessionStatusById, touchActivity]);

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
    diffPreview,
    projectDialogOpen,
    projectAdding,
    handleInterrupt,
    handlePermissionChoose,
  ]);

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
        const classified = classifyAgentError(error);
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
    const outside = await checkOutsideProjectPaths(projectId, candidates).catch(() => []);
    if (outside.length > 0) {
      setPathGrantPrompt({
        paths: outside,
        text: composed,
        sessionId: sid,
        imageAttachments,
      });
      return;
    }

    setQuotePins([]);
    await performSend(sid, composed, imageAttachments);
  };

  const performSend = async (
    sid: string,
    composed: string,
    imageAttachments: ImageAttachment[] = [],
  ) => {
    try {
      // An agent switch leaves handoff notes waiting — attach them to this send
      // (the composer stays clean; only the wire payload carries them).
      const handoff = pendingHandoff(liveEventsRef.current, sid);

      // Snapshot current Composer config for metadata
      const caps = sessionCapabilities;
      const modeId = caps?.currentMode;
      const modeLabel = modeId ? (caps?.modes.find((m) => m.id === modeId)?.label ?? modeId) : undefined;
      const effortId = displaySession.preferredEffortId;
      const effortLabelVal = effortId
        ? (caps?.effortOptions?.find((o) => o.id === effortId)?.label ?? effortId)
        : displaySession.preferredEffort != null
          ? effortLabel(displaySession.preferredEffort)
          : undefined;
      sendMetaRef.current = {
        agentId: currentAgent.id,
        agentLabel: currentAgent.label,
        modelId: activeModelId ?? undefined,
        modelLabel: activeModelId ?? undefined,
        modeLabel,
        effortLabel: effortLabelVal,
      };
      delete turnStartedAtRef.current[sid];

      // Show the user message immediately; wait for ACP only after that.
      // History injection uses events *before* this message.
      const priorForInject = liveEventsRef.current.filter((e) => e.sessionId === sid);
      setLiveEvents((current) => [
        ...current,
        userMessageEvent(sid, composed, {
          ...(sendMetaRef.current ?? {}),
          attachments:
            imageAttachments.length > 0 ? imageAttachments : undefined,
        }),
      ]);
      renameSessionFromText(sid, composed || imageAttachments[0]?.name || "Image");
      touchActivity(sid);
      pushDebug({
        sessionId: sid,
        level: "info",
        source: "composer",
        summary: `send: ${composed.length > 80 ? `${composed.slice(0, 80)}…` : composed}`,
      });
      // Sending supersedes any pending verdict on the last interrupt — do not
      // let a stale watchdog fire mid-turn and mark this session for restart.
      disarmCancelWatchdog(sid);
      // A cancel the agent never acknowledged leaves the process wedged;
      // prompting it again would just hang. Replace it before sending.
      if (cancelIgnoredRef.current.has(sid)) {
        await restartAcpProcess(sid);
      }
      // Ensure agent is up (may already be warming from keystrokes).
      await ensureAcpReady(sid);

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
        const skillsPrefix = await projectContextPrompt(
          displaySession.projectId || currentProjectId,
          currentAgent.id
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
      await sendAcpPrompt(sid, promptText, imagePaths);
      touchActivity(sid);
      pushDebug({
        sessionId: sid,
        level: "info",
        source: "composer",
        summary: `session/prompt accepted (streaming…)${imagePaths.length ? ` images=${imagePaths.length}` : ""}`,
      });
    } catch (error) {
      setSessionStatusById(sid, "error");
      const classified = classifyAgentError(error);
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
        className={`workspace-grid${leftCollapsed ? " is-left-collapsed" : ""}${rightCollapsed ? " is-right-collapsed" : ""}`}
        style={
          {
            "--left-panel-width": `${leftWidth}px`,
            "--right-panel-width": `${rightWidth}px`,
          } as CSSProperties
        }
      >
        {/* Left sidebar: full height (spans both rows) */}
        <aside className={leftCollapsed ? "left-rail is-collapsed" : "left-rail"} aria-label="Projects and sessions">
          <ProjectShelf
            agents={availableAgents}
            projects={availableProjects}
            sessions={availableSessions}
            currentProjectId={currentProjectId}
            currentSessionId={displaySession.id}
            collapsed={leftCollapsed}
            theme={theme}
            desktopNotifyEnabled={desktopNotifyOn}
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
            onToggleTheme={() => setTheme((current) => (current === "dark" ? "light" : "dark"))}
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
          />
          {!leftCollapsed && (
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
            onNewTab={() => createSessionForProject(currentProject?.id ?? "")}
            onRenameSession={handleRenameSession}
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
            authBanner={
              currentAgent.id === "claude-code"
                ? claudeAuthHint
                : null
            }
            onSignIn={currentAgent.id === "claude-code" ? handleClaudeSignIn : undefined}
            signInBusy={signInBusy}
            onTabSelect={openSession}
            onTabClose={closeSessionTab}
            onNewTab={() => createSessionForProject(currentProject?.id ?? "")}
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
            onSend={(text, droppedPaths) => void handleSend(text, droppedPaths)}
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
        </section>

        <ContextPanel
          collapsed={rightCollapsed}
          onCollapse={() => setRightCollapsed(true)}
          onExpand={() => setRightCollapsed(false)}
          usage={usage}
          onUsageRefresh={handleUsageRefreshForce}
          changedFiles={changedFiles}
          changedFilesNote={changedFilesNote}
          onRefreshChangedFiles={() => void refreshChangedFiles()}
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
          todoItems={todoItems}
          onTodosChange={handleTodosChange}
          onAbsorbPlan={handleAbsorbPlan}
          onSendTodosToAi={() => prefillComposer(formatTodosForPrompt(todoItems))}
          onRequestAiTodoUpdate={() => prefillComposer(formatAiUpdatePrompt(todoItems))}
          onPrepareAiTodoMerge={handlePrepareAiTodoMerge}
          resizeDragging={resizingSide === "right"}
          onResizeStart={() => setResizingSide("right")}
        />
      </div>
      {permissionPrompt && (
        <PermissionDialog
          prompt={permissionPrompt}
          busy={permissionBusy}
          onChoose={(optionId) => void handlePermissionChoose(optionId)}
        />
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
            <pre className="diff-dialog__body">{diffPreview.text}</pre>
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
