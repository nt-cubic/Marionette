import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { Expand, Globe, Plus, Search, SendHorizontal, Shrink, Square } from "lucide-react";
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ClipboardEvent as ReactClipboardEvent,
  type DragEvent as ReactDragEvent,
} from "react";
import {
  expandAcpConfigAttempts,
  getAcpSupplement,
  mergeAcpCapabilities,
  normalizeAgentModeId,
} from "../lib/acpSupplements";
import {
  addCustomAgent,
  agentPreflight,
  getSessionCapabilities,
  installAgent,
  isTauriRuntime,
  listAgentCommands,
  pickFiles,
  removeCustomAgent,
  savePastedImage,
  sendAcpPrompt,
  updateAcpSession,
  type PreflightResult,
} from "../lib/api";
import {
  getCachedAgentVersions,
  patchAgentVersion,
  refreshAgentVersions,
  subscribeAgentVersions,
} from "../lib/agentVersionCache";
import {
  cacheAgentCapabilities,
  cachedCapabilitiesFor,
  clearAgentCapabilities,
  invalidateCapsIfAgentUpdated,
} from "../lib/capabilityCache";
import { detectCapabilityDrift } from "../lib/capabilityDrift";
import {
  modelTooltip,
  prettyModelLabel,
  prettyModelTrigger,
} from "../lib/modelLabel";
import {
  applySlashCommand,
  filterSlashCommands,
  resolveSlashCommands,
  slashQueryAtCursor,
} from "../lib/slashCommands";
import {
  suggestFor,
  suggestionRoundKey,
  suggestionsEnabled,
  type Suggestion,
} from "../lib/suggestions";
import {
  applyDelegateCandidate,
  delegateQueryAtCursor,
  filterDelegateCandidates,
  type DelegateCandidate,
} from "../lib/delegate";
import { ProviderConfigDialog } from "./ProviderConfigDialog";
import { ImageAnnotator } from "./ImageAnnotator";
import { recordModelUsage, getRecentModels, recordLastUsedDefaults } from "../lib/recentModels";
import {
  attachmentFromPath,
  isImagePath,
  type ImageAttachment,
  type ImageMark,
} from "../lib/imageAttachments";

import type {
  AcpEvent,
  AgentCommandStatus,
  AgentConfig,
  AgentVersionInfo,
  AvailableCommand,
  CapabilitySnapshot,
  ModelDef,
  SessionComposerPrefs,
  SessionEvent,
  SessionStatus,
} from "../lib/types";

type ComposerProps = {
  agent: AgentConfig;
  agents: AgentConfig[];
  currentAgentId: string;
  sessionId: string;
  sessionStatus: SessionStatus;
  /** Caps pushed from ACP start (avoids invoke race). */
  capabilities?: CapabilitySnapshot | null;
  /** Current session transcript (for suggestion chips). */
  sessionEvents?: SessionEvent[];
  /** Prefill draft (handoff). Applied once per token; does not auto-send. */
  prefillText?: string | null;
  prefillToken?: number;
  /** Disk prefs for this dialog — restored when caps advertise the option. */
  sessionPrefs?: SessionComposerPrefs | null;
  /** Persist model/mode/effort after a successful local change. */
  onSessionPrefsChange?: (prefs: SessionComposerPrefs) => void;
  onAgentChange: (agentId: string) => void;
  onInterrupt: () => void;
  /**
   * `droppedPaths` are the exact absolute paths the OS reported on drop.
   * They are passed separately because the parent's outside-project check
   * otherwise has to re-find them by parsing the composed text, and the path
   * regex deliberately stops at whitespace — so `…\Screen Shot.png` came back
   * as `…\Screen`, failed the exists() check, and silently skipped the prompt.
   */
  onSend: (
    text: string,
    droppedPaths?: string[],
    imageAttachments?: import("../lib/imageAttachments").ImageAttachment[],
    opts?: {
      forceWebSearch?: boolean;
      /**
       * Composer chip snapshot at send time. Parent must not re-derive mode
       * from stale sessionCapabilities — mode switches skip immediate caps
       * reload (agent often echoes the previous mode for a beat).
       */
      modeId?: string | null;
      modeLabel?: string | null;
      modelId?: string | null;
      modelLabel?: string | null;
      effortLabel?: string | null;
    },
  ) => void;
  /** Notify parent when the active model id changes (for provider balance probes). */
  onActiveModelChange?: (modelId: string | null) => void;
  /** Lazy ACP: warm the agent process without blocking the composer. */
  onWarmAgent?: () => void;
  /** Ensure ACP is ready before set_config_option (returns false if failed). */
  onEnsureAgentReady?: () => Promise<boolean>;
  /** Last stream activity (ms) — escalates busy strip when the turn goes quiet. */
  lastActivityAt?: number | null;
  /**
   * Provider keys were edited. The agent process reads auth.json only at
   * startup, so the parent must replace it for the new key to take effect.
   */
  onProviderKeysChanged?: () => void | Promise<void>;
  /**
   * Agent CLI binary is about to change / has changed.
   * - `stop`: release the live process so Windows can replace the binary
   * - `restart`: load the new binary into this dialog (session/new)
   */
  onAgentBinaryUpdated?: (
    agentId: string,
    phase: "stop" | "restart",
  ) => void | Promise<void>;
  /** Reload agent list after custom agent add/remove. */
  onAgentsReload?: () => void | Promise<void>;
  /**
   * Live ACP-advertised slash commands for this dialog. Static fallbacks apply
   * when null/empty (see `resolveSlashCommands`).
   */
  availableCommands?: AvailableCommand[] | null;
};

/**
 * Agents whose credentials live in OpenCode's `auth.json` — the file the provider
 * key dialog writes. Everything else authenticates its own way.
 */
const OPENCODE_AUTH_AGENTS = new Set(["opencode"]);

function formatRecentTime(timestamp: number): string {
  const diff = Date.now() - timestamp;
  if (diff < 60_000) return "刚刚";
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)} 分钟前`;
  if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)} 小时前`;
  return `${Math.floor(diff / 86_400_000)} 天前`;
}

function effortLabel(value: number): string {
  if (value <= 0.1) return "Low";
  if (value >= 0.9) return "High";
  if (Math.abs(value - 0.5) < 0.1) return "Auto";
  return value < 0.5 ? "Medium" : "High";
}

/** Group model id/label like `opencode/foo` or `OpenCode Zen/Bar` by provider. */
function providerOf(model: ModelDef): string {
  const id = model.id.toLowerCase();
  // Claude ACP uses family aliases, not provider/model paths
  if (
    id === "default" ||
    id === "opus" ||
    id === "sonnet" ||
    id === "haiku" ||
    id === "fable" ||
    id === "best" ||
    id === "opusplan" ||
    id.startsWith("opus") ||
    id.startsWith("sonnet") ||
    id.startsWith("haiku") ||
    id.startsWith("fable") ||
    id.startsWith("claude-") ||
    id.includes("claude-")
  ) {
    return "Claude";
  }

  const idPart = model.id.includes("/") ? model.id.split("/")[0] : "";
  const labelPart = model.label.includes("/") ? model.label.split("/")[0] : "";
  const raw = (labelPart || idPart || "Models").trim();
  if (!raw) return "Models";
  const key = raw.toLowerCase();
  if (key.startsWith("opencode")) return "OpenCode";
  if (key.startsWith("deepseek")) return "DeepSeek";
  if (key.startsWith("openrouter")) return "OpenRouter";
  if (key === "zai" || key.startsWith("z.ai") || key.startsWith("glm")) return "Z.AI";
  if (key.startsWith("anthropic") || key.startsWith("claude")) return "Claude";
  if (key.startsWith("openai") || key.startsWith("gpt")) return "OpenAI";
  if (key.startsWith("google") || key.startsWith("gemini")) return "Google";
  if (key.startsWith("xai") || key.startsWith("grok")) return "xAI";
  if (key.startsWith("logfare")) return "Logfare";
  if (key.startsWith("kimi") || key.startsWith("moonshot")) return "Kimi";
  if (key.startsWith("minimax")) return "MiniMax";
  if (key.startsWith("qwen") || key.startsWith("alibaba")) return "Qwen";
  // Bare family names from Claude labels
  if (/^(opus|sonnet|haiku|fable)\b/i.test(model.label)) return "Claude";
  return raw;
}

function shortModelName(model: ModelDef): string {
  return prettyModelLabel(model, { maxLen: 40 });
}

function shortTriggerLabel(label: string | null, id: string | null): string {
  return prettyModelTrigger(label, id);
}

function groupModels(models: ModelDef[]): { provider: string; models: ModelDef[] }[] {
  const map = new Map<string, ModelDef[]>();
  for (const m of models) {
    const p = providerOf(m);
    const list = map.get(p);
    if (list) list.push(m);
    else map.set(p, [m]);
  }
  return [...map.entries()]
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([provider, items]) => ({
      provider,
      models: items.slice().sort((x, y) => shortModelName(x).localeCompare(shortModelName(y))),
    }));
}

function displayModeSafe(caps: CapabilitySnapshot): string | null {
  return caps.currentMode ?? (caps.modes.length > 0 ? caps.modes[0].id : null);
}

/** Soft visual family for the mode rail (muted, not rainbow). */
function modeTone(modeId: string | null | undefined): string {
  const id = (modeId ?? "").toLowerCase();
  if (!id) return "default";
  if (id.includes("plan")) return "plan";
  if (id.includes("ask") || id.includes("chat") || id.includes("talk")) return "ask";
  if (id.includes("debug") || id.includes("review")) return "debug";
  // Full access / unrestricted: distinct from normal agent (workspace sandbox).
  if (id.includes("full-access") || id.includes("full_access") || id.includes("bypass") || id.includes("yolo")) {
    return "build";
  }
  if (
    id.includes("build") ||
    id.includes("agent") ||
    id.includes("code") ||
    id.includes("edit") ||
    id.includes("default") ||
    id.includes("auto")
  ) {
    return "build";
  }
  return "default";
}

const COMPOSER_HEIGHT_MIN = 100;
const COMPOSER_HEIGHT_MAX = 480;
const COMPOSER_HEIGHT_DEFAULT = 112;
const COMPOSER_HEIGHT_KEY = "marionette-composer-height";
/** Enter sends immediately (vs default Ctrl+Enter). */
const SEND_ON_ENTER_KEY = "marionette-send-on-enter";

function readComposerHeight(): number {
  try {
    const raw =
      window.localStorage.getItem(COMPOSER_HEIGHT_KEY) ??
      window.localStorage.getItem("agentshell-composer-height");
    const n = raw ? Number(raw) : NaN;
    if (Number.isFinite(n)) {
      return Math.min(COMPOSER_HEIGHT_MAX, Math.max(COMPOSER_HEIGHT_MIN, n));
    }
  } catch {
    // ignore
  }
  return COMPOSER_HEIGHT_DEFAULT;
}

function readSendOnEnter(): boolean {
  try {
    return window.localStorage.getItem(SEND_ON_ENTER_KEY) === "1";
  } catch {
    return false;
  }
}

function clampComposerHeight(n: number): number {
  return Math.min(COMPOSER_HEIGHT_MAX, Math.max(COMPOSER_HEIGHT_MIN, Math.round(n)));
}

function applyCapabilities(
  result: CapabilitySnapshot | null | undefined,
  setters: {
    setCaps: (c: CapabilitySnapshot | null) => void;
    setCurrentMode: (m: string | null) => void;
    setCurrentModel: (m: string | null) => void;
    setCurrentEffort: (e: number | null) => void;
    setCurrentEffortId: (e: string | null) => void;
  },
) {
  if (!result) return;
  // Normalize in case arrays arrived missing (event bridge edge cases)
  const normalized: CapabilitySnapshot = {
    ...result,
    modes: result.modes ?? [],
    models: result.models ?? [],
    thinkingEffort: result.thinkingEffort ?? null,
    effortOptions: result.effortOptions ?? [],
    supportsCancel: result.supportsCancel ?? false,
    currentMode: result.currentMode ?? null,
    currentModel: result.currentModel ?? null,
    currentEffort: result.currentEffort ?? null,
    currentEffortId: result.currentEffortId ?? null,
    modelConfigId: result.modelConfigId ?? null,
    modeConfigId: result.modeConfigId ?? null,
    effortConfigId: result.effortConfigId ?? null,
  };
  setters.setCaps(normalized);
  setters.setCurrentMode(normalized.currentMode);
  setters.setCurrentModel(normalized.currentModel);
  setters.setCurrentEffortId(normalized.currentEffortId);
  if (normalized.currentEffort != null) {
    setters.setCurrentEffort(normalized.currentEffort);
  } else if (normalized.thinkingEffort) {
    setters.setCurrentEffort(normalized.thinkingEffort.default);
  } else {
    setters.setCurrentEffort(null);
  }
}

/** A pick the user just made, held while the agent echoes the old value back. */
type OptionPins = {
  model?: string;
  mode?: string;
  effortId?: string;
  effort?: number;
};

/** How long a local pick outranks server echoes (agents settle within ~1–2s). */
const OPTION_PIN_MS = 4000;

/**
 * Keep the dialog's saved choice on the chips even when the agent reports its
 * own defaults. Display only — the snapshot stays live, so the restore pass can
 * still tell what needs pushing (and what to revert to).
 *
 * This is what makes the controls stop flickering: disk prefs are the dialog's
 * truth, and every snapshot (cache, handshake, set_config echo) is filtered
 * through them instead of overwriting them.
 */
function applyPreferredDisplay(
  caps: CapabilitySnapshot,
  prefs: SessionComposerPrefs | null | undefined,
  setters: {
    setCurrentMode: (m: string | null) => void;
    setCurrentModel: (m: string | null) => void;
    setCurrentEffort: (e: number | null) => void;
    setCurrentEffortId: (e: string | null) => void;
  },
  agentId?: string | null,
) {
  if (!prefs) return;
  const model = prefs.preferredModel?.trim();
  const modeRaw = prefs.preferredMode?.trim();
  const mode = modeRaw
    ? agentId
      ? normalizeAgentModeId(agentId, modeRaw)
      : modeRaw
    : undefined;
  const effortId = prefs.preferredEffortId?.trim();
  const effort = prefs.preferredEffort;
  if (model && (caps.models ?? []).some((m) => m.id === model)) setters.setCurrentModel(model);
  if (mode && (caps.modes ?? []).some((m) => m.id === mode)) setters.setCurrentMode(mode);
  if (effortId && (caps.effortOptions ?? []).some((o) => o.id === effortId)) {
    setters.setCurrentEffortId(effortId);
  } else if (
    typeof effort === "number" &&
    Number.isFinite(effort) &&
    caps.thinkingEffort != null &&
    effort >= caps.thinkingEffort.min &&
    effort <= caps.thinkingEffort.max
  ) {
    setters.setCurrentEffort(effort);
  }
}

/** A pick made seconds ago outranks both the snapshot and the saved prefs. */
function applyPinnedDisplay(
  pin: { pins: OptionPins; until: number } | null,
  setters: {
    setCurrentMode: (m: string | null) => void;
    setCurrentModel: (m: string | null) => void;
    setCurrentEffort: (e: number | null) => void;
    setCurrentEffortId: (e: string | null) => void;
  },
) {
  if (!pin || Date.now() >= pin.until) return;
  if (pin.pins.model != null) setters.setCurrentModel(pin.pins.model);
  if (pin.pins.mode != null) setters.setCurrentMode(pin.pins.mode);
  if (pin.pins.effortId != null) setters.setCurrentEffortId(pin.pins.effortId);
  if (pin.pins.effort != null) setters.setCurrentEffort(pin.pins.effort);
}

/**
 * Unsent composer text survives dialog switches. Composer remounts on
 * `sessionId:agentId` (hard rule so caps never leak); this cache is keyed by
 * session only so draft follows the dialog, not the agent chip.
 */
const draftBySessionId = new Map<string, string>();

function readDraftCache(sessionId: string): string {
  if (!sessionId || sessionId.startsWith("session-empty-")) return "";
  return draftBySessionId.get(sessionId) ?? "";
}

function writeDraftCache(sessionId: string, text: string) {
  if (!sessionId || sessionId.startsWith("session-empty-")) return;
  if (text) draftBySessionId.set(sessionId, text);
  else draftBySessionId.delete(sessionId);
}

export function Composer({
  agent,
  agents,
  currentAgentId,
  sessionId,
  sessionStatus,
  capabilities: capabilitiesProp,
  sessionEvents = [],
  prefillText = null,
  prefillToken = 0,
  sessionPrefs = null,
  onSessionPrefsChange,
  onAgentChange,
  onInterrupt,
  onSend,
  onActiveModelChange,
  onWarmAgent,
  onEnsureAgentReady,
  lastActivityAt = null,
  onProviderKeysChanged,
  onAgentBinaryUpdated,
  onAgentsReload,
  availableCommands = null,
}: ComposerProps) {
  /**
   * Offline-first controls: the last catalog this agent advertised, with this
   * dialog's saved model/mode/effort overlaid. Computed once per mount (App
   * remounts Composer whenever dialog identity changes) so the chips are on
   * screen in the first paint instead of appearing after a handshake.
   */
  const initialCaps = useMemo(
    () => cachedCapabilitiesFor(agent.id, sessionPrefs),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [],
  );
  const [caps, setCaps] = useState<CapabilitySnapshot | null>(initialCaps);
  const [currentMode, setCurrentMode] = useState<string | null>(initialCaps?.currentMode ?? null);
  const [currentModel, setCurrentModel] = useState<string | null>(initialCaps?.currentModel ?? null);
  const [currentEffort, setCurrentEffort] = useState<number | null>(initialCaps?.currentEffort ?? null);
  const [currentEffortId, setCurrentEffortId] = useState<string | null>(
    initialCaps?.currentEffortId ?? null,
  );
  /** Fresh chip snapshot for side effects that must not dep-track every setter. */
  const chipStateRef = useRef<{
    model: string | null;
    mode: string | null;
    effortId: string | null;
    effort: number | null;
  }>({
    model: currentModel,
    mode: currentMode,
    effortId: currentEffortId,
    effort: currentEffort,
  });
  useEffect(() => {
    chipStateRef.current = {
      model: currentModel,
      mode: currentMode,
      effortId: currentEffortId,
      effort: currentEffort,
    };
  }, [currentModel, currentMode, currentEffortId, currentEffort]);
  // Grok always-approve: not an ACP mode. Default false (ask) until prefs/restore say otherwise.
  const [alwaysApprove, setAlwaysApprove] = useState<boolean>(
    sessionPrefs?.preferredAlwaysApprove === true,
  );
  // Keep chip in sync when parent reloads session prefs (disk / new-tab inherit).
  useEffect(() => {
    if (typeof sessionPrefs?.preferredAlwaysApprove === "boolean") {
      setAlwaysApprove(sessionPrefs.preferredAlwaysApprove);
    }
  }, [sessionPrefs?.preferredAlwaysApprove]);
  /** True once caps came from a live agent — cached caps must not act as live. */
  const [capsLive, setCapsLive] = useState(false);
  const [menu, setMenu] = useState<"mode" | "model" | "effort" | "agent" | "send" | null>(null);
  const [modelQuery, setModelQuery] = useState("");
  const [draft, setDraft] = useState(() => readDraftCache(sessionId));
  /** Image attachments as Codex-style pills (not raw paths in the textarea). */
  const [imageAttachments, setImageAttachments] = useState<ImageAttachment[]>([]);
  const [annotatingId, setAnnotatingId] = useState<string | null>(null);
  /**
   * Force web-search instruction on send (prompt prefix only — not a network firewall).
   * Remounted per dialog (`key` on Composer), so it is session-scoped for free.
   */
  const [forceWebSearch, setForceWebSearch] = useState(false);
  /** Draft snapshot right after a file drop — treats path-only draft as "empty" for chips. */
  const [draftAfterDrop, setDraftAfterDrop] = useState<string | null>(null);
  /** Paths from the most recent drop (feeds drop-source chips). */
  const [suggestDroppedPaths, setSuggestDroppedPaths] = useState<string[]>([]);
  /** Round key the user dismissed by typing (show once per event tail). */
  const [dismissedSuggestKey, setDismissedSuggestKey] = useState<string | null>(null);
  /** Mirror of composingRef so chip visibility re-renders on IME start/end. */
  const [isComposing, setIsComposing] = useState(false);
  const [composerHeight, setComposerHeight] = useState(readComposerHeight);
  const [resizingComposer, setResizingComposer] = useState(false);
  /** Enter sends immediately instead of Ctrl+Enter. Persisted globally. */
  const [sendOnEnter, setSendOnEnter] = useState(readSendOnEnter);
  const [showProviderDialog, setShowProviderDialog] = useState(false);
  /** A key was added/removed this visit — the agent must restart to see it. */
  const [providerKeysDirty, setProviderKeysDirty] = useState(false);
  const [flashError, setFlashError] = useState("");
  const [dropActive, setDropActive] = useState(false);
  /** CLI availability per agent — loaded when the agent menu opens. */
  const [agentStatuses, setAgentStatuses] = useState<Record<string, AgentCommandStatus>>({});
  /** Versions live in a module cache so remounts / reopening the menu keep them. */
  const [agentVersionInfo, setAgentVersionInfo] = useState<Record<string, AgentVersionInfo>>(
    () => getCachedAgentVersions(),
  );
  const [installingAgentId, setInstallingAgentId] = useState<string | null>(null);
  const [installNote, setInstallNote] = useState("");
  const [preflightById, setPreflightById] = useState<Record<string, PreflightResult>>({});
  /** Highlighted row in the `/` autocomplete list. */
  const [slashIndex, setSlashIndex] = useState(0);
  /** Caret position — drives `/` token detection without remounting the textarea. */
  const [caret, setCaret] = useState(0);
  /** One drift toast per (session, agent) after live caps land. */
  const driftCheckedKey = useRef("");
  const errorTimer = useRef<ReturnType<typeof setTimeout>>();
  const updating = useRef(false);
  const modelSearchRef = useRef<HTMLInputElement>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const composerRef = useRef<HTMLElement>(null);
  const dropDepth = useRef(0);
  /** IME composition must not re-render placeholder / warm ACP mid-input. */
  const composingRef = useRef(false);
  const warmedThisFocusRef = useRef(false);
  // Ignore stale getSessionCapabilities responses (starting→null overwriting ready).
  const loadSeq = useRef(0);
  const appliedPrefillToken = useRef(0);
  /** Restore disk prefs once per (session, agent) after caps are known. */
  const prefsRestoredKey = useRef("");
  /** Exact absolute paths from OS drops, kept verbatim for the grant check. */
  const droppedPathsRef = useRef<Set<string>>(new Set());
  /** After the user picks an option, ignore server echoes still reporting the old one. */
  const pinsRef = useRef<{ pins: OptionPins; until: number } | null>(null);
  const composerHeightRef = useRef(composerHeight);
  composerHeightRef.current = composerHeight;
  /** Latest disk prefs, read by effects that must not re-run on every write. */
  const sessionPrefsRef = useRef(sessionPrefs);
  sessionPrefsRef.current = sessionPrefs;

  const pinOptions = useCallback((patch: OptionPins) => {
    const live =
      pinsRef.current && Date.now() < pinsRef.current.until ? pinsRef.current.pins : {};
    pinsRef.current = { pins: { ...live, ...patch }, until: Date.now() + OPTION_PIN_MS };
  }, []);

  /** Drop a pin when the agent rejected the change (revert path). */
  const unpinOption = useCallback((key: keyof OptionPins) => {
    const current = pinsRef.current;
    if (!current) return;
    const pins = { ...current.pins };
    delete pins[key];
    pinsRef.current = Object.keys(pins).length > 0 ? { pins, until: current.until } : null;
  }, []);

  // Persist unsent text across dialog switches (Composer remounts on key change).
  useEffect(() => {
    writeDraftCache(sessionId, draft);
  }, [sessionId, draft]);

  // Handoff / external prefill — never auto-send.
  useEffect(() => {
    if (!prefillText || !prefillToken) return;
    if (appliedPrefillToken.current === prefillToken) return;
    appliedPrefillToken.current = prefillToken;
    setDraft(prefillText);
  }, [prefillText, prefillToken]);

  const capSetters = {
    setCaps,
    setCurrentMode,
    setCurrentModel,
    setCurrentEffort,
    setCurrentEffortId,
  };

  /**
   * `source` tells the difference between what an agent is really offering
   * right now and what we remembered from last time:
   *  - `acp`      live negotiation → refresh the offline cache
   *  - `handshake` live session/new → also refresh the remembered defaults
   *  - `cache`    replay of the offline snapshot (must not count as live)
   */
  const applyCaps = useCallback(
    (
      result: CapabilitySnapshot | null | undefined,
      source: "acp" | "handshake" | "cache" = "acp",
    ) => {
      applyCapabilities(result, capSetters);
      if (!result) return;
      // Layered display: snapshot → this dialog's saved choice → the pick the
      // user just made. Without this the chips flash agent-default → saved →
      // picked on every connect and every set_config echo.
      applyPreferredDisplay(result, sessionPrefsRef.current, capSetters, agent.id);
      applyPinnedDisplay(pinsRef.current, capSetters);
      if (source === "cache") return;
      setCapsLive(true);
      cacheAgentCapabilities(agent.id, result, { handshake: source === "handshake" });
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [agent.id],
  );

  // ── Shared capability loader ────────────────────────────────────────────────
  const loadCapabilities = useCallback(
    (id: string) => {
      const seq = ++loadSeq.current;
      getSessionCapabilities(id).then((result) => {
        if (seq !== loadSeq.current) return;
        // No live ACP session → keep what is on screen (cached catalog / last
        // negotiation). Replacing it with a static supplement stub would drop
        // the real model list and mark a dead agent as live.
        if (!result) return;
        // Merge ACP negotiation with static supplements (e.g. Grok modes).
        const merged = mergeAcpCapabilities(agent.id, result);
        if (merged) applyCaps(merged);
      });
    },
    [agent.id, applyCaps],
  );

  // Persist composer height.
  useEffect(() => {
    try {
      window.localStorage.setItem(COMPOSER_HEIGHT_KEY, String(composerHeight));
    } catch {
      // ignore
    }
  }, [composerHeight]);

  // Persist send-on-Enter preference.
  useEffect(() => {
    try {
      window.localStorage.setItem(SEND_ON_ENTER_KEY, sendOnEnter ? "1" : "0");
    } catch {
      // ignore
    }
  }, [sendOnEnter]);

  // Drag-resize composer from the top edge (no React thrash mid-drag).
  useEffect(() => {
    if (!resizingComposer) return;
    let raf = 0;
    const onMove = (event: MouseEvent) => {
      if (raf) cancelAnimationFrame(raf);
      raf = requestAnimationFrame(() => {
        raf = 0;
        const el = composerRef.current;
        if (!el) return;
        const bottom = el.getBoundingClientRect().bottom;
        const next = clampComposerHeight(bottom - event.clientY);
        composerHeightRef.current = next;
        el.style.height = `${next}px`;
      });
    };
    const onUp = () => {
      if (raf) cancelAnimationFrame(raf);
      setComposerHeight(composerHeightRef.current);
      setResizingComposer(false);
    };
    document.body.classList.add("is-composer-resizing");
    window.addEventListener("mousemove", onMove, { passive: true });
    window.addEventListener("mouseup", onUp);
    return () => {
      if (raf) cancelAnimationFrame(raf);
      document.body.classList.remove("is-composer-resizing");
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };
  }, [resizingComposer]);

  // HARD RULE: capability state is rebound whenever dialog identity changes.
  // Prevents "OpenCode selected but Claude models still listed". Re-seeding from
  // the cache of *this* agent keeps that rule while avoiding an empty toolbar.
  useEffect(() => {
    loadSeq.current += 1;
    setMenu(null);
    setModelQuery("");
    pinsRef.current = null;
    // Keep draft/prefill + remembered height when switching dialog/agent.
    prefsRestoredKey.current = "";
    setCapsLive(false);
    // Rebind from this agent's own last known catalog.
    const rebound = cachedCapabilitiesFor(agent.id, sessionPrefsRef.current);
    if (rebound) {
      applyCaps(rebound, "cache");
      return;
    }
    setCaps(null);
    setCurrentMode(null);
    setCurrentModel(null);
    setCurrentEffort(null);
    setCurrentEffortId(null);
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessionId, agent.id]);

  // Parent-pushed caps from startAcpSession + supplements for missing mode/effort.
  useEffect(() => {
    if (capabilitiesProp) {
      loadSeq.current += 1;
      const merged = mergeAcpCapabilities(agent.id, capabilitiesProp);
      if (merged) applyCaps(merged, "handshake");
    } else {
      // Parent cleared caps (session switch / process died). Keep the last known
      // controls on screen, but the agent is gone: the next connect starts from
      // its own defaults, so this dialog's saved prefs must be pushed again.
      setCapsLive(false);
      prefsRestoredKey.current = "";
    }
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [agent.id, capabilitiesProp, applyCaps]);

  // ── Fetch capabilities when session is ready ────────────────────────────────
  useEffect(() => {
    if (!sessionId || sessionId.startsWith("session-empty-")) {
      return;
    }
    // Do not fetch on "starting": handshake not done yet, null would race-wipe UI.
    // waiting/running: session/new completed and caps are stored server-side.
    if (sessionStatus === "waiting" || sessionStatus === "running") {
      loadCapabilities(sessionId);
    }
  }, [sessionId, sessionStatus, loadCapabilities]);

  // ── Apply caps from session/ready (primary path, avoids race with invoke) ──
  useEffect(() => {
    if (!sessionId || sessionId.startsWith("session-empty-") || !isTauriRuntime()) return;

    let unlisten: (() => void) | undefined;
    listen<AcpEvent>("acp-event", (event) => {
      const payload = event.payload;
      if (payload.sessionId !== sessionId) return;
      if (payload.kind === "system" && payload.method === "session/ready") {
        const data = payload.data as { capabilities?: CapabilitySnapshot } | null;
        if (data?.capabilities) {
          loadSeq.current += 1; // invalidate in-flight null fetches
          const merged = mergeAcpCapabilities(agent.id, data.capabilities);
          if (merged) applyCaps(merged, "handshake");
        } else {
          loadCapabilities(sessionId);
        }
      }
    }).then((dispose) => {
      unlisten = dispose;
    });
    return () => unlisten?.();
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessionId, loadCapabilities]);

  // ── Update helpers ─────────────────────────────────────────────────────────
  const flash = useCallback((msg: string) => {
    setFlashError(msg);
    if (errorTimer.current) clearTimeout(errorTimer.current);
    errorTimer.current = setTimeout(() => setFlashError(""), 3000);
  }, []);

  /** Only pass fields that actually changed — parent merges with disk snapshot. */
  const persistPrefs = useCallback(
    (patch: SessionComposerPrefs) => {
      if (!onSessionPrefsChange || sessionId.startsWith("session-empty-")) return;
      onSessionPrefsChange(patch);
    },
    [onSessionPrefsChange, sessionId],
  );

  const commitUpdate = useCallback(
    async (patch: Record<string, unknown>, revert: () => void) => {
      // Early-exit used to leave optimistic chip paints stuck with no persist.
      // Throw so the caller's revert runs (Always approve was the worst case).
      if (!sessionId) {
        revert();
        return;
      }
      if (updating.current) {
        revert();
        flash("Another setting is still applying — try again in a moment");
        return;
      }
      updating.current = true;
      try {
        // Lazy ACP: mode/model/effort require a live session.
        if (onEnsureAgentReady) {
          const ready = await onEnsureAgentReady();
          if (!ready) {
            throw new Error("Agent is not connected yet — type or wait for ACP warm-up");
          }
        }
        // Agents without session/set_config_option (Grok) are handled in Rust,
        // which retries the pre-v2 per-knob RPCs on -32601.
        // alwaysApprove is a slash command, not set_config — handled separately.
        if (typeof patch.alwaysApprove === "boolean") {
          const aa = getAcpSupplement(agent.id)?.alwaysApprove;
          if (!aa) throw new Error("This agent has no always-approve control");
          const cmd = patch.alwaysApprove ? aa.on : aa.off;
          await sendAcpPrompt(sessionId, cmd);
          persistPrefs({ preferredAlwaysApprove: patch.alwaysApprove });
          // Same localStorage last-used path as model/mode so a *new* Grok
          // dialog inherits Always approve instead of always opening as Ask.
          recordLastUsedDefaults(agent.id, {
            alwaysApprove: patch.alwaysApprove,
          });
          return;
        }
        const attempts = expandAcpConfigAttempts(agent.id, patch, caps);
        if (attempts.length === 0) {
          // Launch-time-only options (Grok model): keep the local chip
          // + disk prefs, but do not pretend set_config succeeded on the wire.
          // Effort is NOT launch-time — it routes through legacy_set_config
          // (see effortViaLegacyModel in acpSupplements.ts), so it will have
          // produced an attempt above and this block won't be reached.
          if (agent.id === "grok-build" && typeof patch.model === "string") {
            persistPrefs({ preferredModel: patch.model });
            return;
          }
          if (patch.effortId != null || patch.thinkingEffort != null) {
            throw new Error(
              "This model does not expose an Effort control (not a login issue)",
            );
          }
          throw new Error("No config mapping for this control");
        }
        let lastError: unknown = null;
        let ok = false;
        for (const attempt of attempts) {
          try {
            await updateAcpSession(sessionId, attempt);
            ok = true;
            break;
          } catch (error) {
            lastError = error;
          }
        }
        if (!ok) {
          const msg =
            lastError instanceof Error
              ? lastError.message
              : typeof lastError === "string"
                ? lastError
                : "Agent rejected the config change";
          throw new Error(msg);
        }
        // Mode-only: skip immediate caps reload — agent often still echoes the
        // previous currentMode for a beat, which made the chip flicker.
        if (typeof patch.mode !== "string") {
          loadCapabilities(sessionId);
        } else {
          // Soft refresh later after the agent has settled.
          window.setTimeout(() => loadCapabilities(sessionId), 1200);
        }
        // Disk SSOT: only fields that changed (parent merges; never wipe siblings).
        const prefsPatch: SessionComposerPrefs = {};
        if (typeof patch.model === "string") prefsPatch.preferredModel = patch.model;
        if (typeof patch.mode === "string") {
          prefsPatch.preferredMode = normalizeAgentModeId(agent.id, patch.mode);
        }
        if (typeof patch.thinkingEffort === "number") prefsPatch.preferredEffort = patch.thinkingEffort;
        if (typeof patch.effortId === "string") prefsPatch.preferredEffortId = patch.effortId;
        if (typeof patch.alwaysApprove === "boolean") {
          prefsPatch.preferredAlwaysApprove = patch.alwaysApprove;
        }
        if (Object.keys(prefsPatch).length > 0) persistPrefs(prefsPatch);
        // Track recently used model
        if (typeof patch.model === "string") {
          recordModelUsage(patch.model);
        }
        // Remember the chips the user actually settled on — they become the
        // default for the agent's next session.
        recordLastUsedDefaults(agent.id, {
          modelId: typeof patch.model === "string" ? patch.model : chipStateRef.current.model,
          modeId: typeof patch.mode === "string" ? patch.mode : chipStateRef.current.mode,
          effortId:
            typeof patch.effortId === "string" ? patch.effortId : chipStateRef.current.effortId,
          effort:
            typeof patch.thinkingEffort === "number"
              ? patch.thinkingEffort
              : chipStateRef.current.effort,
        });
      } catch (error) {
        revert();
        const msg =
          error instanceof Error
            ? error.message
            : "Failed to update (agent may not support this option live)";
        // Keep toast short but specific (Claude often rejects wrong effort/mode values).
        flash(msg.length > 140 ? `${msg.slice(0, 140)}…` : msg);
      } finally {
        updating.current = false;
      }
    },
    [sessionId, flash, loadCapabilities, agent.id, caps, onEnsureAgentReady, persistPrefs],
  );

  // Restore preferred model/mode/effort once caps are available for this dialog.
  // Cached caps only paint the chips — pushing set_config at an agent that is
  // not connected would start a process the user never asked for.
  //
  // Order matters: model first, then wait for the agent to republish effort
  // options for that model, then push effort. OpenCode resets thought level to
  // the model default (often "high") on every set_model — if we skip or race
  // the effort step, a disk-saved "max" never reaches the agent.
  useEffect(() => {
    if (!caps || !capsLive || sessionId.startsWith("session-empty-")) return;
    const key = `${sessionId}:${agent.id}`;
    if (prefsRestoredKey.current === key) return;

    const prefModel = sessionPrefs?.preferredModel?.trim() || null;
    // Codex disk prefs may still hold pre-acp labels (full-auto → agent-full-access).
    const prefModeRaw = sessionPrefs?.preferredMode?.trim() || null;
    const prefMode = prefModeRaw
      ? normalizeAgentModeId(agent.id, prefModeRaw)
      : null;
    const prefEffortId = sessionPrefs?.preferredEffortId?.trim() || null;
    const prefEffort =
      typeof sessionPrefs?.preferredEffort === "number" && Number.isFinite(sessionPrefs.preferredEffort)
        ? sessionPrefs.preferredEffort
        : null;
    const prefAlways =
      typeof sessionPrefs?.preferredAlwaysApprove === "boolean"
        ? sessionPrefs.preferredAlwaysApprove
        : null;
    const aaSpec = getAcpSupplement(agent.id)?.alwaysApprove;

    const hasAny = Boolean(
      prefModel || prefMode || prefEffortId || prefEffort != null || (aaSpec && prefAlways != null),
    );
    if (!hasAny) {
      prefsRestoredKey.current = key;
      return;
    }

    // Claim the key before the async work so handshake re-entries do not fan
    // out parallel restores. Failures still leave disk prefs intact for next warm.
    prefsRestoredKey.current = key;

    const modelOk =
      prefModel &&
      (caps.models.length === 0 || caps.models.some((m) => m.id === prefModel));
    const modeOk =
      prefMode &&
      (caps.modes.length === 0 || caps.modes.some((m) => m.id === prefMode));

    // Optimistic UI for model/mode; effort waits until we know the model list.
    if (modelOk && prefModel) setCurrentModel(prefModel);
    if (modeOk && prefMode) setCurrentMode(prefMode);
    if (aaSpec && prefAlways != null) setAlwaysApprove(prefAlways);
    // Show saved effort immediately when the *current* catalog already has it
    // (same-model reconnect). After a model switch the list is wrong until live.
    if (
      prefEffortId &&
      (caps.effortOptions ?? []).some((o) => o.id === prefEffortId)
    ) {
      setCurrentEffortId(prefEffortId);
    } else if (
      prefEffort != null &&
      caps.thinkingEffort != null &&
      prefEffort >= caps.thinkingEffort.min &&
      prefEffort <= caps.thinkingEffort.max
    ) {
      setCurrentEffort(prefEffort);
    }

    // Pin so late set_config / loadCapabilities echoes of "high" cannot paint
    // over the dialog's saved "max" while we finish the push.
    if (prefEffortId) pinOptions({ effortId: prefEffortId });
    else if (prefEffort != null) pinOptions({ effort: prefEffort });
    if (prefModel) pinOptions({ model: prefModel });
    if (prefMode) pinOptions({ mode: prefMode });

    let cancelled = false;

    void (async () => {
      const baseline = caps;

      // 1) Model — switching resets agent-side effort to the model default.
      if (modelOk && prefModel && prefModel !== baseline.currentModel) {
        await commitUpdate({ model: prefModel }, () => {
          if (!cancelled) setCurrentModel(baseline.currentModel);
        });
      }

      // 2) Wait until live caps reflect the preferred model (effort options
      //    are re-advertised with it). getSessionCapabilities is in-memory and
      //    cheap; a few short polls cover OpenCode's post-set_model refresh.
      const needModelWait =
        Boolean(modelOk && prefModel) && prefModel !== baseline.currentModel;
      let live = baseline;
      if (needModelWait) {
        for (let i = 0; i < 8; i++) {
          if (cancelled) return;
          await new Promise((r) => window.setTimeout(r, 80 + i * 40));
          const next = await getSessionCapabilities(sessionId).catch(() => null);
          if (!next) continue;
          live = next;
          if (next.currentModel === prefModel) break;
        }
      } else {
        const next = await getSessionCapabilities(sessionId).catch(() => null);
        if (next) live = next;
      }
      if (cancelled) return;

      // 3) Drift only after the preferred model is the one we are judging —
      //    otherwise a default-model catalog (no "max") erases a deepseek "max".
      //    Pass agentId so Codex legacy mode labels remap instead of wipe.
      if (driftCheckedKey.current !== key) {
        driftCheckedKey.current = key;
        const drift = detectCapabilityDrift(sessionPrefs, live, agent.id);
        if (drift) {
          // Silent rewrite (legacy full-auto → agent-full-access) — no toast.
          if (drift.issues.length > 0) {
            flash(drift.summary.length > 140 ? `${drift.summary.slice(0, 140)}…` : drift.summary);
          }
          onSessionPrefsChange?.(drift.clearedPrefs);
        }
      }

      // 4) Mode — Codex always re-push: optimistic currentMode can match the
      //    chip while the harness is still on default "agent" (workspace sandbox).
      const codexMode = agent.id === "codex" || agent.id === "codex-acp";
      if (modeOk && prefMode && (codexMode || prefMode !== live.currentMode)) {
        await commitUpdate({ mode: prefMode }, () => {
          if (!cancelled) setCurrentMode(live.currentMode);
        });
      }

      // 5) Always-approve is session-local on Grok; replay the slash after warm.
      // Only push when the user wants ON — default is ask, so re-sending "off"
      // just burns a turn. Failures keep the disk pref for the next reconnect.
      if (aaSpec && prefAlways === true) {
        await commitUpdate({ alwaysApprove: true }, () => {
          if (!cancelled) setAlwaysApprove(false);
        });
      }

      // 6) Effort — always against post-model caps. OpenCode's model switch
      //    leaves currentEffortId at "high" even when disk says "max".
      //
      // Prefer membership in effortOptions, but after a model switch the
      // refreshed list can lag: if the agent still advertises an effort
      // config id, try the saved value once. Rejection keeps disk intact
      // (commitUpdate reverts the chip).
      const options = live.effortOptions ?? [];
      const liveEffortIdOk =
        Boolean(prefEffortId) && options.some((o) => o.id === prefEffortId);
      const liveNumericOk =
        prefEffort != null &&
        live.thinkingEffort != null &&
        prefEffort >= live.thinkingEffort.min &&
        prefEffort <= live.thinkingEffort.max;
      const canTryEffortId =
        Boolean(prefEffortId) &&
        (liveEffortIdOk || (Boolean(live.effortConfigId) && needModelWait));

      if (prefEffortId && !canTryEffortId) {
        // Not on this model's menu and we have no config id to probe — leave
        // the agent default on the chip; disk still holds the preference.
        if (!cancelled) setCurrentEffortId(live.currentEffortId);
        unpinOption("effortId");
      } else if (canTryEffortId && prefEffortId) {
        if (!cancelled) setCurrentEffortId(prefEffortId);
        if (prefEffortId !== live.currentEffortId) {
          await commitUpdate({ effortId: prefEffortId }, () => {
            if (!cancelled) {
              unpinOption("effortId");
              setCurrentEffortId(live.currentEffortId);
            }
          });
        } else {
          unpinOption("effortId");
        }
      } else if (liveNumericOk && prefEffort != null) {
        if (!cancelled) setCurrentEffort(prefEffort);
        if (live.currentEffort == null || Math.abs(live.currentEffort - prefEffort) > 1e-6) {
          await commitUpdate({ thinkingEffort: prefEffort }, () => {
            if (!cancelled) {
              unpinOption("effort");
              setCurrentEffort(live.currentEffort);
            }
          });
        } else {
          unpinOption("effort");
        }
      } else {
        unpinOption("effortId");
        unpinOption("effort");
      }

      // Drop model/mode pins after the full restore sequence settles.
      unpinOption("model");
      unpinOption("mode");
    })();

    return () => {
      cancelled = true;
    };
    // commitUpdate / pin helpers are stable enough; re-run only when the
    // dialog's live caps identity or saved prefs change.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [caps, capsLive, sessionId, agent.id, sessionPrefs]);

  // Keep version badges warm for the whole app life: subscribe + background
  // refresh (local `--version` always, registry on a TTL). Opening the menu
  // no longer re-probes from scratch.
  useEffect(() => {
    if (!isTauriRuntime()) return;
    setAgentVersionInfo(getCachedAgentVersions());
    const unsub = subscribeAgentVersions(setAgentVersionInfo);
    void refreshAgentVersions();
    return unsub;
  }, []);

  // Drop offline catalog when the installed CLI version advanced (post-update).
  useEffect(() => {
    if (!isTauriRuntime()) return;
    const info = agentVersionInfo[agent.id];
    if (info?.installed) {
      invalidateCapsIfAgentUpdated(agent.id, info.installed);
    }
  }, [agent.id, agentVersionInfo]);

  // ── Send / Interrupt ───────────────────────────────────────────────────────
  // Only a live turn (`running`) blocks send. `starting` must NOT freeze the
  // composer — warm happens in the background; send will wait for ready.
  const isBusy = sessionStatus === "running";
  const isWarming = sessionStatus === "starting";
  // Prefer advertised cancel; still offer interrupt while running (Esc×2 / button).
  const canCancel = isBusy && (caps?.supportsCancel ?? true);

  const clearDropSuggest = useCallback(() => {
    setDraftAfterDrop(null);
    setSuggestDroppedPaths([]);
  }, []);

  const submitText = useCallback(
    (text: string) => {
      if (isBusy) return;
      if (!sessionId || sessionId.startsWith("session-empty-")) return;
      // Allow send with only images / empty text when attachments exist.
      if (!text.trim() && imageAttachments.length === 0) return;
      onWarmAgent?.();
      const dropped = [
        ...droppedPathsRef.current,
        ...imageAttachments.map((a) => a.path),
      ].filter((p, i, arr) => arr.indexOf(p) === i);
      const droppedInText = dropped.filter((p) => text.includes(p) || imageAttachments.some((a) => a.path === p));
      const sentWith = currentModel ?? (caps && caps.models.length > 0 ? caps.models[0].id : null);
      if (sentWith) recordModelUsage(sentWith);

      // Snapshot chips the user actually sees (optimistic currentMode/currentModel),
      // not parent sessionCapabilities — mode switch delays caps reload on purpose.
      const modeId = currentMode ?? (caps && caps.modes.length > 0 ? caps.modes[0].id : null);
      const modeLabel =
        (modeId && caps?.modes.find((m) => m.id === modeId)?.label) || modeId || null;
      const modelId = sentWith;
      const modelLabel =
        (modelId && caps?.models.find((m) => m.id === modelId)?.label) || modelId || null;
      const effortOpts = caps?.effortOptions ?? [];
      const effortLabelVal =
        currentEffortId != null
          ? (effortOpts.find((o) => o.id === currentEffortId)?.label ?? currentEffortId)
          : currentEffort != null
            ? effortLabel(currentEffort)
            : null;

      // Keep draft text clean for the You card; App injects the wire prefix.
      onSend(
        text,
        droppedInText,
        imageAttachments.length > 0 ? imageAttachments : undefined,
        {
          ...(forceWebSearch ? { forceWebSearch: true } : {}),
          modeId,
          modeLabel,
          modelId,
          modelLabel,
          effortLabel: effortLabelVal,
        },
      );
      droppedPathsRef.current.clear();
      setDraft("");
      setImageAttachments([]);
      setAnnotatingId(null);
      clearDropSuggest();
    },
    [
      isBusy,
      sessionId,
      onWarmAgent,
      currentModel,
      currentMode,
      currentEffort,
      currentEffortId,
      caps,
      onSend,
      clearDropSuggest,
      imageAttachments,
      forceWebSearch,
    ],
  ); // forceWebSearch passed as flag, not baked into text

  const submit = () => {
    // Empty draft is OK when parent has quote-pins (App merges on send).
    submitText(draft);
  };

  const suggestRound = suggestionRoundKey(sessionId, sessionEvents);
  const suggestions: Suggestion[] = useMemo(() => {
    if (!suggestionsEnabled()) return [];
    if (isComposing) return [];
    if (dismissedSuggestKey === suggestRound) return [];
    return suggestFor({
      events: sessionEvents,
      sessionStatus,
      droppedPaths: suggestDroppedPaths,
      draft,
      draftAfterDrop,
    });
  }, [
    sessionEvents,
    sessionStatus,
    suggestDroppedPaths,
    draft,
    draftAfterDrop,
    isComposing,
    dismissedSuggestKey,
    suggestRound,
  ]);

  const sendSuggestion = (chip: Suggestion) => {
    // Paths from drop stay in the draft; append chip intent when draft is path-only.
    let text = chip.text;
    if (draftAfterDrop != null && draft === draftAfterDrop && draft.trim()) {
      text = `${draft.trim()}\n${chip.text}`;
    }
    submitText(text);
  };

  /** Insert dropped paths: images → pills; other files → draft text. */
  const insertDroppedPaths = useCallback((paths: string[]) => {
    const unique = [...new Set(paths.map((p) => p.trim()).filter(Boolean))];
    if (unique.length === 0) return;
    const images = unique.filter(isImagePath);
    const others = unique.filter((p) => !isImagePath(p));

    if (images.length > 0) {
      setImageAttachments((current) => {
        const existing = new Set(current.map((a) => a.path.toLowerCase()));
        const added = images
          .filter((p) => !existing.has(p.toLowerCase()))
          .map(attachmentFromPath);
        // Auto-open annotator for the first new image.
        if (added[0]) {
          requestAnimationFrame(() => setAnnotatingId(added[0].id));
        }
        return [...current, ...added];
      });
      setSuggestDroppedPaths(images);
      // Path-only draft after image drop still counts as "empty" for chips.
      setDraftAfterDrop((d) => d ?? draft);
    }

    if (others.length === 0) return;
    for (const path of others) droppedPathsRef.current.add(path);
    const chunk = others.join("\n");
    const el = textareaRef.current;
    if (!el) {
      setDraft((current) => {
        const next = current ? `${current}\n${chunk}` : chunk;
        setDraftAfterDrop(next);
        setSuggestDroppedPaths((prev) => [...new Set([...prev, ...others])]);
        return next;
      });
      return;
    }
    const start = el.selectionStart ?? el.value.length;
    const end = el.selectionEnd ?? start;
    const before = el.value.slice(0, start);
    const after = el.value.slice(end);
    const needsLead = before.length > 0 && !before.endsWith("\n") && !before.endsWith(" ");
    const needsTrail = after.length > 0 && !after.startsWith("\n") && !after.startsWith(" ");
    const inserted = `${needsLead ? "\n" : ""}${chunk}${needsTrail ? "\n" : ""}`;
    const next = `${before}${inserted}${after}`;
    setDraft(next);
    setDraftAfterDrop(next);
    setSuggestDroppedPaths((prev) => [...new Set([...prev, ...others])]);
    requestAnimationFrame(() => {
      const caret = before.length + inserted.length;
      el.focus();
      el.setSelectionRange(caret, caret);
    });
  }, [draft]);

  /** Encode a File/Blob as base64 (chunked — large screenshots blow the call stack with spread). */
  const fileToBase64 = async (file: Blob): Promise<string> => {
    const bytes = new Uint8Array(await file.arrayBuffer());
    const chunk = 0x8000;
    let binary = "";
    for (let i = 0; i < bytes.length; i += chunk) {
      binary += String.fromCharCode(...bytes.subarray(i, i + chunk));
    }
    return btoa(binary);
  };

  /**
   * Clipboard paste: screenshots / "Copy Image" → temp file → image pills.
   * Plain text (and non-image files without a path we can use) keep the default paste.
   */
  const onComposerPaste = useCallback(
    (event: ReactClipboardEvent<HTMLTextAreaElement>) => {
      const dt = event.clipboardData;
      if (!dt) return;

      // Prefer image items (Win+Shift+S, browser "Copy Image").
      const imageItems = Array.from(dt.items ?? []).filter(
        (item) => item.kind === "file" && item.type.startsWith("image/"),
      );

      // Also pick up FileList entries that look like images (Explorer copy + paste).
      const fileListImages = Array.from(dt.files ?? []).filter(
        (f) =>
          (f.type && f.type.startsWith("image/")) ||
          isImagePath(f.name) ||
          isImagePath((f as File & { path?: string }).path ?? ""),
      );

      const hasImagePayload = imageItems.length > 0 || fileListImages.length > 0;
      if (!hasImagePayload) return; // text paste → default

      event.preventDefault();

      void (async () => {
        try {
          if (!isTauriRuntime()) {
            flash("Paste image requires the desktop app");
            return;
          }

          const paths: string[] = [];
          const seen = new Set<string>();

          const absorbFile = async (file: File | null) => {
            if (!file) return;
            const existingPath = (file as File & { path?: string }).path?.trim();
            if (existingPath && isImagePath(existingPath)) {
              const key = existingPath.toLowerCase();
              if (!seen.has(key)) {
                seen.add(key);
                paths.push(existingPath);
              }
              return;
            }
            // Raw clipboard bitmap — no filesystem path; materialize under ~/.marionette/clipboard.
            const mime = file.type && file.type.startsWith("image/") ? file.type : "image/png";
            const b64 = await fileToBase64(file);
            const saved = await savePastedImage(b64, mime);
            const key = saved.path.toLowerCase();
            if (!seen.has(key)) {
              seen.add(key);
              paths.push(saved.path);
            }
          };

          if (imageItems.length > 0) {
            for (const item of imageItems) {
              await absorbFile(item.getAsFile());
            }
          } else {
            for (const file of fileListImages) {
              await absorbFile(file);
            }
          }

          if (paths.length === 0) {
            flash("Could not read pasted image");
            return;
          }
          insertDroppedPaths(paths);
        } catch (err) {
          const msg = err instanceof Error ? err.message : String(err);
          flash(msg.length > 140 ? `${msg.slice(0, 140)}…` : msg);
        }
      })();
    },
    [flash, insertDroppedPaths],
  );

  const pathsFromDataTransfer = (dt: DataTransfer | null): string[] => {
    if (!dt) return [];
    const fromFiles: string[] = [];
    if (dt.files?.length) {
      for (const file of Array.from(dt.files)) {
        // Tauri webview File objects often include absolute path.
        const path = (file as File & { path?: string }).path;
        fromFiles.push(path && path.trim() ? path : file.name);
      }
    }
    if (fromFiles.length > 0) return fromFiles;

    const uriList = dt.getData("text/uri-list");
    if (uriList) {
      return uriList
        .split(/\r?\n/)
        .map((line) => line.trim())
        .filter((line) => line && !line.startsWith("#"))
        .map((line) => {
          if (line.startsWith("file:///")) {
            try {
              const decoded = decodeURIComponent(line.replace(/^file:\/\/\//, "").replace(/^file:\/\//, ""));
              // Windows: file:///D:/foo → D:/foo
              return decoded.replace(/^\/([A-Za-z]:)/, "$1").replace(/\//g, "\\");
            } catch {
              return line;
            }
          }
          return line;
        });
    }

    const plain = dt.getData("text/plain");
    if (plain && /[\\/]/.test(plain)) {
      return plain
        .split(/\r?\n/)
        .map((line) => line.trim())
        .filter(Boolean);
    }
    return [];
  };

  // Browser mock: HTML5 DnD. Desktop (Tauri) uses onDragDropEvent below for absolute paths.
  const onComposerDragEnter = (event: ReactDragEvent) => {
    if (isTauriRuntime()) return;
    if (![...event.dataTransfer.types].some((t) => t === "Files" || t === "text/uri-list" || t === "text/plain")) {
      return;
    }
    event.preventDefault();
    dropDepth.current += 1;
    setDropActive(true);
  };

  const onComposerDragOver = (event: ReactDragEvent) => {
    if (isTauriRuntime()) return;
    if (![...event.dataTransfer.types].some((t) => t === "Files" || t === "text/uri-list" || t === "text/plain")) {
      return;
    }
    event.preventDefault();
    event.dataTransfer.dropEffect = "copy";
  };

  const onComposerDragLeave = (event: ReactDragEvent) => {
    if (isTauriRuntime()) return;
    event.preventDefault();
    dropDepth.current = Math.max(0, dropDepth.current - 1);
    if (dropDepth.current === 0) setDropActive(false);
  };

  const onComposerDrop = (event: ReactDragEvent) => {
    if (isTauriRuntime()) return;
    event.preventDefault();
    dropDepth.current = 0;
    setDropActive(false);
    const paths = pathsFromDataTransfer(event.dataTransfer);
    if (paths.length === 0) {
      flash("Could not read dropped file path");
      return;
    }
    insertDroppedPaths(paths);
  };

  // Tauri-native file drops give absolute paths (HTML5 File.path is not always present).
  useEffect(() => {
    if (!isTauriRuntime()) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    const field = () => textareaRef.current?.closest(".composer__field") as HTMLElement | null;

    void getCurrentWebview()
      .onDragDropEvent((event) => {
        if (disposed) return;
        const payload = event.payload;
        const el = field();
        if (!el) return;

        if (payload.type === "leave") {
          dropDepth.current = 0;
          setDropActive(false);
          return;
        }

        const pos = "position" in payload ? payload.position : null;
        if (!pos) return;
        const rect = el.getBoundingClientRect();
        // Tauri reports physical pixels; scale back to CSS coords.
        const scale = window.devicePixelRatio || 1;
        const x = pos.x / scale;
        const y = pos.y / scale;
        const over =
          x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom;

        if (payload.type === "enter" || payload.type === "over") {
          setDropActive(over);
          return;
        }

        if (payload.type === "drop") {
          setDropActive(false);
          dropDepth.current = 0;
          if (!over || !payload.paths?.length) return;
          insertDroppedPaths(payload.paths);
        }
      })
      .then((dispose) => {
        if (disposed) dispose();
        else unlisten = dispose;
      })
      .catch(() => undefined);

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [insertDroppedPaths]);

  /** OpenCode-style: Tab cycles execution mode when ≥2 modes exist. */
  const cycleMode = useCallback(
    (direction: 1 | -1) => {
      if (!caps || caps.modes.length < 2) return;
      const modes = caps.modes;
      const current =
        currentMode ?? displayModeSafe(caps) ?? modes[0]?.id ?? null;
      if (!current) return;
      const idx = modes.findIndex((m) => m.id === current);
      const next = modes[(idx < 0 ? 0 : idx + direction + modes.length) % modes.length];
      if (!next || next.id === current) return;
      const prev = current;
      pinOptions({ mode: next.id });
      setCurrentMode(next.id);
      setMenu(null);
      void commitUpdate({ mode: next.id }, () => {
        unpinOption("mode");
        setCurrentMode(prev);
      });
      // No flash toast — label lives on the mode chip; toast felt like flicker.
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [caps, currentMode, commitUpdate, pinOptions, unpinOption],
  );

  const warmIfNeeded = useCallback(
    (opts?: { force?: boolean }) => {
      // Never start ACP mid-IME — parent status updates re-render and freeze composition.
      if (composingRef.current && !opts?.force) return;
      if (!opts?.force && warmedThisFocusRef.current) return;
      warmedThisFocusRef.current = true;
      // Defer so the keystroke paints first.
      window.setTimeout(() => {
        if (composingRef.current && !opts?.force) return;
        onWarmAgent?.();
      }, 0);
    },
    [onWarmAgent],
  );

  // ── Derived state ──────────────────────────────────────────────────────────
  const hasModes = caps != null && caps.modes.length > 1;
  const hasModels = caps != null && caps.models.length > 0;
  const effortOptions = caps?.effortOptions ?? [];
  const agentSupplement = getAcpSupplement(agent.id);
  const alwaysApproveSpec = agentSupplement?.alwaysApprove ?? null;
  // Only show effort when the live agent registered the option. Claude omits
  // `effort` entirely for models without supportsEffort (e.g. Haiku after
  // model switch) — inventing it yields:
  //   Unknown config option: effort
  // and a stale Effort menu would crash on thinkingEffort!.default.
  // Exception: Grok carries effort on session/set_model `_meta` (no config id).
  const hasStringEffort =
    effortOptions.length > 0 &&
    (Boolean(caps?.effortConfigId) || agentSupplement?.effortViaLegacyModel === true);
  const hasNumericEffort =
    caps != null &&
    caps.thinkingEffort != null &&
    Boolean(caps.effortConfigId);
  const hasEffort = hasStringEffort || hasNumericEffort;
  // The key dialog writes OpenCode's auth.json, so only offer it for the agent
  // that reads that file. Codex / Claude Code / Grok each own their credentials;
  // showing them "add API key" would write a key none of them ever looks at.
  const usesOpencodeAuth = OPENCODE_AUTH_AGENTS.has(agent.id);
  // An empty model list means "no key yet" only for OpenCode. Note this is
  // rendered *alongside* the effort block, never instead of it — see the menu.
  const showKeyPrompt = usesOpencodeAuth && caps != null && caps.models.length === 0;
  const numericEffortPresets =
    hasNumericEffort && caps?.thinkingEffort
      ? [
          { label: "Auto", value: caps.thinkingEffort.default },
          { label: "Low", value: caps.thinkingEffort.min },
          { label: "High", value: caps.thinkingEffort.max },
        ]
      : [];

  // Model switch can drop effort (Haiku). Never leave menu stuck on "effort".
  useEffect(() => {
    if (menu === "effort" && !hasEffort) {
      setMenu(null);
    }
  }, [menu, hasEffort]);

  const displayModel =
    currentModel ?? (caps && caps.models.length > 0 ? caps.models[0].id : null);
  const displayMode =
    currentMode ?? (caps && caps.modes.length > 0 ? caps.modes[0].id : null);

  useEffect(() => {
    onActiveModelChange?.(displayModel);
  }, [displayModel, onActiveModelChange]);
  const displayEffort = hasStringEffort
    ? (effortOptions.find((o) => o.id === currentEffortId)?.label ??
        currentEffortId)
    : hasEffort && currentEffort != null
      ? effortLabel(currentEffort)
      : null;
  const displayModelFull =
    (displayModel && caps?.models.find((m) => m.id === displayModel)?.label) || displayModel;
  const displayModelLabel = shortTriggerLabel(displayModelFull, displayModel);
  const displayModeLabel =
    (displayMode && caps?.modes.find((m) => m.id === displayMode)?.label) || displayMode;

  const modelGroups = useMemo(() => {
    if (!caps?.models?.length) return [];
    const q = modelQuery.trim().toLowerCase();
    const filtered = q
      ? caps.models.filter(
          (m) =>
            m.id.toLowerCase().includes(q) ||
            m.label.toLowerCase().includes(q) ||
            (m.description ?? "").toLowerCase().includes(q) ||
            providerOf(m).toLowerCase().includes(q),
        )
      : caps.models;
    return groupModels(filtered);
  }, [caps?.models, modelQuery]);

  const recentModels = useMemo(() => {
    if (!caps?.models?.length) return [];
    const validIds = new Set(caps.models.map((m) => m.id));
    return getRecentModels(validIds);
  }, [caps?.models]);

  // Reset + focus search when opening model menu
  useEffect(() => {
    if (menu === "model") {
      setModelQuery("");
      requestAnimationFrame(() => modelSearchRef.current?.focus());
    }
  }, [menu]);

  // Which agents can actually run — probed when the switcher opens, so the list
  // never offers a harness that would fail with "command not found" on send.
  const refreshAgentStatuses = useCallback(async () => {
    const list = await listAgentCommands();
    if (list.length === 0) return;
    setAgentStatuses(Object.fromEntries(list.map((status) => [status.id, status])));
  }, []);

  useEffect(() => {
    if (menu !== "agent") return;
    void refreshAgentStatuses();
    // Versions are already cached; a quiet refresh picks up anything that
    // changed since the last background pass without blanking the UI.
    void refreshAgentVersions();
    // Light preflight for visible agents (PATH / Node / uv).
    void (async () => {
      const enabled = agents.filter((a) => a.enabled);
      const results = await Promise.all(
        enabled.map(async (a) => {
          const r = await agentPreflight(a.id);
          return r ? ([a.id, r] as const) : null;
        }),
      );
      const map: Record<string, PreflightResult> = {};
      for (const row of results) {
        if (row) map[row[0]] = row[1];
      }
      setPreflightById(map);
    })();
  }, [menu, refreshAgentStatuses, agents]);

  const runAddCustomAgent = useCallback(async () => {
    const id = window.prompt("自定义 agent id（字母数字-_，建议 custom-xxx）", "custom-");
    if (!id?.trim()) return;
    const label = window.prompt("显示名称", id.trim());
    if (!label?.trim()) return;
    const command = window.prompt("可执行命令（须在 PATH 上，或配合 npm 包名）", "");
    if (!command?.trim()) return;
    const npmPackage = window.prompt("可选：npm 包名（有则支持 Install 按钮）", "") ?? "";
    try {
      await addCustomAgent({
        id: id.trim(),
        label: label.trim(),
        command: command.trim(),
        args: [],
        npmPackage: npmPackage.trim() || null,
        note: null,
      });
      setInstallNote(`已添加自定义 agent「${label.trim()}」。`);
      await onAgentsReload?.();
      void refreshAgentStatuses();
    } catch (e) {
      setInstallNote(e instanceof Error ? e.message : String(e));
    }
  }, [onAgentsReload, refreshAgentStatuses]);

  const runInstall = useCallback(
    async (agentId: string, options?: { upgrade?: boolean }) => {
      if (installingAgentId) return;
      const upgrade = options?.upgrade === true;
      const touchesCurrent = agentId === agent.id;
      setInstallingAgentId(agentId);
      setInstallNote(
        upgrade
          ? `Updating ${agentId}… (npm global, this can take a minute)`
          : `Installing ${agentId}… (npm global, this can take a minute)`,
      );
      try {
        // Windows locks the running binary — release it before `npm install -g`.
        if (upgrade && touchesCurrent && onAgentBinaryUpdated) {
          setInstallNote(`Stopping ${agent.label} so the binary can be replaced…`);
          await onAgentBinaryUpdated(agentId, "stop");
        }
        setInstallNote(
          upgrade
            ? `Updating ${agentId}… (npm global, this can take a minute)`
            : `Installing ${agentId}… (npm global, this can take a minute)`,
        );
        // `force: true` is what actually upgrades an already-present CLI.
        const result = await installAgent(agentId, true, upgrade);
        setAgentStatuses((current) => ({ ...current, [agentId]: result.status }));
        setInstallNote(result.message);
        // New binary → forget stale model/mode/effort catalog.
        clearAgentCapabilities(agentId);
        // Re-read `--version` so the badge and update flag match the disk.
        const versions = await refreshAgentVersions({ forceRegistry: true });
        const info = versions[agentId];
        if (info?.installed) {
          patchAgentVersion(agentId, {
            installed: info.installed,
            latest: info.latest,
            updateAvailable: info.updateAvailable,
          });
        } else if (upgrade) {
          patchAgentVersion(agentId, { updateAvailable: false });
        }
        void refreshAgentStatuses();
        // Reload the dialog's agent so the next prompt uses the new binary —
        // no app restart required.
        if (touchesCurrent && onAgentBinaryUpdated) {
          setInstallNote(`${result.message} — restarting agent…`);
          await onAgentBinaryUpdated(agentId, "restart");
          setInstallNote(`${result.message} — agent restarted.`);
        }
      } catch (error) {
        setInstallNote(error instanceof Error ? error.message : String(error));
      } finally {
        setInstallingAgentId(null);
      }
    },
    [installingAgentId, refreshAgentStatuses, agent.id, agent.label, onAgentBinaryUpdated],
  );

  const currentAgentVersion = agentVersionInfo[agent.id];
  const currentHasUpdate = currentAgentVersion?.updateAvailable === true;

  // ── Slash command autocomplete ────────────────────────────────────────────
  const slashCatalog = useMemo(
    () => resolveSlashCommands(agent.id, availableCommands),
    [agent.id, availableCommands],
  );
  const slashToken = useMemo(
    () => slashQueryAtCursor(draft, caret),
    [draft, caret],
  );
  const slashMatches = useMemo(() => {
    if (!slashToken) return [] as AvailableCommand[];
    return filterSlashCommands(slashCatalog, slashToken.query);
  }, [slashCatalog, slashToken]);
  const showSlashMenu = slashMatches.length > 0 && slashToken != null;

  // ── @ delegate autocomplete (line-start only; only when agent prefix matches) ──
  const [delegateIndex, setDelegateIndex] = useState(0);
  const [installedAgentIds, setInstalledAgentIds] = useState<Set<string>>(() => new Set());
  const delegateToken = useMemo(
    () => delegateQueryAtCursor(draft, caret),
    [draft, caret],
  );
  const recentModelIds = useMemo(() => {
    // Best-effort: recent models across agents for ranking under a resolved agent.
    try {
      const raw = localStorage.getItem("marionette-recent-models");
      if (!raw) return [] as string[];
      const parsed = JSON.parse(raw) as Array<{ modelId?: string }>;
      if (!Array.isArray(parsed)) return [];
      return parsed.map((e) => e.modelId).filter((id): id is string => typeof id === "string");
    } catch {
      return [];
    }
  }, [draft]);
  const delegateMatches = useMemo(() => {
    if (!delegateToken) return [] as DelegateCandidate[];
    return filterDelegateCandidates(agents, installedAgentIds, delegateToken, recentModelIds);
  }, [agents, installedAgentIds, delegateToken, recentModelIds]);
  const showDelegateMenu = delegateMatches.length > 0 && delegateToken != null && !showSlashMenu;

  useEffect(() => {
    if (!showDelegateMenu && menu !== "agent") return;
    void listAgentCommands().then((list) => {
      setInstalledAgentIds(
        new Set(list.filter((s) => s.status === "installed").map((s) => s.id)),
      );
    });
  }, [showDelegateMenu, menu]);

  useEffect(() => {
    setDelegateIndex(0);
  }, [delegateToken?.raw, showDelegateMenu]);

  const pickDelegate = useCallback(
    (candidate: DelegateCandidate) => {
      if (!delegateToken) return;
      const next = applyDelegateCandidate(
        draft,
        delegateToken.start,
        delegateToken.end,
        candidate,
      );
      setDraft(next);
      requestAnimationFrame(() => {
        const el = textareaRef.current;
        if (!el) return;
        const pos = delegateToken.start + (candidate.modelId
          ? `@${candidate.agentId}/${candidate.modelId} `.length
          : cachedCapabilitiesFor(candidate.agentId)?.models.length
            ? `@${candidate.agentId}/`.length
            : `@${candidate.agentId} `.length);
        el.focus();
        el.setSelectionRange(pos, pos);
        setCaret(pos);
      });
    },
    [delegateToken, draft],
  );

  useEffect(() => {
    setSlashIndex(0);
  }, [slashToken?.query, showSlashMenu]);

  const pickSlash = useCallback(
    (cmd: AvailableCommand) => {
      if (!slashToken) return;
      const next = applySlashCommand(draft, slashToken.start, slashToken.end, cmd);
      const pos = slashToken.start + cmd.name.length + 2; // `/name `
      setDraft(next);
      setCaret(pos);
      requestAnimationFrame(() => {
        const el = textareaRef.current;
        if (!el) return;
        el.focus();
        el.setSelectionRange(pos, pos);
      });
    },
    [draft, slashToken],
  );

  // A note about a finished install should not outlive the menu.
  useEffect(() => {
    if (menu !== "agent" && installNote && !installingAgentId) setInstallNote("");
  }, [menu, installNote, installingAgentId]);

  const tone = modeTone(displayMode);
  const isTall = composerHeight > COMPOSER_HEIGHT_DEFAULT + 40;

  return (
    <footer
      ref={composerRef}
      className={[
        "composer",
        isBusy ? "is-busy" : "",
        isWarming ? "is-warming" : "",
        resizingComposer ? "is-resizing" : "",
        hasModes ? "has-mode" : "",
      ]
        .filter(Boolean)
        .join(" ")}
      data-mode-tone={tone}
      style={{ height: composerHeight }}
      aria-label="Composer"
    >
      {/* Drag handle: top edge — free height between MIN/MAX, remembered. */}
      <button
        type="button"
        className="composer__resize"
        aria-label="Resize composer height"
        title="Drag to resize"
        onMouseDown={(event) => {
          event.preventDefault();
          setResizingComposer(true);
        }}
      />
      {/* Left rail — always-on mode cue, colored by tone. */}
      {hasModes && <div className="composer__mode-rail" aria-hidden />}
      {flashError && <div className="composer__error-toast">{flashError}</div>}
      {/* Connect/warm phase. Floats above the composer like the error toast:
          reconnecting is background work and must never resize the transcript
          or the composer under the user's cursor. */}
      {isWarming && (
        <div className="composer__warming" role="status" aria-live="polite">
          <span className="composer__warming-pulse" aria-hidden />
          <span>Agent connecting in background…</span>
        </div>
      )}
      {/* Fixed-height chip tray — opacity only, never resizes the composer. */}
      <div
        className={
          suggestions.length > 0
            ? "composer__suggestions is-visible"
            : "composer__suggestions"
        }
        aria-hidden={suggestions.length === 0}
      >
        {suggestions.map((chip) => (
          <button
            key={chip.id}
            type="button"
            className="composer__suggestion-chip"
            title={chip.text}
            onMouseDown={(e) => e.preventDefault()}
            onClick={() => sendSuggestion(chip)}
          >
            {chip.label}
          </button>
        ))}
      </div>
      <div
        className={
          dropActive
            ? "composer__field is-drop-target"
            : imageAttachments.length > 0
              ? "composer__field has-attachments"
              : "composer__field"
        }
        onDragEnter={onComposerDragEnter}
        onDragOver={onComposerDragOver}
        onDragLeave={onComposerDragLeave}
        onDrop={onComposerDrop}
      >
        {/* Expand/collapse height — floats top-right, clear of the textarea. */}
        <button
          className="composer-expand"
          type="button"
          title={isTall ? "Collapse composer height" : "Expand composer height"}
          aria-label={isTall ? "Collapse composer height" : "Expand composer height"}
          aria-pressed={isTall}
          onMouseDown={(event) => {
            event.preventDefault();
          }}
          onClick={(event) => {
            event.preventDefault();
            event.stopPropagation();
            setComposerHeight((h) =>
              h > COMPOSER_HEIGHT_DEFAULT + 40
                ? COMPOSER_HEIGHT_DEFAULT
                : Math.min(COMPOSER_HEIGHT_MAX, Math.round(window.innerHeight * 0.36))
            );
            requestAnimationFrame(() => textareaRef.current?.focus());
          }}
        >
          {isTall ? <Shrink size={13} /> : <Expand size={13} />}
        </button>
        {dropActive && <div className="composer__drop-hint">Drop to insert file path</div>}
        {/* Pills live inside the field (Codex-style), not above it next to suggestions. */}
        {imageAttachments.length > 0 && (
          <div className="composer__attachments" aria-label="Image attachments">
            {imageAttachments.map((att) => (
              <span key={att.id} className="composer__attachment-pill">
                <button
                  type="button"
                  className="composer__attachment-main"
                  title={`${att.path}${att.marks.length ? ` · ${att.marks.length} 条批注` : " · 点击标注"}`}
                  onClick={() => setAnnotatingId(att.id)}
                >
                  <span className="composer__attachment-name">{att.name}</span>
                  {att.marks.length > 0 && (
                    <span className="composer__attachment-badge">{att.marks.length}</span>
                  )}
                </button>
                <button
                  type="button"
                  className="composer__attachment-x"
                  title="移除"
                  aria-label={`移除 ${att.name}`}
                  onClick={() => {
                    setImageAttachments((cur) => cur.filter((a) => a.id !== att.id));
                    if (annotatingId === att.id) setAnnotatingId(null);
                  }}
                >
                  ✕
                </button>
              </span>
            ))}
          </div>
        )}
        {showSlashMenu && (
          <div className="composer-slash" role="listbox" aria-label="Slash commands">
            <div className="composer-slash__hint">Commands</div>
            {slashMatches.map((cmd, i) => (
              <button
                key={cmd.name}
                type="button"
                role="option"
                aria-selected={i === slashIndex}
                className={i === slashIndex ? "composer-slash__item is-active" : "composer-slash__item"}
                onMouseDown={(e) => {
                  // Prevent textarea blur before click applies.
                  e.preventDefault();
                }}
                onClick={() => pickSlash(cmd)}
              >
                <span className="composer-slash__name">/{cmd.name}</span>
                <span className="composer-slash__desc">
                  {cmd.description}
                  {cmd.input?.hint ? ` · ${cmd.input.hint}` : ""}
                </span>
              </button>
            ))}
          </div>
        )}
        {showDelegateMenu && (
          <div className="composer-slash" role="listbox" aria-label="Delegate to agent">
            <div className="composer-slash__hint">派给 Agent · 行首 @</div>
            {delegateMatches.map((c, i) => (
              <button
                key={`${c.agentId}:${c.modelId ?? "_"}`}
                type="button"
                role="option"
                aria-selected={i === delegateIndex}
                className={i === delegateIndex ? "composer-slash__item is-active" : "composer-slash__item"}
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => pickDelegate(c)}
              >
                <span className="composer-slash__name">
                  @{c.agentId}
                  {c.modelId ? `/${c.modelId}` : ""}
                </span>
                <span className="composer-slash__desc">
                  {c.modelLabel || c.agentLabel}
                  {c.recent ? " · 上次用过" : ""}
                  {c.installed ? " · ✓ 已安装" : " · ⚠ 未安装"}
                </span>
              </button>
            ))}
          </div>
        )}
        <textarea
          ref={textareaRef}
          aria-label="Prompt composer"
          placeholder={
            isWarming && !composingRef.current
              ? "Agent connecting in background… keep typing"
              : sendOnEnter
                ? "Message the Agent (Enter to send · Shift+Enter newline) · / commands · Tab cycles mode"
                : "Message the Agent (Ctrl+Enter to send) · / commands · Tab cycles mode"
          }
          rows={isTall ? 10 : 2}
          value={draft}
          onFocus={() => {
            warmedThisFocusRef.current = false;
            // Warm after focus settles — not on every keystroke.
            warmIfNeeded();
          }}
          onBlur={() => {
            warmedThisFocusRef.current = false;
          }}
          onCompositionStart={() => {
            composingRef.current = true;
            setIsComposing(true);
          }}
          onCompositionEnd={(event) => {
            composingRef.current = false;
            setIsComposing(false);
            // Ensure final composed text is stored (some IMEs need this).
            setDraft(event.currentTarget.value);
            warmIfNeeded();
          }}
          onChange={(event) => {
            const next = event.target.value;
            setDraft(next);
            setCaret(event.target.selectionStart ?? next.length);
            // Typing dismisses chips for this event-tail round (drop path-only is not typing).
            if (
              draftAfterDrop == null ||
              next !== draftAfterDrop
            ) {
              if (next.trim()) {
                setDismissedSuggestKey(suggestRound);
                if (draftAfterDrop != null && next !== draftAfterDrop) {
                  clearDropSuggest();
                }
              }
            }
            // Do not warm on every keypress — freezes IME while ACP handshake runs.
          }}
          onSelect={(event) => {
            setCaret(event.currentTarget.selectionStart ?? 0);
          }}
          onPaste={onComposerPaste}
          onKeyDown={(event) => {
            if (showDelegateMenu && !composingRef.current) {
              if (event.key === "ArrowDown") {
                event.preventDefault();
                setDelegateIndex((i) => Math.min(delegateMatches.length - 1, i + 1));
                return;
              }
              if (event.key === "ArrowUp") {
                event.preventDefault();
                setDelegateIndex((i) => Math.max(0, i - 1));
                return;
              }
              // Spec: Tab/Enter only consume when selecting a candidate — but plain Enter
              // with the menu open still **sends** (unlike slash). Tab always picks.
              if (event.key === "Tab" && !event.altKey && !event.metaKey && !event.ctrlKey) {
                event.preventDefault();
                const c = delegateMatches[delegateIndex];
                if (c) pickDelegate(c);
                return;
              }
              if (event.key === "Escape") {
                event.preventDefault();
                event.stopPropagation();
                setCaret(-1);
                return;
              }
              // Enter: fall through to normal send handling below.
            }
            if (showSlashMenu && !composingRef.current) {
              if (event.key === "ArrowDown") {
                event.preventDefault();
                setSlashIndex((i) => Math.min(slashMatches.length - 1, i + 1));
                return;
              }
              if (event.key === "ArrowUp") {
                event.preventDefault();
                setSlashIndex((i) => Math.max(0, i - 1));
                return;
              }
              if (event.key === "Enter" && !event.ctrlKey && !event.metaKey && !event.shiftKey) {
                event.preventDefault();
                const cmd = slashMatches[slashIndex];
                if (cmd) pickSlash(cmd);
                return;
              }
              if (event.key === "Tab" && !event.altKey && !event.metaKey && !event.ctrlKey) {
                event.preventDefault();
                const cmd = slashMatches[slashIndex];
                if (cmd) pickSlash(cmd);
                return;
              }
              if (event.key === "Escape") {
                event.preventDefault();
                event.stopPropagation();
                // Leave the token incomplete but close the menu (caret off token).
                setCaret(-1);
                return;
              }
            }
            // Send trigger depends on the toggle: Enter (no modifiers) or
            // Ctrl/Cmd+Enter. Shift+Enter always inserts a newline.
            // In Ctrl+Enter mode the pre-toggle behaviour is kept verbatim,
            // including Ctrl+Shift+Enter sending.
            const enterSends =
              sendOnEnter &&
              event.key === "Enter" &&
              !event.shiftKey &&
              !event.ctrlKey &&
              !event.metaKey &&
              !event.altKey;
            const ctrlEnterSends =
              !sendOnEnter &&
              event.key === "Enter" &&
              (event.ctrlKey || event.metaKey);
            if (event.key === "Enter" && (enterSends || ctrlEnterSends)) {
              // Do not swallow IME confirm keydowns — never preventDefault during
              // an active composition (the submit guard is right below).
              if (composingRef.current) return;
              event.preventDefault();
              submit();
              return;
            }
            // P2-UX-1: Tab cycles mode (OpenCode-like). Shift+Tab reverse.
            if (event.key === "Tab" && !event.altKey && !event.metaKey && !event.ctrlKey) {
              if (composingRef.current) return;
              if (!hasModes) return; // allow default focus move when no modes
              event.preventDefault();
              cycleMode(event.shiftKey ? -1 : 1);
              return;
            }
            // Esc: close menus first; if tall with no menu, collapse height.
            if (event.key === "Escape") {
              if (menu) {
                event.preventDefault();
                event.stopPropagation();
                setMenu(null);
                return;
              }
              if (isTall) {
                event.preventDefault();
                event.stopPropagation();
                setComposerHeight(COMPOSER_HEIGHT_DEFAULT);
              }
            }
          }}
        />
        <div className="composer__toolbar">
          <div className="composer__controls">
            {/* Browser fallback only — desktop uses native pick_files (absolute paths). */}
            <input
              ref={fileInputRef}
              type="file"
              multiple
              style={{ display: "none" }}
              onChange={(e) => {
                const files = e.target.files;
                if (!files || files.length === 0) return;
                const paths: string[] = [];
                for (const file of Array.from(files)) {
                  const path = (file as File & { path?: string }).path;
                  paths.push(path && path.trim() ? path : file.name);
                }
                insertDroppedPaths(paths);
                e.target.value = "";
              }}
            />
            <button
              className="pill-action pill-action--icon composer-tool"
              type="button"
              title="Add files or context"
              onClick={() => {
                if (isTauriRuntime()) {
                  void pickFiles().then((paths) => {
                    if (paths.length > 0) insertDroppedPaths(paths);
                  });
                  return;
                }
                fileInputRef.current?.click();
              }}
            >
              <Plus size={14} />
            </button>
            {/* Execution mode — flat control beside the tools, matching model/agent. */}
            {hasModes && displayMode && (
              <div className="composer-menu-anchor composer-menu-anchor--mode">
                <button
                  className="composer-mode-chip"
                  type="button"
                  title="Execution mode (Tab to cycle)"
                  aria-expanded={menu === "mode"}
                  onClick={() => setMenu(menu === "mode" ? null : "mode")}
                >
                  <span className="composer-mode-chip__label">{displayModeLabel}</span>
                </button>
                {menu === "mode" && (
                  <div className="composer-menu composer-menu--mode" role="menu" aria-label="Execution mode">
                    {caps!.modes.map((m) => (
                      <button
                        key={m.id}
                        className={displayMode === m.id ? "is-selected" : ""}
                        type="button"
                        role="menuitem"
                        data-mode-tone={modeTone(m.id)}
                        onClick={() => {
                          const prev = displayMode;
                          pinOptions({ mode: m.id });
                          setCurrentMode(m.id);
                          setMenu(null);
                          void commitUpdate({ mode: m.id }, () => {
                            unpinOption("mode");
                            setCurrentMode(prev);
                          });
                        }}
                      >
                        <span className="composer-menu__mode-dot" aria-hidden />
                        {m.label}
                      </button>
                    ))}
                  </div>
                )}
              </div>
            )}
            {/* Grok always-approve: the important permission axis (not plan/build). */}
            {alwaysApproveSpec && (
              <button
                className={
                  alwaysApprove
                    ? "composer-mode-chip composer-mode-chip--always-on"
                    : "composer-mode-chip composer-mode-chip--always-off"
                }
                type="button"
                title={
                  alwaysApprove
                    ? "Always approve is ON — agent skips permission prompts (click to ask first)"
                    : "Ask permissions — agent will prompt before edits (click for always-approve)"
                }
                aria-pressed={alwaysApprove}
                onClick={() => {
                  const next = !alwaysApprove;
                  const prev = alwaysApprove;
                  setAlwaysApprove(next);
                  void commitUpdate({ alwaysApprove: next }, () => {
                    setAlwaysApprove(prev);
                  });
                }}
              >
                <span className="composer-mode-chip__label">
                  {alwaysApprove
                    ? (alwaysApproveSpec.onLabel ?? "Always approve")
                    : (alwaysApproveSpec.offLabel ?? "Ask permissions")}
                </span>
              </button>
            )}
            {/* Force web search via prompt prefix — not a network firewall. */}
            <button
              className={
                forceWebSearch
                  ? "composer-mode-chip composer-mode-chip--web-on composer-mode-chip--icon-only"
                  : "composer-mode-chip composer-mode-chip--web-off composer-mode-chip--icon-only"
              }
              type="button"
              title={
                forceWebSearch
                  ? "已开启：发送时会要求模型先联网检索再回答（需 agent 有搜索/抓取工具）"
                  : "点击开启：在提示词中强制先联网检索官方文档再回答（不是断网开关）"
              }
              aria-pressed={forceWebSearch}
              aria-label={forceWebSearch ? "关闭强制联网检索" : "开启强制联网检索"}
              onClick={() => setForceWebSearch((v) => !v)}
            >
              <Globe size={13} aria-hidden />
            </button>
          </div>
          <div className="composer__actions">
            {/* ── Model + Effort selector ─────────────────────────────── */}
            {/* Always show when caps exist (even empty for add-key prompt), or has effort */}
            {(hasModels || hasEffort || showKeyPrompt) && (
              <div className="composer-menu-anchor">
                <button
                  className="composer-select composer-select--model"
                  type="button"
                  title={displayModelFull ?? "Choose model"}
                  aria-expanded={menu === "model" || menu === "effort"}
                  onClick={() => setMenu(menu === "model" ? null : "model")}
                >
                  {displayModelLabel ?? "default"}
                  {displayEffort != null && hasEffort ? ` · ${displayEffort}` : ""}
                </button>

                {menu === "model" && (
                  <div className="composer-menu composer-menu--models" role="listbox" aria-label={`${agent.label} models`}>
                    {/* The key prompt renders *above* the rest instead of replacing
                        it. As a ternary it also swallowed the effort block below,
                        and since nothing ever sets `menu = "effort"`, an agent with
                        effort options but no model list lost every way to change
                        effort. */}
                    {!hasModels && showKeyPrompt && (
                      <div className="composer-menu__empty-state">
                        <div className="composer-menu__empty-icon">🔑</div>
                        <div className="composer-menu__empty-title">还没有配置 API Key</div>
                        <div className="composer-menu__empty-desc">
                          选择服务商并输入 Key 即可开始使用
                        </div>
                        <button
                          className="composer-menu__add-key-btn"
                          type="button"
                          onClick={() => { setMenu(null); setShowProviderDialog(true); }}
                        >
                          添加 API Key
                        </button>
                      </div>
                    )}
                    {!hasModels && !showKeyPrompt && !hasEffort && (
                      <div className="composer-menu__empty">这个 agent 没有可切换的模型</div>
                    )}
                        {hasModels && (
                          <>
                            {/* ── Recent models ── */}
                            {recentModels.length > 0 && (
                              <div className="composer-menu__recent">
                                <span className="composer-menu__label">最近使用</span>
                                {recentModels.map((entry) => {
                                  const model = caps?.models.find((m) => m.id === entry.modelId);
                                  if (!model) return null;
                                  return (
                                    <button
                                      key={entry.modelId}
                                      className={displayModel === entry.modelId ? "is-selected" : ""}
                                      type="button"
                                      role="option"
                                      aria-selected={displayModel === entry.modelId}
                                      onClick={() => {
                                        const prev = displayModel;
                                        pinOptions({ model: entry.modelId });
                                        setCurrentModel(entry.modelId);
                                        setMenu(null);
                                        void commitUpdate({ model: entry.modelId }, () => {
                                          unpinOption("model");
                                          setCurrentModel(prev);
                                        });
                                      }}
                                    >
                                      <span className="composer-menu__model-name">{shortModelName(model)}</span>
                                      <span className="composer-menu__model-recent-hint">{formatRecentTime(entry.lastUsedAt)}</span>
                                    </button>
                                  );
                                })}
                              </div>
                            )}
                            {recentModels.length > 0 && <span className="composer-menu__divider" />}

                            {/* ── Search ── */}
                            <div className="composer-menu__search">
                              <Search size={13} aria-hidden />
                              <input
                                ref={modelSearchRef}
                                type="search"
                                value={modelQuery}
                                placeholder="Search provider or model…"
                                aria-label="Search models"
                                onChange={(e) => setModelQuery(e.target.value)}
                                onKeyDown={(e) => {
                                  if (e.key === "Escape") {
                                    e.stopPropagation();
                                    setMenu(null);
                                  }
                                }}
                              />
                            </div>
                            <div className="composer-menu__scroll">
                              {modelGroups.length === 0 && (
                                <div className="composer-menu__empty">No models match “{modelQuery}”</div>
                              )}
                              {modelGroups.map((group) => (
                                <div className="composer-menu__group" key={group.provider}>
                                  <span className="composer-menu__label">{group.provider}</span>
                                  {group.models.map((m) => (
                                    <button
                                      key={m.id}
                                      className={displayModel === m.id ? "is-selected" : ""}
                                      type="button"
                                      role="option"
                                      aria-selected={displayModel === m.id}
                                      title={modelTooltip(m)}
                                      onClick={() => {
                                        const prev = displayModel;
                                        pinOptions({ model: m.id });
                                        setCurrentModel(m.id);
                                        // Never jump to Effort before the agent re-negotiates
                                        // options — Haiku drops effort and a stale menu white-screens.
                                        setMenu(null);
                                        void commitUpdate({ model: m.id }, () => {
                                          unpinOption("model");
                                          setCurrentModel(prev);
                                        });
                                      }}
                                    >
                                      <span className="composer-menu__model-name">{shortModelName(m)}</span>
                                    </button>
                                  ))}
                                </div>
                              ))}
                            </div>

                            {/* ── Add API Key entry at bottom ── */}
                            {usesOpencodeAuth && (
                              <>
                                <span className="composer-menu__divider" />
                                <button
                                  className="composer-menu__add-key-item"
                                  type="button"
                                  onClick={() => { setMenu(null); setShowProviderDialog(true); }}
                                >
                                  ＋ 添加 API Key…
                                </button>
                              </>
                            )}
                          </>
                        )}
                        {hasEffort && (hasModels || showKeyPrompt) && (
                          <span className="composer-menu__divider" />
                        )}
                        {hasEffort && (
                          <div className="composer-menu__effort-inline">
                            <span className="composer-menu__label">
                              {hasStringEffort ? "Effort" : "Response strength"}
                            </span>
                            {hasStringEffort
                              ? effortOptions.map((opt) => (
                                  <button
                                    key={opt.id}
                                    className={currentEffortId === opt.id ? "is-selected" : ""}
                                    type="button"
                                    onClick={() => {
                                      const prev = currentEffortId;
                                      pinOptions({ effortId: opt.id });
                                      setCurrentEffortId(opt.id);
                                      setMenu(null);
                                      void commitUpdate({ effortId: opt.id }, () => {
                                        unpinOption("effortId");
                                        setCurrentEffortId(prev);
                                      });
                                    }}
                                  >
                                    {opt.label}
                                  </button>
                                ))
                              : numericEffortPresets.map((preset) => (
                                  <button
                                    key={preset.label}
                                    className={
                                      currentEffort != null &&
                                      Math.abs(currentEffort - preset.value) < 0.01
                                        ? "is-selected"
                                        : ""
                                    }
                                    type="button"
                                    onClick={() => {
                                      const prev = currentEffort;
                                      pinOptions({ effort: preset.value });
                                      setCurrentEffort(preset.value);
                                      setMenu(null);
                                      void commitUpdate({ thinkingEffort: preset.value }, () => {
                                        unpinOption("effort");
                                        setCurrentEffort(prev);
                                      });
                                    }}
                                  >
                                    {preset.label}
                                  </button>
                                ))}
                          </div>
                        )}
                  </div>
                )}

                {menu === "effort" && hasEffort && (
                  <div className="composer-menu" role="menu" aria-label="Effort">
                    <span className="composer-menu__label">
                      {hasStringEffort ? "Effort" : "Response strength"}
                    </span>
                    {hasStringEffort
                      ? effortOptions.map((opt) => (
                          <button
                            key={opt.id}
                            className={currentEffortId === opt.id ? "is-selected" : ""}
                            type="button"
                            role="menuitem"
                            onClick={() => {
                              const prev = currentEffortId;
                              pinOptions({ effortId: opt.id });
                              setCurrentEffortId(opt.id);
                              setMenu(null);
                              void commitUpdate({ effortId: opt.id }, () => {
                                unpinOption("effortId");
                                setCurrentEffortId(prev);
                              });
                            }}
                          >
                            {opt.label}
                          </button>
                        ))
                      : numericEffortPresets.map((preset) => (
                          <button
                            key={preset.label}
                            className={
                              currentEffort != null &&
                              Math.abs(currentEffort - preset.value) < 0.01
                                ? "is-selected"
                                : ""
                            }
                            type="button"
                            role="menuitem"
                            onClick={() => {
                              const prev = currentEffort;
                              pinOptions({ effort: preset.value });
                              setCurrentEffort(preset.value);
                              setMenu(null);
                              void commitUpdate({ thinkingEffort: preset.value }, () => {
                                unpinOption("effort");
                                setCurrentEffort(prev);
                              });
                            }}
                          >
                            {preset.label} ({preset.value.toFixed(1)})
                          </button>
                        ))}
                  </div>
                )}
              </div>
            )}

            {/* ── Agent switcher ──────────────────────────────────────── */}
            <div className="composer-menu-anchor">
              <button
                className={
                  currentHasUpdate
                    ? "composer-select composer-select--agent has-update"
                    : "composer-select composer-select--agent"
                }
                type="button"
                title={
                  currentAgentVersion?.installed
                    ? currentHasUpdate
                      ? `${agent.label} v${currentAgentVersion.installed} · update ${currentAgentVersion.latest} available`
                      : `${agent.label} v${currentAgentVersion.installed}`
                    : `Agent for this dialog (bound to session · ${agent.id})`
                }
                aria-expanded={menu === "agent"}
                onClick={() => setMenu(menu === "agent" ? null : "agent")}
              >
                <span className="composer-select__agent-label">{agent.label}</span>
                {currentAgentVersion?.installed && (
                  <span className="composer-select__agent-version">
                    v{currentAgentVersion.installed}
                  </span>
                )}
                {currentHasUpdate && (
                  <span className="composer-select__agent-update" aria-label="Update available">
                    ↑
                  </span>
                )}
              </button>
              {menu === "agent" && (
                <div className="composer-menu composer-menu--agent" role="listbox" aria-label="Switch agent">
                  {agents
                    .filter((candidate) => candidate.enabled)
                    .map((candidate) => {
                      const status = agentStatuses[candidate.id];
                      const ready = !status || status.status === "installed";
                      const busyInstall = installingAgentId === candidate.id;
                      const canInstall =
                        !ready && (status?.installable ?? false) && !busyInstall;
                      // Grok / Hermes / Cursor: no in-app installer — show setup help instead of a blank gap.
                      const needsManualSetup =
                        !ready &&
                        !canInstall &&
                        !busyInstall &&
                        candidate.install.manager === "manual";
                      const manualHint =
                        candidate.install.note?.trim() ||
                        status?.message?.trim() ||
                        `Install \`${candidate.command}\` and put it on PATH, then restart Marionette.`;
                      const version = agentVersionInfo[candidate.id];
                      // Offer the upgrade from cached registry state — no need
                      // to re-open the menu to re-test.
                      const canUpdate =
                        ready && (version?.updateAvailable ?? false) && !busyInstall;
                      return (
                        <div className="composer-agent-row" key={candidate.id}>
                          <button
                            className={agent.id === candidate.id ? "is-selected" : ""}
                            type="button"
                            role="option"
                            aria-selected={agent.id === candidate.id}
                            title={
                              needsManualSetup
                                ? manualHint
                                : (status?.message ?? candidate.command)
                            }
                            onClick={() => {
                              setMenu(null);
                              // Always mirror session.agentId (via agent prop), never a floating selection.
                              onAgentChange(candidate.id);
                            }}
                          >
                            <span className="composer-agent-row__name">{candidate.label}</span>
                            {version?.installed ? (
                              <span
                                className="composer-agent-row__version"
                                title={
                                  version.latest
                                    ? `installed ${version.installed} · latest ${version.latest}`
                                    : (version.note ?? `installed ${version.installed}`)
                                }
                              >
                                v{version.installed}
                              </span>
                            ) : version?.note ? (
                              <span
                                className="composer-agent-row__version"
                                title={version.note}
                              >
                                ?
                              </span>
                            ) : null}
                            {!ready && (
                              <span className="composer-agent-row__tag">
                                {busyInstall
                                  ? "installing…"
                                  : status?.status === "incomplete"
                                    ? "needs CLI"
                                    : needsManualSetup
                                      ? "需本机安装"
                                      : "not installed"}
                              </span>
                            )}
                          </button>
                          {canUpdate && (
                            <button
                              className="composer-agent-row__install"
                              type="button"
                              title={`npm install -g ${version?.package ?? ""} (${version?.installed} → ${version?.latest})`}
                              onClick={(event) => {
                                event.stopPropagation();
                                void runInstall(candidate.id, { upgrade: true });
                              }}
                            >
                              {`→ ${version?.latest}`}
                            </button>
                          )}
                          {canInstall && (
                            <button
                              className="composer-agent-row__install"
                              type="button"
                              title={`npm install -g ${candidate.install.package ?? ""}`.trim()}
                              onClick={(event) => {
                                event.stopPropagation();
                                void runInstall(candidate.id);
                              }}
                            >
                              Install
                            </button>
                          )}
                          {needsManualSetup && (
                            <button
                              className="composer-agent-row__install composer-agent-row__install--manual"
                              type="button"
                              title={manualHint}
                              onClick={(event) => {
                                event.stopPropagation();
                                const pf = preflightById[candidate.id];
                                const pfLines = pf
                                  ? pf.checks
                                      .map((c) => `${c.status === "pass" ? "✓" : c.status === "warn" ? "!" : "✗"} ${c.label}: ${c.message}`)
                                      .join("\n")
                                  : "";
                                setInstallNote(
                                  `${candidate.label} 无法在应用内一键安装。\n\n${manualHint}${
                                    pfLines ? `\n\n环境检查：\n${pfLines}` : ""
                                  }\n\n装好后请重启 Marionette，再选该 agent。`,
                                );
                                void navigator.clipboard?.writeText(manualHint).catch(() => undefined);
                              }}
                            >
                              如何安装
                            </button>
                          )}
                          {!ready && !needsManualSetup && preflightById[candidate.id] && !preflightById[candidate.id].passed && (
                            <button
                              className="composer-agent-row__install composer-agent-row__install--manual"
                              type="button"
                              title="环境检查未通过"
                              onClick={(event) => {
                                event.stopPropagation();
                                const pf = preflightById[candidate.id];
                                if (!pf) return;
                                setInstallNote(
                                  `${pf.agentName} 环境检查：\n` +
                                    pf.checks
                                      .map(
                                        (c) =>
                                          `${c.status === "pass" ? "✓" : c.status === "warn" ? "!" : "✗"} ${c.label}: ${c.message}`,
                                      )
                                      .join("\n"),
                                );
                              }}
                            >
                              检查
                            </button>
                          )}
                          {candidate.id.startsWith("custom-") || candidate.install.note?.includes("Custom agent") ? (
                            <button
                              className="composer-agent-row__install composer-agent-row__install--manual"
                              type="button"
                              title="删除自定义 agent"
                              onClick={(event) => {
                                event.stopPropagation();
                                if (!window.confirm(`删除自定义 agent「${candidate.label}」？`)) return;
                                void removeCustomAgent(candidate.id)
                                  .then(async () => {
                                    setInstallNote(`已删除 ${candidate.label}`);
                                    await onAgentsReload?.();
                                    void refreshAgentStatuses();
                                  })
                                  .catch((e) =>
                                    setInstallNote(e instanceof Error ? e.message : String(e)),
                                  );
                              }}
                            >
                              删除
                            </button>
                          ) : null}
                        </div>
                      );
                    })}
                  <button
                    type="button"
                    className="composer-menu__add-custom"
                    onClick={() => void runAddCustomAgent()}
                  >
                    + 添加自定义 agent
                  </button>
                  {installNote && <div className="composer-menu__note">{installNote}</div>}
                </div>
              )}
            </div>

            {/* ── Send / Interrupt (Esc×2 also interrupts) ──
                Right-click toggles the Enter vs Ctrl+Enter send shortcut. */}
            <div className="composer-menu-anchor composer-menu-anchor--send">
              <button
                className={
                  isBusy && canCancel ? "send-button is-interrupting" : "send-button"
                }
                type="button"
                title={
                  isBusy && canCancel
                    ? "Interrupt (or double-press Esc)"
                    : isBusy && !canCancel
                      ? "Agent is busy"
                      : "Send · 右键切换 Enter / Ctrl+Enter 发送"
                }
                aria-label={
                  isBusy && canCancel
                    ? "Interrupt conversation"
                    : isBusy && !canCancel
                      ? "Agent is busy"
                      : "Send"
                }
                aria-haspopup="menu"
                aria-expanded={menu === "send"}
                disabled={isBusy && !canCancel}
                onClick={() => {
                  setMenu(null);
                  if (isBusy && canCancel) onInterrupt();
                  else if (!isBusy) submit();
                }}
                onContextMenu={(event) => {
                  event.preventDefault();
                  setMenu(menu === "send" ? null : "send");
                }}
              >
                {isBusy && canCancel ? (
                  <Square size={12} fill="currentColor" />
                ) : (
                  <SendHorizontal size={15} />
                )}
              </button>
              {menu === "send" && (
                <div
                  className="composer-menu composer-menu--send"
                  role="menu"
                  aria-label="发送快捷键"
                >
                  <div className="composer-menu__label">发送快捷键</div>
                  <button
                    type="button"
                    role="menuitem"
                    className={!sendOnEnter ? "is-selected" : ""}
                    onClick={() => {
                      setSendOnEnter(false);
                      setMenu(null);
                    }}
                  >
                    Ctrl+Enter 发送
                  </button>
                  <button
                    type="button"
                    role="menuitem"
                    className={sendOnEnter ? "is-selected" : ""}
                    onClick={() => {
                      setSendOnEnter(true);
                      setMenu(null);
                    }}
                  >
                    Enter 发送
                  </button>
                </div>
              )}
            </div>
          </div>
        </div>
      </div>
      {annotatingId &&
        (() => {
          const att = imageAttachments.find((a) => a.id === annotatingId);
          if (!att) return null;
          return (
            <ImageAnnotator
              attachment={att}
              onClose={() => setAnnotatingId(null)}
              onSave={(marks: ImageMark[]) => {
                setImageAttachments((cur) =>
                  cur.map((a) => (a.id === att.id ? { ...a, marks } : a)),
                );
              }}
            />
          );
        })()}
      {showProviderDialog && (
        <ProviderConfigDialog
          restartPending={providerKeysDirty}
          onKeysChanged={() => setProviderKeysDirty(true)}
          onClose={() => {
            setShowProviderDialog(false);
            if (!providerKeysDirty) return;
            setProviderKeysDirty(false);
            // Re-reading capabilities off the *live* process is not enough:
            // OpenCode loads auth.json once at startup, so the running agent
            // still reports the old (empty) model list and the selector stays
            // blank — the exact dead end this dialog exists to fix. Only a new
            // process picks the key up. Batched to dialog close so adding three
            // keys restarts once.
            void onProviderKeysChanged?.();
          }}
        />
      )}
    </footer>
  );
}
