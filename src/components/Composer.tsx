import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { Check, ChevronRight, Expand, Plus, Search, SendHorizontal, Settings2, Shrink, SlidersHorizontal, Square } from "lucide-react";
import {
  useCallback,
  useEffect,
  useLayoutEffect,
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
  modelAssertForEffortRefresh,
  normalizeAgentModeId,
} from "../lib/acpSupplements";
import {
  agentPreflight,
  bridgeBind,
  bridgeGetBinding,
  getSessionCapabilities,
  installAgent,
  isTauriRuntime,
  listAgentCommands,
  listCustomAgents,
  pickFiles,
  readImageDataUrl,
  removeCustomAgent,
  savePastedImage,
  sendAcpPrompt,
  updateAcpSession,
  type BridgeModel,
  type BridgeModelsResult,
  type PreflightResult,
} from "../lib/api";
import {
  bridgePresetFor,
  getBridgeModels,
  invalidateBridgeModels,
  resolveBridgeSources,
  subscribeBridgeModels,
} from "../lib/bridge";
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
import { t, useT } from "../lib/i18n";
import { ensureModelSources, expandHiddenProviders, libraryGroups, refreshModelSources, visibleLibraryGroups, useModelLibrary } from "../lib/modelLibrary";
import { modeTone } from "../lib/modeTone";
import { DOCK_DROP_PATHS_EVENT, DOCK_DROP_TEXT_EVENT } from "../lib/dockDrop";
import { isPageDragSource, onDragText } from "../lib/dragText";
import { foldLocalSingleGroups, splitModelMeta } from "../lib/modelMenu";
import {
  modelProviderGroup,
  modelRowLabel,
  modelTooltip,
  prettyEffortLabel,
  prettyModelChip,
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
import { ModelLibraryDialog } from "./ModelLibraryDialog";
import { CredentialsDialog } from "./CredentialsDialog";
import { ConfirmDialog } from "./ConfirmDialog";
import { ImageAnnotator } from "./ImageAnnotator";
import { CustomAgentDialog } from "./CustomAgentDialog";
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
  SessionComposerPrefs,
  SessionEvent,
  SessionStatus,
} from "../lib/types";

let suggestionsEnabledCache: boolean | undefined;
const suggestionsEnabledCached = (): boolean => {
  if (suggestionsEnabledCache === undefined) {
    suggestionsEnabledCache = suggestionsEnabled();
  }
  return suggestionsEnabledCache;
};

type ComposerProps = {
  agent: AgentConfig;
  agents: AgentConfig[];
  sessionId: string;
  sessionStatus: SessionStatus;
  /**
   * 当场重启会话的 agent 进程（复用挂起机制：历史保留、下一条消息带历史自动
   * 拉起）。跨用模型切换/切回原生时，注入是 spawn 级的（env/配置表），活进程
   * 必须换血才能立即生效 —— 「下次打开才有效」不可接受。
   */
  onRestartAgentSession?: (sessionId: string, banner: string) => void;
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
      /**
       * Composer chip snapshot at send time. Parent must not re-derive mode
       * from stale sessionCapabilities — mode switches skip immediate caps
       * reload (agent often echoes the previous mode for a beat).
       */
      modeId?: string | null;
      modeLabel?: string | null;
      modelId?: string | null;
      modelLabel?: string | null;
      /**
       * Effort as displayed. The parent persists these with the dialog so a
       * selection that was inherited (never explicitly picked here) is not
       * lost on restart — `preferredEffort` on disk may still be empty.
       */
      effortId?: string | null;
      effort?: number | null;
      effortLabel?: string | null;
    },
  ) => void;
  /** Right-click send → 定时发送. Same payload as onSend, plus fire time. */
  onScheduleSend?: (
    atMs: number,
    text: string,
    droppedPaths?: string[],
    imageAttachments?: import("../lib/imageAttachments").ImageAttachment[],
    opts?: {
      modeId?: string | null;
      modeLabel?: string | null;
      modelId?: string | null;
      modelLabel?: string | null;
      effortId?: string | null;
      effort?: number | null;
      effortLabel?: string | null;
    },
  ) => void;
  /** Notify parent when the active model id changes (for provider balance probes). */
  onActiveModelChange?: (modelId: string | null) => void;
  /** Lazy ACP: warm the agent process without blocking the composer. */
  onWarmAgent?: () => void;
  /** Ensure ACP is ready before set_config_option (returns false if failed). */
  onEnsureAgentReady?: () => Promise<boolean>;
  /**
   * Agent-side housekeeping prompt (Grok `/always-approve`) that Marionette
   * sends on the user's behalf. It still consumes the connection's one turn
   * slot, so it must run through the host pipeline — which marks the session
   * running — instead of a raw `sendAcpPrompt`. Otherwise the replay is
   * invisible and the user's next send is refused as "already in flight".
   */
  onSilentPrompt?: (text: string) => Promise<unknown>;
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
  /** Focus the textarea on mount (after creating a Chat from an empty workspace). */
  autoFocus?: boolean;
  /**
   * This pane is the workspace focus. Unfocused composers keep their draft
   * but skip extra capability fetches.
   */
  isFocused?: boolean;
  /**
   * Empty workspace: interacting with the composer should create a Chat.
   * Parent dedupes in-flight creates.
   */
  onEmptyActivate?: () => void;
  /**
   * Docked strip: height starts content-driven (auto); dragging the top-edge
   * handle pins it (persisted separately from the floating height — key
   * absent = auto), double-click the handle returns to auto.
   */
  docked?: boolean;
};

/**
 * Agents whose credentials live in OpenCode's `auth.json` — the file the provider
 * key dialog writes. Everything else authenticates its own way.
 */
const OPENCODE_AUTH_AGENTS = new Set(["opencode"]);

/**
 * Native model ids the bridge injects into an agent's own catalog —
 * grok/opencode/pi `marionette/<id>`, DSH `marionette::<id>`, zcode
 * `marionette\<id>`. They must exist in the agent's native list (that is how
 * a pinned cross-use preference keeps working after a restart), but the
 * picker never shows them there: the cross-use sections are the single entry
 * point for shared models.
 */

function toDatetimeLocalValue(ms: number): string {
  const d = new Date(ms);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

function formatRecentTime(timestamp: number): string {
  const diff = Date.now() - timestamp;
  if (diff < 60_000) return t("composer.justNow");
  if (diff < 3_600_000) return t("common.minutesAgo", { n: Math.floor(diff / 60_000) });
  if (diff < 86_400_000) return t("common.hoursAgo", { n: Math.floor(diff / 3_600_000) });
  return t("common.daysAgo", { n: Math.floor(diff / 86_400_000) });
}

function effortLabel(value: number): string {
  if (value <= 0.1) return "Low";
  if (value >= 0.9) return "High";
  if (Math.abs(value - 0.5) < 0.1) return "Auto";
  return value < 0.5 ? "Medium" : "High";
}

function shortTriggerLabel(label: string | null, id: string | null): string {
  return prettyModelTrigger(label, id);
}

/** Menu row: the model's name, its parenthetical facts and origin as quiet trailing meta, a check when chosen. */
function ModelMenuName({ label, hint, selected }: { label: string; hint?: string; selected: boolean }) {
  const { name, meta } = splitModelMeta(label);
  // Inside 本机 the host already says it is local.
  const facts = hint ? meta.split(/,\s*/).filter((part) => !/^local$/i.test(part)).join(", ") : meta;
  const tail = [facts, hint].filter(Boolean).join(" · ");
  return (
    <>
      <span className="composer-menu__model-name">{name}</span>
      {tail && <span className="composer-menu__model-meta">{tail}</span>}
      <span className="composer-menu__model-check" aria-hidden>
        {selected ? <Check size={12} strokeWidth={2.5} /> : null}
      </span>
    </>
  );
}

/**
 * Closed model chip as spans (text nodes in a flex chip never ellipsize):
 * the source shrinks first, the model name holds, effort trails quietly.
 */
function ModelChipLabel({ text, effort }: { text: string; effort: string | null }) {
  const cut = text.indexOf(" · ");
  const source = cut > 0 ? text.slice(0, cut) : "";
  const name = cut > 0 ? text.slice(cut + 3) : text;
  return (
    <>
      {source && <span className="composer-select__source">{source}</span>}
      <span className="composer-select__name">{name}</span>
      {effort && <span className="composer-select__effort">{effort}</span>}
    </>
  );
}

function displayModeSafe(caps: CapabilitySnapshot): string | null {
  return caps.currentMode ?? (caps.modes.length > 0 ? caps.modes[0]!.id : null);
}



const COMPOSER_HEIGHT_MIN = 100;
const COMPOSER_HEIGHT_MAX = 480;
const COMPOSER_HEIGHT_DEFAULT = 112;
const COMPOSER_HEIGHT_KEY = "marionette-composer-height";
/**
 * Collapse the chip row into the dock hub once the composer itself (not the
 * window) is too narrow. Enter/exit are split so dragging a split sash across
 * the boundary does not flicker. 580 is about where even truncated chips
 * (mode + permission + always-approve + model + agent + send) collide.
 */
const COMPACT_TOOLBAR_ENTER_PX = 580;
const COMPACT_TOOLBAR_EXIT_PX = 640;
/**
 * Docked strip composer height. Absent key = content-driven auto sizing
 * (default); a number is the height the user pinned by dragging the top-edge
 * handle. Kept separate from the floating-window height so docking never
 * drags the remembered floating size along.
 */
const DOCK_COMPOSER_HEIGHT_KEY = "marionette-dock-composer-height";
const DOCK_COMPOSER_HEIGHT_MIN = 84;
const DOCK_COMPOSER_HEIGHT_MAX = 340;
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

function clampDockComposerHeight(n: number): number {
  // The dock css caps the box at min(52vh, 340px); honor the vh leg too so a
  // drag stops exactly where the stylesheet would clamp it anyway.
  const vhCap = Math.round(window.innerHeight * 0.52);
  const max = Math.min(DOCK_COMPOSER_HEIGHT_MAX, Math.max(DOCK_COMPOSER_HEIGHT_MIN, vhCap));
  return Math.min(max, Math.max(DOCK_COMPOSER_HEIGHT_MIN, Math.round(n)));
}

function readDockComposerHeight(): number | null {
  try {
    const raw = window.localStorage.getItem(DOCK_COMPOSER_HEIGHT_KEY);
    const n = raw ? Number(raw) : NaN;
    if (Number.isFinite(n)) return clampDockComposerHeight(n);
  } catch {
    // ignore
  }
  return null;
}

function applyCapabilities(
  result: CapabilitySnapshot | null | undefined,
  setters: {
    setCaps: (c: CapabilitySnapshot | null) => void;
    setCurrentMode: (m: string | null) => void;
    setCurrentModel: (m: string | null) => void;
    setCurrentEffort: (e: number | null) => void;
    setCurrentEffortId: (e: string | null) => void;
    setCurrentPermission: (p: string | null) => void;
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
    effortRejectModels: result.effortRejectModels ?? [],
    permissionOptions: result.permissionOptions ?? [],
    permissionConfigId: result.permissionConfigId ?? null,
    currentPermission: result.currentPermission ?? null,
    permissionLabel: result.permissionLabel ?? null,
  };
  setters.setCaps(normalized);
  setters.setCurrentMode(normalized.currentMode);
  setters.setCurrentModel(normalized.currentModel);
  setters.setCurrentEffortId(normalized.currentEffortId);
  setters.setCurrentPermission(normalized.currentPermission);
  if (normalized.currentEffort != null) {
    setters.setCurrentEffort(normalized.currentEffort);
  } else if (normalized.thinkingEffort) {
    setters.setCurrentEffort(normalized.thinkingEffort.default);
  } else {
    setters.setCurrentEffort(null);
  }
}

/**
 * The agent already rejected reasoning-effort writes for this model (grok's
 * per-model gate, latched in Rust from the agent's own -32602 answer). While
 * true the Effort control stays dropped: a click or an automatic re-sync would
 * each be another doomed write.
 */
function effortWriteRejectedFor(
  caps: Pick<CapabilitySnapshot, "effortRejectModels"> | null | undefined,
  model: string | null | undefined,
): boolean {
  if (!model) return false;
  return (caps?.effortRejectModels ?? []).includes(model);
}

/** A pick the user just made, held while the agent echoes the old value back. */
type OptionPins = {
  model?: string;
  mode?: string;
  effortId?: string;
  effort?: number;
  permission?: string;
};

/** How long a local pick outranks server echoes (agents settle within ~1–2s). */
const OPTION_PIN_MS = 4000;

/**
 * One cross-use (跨用) group in the model picker — 1:1 with a source entry.
 * `hasPresetModels` false means the source's model list is not enumerable
 * (custom-id-only, e.g. openrouter): the group renders a hint line instead.
 */


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
    setCurrentPermission: (p: string | null) => void;
  },
) {
  if (!pin || Date.now() >= pin.until) return;
  if (pin.pins.model != null) setters.setCurrentModel(pin.pins.model);
  if (pin.pins.mode != null) setters.setCurrentMode(pin.pins.mode);
  if (pin.pins.effortId != null) setters.setCurrentEffortId(pin.pins.effortId);
  if (pin.pins.effort != null) setters.setCurrentEffort(pin.pins.effort);
  if (pin.pins.permission != null) setters.setCurrentPermission(pin.pins.permission);
}

/**
 * Unsent composer text survives dialog switches. Composer remounts on
 * `sessionId:agentId` (hard rule so caps never leak); this cache is keyed by
 * session only so draft follows the dialog, not the agent chip.
 */
const draftBySessionId = new Map<string, string>();

function readDraftCache(sessionId: string): string {
  if (!sessionId) return "";
  return draftBySessionId.get(sessionId) ?? "";
}

function writeDraftCache(sessionId: string, text: string) {
  if (!sessionId) return;
  if (text) draftBySessionId.set(sessionId, text);
  else draftBySessionId.delete(sessionId);
}

/** Carry unsent text from the empty placeholder composer onto a new Chat. */
export function transferEmptyDraft(toSessionId: string): void {
  if (!toSessionId || toSessionId.startsWith("session-empty-")) return;
  let text = "";
  for (const [key, value] of draftBySessionId) {
    if (key.startsWith("session-empty-") && value) text = value;
  }
  for (const key of [...draftBySessionId.keys()]) {
    if (key.startsWith("session-empty-")) draftBySessionId.delete(key);
  }
  if (text) draftBySessionId.set(toSessionId, text);
}

export function Composer({
  agent,
  agents,
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
  onRestartAgentSession,
  onSend,
  onScheduleSend,
  onActiveModelChange,
  onWarmAgent,
  onEnsureAgentReady,
  onSilentPrompt,
  onProviderKeysChanged,
  onAgentBinaryUpdated,
  onAgentsReload,
  availableCommands = null,
  autoFocus = false,
  isFocused = true,
  onEmptyActivate,
  docked = false,
}: ComposerProps) {
  const t = useT();
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
  const { preferences: modelPreferences, sources: modelSources, aliases: modelAliases, accounts: engineAccounts } = useModelLibrary();
  const [caps, setCaps] = useState<CapabilitySnapshot | null>(initialCaps);
  const [currentMode, setCurrentMode] = useState<string | null>(initialCaps?.currentMode ?? null);
  const [currentModel, setCurrentModel] = useState<string | null>(initialCaps?.currentModel ?? null);
  const [currentEffort, setCurrentEffort] = useState<number | null>(initialCaps?.currentEffort ?? null);
  const [currentEffortId, setCurrentEffortId] = useState<string | null>(
    initialCaps?.currentEffortId ?? null,
  );
  /**
   * File permission / sandbox level. Never restored from disk or the offline
   * cache (see `capabilityCache.normalize`): a session always starts on the
   * level the agent itself reports, and the chip follows it from there.
   */
  const [currentPermission, setCurrentPermission] = useState<string | null>(
    initialCaps?.currentPermission ?? null,
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
  const [menu, setMenu] = useState<
    "mode" | "model" | "effort" | "permission" | "agent" | "send" | "hub" | null
  >(null);
  const [scheduleCustom, setScheduleCustom] = useState("");
  const [modelQuery, setModelQuery] = useState("");
  const [draft, setDraft] = useState(() => readDraftCache(sessionId));
  /** Image attachments as Codex-style pills (not raw paths in the textarea). */
  const [imageAttachments, setImageAttachments] = useState<ImageAttachment[]>([]);
  const attachmentsRef = useRef<HTMLDivElement>(null);
  /**
   * Height the attachment pills add inside .composer__field. The field's
   * fixed-height grid otherwise pushes the toolbar below the composer and
   * clips it; growing the composer upward keeps every control where it was
   * (2026-10-24).
   */
  const [attachmentsHeight, setAttachmentsHeight] = useState(0);
  const attachmentsExtra = imageAttachments.length > 0 ? attachmentsHeight + 6 : 0;
  const attachmentsExtraRef = useRef(0);
  attachmentsExtraRef.current = attachmentsExtra;
  const [annotatingId, setAnnotatingId] = useState<string | null>(null);
  /** Draft snapshot right after a file drop — treats path-only draft as "empty" for chips. */
  const [draftAfterDrop, setDraftAfterDrop] = useState<string | null>(null);
  /** Paths from the most recent drop (feeds drop-source chips). */
  const [suggestDroppedPaths, setSuggestDroppedPaths] = useState<string[]>([]);
  /** Round key the user dismissed by typing (show once per event tail). */
  const [dismissedSuggestKey, setDismissedSuggestKey] = useState<string | null>(null);
  /** Mirror of composingRef so chip visibility re-renders on IME start/end. */
  const [isComposing, setIsComposing] = useState(false);
  const [composerHeight, setComposerHeight] = useState(readComposerHeight);
  /** Docked strip: null = content-driven auto; a number = user-pinned height. */
  const [dockComposerHeight, setDockComposerHeight] = useState<number | null>(readDockComposerHeight);
  /** Floating/split: composer narrower than COMPACT_TOOLBAR_ENTER_PX. */
  const [compactByWidth, setCompactByWidth] = useState(false);
  const [resizingComposer, setResizingComposer] = useState(false);
  /** Enter sends immediately instead of Ctrl+Enter. Persisted globally. */
  const [sendOnEnter, setSendOnEnter] = useState(readSendOnEnter);
  const [showProviderDialog, setShowProviderDialog] = useState(false);
  /** Opened from the model menu — display/order settings for every source. */
  const [showModelLibrary, setShowModelLibrary] = useState(false);
  const [showSourcesDialog, setShowSourcesDialog] = useState(false);
  /** A key was added/removed this visit — the agent must restart to see it. */
  const [providerKeysDirty, setProviderKeysDirty] = useState(false);
  /**
   * Cross-use (跨用) model bridge: Marionette-side catalog + the dialog's
   * current binding. Never part of agent-advertised caps — rendered as its own
   * group after the native ones (§4.6/§4.8 of cross-agent-bridge.md).
   */
  const [bridgeCatalog, setBridgeCatalog] = useState<BridgeModelsResult | null>(null);
  const [bridgeSelection, setBridgeSelection] = useState<BridgeModel | null>(null);
  /**
   * Inline custom-model input inside one cross-use group (§ bridge_bind only
   * requires a non-empty model_id — the preset list cannot enumerate every
   * vendor model). Keyed by the group's entryId; one input open at a time.
   */
  const [bridgeCustomEntryId, setBridgeCustomEntryId] = useState<string | null>(null);
  const [bridgeCustomDraft, setBridgeCustomDraft] = useState("");
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
  const [pendingDeleteAgent, setPendingDeleteAgent] = useState<{ id: string; label: string } | null>(null);
  const [showCustomAgentDialog, setShowCustomAgentDialog] = useState(false);
  const [customAgentIds, setCustomAgentIds] = useState<Set<string>>(() => new Set());
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
  const bridgeCustomInputRef = useRef<HTMLInputElement>(null);
  const modelMenuRef = useRef<HTMLDivElement>(null);
  const modelMenuAnchorRef = useRef<HTMLDivElement>(null);
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
  /**
   * Grok's Always-approve replay is a `session/prompt`; a live turn would reject
   * it ("already in flight"), so it waits for the session to go idle instead of
   * flashing a warning the user cannot act on.
   */
  const deferredAlwaysApprove = useRef(false);
  /** Latest status for closures that must not re-run when it changes. */
  const sessionStatusRef = useRef(sessionStatus);
  useEffect(() => {
    sessionStatusRef.current = sessionStatus;
  }, [sessionStatus]);
  /** Exact absolute paths from OS drops, kept verbatim for the grant check. */
  const droppedPathsRef = useRef<Set<string>>(new Set());
  /** After the user picks an option, ignore server echoes still reporting the old one. */
  const pinsRef = useRef<{ pins: OptionPins; until: number } | null>(null);
  const composerHeightRef = useRef(composerHeight);
  composerHeightRef.current = composerHeight;
  const dockComposerHeightRef = useRef(dockComposerHeight);
  dockComposerHeightRef.current = dockComposerHeight;
  const compactByWidthRef = useRef(compactByWidth);
  compactByWidthRef.current = compactByWidth;
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

  const isEmptySession = !sessionId || sessionId.startsWith("session-empty-");
  const activateEmpty = useCallback(() => {
    if (!isEmptySession) return;
    onEmptyActivate?.();
  }, [isEmptySession, onEmptyActivate]);

  useEffect(() => {
    if (!autoFocus) return;
    textareaRef.current?.focus();
  }, [autoFocus]);

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
    setCurrentPermission,
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

  // Measure the attachment strip before paint so adding/removing an image
  // never flashes a clipped toolbar. The composer grows upward by this much.
  useLayoutEffect(() => {
    const el = attachmentsRef.current;
    if (!el) {
      setAttachmentsHeight(0);
      return;
    }
    const apply = () => setAttachmentsHeight(el.offsetHeight);
    apply();
    const ro = new ResizeObserver(apply);
    ro.observe(el);
    return () => ro.disconnect();
  }, [imageAttachments.length]);

  // Collapse chips into the dock hub when *this* composer is too narrow
  // (split pane, torn-off window, or a squeezed floating center). Docked
  // always uses the hub regardless of width.
  useLayoutEffect(() => {
    if (docked) return;
    const el = composerRef.current;
    if (!el) return;
    const apply = (width: number) => {
      const next =
        width <= COMPACT_TOOLBAR_ENTER_PX
          ? true
          : width >= COMPACT_TOOLBAR_EXIT_PX
            ? false
            : compactByWidthRef.current;
      if (next !== compactByWidthRef.current) setCompactByWidth(next);
    };
    apply(el.getBoundingClientRect().width);
    const ro = new ResizeObserver((entries) => {
      const width = entries[0]?.contentRect.width ?? el.getBoundingClientRect().width;
      apply(width);
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, [docked]);

  // Persist composer height.
  useEffect(() => {
    try {
      window.localStorage.setItem(COMPOSER_HEIGHT_KEY, String(composerHeight));
    } catch (error) {
      console.warn("[Composer] failed to persist composer height", error);
    }
  }, [composerHeight]);

  // Persist docked composer height (absent key = auto sizing).
  useEffect(() => {
    try {
      if (dockComposerHeight == null) {
        window.localStorage.removeItem(DOCK_COMPOSER_HEIGHT_KEY);
      } else {
        window.localStorage.setItem(DOCK_COMPOSER_HEIGHT_KEY, String(dockComposerHeight));
      }
    } catch (error) {
      console.warn("[Composer] failed to persist docked composer height", error);
    }
  }, [dockComposerHeight]);

  // Persist send-on-Enter preference.
  useEffect(() => {
    try {
      window.localStorage.setItem(SEND_ON_ENTER_KEY, sendOnEnter ? "1" : "0");
    } catch (error) {
      console.warn("[Composer] failed to persist send-on-enter preference", error);
    }
  }, [sendOnEnter]);

  // Drag-resize composer from the top edge (no React thrash mid-drag). Docked
  // drags pin the strip composer's height (double-click the handle returns to
  // auto); floating drags keep the shared remembered height as before.
  useEffect(() => {
    if (!resizingComposer) return;
    let raf = 0;
    let moved = false;
    const onMove = (event: MouseEvent) => {
      if (raf) cancelAnimationFrame(raf);
      raf = requestAnimationFrame(() => {
        raf = 0;
        const el = composerRef.current;
        if (!el) return;
        moved = true;
        const bottom = el.getBoundingClientRect().bottom;
        const clamp = docked ? clampDockComposerHeight : clampComposerHeight;
        // Persist the base height without the attachment strip; the strip is
        // added on top when pills are present.
        const base = clamp(clamp(bottom - event.clientY) - attachmentsExtraRef.current);
        if (docked) dockComposerHeightRef.current = base;
        else composerHeightRef.current = base;
        el.style.height = `${base + attachmentsExtraRef.current}px`;
      });
    };
    const onUp = () => {
      if (raf) cancelAnimationFrame(raf);
      // A press without movement must not pin an auto-height strip composer.
      if (moved) {
        if (docked) setDockComposerHeight(dockComposerHeightRef.current);
        else setComposerHeight(composerHeightRef.current);
      }
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
  }, [resizingComposer, docked]);

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
    // Cross-use bridge state is dialog-scoped too — the loader below repopulates.
    setBridgeCatalog(null);
    setBridgeSelection(null);
    setBridgeCustomEntryId(null);
    setBridgeCustomDraft("");
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
    setCurrentPermission(null);
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessionId, agent.id]);

  // ── Cross-use (bridge) catalog + binding restore ──────────────────────────
  // Loaded once per dialog; the module cache (lib/bridge.ts) keeps it cheap.
  // Re-fetched on every invalidation (engine state flip, credential change) so
  // an already-open dialog stops showing the stale "模型共享未运行" note and
  // picks up the 共享 sections once the gateway is actually up (2026-10-23
  // user report: the menu only refreshed after switching sessions).
  const bridgeLoadSeq = useRef(0);
  const reloadBridgeCatalog = useCallback(() => {
    if (!isTauriRuntime()) return;
    if (!sessionId || sessionId.startsWith("session-empty-")) return;
    const seq = (bridgeLoadSeq.current += 1);
    void getBridgeModels(agent.id)
      .then((catalog) => {
        if (seq === bridgeLoadSeq.current) setBridgeCatalog(catalog);
      })
      .catch(() => undefined);
  }, [agent.id, sessionId]);

  useEffect(() => subscribeBridgeModels(reloadBridgeCatalog), [reloadBridgeCatalog]);

  useEffect(() => {
    if (!isTauriRuntime()) return;
    if (!sessionId || sessionId.startsWith("session-empty-")) return;
    let cancelled = false;
    const seq = (bridgeLoadSeq.current += 1);
    void (async () => {
      const [catalog, binding] = await Promise.all([
        getBridgeModels(agent.id),
        bridgeGetBinding(sessionId),
      ]);
      if (cancelled) return;
      // A newer reload (engine/key change) already landed — its catalog wins.
      if (seq === bridgeLoadSeq.current) setBridgeCatalog(catalog);
      // Restore the display state for an existing binding (suspend/restart);
      // the backend applies the binding itself at spawn time.
      if (!binding) return;
      const match = catalog.models.find(
        (m) => m.entryId === binding.entryId && m.modelId === binding.modelId,
      );
      if (match) {
        setBridgeSelection(match);
      } else {
        // Custom model ids are never in the preset catalog — keep the binding
        // state and show the raw id as the label instead of dropping it.
        setBridgeSelection({
          entryId: binding.entryId,
          modelId: binding.modelId,
          label: binding.modelId,
          preset: bridgePresetFor(catalog, binding.entryId),
        });
      }
    })();
    return () => {
      cancelled = true;
    };
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
    if (!isFocused) return;
    if (!sessionId || sessionId.startsWith("session-empty-")) {
      return;
    }
    // Do not fetch on "starting": handshake not done yet, null would race-wipe UI.
    // waiting/running: session/new completed and caps are stored server-side.
    if (sessionStatus === "waiting" || sessionStatus === "running") {
      loadCapabilities(sessionId);
    }
  }, [sessionId, sessionStatus, loadCapabilities, isFocused]);

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

  /**
   * The cross-use chip wins over the native model until the agent re-exposes
   * the binding as a native option after a restart — from that point the
   * normal model preference channel takes over. Native id shape per agent:
   * grok/opencode/pi advertise `marionette/<modelId>`, DSH advertises
   * `marionette::<modelId>` (pi-ai `provider::model`, split on first `::`),
   * zcode advertises `marionette\<modelId>` (adapter configOptions compose
   * `provider\model`).
   */
  const nativeBridgeModelId = (modelId: string) => {
    if (agent.id === "deepseek") return `marionette::${modelId}`;
    if (agent.id === "zcode") return `marionette\\${modelId}`;
    return `marionette/${modelId}`;
  };
  const bridgeActive =
    bridgeSelection != null && currentModel !== nativeBridgeModelId(bridgeSelection.modelId);
  /**
   * Truth check for adapters that surface shared models natively (zcode
   * `marionette\…`, opencode/pi `marionette/…`, dsh `marionette::…`): they
   * report the live id as currentModel, so currentModel is ground truth. When
   * it names a different shared model than the pick, the binding never took
   * effect — e.g. a fresh zcode session silently fell back to its
   * modelOrder[0] default. Showing the pick anyway is how the 2026-10-24
   * incident hid pay-per-use DeepSeek behind a chip that said GLM 5.3.
   */
  const reportedSharedModelId =
    currentModel != null && /^(?:marionette\\|marionette::|marionette\/)/.test(currentModel)
      ? currentModel
      : null;
  const bridgeStale =
    bridgeActive &&
    bridgeSelection != null &&
    reportedSharedModelId != null &&
    reportedSharedModelId !== nativeBridgeModelId(bridgeSelection.modelId);

  /**
   * Cross-use pick (§4.8): never sends session/set_model from here — the
   * backend pins the model at spawn time (claude env / grok owned TOML table).
   * grok additionally persists `marionette/<modelId>` as the dialog's normal
   * model preference so the native catalog takes over after restart.
   */
  const pickBridgeModel = useCallback(
    (model: BridgeModel) => {
      setMenu(null);
      // Re-clicking the selected cross item keeps the selection — no repeat bind.
      if (
        bridgeSelection &&
        bridgeSelection.entryId === model.entryId &&
        bridgeSelection.modelId === model.modelId
      ) {
        return;
      }
      if (!sessionId || sessionId.startsWith("session-empty-")) return;
      const prev = bridgeSelection;
      setBridgeSelection(model);
      void bridgeBind(sessionId, { entryId: model.entryId, modelId: model.modelId })
        .then(() => {
          // grok / opencode / dsh / pi / zcode：注入是 spawn 级的（owned 表 /
          // OPENCODE_CONFIG provider / dsh llm-pi-ai 路由 / pi models.json
          // provider / zcode provider_config.json 托管 provider），把原生形态的
          // marionette id 钉成偏好，重启后原生目录接管、跨用 chip 退位。
          if (
            agent.id === "grok-build" ||
            agent.id === "opencode" ||
            agent.id === "deepseek" ||
            agent.id === "pi" ||
            agent.id === "zcode"
          ) {
            persistPrefs({ preferredModel: nativeBridgeModelId(model.modelId) });
          }
          // 注入是 spawn 级的（env / 配置表）：活进程当场换血，历史保留，
          // 下一条消息即以新模型继续 —— 不等「下次打开会话」。
          const status = sessionStatusRef.current;
          if (
            onRestartAgentSession &&
            (status === "running" ||
              status === "waiting" ||
              status === "starting" ||
              status === "error")
          ) {
            onRestartAgentSession(
              sessionId,
              t("composer.bridgeSwitchedBanner", { label: model.label }),
            );
            flash(t("composer.bridgeSwitchedFlash", { label: model.label }));
          }
        })
        .catch((error) => {
          setBridgeSelection(prev);
          const msg =
            error instanceof Error ? error.message : typeof error === "string" ? error : "";
          flash(msg.length > 140 ? `${msg.slice(0, 140)}…` : msg || t("composer.bridgeBindFailed"));
        });
    },
    [agent.id, bridgeSelection, flash, onRestartAgentSession, persistPrefs, sessionId],
  );

  /** Switching back to a native model drops the dialog's bridge binding. */
  const unbindBridge = useCallback(() => {
    if (!bridgeSelection) return;
    const agentId = agent.id;
    setBridgeSelection(null);
    if (sessionId && !sessionId.startsWith("session-empty-")) {
      void bridgeBind(sessionId, null)
        .then(() => {
          // opencode / dsh / pi：跨用 provider 是 spawn 级刷新的常驻目录——
          // 解绑后它仍在菜单里，钉着的 marionette 偏好会把会话又拉回去；清掉
          // 偏好让重启落回原生默认。grok 无此问题（owned 表随解绑移除）。
          if (agentId === "opencode" || agentId === "deepseek" || agentId === "pi") {
            persistPrefs({ preferredModel: null });
          }
          // 活进程的 env 还指着跨用端点 —— 同样当场换血，切回原生立即生效。
          const status = sessionStatusRef.current;
          if (
            onRestartAgentSession &&
            (status === "running" ||
              status === "waiting" ||
              status === "starting" ||
              status === "error")
          ) {
            onRestartAgentSession(
              sessionId,
              t("composer.nativeSwitchedBanner"),
            );
          }
        })
        .catch(() => undefined);
    }
  }, [agent.id, bridgeSelection, onRestartAgentSession, persistPrefs, sessionId]);

  /**
   * Confirm the inline custom-model input: validate + trim the id, then bind
   * exactly like a catalog pick (`bridge_bind` only requires a non-empty
   * model_id — preset lists cannot enumerate every vendor model, e.g.
   * opencode-go's cat-jam-free). Empty input keeps the box open.
   */
  const confirmBridgeCustomModel = useCallback(
    (entryId: string) => {
      const modelId = bridgeCustomDraft.trim();
      if (!modelId) return;
      setBridgeCustomEntryId(null);
      setBridgeCustomDraft("");
      const preset = bridgeCatalog ? bridgePresetFor(bridgeCatalog, entryId) : "";
      pickBridgeModel({ entryId, modelId, label: modelId, preset });
    },
    [bridgeCatalog, bridgeCustomDraft, pickBridgeModel],
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
          // Host pipeline when wired: the replay owns the turn slot, and the
          // session has to say so (see onSilentPrompt). Raw send is the fallback
          // for an isolated render.
          if (onSilentPrompt) await onSilentPrompt(cmd);
          else await sendAcpPrompt(sessionId, cmd);
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
          if (patch.permission != null) {
            throw new Error("This agent exposes no file-permission control");
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
        // Tauri rejects with a plain string for Rust `Err(String)`, and those
        // messages name the real cause ("A turn is already in flight…") — a
        // generic "may not support this option" toast hides exactly that.
        const msg =
          error instanceof Error
            ? error.message
            : typeof error === "string" && error.trim()
              ? error.trim()
              : "Failed to update (agent may not support this option live)";
        if (msg.includes("does not support reasoning effort")) {
          // grok's per-model gate: Rust just latched the model, so reload caps
          // to drop the Effort control now rather than after the next touch.
          flash(t("composer.effortWriteRejectedFlash"));
          loadCapabilities(sessionId);
        } else {
          // Keep toast short but specific (Claude often rejects wrong effort/mode values).
          flash(msg.length > 140 ? `${msg.slice(0, 140)}…` : msg);
        }
      } finally {
        updating.current = false;
      }
    },
    [sessionId, flash, loadCapabilities, agent.id, caps, onEnsureAgentReady, onSilentPrompt, persistPrefs],
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

    const modelOk =
      prefModel &&
      (caps.models.length === 0 || caps.models.some((m) => m.id === prefModel));
    const modeOk =
      prefMode &&
      (caps.modes.length === 0 || caps.modes.some((m) => m.id === prefMode));

    // OpenCode may already sit on the preferred model while omitting Effort
    // from session/new. Re-asserting that model is still work we must do even
    // when disk prefs are otherwise empty.
    const modelAssert = modelAssertForEffortRefresh(
      caps,
      modelOk ? prefModel : caps.currentModel,
    );

    const hasAny = Boolean(
      prefModel ||
        prefMode ||
        prefEffortId ||
        prefEffort != null ||
        (aaSpec && prefAlways != null) ||
        modelAssert,
    );
    if (!hasAny) {
      prefsRestoredKey.current = key;
      // Nothing saved for this dialog: adopt the chips the user is looking at
      // (agent cache / harness defaults) as its prefs. Without this a dialog
      // whose model/effort were inherited never wrote them, so the next launch
      // repainted the harness defaults over the selection ("it changes by
      // itself when I click the dialog").
      const adopted: SessionComposerPrefs = {};
      if (caps.currentModel) adopted.preferredModel = caps.currentModel;
      if (caps.currentMode) {
        adopted.preferredMode = normalizeAgentModeId(agent.id, caps.currentMode);
      }
      if (caps.currentEffortId) adopted.preferredEffortId = caps.currentEffortId;
      else if (caps.currentEffort != null) adopted.preferredEffort = caps.currentEffort;
      else if (caps.thinkingEffort != null) adopted.preferredEffort = caps.thinkingEffort.default;
      if (Object.keys(adopted).length > 0) persistPrefs(adopted);
      return;
    }

    // Claim the key before the async work so handshake re-entries do not fan
    // out parallel restores. Failures still leave disk prefs intact for next warm.
    prefsRestoredKey.current = key;

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
      //    Also re-assert when preferred already matches but Effort was omitted
      //    from session/new (OpenCode / Zed #31961 class of bug).
      if (modelAssert) {
        await commitUpdate({ model: modelAssert.model }, () => {
          if (!cancelled) setCurrentModel(baseline.currentModel);
        });
      }

      // 2) Wait until live caps reflect the preferred model (effort options
      //    are re-advertised with it). getSessionCapabilities is in-memory and
      //    cheap; a few short polls cover OpenCode's post-set_model refresh.
      const needModelWait = Boolean(modelAssert);
      let live = baseline;
      if (needModelWait && modelAssert) {
        for (let i = 0; i < 8; i++) {
          if (cancelled) return;
          await new Promise((r) => window.setTimeout(r, 80 + i * 40));
          const next = await getSessionCapabilities(sessionId).catch(() => null);
          if (!next) continue;
          live = next;
          const onTarget = next.currentModel === modelAssert.model;
          const effortReady =
            Boolean(next.effortConfigId) || (next.effortOptions?.length ?? 0) > 0;
          // switch: land on the model first; republish: wait for Effort echo.
          if (modelAssert.reason === "switch" ? onTarget : effortReady) break;
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
      // just burns a turn. A live turn owns the connection, so the replay waits
      // for it to finish rather than being rejected as "already in flight".
      if (aaSpec && prefAlways === true) {
        if (sessionStatusRef.current === "running") {
          deferredAlwaysApprove.current = true;
        } else {
          await commitUpdate({ alwaysApprove: true }, () => {
            if (!cancelled) setAlwaysApprove(false);
          });
        }
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
      const effortRejectedLive = effortWriteRejectedFor(live, live.currentModel);
      const canTryEffortId =
        !effortRejectedLive &&
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
      } else if (liveNumericOk && !effortRejectedLive && prefEffort != null) {
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

      // Wholly-unsaved dialog (e.g. OpenCode republished a model to surface the
      // Effort menu): remember the settled live chips, or the next launch will
      // have nothing to restore.
      if (!cancelled && !prefModel && !prefMode && !prefEffortId && prefEffort == null) {
        const adopted: SessionComposerPrefs = {};
        if (live.currentModel) adopted.preferredModel = live.currentModel;
        if (live.currentMode) {
          adopted.preferredMode = normalizeAgentModeId(agent.id, live.currentMode);
        }
        if (live.currentEffortId) adopted.preferredEffortId = live.currentEffortId;
        else if (live.currentEffort != null) adopted.preferredEffort = live.currentEffort;
        if (Object.keys(adopted).length > 0) persistPrefs(adopted);
      }
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

  // Replay a deferred Grok Always-approve once the live turn finishes.
  useEffect(() => {
    if (isBusy || !deferredAlwaysApprove.current) return;
    if (!getAcpSupplement(agent.id)?.alwaysApprove) return;
    if (!sessionId || sessionId.startsWith("session-empty-")) return;
    deferredAlwaysApprove.current = false;
    void commitUpdate({ alwaysApprove: true }, () => setAlwaysApprove(false));
  }, [isBusy, agent.id, sessionId, commitUpdate]);

  const clearDropSuggest = useCallback(() => {
    setDraftAfterDrop(null);
    setSuggestDroppedPaths([]);
  }, []);

  const submitText = useCallback(
    (text: string, scheduleAtMs?: number) => {
      // Busy turns barge in via App (cancel the live prompt, then send).
      // Empty draft while busy still uses the Stop button / Esc×2 for interrupt.
      if (!sessionId) return;
      // Allow send with only images / empty text when attachments exist.
      if (!text.trim() && imageAttachments.length === 0) return;
      if (scheduleAtMs != null && !onScheduleSend) return;
      if (scheduleAtMs == null) onWarmAgent?.();
      // Images go as ACP image blocks, not as files the agent must open.
      const dropped = [...droppedPathsRef.current].filter(
        (p, i, arr) => arr.indexOf(p) === i,
      );
      const droppedInText = dropped.filter((p) => text.includes(p));
      const sentWith = currentModel ?? (caps && caps.models.length > 0 ? caps.models[0]!.id : null);
      if (sentWith) recordModelUsage(sentWith);

      // Snapshot chips the user actually sees (optimistic currentMode/currentModel),
      // not parent sessionCapabilities — mode switch delays caps reload on purpose.
      const modeId = currentMode ?? (caps && caps.modes.length > 0 ? caps.modes[0]!.id : null);
      const modeLabel =
        (modeId && caps?.modes.find((m) => m.id === modeId)?.label) || modeId || null;
      // Cross-use pick: snapshot shows the bridged model the chip displays, not
      // the native id the agent still reports underneath — unless the agent
      // reports a *different* shared id (bridgeStale): then the binding never
      // took effect and the snapshot must tell the truth (2026-10-24 incident:
      // chip said GLM 5.3 while the session ran pay-per-use deepseek-v4-pro).
      const modelId =
        bridgeActive && bridgeSelection && !bridgeStale ? bridgeSelection.modelId : sentWith;
      const modelLabel =
        bridgeActive && bridgeSelection && !bridgeStale
          ? bridgeSelection.label
          : (sentWith && caps?.models.find((m) => m.id === sentWith)?.label) || sentWith || null;
      const effortOpts = caps?.effortOptions ?? [];
      const effortLabelVal =
        currentEffortId != null
          ? prettyEffortLabel(
              effortOpts.find((o) => o.id === currentEffortId)?.label,
              currentEffortId,
            )
          : currentEffort != null
            ? effortLabel(currentEffort)
            : null;

      // Keep draft text clean for the You card. Flush the cache now so an
      // in-flight empty→Chat create cannot resurrect the sent text.
      writeDraftCache(sessionId, "");
      const snap = {
        modeId,
        modeLabel,
        modelId,
        modelLabel,
        effortId: currentEffortId,
        effort: currentEffort,
        effortLabel: effortLabelVal,
      };
      const images = imageAttachments.length > 0 ? imageAttachments : undefined;
      if (scheduleAtMs != null && onScheduleSend) {
        onScheduleSend(scheduleAtMs, text, droppedInText, images, snap);
      } else {
        onSend(text, droppedInText, images, snap);
      }
      droppedPathsRef.current.clear();
      setDraft("");
      setImageAttachments([]);
      setAnnotatingId(null);
      clearDropSuggest();
    },
    [
      sessionId,
      onWarmAgent,
      currentModel,
      currentMode,
      currentEffort,
      currentEffortId,
      caps,
      onSend,
      onScheduleSend,
      clearDropSuggest,
      imageAttachments,
      bridgeActive,
      bridgeSelection,
      bridgeStale,
    ],
  );

  const submit = useCallback(() => {
    // Empty draft is OK when parent has quote-pins (App merges on send).
    submitText(draft);
  }, [submitText, draft]);

  const suggestRound = useMemo(
    () => suggestionRoundKey(sessionId, sessionEvents),
    [sessionId, sessionEvents],
  );
  const suggestions: Suggestion[] = useMemo(() => {
    if (!suggestionsEnabledCached()) return [];
    if (isComposing) return [];
    if (dismissedSuggestKey === suggestRound) return [];
    if (draft.trim() && !(draftAfterDrop != null && draft === draftAfterDrop)) return [];
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
        const firstAdded = added[0];
        if (firstAdded) {
          requestAnimationFrame(() => setAnnotatingId(firstAdded.id));
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

  /** Text/link drops (vendor wry patch channel): insert at the caret, no attachments. */
  const insertDroppedText = useCallback((raw: string) => {
    const chunk = raw.trim();
    if (!chunk) return;
    const el = textareaRef.current;
    if (!el) {
      setDraft((current) => (current ? `${current}\n${chunk}` : chunk));
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
    requestAnimationFrame(() => {
      const caret = before.length + inserted.length;
      el.focus();
      el.setSelectionRange(caret, caret);
    });
  }, []);

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

  // Browser mock: HTML5 DnD. Desktop (Tauri) uses onDragDropEvent below for
  // absolute paths — EXCEPT in-page memo drags, which never cross the OS/OLE
  // boundary and only exist as HTML5 events (must match lib/memos.ts).
  const isMemoDrag = (dt: DataTransfer) =>
    Array.from(dt.types).includes("application/x-marionette-memo");

  const onComposerDragEnter = (event: ReactDragEvent) => {
    if (isTauriRuntime() && !isMemoDrag(event.dataTransfer)) return;
    if (
      ![...event.dataTransfer.types].some(
        (t) => t === "Files" || t === "text/uri-list" || t === "text/plain" || t === "application/x-marionette-memo",
      )
    ) {
      return;
    }
    event.preventDefault();
    dropDepth.current += 1;
    setDropActive(true);
  };

  const onComposerDragOver = (event: ReactDragEvent) => {
    if (isTauriRuntime() && !isMemoDrag(event.dataTransfer)) return;
    if (
      ![...event.dataTransfer.types].some(
        (t) => t === "Files" || t === "text/uri-list" || t === "text/plain" || t === "application/x-marionette-memo",
      )
    ) {
      return;
    }
    event.preventDefault();
    event.dataTransfer.dropEffect = "copy";
  };

  const onComposerDragLeave = (event: ReactDragEvent) => {
    if (isTauriRuntime() && !isMemoDrag(event.dataTransfer)) return;
    event.preventDefault();
    dropDepth.current = Math.max(0, dropDepth.current - 1);
    if (dropDepth.current === 0) setDropActive(false);
  };

  const onComposerDrop = (event: ReactDragEvent) => {
    if (isTauriRuntime() && !isMemoDrag(event.dataTransfer)) return;
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
  // Off while docked: App's dock drop router owns native drops for the whole
  // strip (top band → 收纳盒, conversation → here via DOCK_DROP_PATHS_EVENT).
  useEffect(() => {
    if (!isTauriRuntime() || docked) return;
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
  }, [insertDroppedPaths, docked]);

  // Chat-zone drops handed over by the dock router (the effect above is off
  // while docked) — same insert path as a direct field drop.
  useEffect(() => {
    if (!docked) return;
    const onDockDropPaths = (event: Event) => {
      const paths = (event as CustomEvent<string[]>).detail;
      if (Array.isArray(paths) && paths.length > 0) insertDroppedPaths(paths);
    };
    const onDockDropText = (event: Event) => {
      const text = (event as CustomEvent<string>).detail;
      if (typeof text === "string" && text.trim()) insertDroppedText(text);
    };
    window.addEventListener(DOCK_DROP_PATHS_EVENT, onDockDropPaths);
    window.addEventListener(DOCK_DROP_TEXT_EVENT, onDockDropText);
    return () => {
      window.removeEventListener(DOCK_DROP_PATHS_EVENT, onDockDropPaths);
      window.removeEventListener(DOCK_DROP_TEXT_EVENT, onDockDropText);
    };
  }, [docked, insertDroppedPaths, insertDroppedText]);

  // Text/link drags over the field (vendor wry patch channel) while floating.
  // Rect-tested like the file effect above; the memo tab page replaces the
  // composer column while open, so no second consumer competes for the drop.
  useEffect(() => {
    if (!isTauriRuntime() || docked) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    const field = () => textareaRef.current?.closest(".composer__field") as HTMLElement | null;

    void onDragText((event) => {
      if (disposed || isPageDragSource()) return;
      const el = field();
      if (!el) return;
      if (event.kind === "leave") {
        setDropActive(false);
        return;
      }
      const rect = el.getBoundingClientRect();
      // The channel reports physical pixels, same as onDragDropEvent.
      const scale = window.devicePixelRatio || 1;
      const x = event.x / scale;
      const y = event.y / scale;
      const over = x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom;
      if (event.kind === "enter" || event.kind === "over") {
        setDropActive(over);
        return;
      }
      setDropActive(false);
      if (event.kind === "drop" && over && event.text.trim()) insertDroppedText(event.text);
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
  }, [docked, insertDroppedText]);

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
  const hasModels = (caps?.models.length ?? 0) > 0 || (bridgeCatalog?.models.length ?? 0) > 0;
  const effortOptions = caps?.effortOptions ?? [];
  const agentSupplement = getAcpSupplement(agent.id);
  const alwaysApproveSpec = agentSupplement?.alwaysApprove ?? null;
  // Only show effort when the live agent registered the option. Claude omits
  // `effort` entirely for models without supportsEffort (e.g. Haiku after
  // model switch) — inventing it yields:
  //   Unknown config option: effort
  // and a stale Effort menu would crash on thinkingEffort!.default.
  // Exception: Grok carries effort on session/set_model `_meta` (no config id).
  // A model the agent rejected effort writes for (grok ≥1.0.5x per-model gate)
  // loses the knob the same way — advertised tiers or not, no write sticks.
  const effortRejected = effortWriteRejectedFor(
    caps,
    currentModel ?? caps?.currentModel ?? null,
  );
  const hasStringEffort =
    !effortRejected &&
    effortOptions.length > 0 &&
    (Boolean(caps?.effortConfigId) || agentSupplement?.effortViaLegacyModel === true);
  const hasNumericEffort =
    caps != null &&
    !effortRejected &&
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
    currentModel ?? (caps && caps.models.length > 0 ? caps.models[0]!.id : null);
  const displayMode =
    currentMode ?? (caps && caps.modes.length > 0 ? caps.modes[0]!.id : null);

  useEffect(() => {
    onActiveModelChange?.(displayModel);
  }, [displayModel, onActiveModelChange]);
  const displayEffort = hasStringEffort
    ? prettyEffortLabel(
        effortOptions.find((o) => o.id === currentEffortId)?.label,
        currentEffortId,
      ) || null
    : hasEffort && currentEffort != null
      ? effortLabel(currentEffort)
      : null;
  const allLibraryGroups = useMemo(() => libraryGroups(agent.id, caps?.models ?? [], bridgeCatalog, modelSources, modelAliases, engineAccounts), [agent.id, caps?.models, bridgeCatalog, modelSources, modelAliases, engineAccounts]);
  // Settings saved before a source merge still name the old provider/row keys;
  // migrate them against this catalog so hiding and order survive the merge.
  const effectiveModelPreferences = useMemo(() => expandHiddenProviders(modelPreferences, allLibraryGroups), [modelPreferences, allLibraryGroups]);
  const displayLibraryModel = allLibraryGroups.flatMap(group => group.rows.map(row => ({ group, row }))).find(({ row }) =>
    bridgeActive && bridgeSelection && !bridgeStale ? row.bridge?.entryId === bridgeSelection.entryId && row.bridge.modelId === bridgeSelection.modelId
      : row.native?.id === displayModel || (row.bridge != null && displayModel === nativeBridgeModelId(row.bridge.modelId)));
  const displayModelDef =
    displayModel && caps?.models
      ? caps.models.find((m) => m.id === displayModel)
      : undefined;
  const displayModelFull = displayModelDef?.label || displayModel;
  const displayModelLabel = displayLibraryModel ? `${displayLibraryModel.group.label} · ${displayLibraryModel.row.label}` : displayModelDef
    ? prettyModelChip(displayModelDef)
    : displayModelFull
      ? shortTriggerLabel(displayModelFull, displayModel)
      : null;
  const displayModelTitle = displayModelDef
    ? [modelProviderGroup(displayModelDef), displayModelDef.label || displayModelDef.id, displayModelDef.id]
        .filter((value, index, list) => value && list.indexOf(value) === index)
        .join("\n")
    : (displayModelFull ?? "Choose model");
  /** Cross-use chip display — local state, independent of agent advertisement.
   *  Stale bindings (agent reports a different shared id) hide the chip so the
   *  generic model chip shows what is actually running. */
  const bridgeChipLabel =
    bridgeActive && bridgeSelection && !bridgeStale ? (displayLibraryModel ? `${displayLibraryModel.group.label} · ${displayLibraryModel.row.label}` : bridgeSelection.label) : null;
  const bridgeChipTitle =
    bridgeActive && bridgeSelection && !bridgeStale
      ? [t("composer.bridgeChipTitle", { label: bridgeSelection.label }), bridgeSelection.preset, bridgeSelection.modelId].join("\n")
      : null;
  const displayModeLabel =
    (displayMode && caps?.modes.find((m) => m.id === displayMode)?.label) || displayMode;

  // ── File permission / sandbox chip ─────────────────────────────────────────
  // Only agents that advertise a permission knob of their own get this control
  // (DeepSeek Harness `sandbox`). Codex carries permission in its *modes* and Grok
  // in a slash command, so neither grows a second, competing switch here.
  //
  // No fallback to `permissionOptions[0]`: unlike a model, guessing "read-only"
  // or "full access" and painting it as the current state would be a claim about
  // what the agent may touch. Until the agent says, the chip is not rendered.
  const permissionOptions = caps?.permissionOptions ?? [];
  const hasPermission =
    Boolean(caps?.permissionConfigId) && permissionOptions.length > 1 && currentPermission != null;
  const displayPermissionLabel = hasPermission
    ? (permissionOptions.find((o) => o.id === currentPermission)?.label ?? currentPermission)
    : null;
  const permissionTitle = caps?.permissionLabel?.trim() || "File permissions";
  /** Full access / bypass levels read as a warning, the rest stay muted. */
  const permissionIsLoose = (id: string | null | undefined) => {
    const v = (id ?? "").toLowerCase();
    return (
      v.includes("danger") ||
      v.includes("full-access") ||
      v.includes("full_access") ||
      v.includes("bypass") ||
      v.includes("yolo")
    );
  };
  const permissionIsReadOnly = (id: string | null | undefined) => {
    const v = (id ?? "").toLowerCase();
    return v.includes("read-only") || v.includes("read_only") || v === "read";
  };

  const modelGroups = useMemo(() => visibleLibraryGroups(allLibraryGroups, effectiveModelPreferences, modelQuery), [allLibraryGroups, effectiveModelPreferences, modelQuery]);
  const menuGroups = useMemo(() => foldLocalSingleGroups(modelGroups), [modelGroups]);
  const recentModels = useMemo(() => {
    const shown = visibleLibraryGroups(allLibraryGroups, effectiveModelPreferences);
    const ids = new Set(shown.flatMap(group => group.rows.flatMap(row => row.native ? [row.native.id] : [])));
    return getRecentModels(ids);
  }, [allLibraryGroups, effectiveModelPreferences]);
  const crossSections = useMemo(() => ({ customSources: bridgeCatalog ? resolveBridgeSources(bridgeCatalog)
    .filter(source => {
      const key = modelAliases[`entry:${source.entryId}`] ?? `entry:${source.entryId}`;
      return source.entryId !== "engine" && !effectiveModelPreferences.hiddenProviders.includes(key) && !allLibraryGroups.find(group => group.key === key)?.rows.length;
    })
    .map(source => ({ entryId: source.entryId, sampleModelId: "provider-model-id", hasPresetModels: false, label: source.label })) : [] }), [bridgeCatalog, effectiveModelPreferences, modelAliases, allLibraryGroups]);

  // Reopening the menu also picks up sources changed in another window.
  useEffect(() => {
    if (menu !== "model") return;
    let cancelled = false;
    void ensureModelSources().then(() => getBridgeModels(agent.id)).then(catalog => { if (!cancelled) setBridgeCatalog(catalog); }).catch(error => { if (!cancelled) flash(String(error)); });
    return () => { cancelled = true; };
  }, [menu, agent.id, modelSources]);

  /** Backend explanation when cross-use is not available for this agent (e.g. codex M2). */
  const bridgeNote = bridgeCatalog?.note?.trim() || null;

  // Reset + focus search when opening model menu
  useEffect(() => {
    if (menu === "model") {
      setModelQuery("");
      setBridgeCustomEntryId(null);
      setBridgeCustomDraft("");
      requestAnimationFrame(() => modelSearchRef.current?.focus());
    }
  }, [menu]);

  // Focus the inline custom-model input when it opens inside a cross-use group.
  useEffect(() => {
    if (bridgeCustomEntryId) {
      requestAnimationFrame(() => bridgeCustomInputRef.current?.focus());
    }
  }, [bridgeCustomEntryId]);

  // Size the model menu to the space *above* the trigger, inside its real clip
  // box. Sessions now live in `.session-pane__body { overflow: hidden }` (split
  // workspace), so the box is the nearest ancestor that clips — measuring to
  // `.center-workspace` overflowed the pane and cropped off the search box.
  useLayoutEffect(() => {
    if (menu !== "model") return;
    const menuEl = modelMenuRef.current;
    const anchor = modelMenuAnchorRef.current;
    if (!menuEl || !anchor) return;

    const apply = () => {
      let clip: HTMLElement | null = null;
      for (let el = anchor.parentElement; el && !clip; el = el.parentElement) {
        const s = window.getComputedStyle(el);
        if (s.overflowY !== "visible" || s.overflowX !== "visible") clip = el;
      }
      const clipTop = Math.max(clip?.getBoundingClientRect().top ?? 0, 0);
      const available = Math.floor(anchor.getBoundingClientRect().top - clipTop - 8);
      // No floor: a floor above `available` would re-clip the search box in
      // short panes. A tiny menu that still shows its search beats a cropped one.
      menuEl.style.maxHeight = `${Math.max(available, 0)}px`;
    };
    apply();
    const ro = new ResizeObserver(apply);
    ro.observe(anchor);
    if (composerRef.current) ro.observe(composerRef.current);
    let clipEl: HTMLElement | null = null;
    for (let el = anchor.parentElement; el && !clipEl; el = el.parentElement) {
      const s = window.getComputedStyle(el);
      if (s.overflowY !== "visible" || s.overflowX !== "visible") clipEl = el;
    }
    if (clipEl) ro.observe(clipEl);
    window.addEventListener("resize", apply);
    return () => {
      ro.disconnect();
      window.removeEventListener("resize", apply);
    };
  }, [menu]);

  /**
   * Keep every composer popover inside the window.
   *
   * Chips flow left to right, but the model / agent / send menus are anchored
   * `right: 0` on their own chip. In the 400px docked strip the model chip sits
   * near x=8, so a 328px menu opened further left and WebView clipped almost
   * all of it (the list "disappeared outside the sidebar"). Measure after the
   * menu paints and pin its left edge inside an 8px viewport gutter; the same
   * pass also rescues menus that would spill over the right edge.
   */
  useLayoutEffect(() => {
    if (!menu) return;
    const root = composerRef.current;
    if (!root) return;
    const clamp = () => {
      const vw = window.innerWidth;
      const margin = 8;
      for (const el of root.querySelectorAll<HTMLElement>(".composer-menu")) {
        el.style.left = "";
        el.style.right = "";
        const rect = el.getBoundingClientRect();
        let dx = 0;
        if (rect.left < margin) dx = margin - rect.left;
        else if (rect.right > vw - margin) dx = vw - margin - rect.right;
        if (dx === 0) continue;
        const parent = el.offsetParent as HTMLElement | null;
        const base = parent?.getBoundingClientRect().left ?? 0;
        el.style.left = `${rect.left + dx - base}px`;
        el.style.right = "auto";
      }
    };
    clamp();
    const frames = [
      window.requestAnimationFrame(clamp),
      window.requestAnimationFrame(() => window.requestAnimationFrame(clamp)),
    ];
    const ro = new ResizeObserver(clamp);
    ro.observe(root);
    for (const el of root.querySelectorAll<HTMLElement>(".composer-menu")) ro.observe(el);
    window.addEventListener("resize", clamp);
    return () => {
      for (const frame of frames) window.cancelAnimationFrame(frame);
      ro.disconnect();
      window.removeEventListener("resize", clamp);
      for (const el of root.querySelectorAll<HTMLElement>(".composer-menu")) {
        el.style.left = "";
        el.style.right = "";
      }
    };
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

  const refreshCustomIds = useCallback(async () => {
    const list = await listCustomAgents();
    setCustomAgentIds(new Set(list.map((d) => d.id)));
  }, []);

  useEffect(() => {
    if (menu !== "agent") return;
    void refreshCustomIds();
  }, [menu, refreshCustomIds]);

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

  /** Shared by the floating chip and the dock hub row: cold dialogs only
   *  persist the preference — spinning an agent process up just to sync one
   *  switch is waste; the next wake's pref restore replays the slash. */
  const toggleAlwaysApprove = () => {
    const next = !alwaysApprove;
    const prev = alwaysApprove;
    const status = sessionStatusRef.current;
    if (status === "exited" || status === "error") {
      setAlwaysApprove(next);
      persistPrefs({ preferredAlwaysApprove: next });
      recordLastUsedDefaults(agent.id, { alwaysApprove: next });
      return;
    }
    setAlwaysApprove(next);
    void commitUpdate({ alwaysApprove: next }, () => {
      setAlwaysApprove(prev);
    });
  };

  const dockFixedHeight = docked && dockComposerHeight != null ? dockComposerHeight : null;
  /** Same hub as the docked strip: chips collapse, ＋ and send stay out. */
  const compactToolbar = docked || compactByWidth;

  useEffect(() => {
    if (!compactToolbar && menu === "hub") setMenu(null);
  }, [compactToolbar, menu]);

  return (
    <footer
      ref={composerRef}
      className={[
        "composer",
        isBusy ? "is-busy" : "",
        isWarming ? "is-warming" : "",
        resizingComposer ? "is-resizing" : "",
        hasModes ? "has-mode" : "",
        docked ? "is-docked" : "",
        compactToolbar ? "is-compact" : "",
        dockFixedHeight != null ? "is-dock-fixed" : "",
      ]
        .filter(Boolean)
        .join(" ")}
      data-mode-tone={tone}
      style={
        docked
          ? dockFixedHeight != null
            ? { height: dockFixedHeight + attachmentsExtra }
            : undefined
          : { height: composerHeight + attachmentsExtra }
      }
      aria-label="Composer"
      onPointerDown={activateEmpty}
    >
      {/* Drag handle: top edge — free height between MIN/MAX, remembered.
          Docked: double-click returns to content-driven auto height. */}
      <button
        type="button"
        className="composer__resize"
        aria-label="Resize composer height"
        title={docked ? "Drag to resize · double-click to reset" : "Drag to resize"}
        onMouseDown={(event) => {
          event.preventDefault();
          setResizingComposer(true);
        }}
        onDoubleClick={() => {
          if (docked) setDockComposerHeight(null);
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
          <div ref={attachmentsRef} className="composer__attachments" aria-label="Image attachments">
            {imageAttachments.map((att) => (
              <span key={att.id} className="composer__attachment-pill">
                <ComposerAttachmentThumb path={att.path} name={att.name} />
                <button
                  type="button"
                  className="composer__attachment-main"
                  title={`${att.path}${att.marks.length ? ` · ${t("annotator.marksCount", { count: att.marks.length })}` : ` · ${t("annotator.clickToMark")}`}`}
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
                  title={t("common.remove")}
                  aria-label={t("annotator.removeAttachment", { name: att.name })}
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
            <div className="composer-slash__hint">{t("composer.delegateHint")}</div>
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
                  {c.recent ? ` · ${t("composer.usedRecently")}` : ""}
                  {c.installed ? ` · ${t("composer.installedMark")}` : ` · ${t("composer.notInstalledMark")}`}
                </span>
              </button>
            ))}
          </div>
        )}
        <textarea
          ref={textareaRef}
          aria-label="Prompt composer"
          autoFocus={autoFocus}
          placeholder={
            isWarming && !composingRef.current
              ? "Agent connecting in background… keep typing"
              : sendOnEnter
                ? "Message the Agent (Enter to send · Shift+Enter newline) · / commands · Tab cycles mode"
                : "Message the Agent (Ctrl+Enter to send) · / commands · Tab cycles mode"
          }
          rows={docked ? 2 : isTall ? 10 : 2}
          value={draft}
          onFocus={() => {
            activateEmpty();
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
            {/* Compact toolbar hub. Always in the tree so a container query
                can reveal it on the first paint of a narrow pane (JS hysteresis
                lags one frame). Hidden by CSS when the composer is wide. */}
            <div className="composer-menu-anchor composer-menu-anchor--hub">
                <button
                  className="composer-dock-hub"
                  type="button"
                  title={t("composer.hubTitle")}
                  aria-expanded={menu === "hub"}
                  onClick={() => setMenu(menu === "hub" ? null : "hub")}
                >
                  <SlidersHorizontal size={13} />
                  <span className="composer-dock-hub__label">
                    {bridgeChipLabel ?? displayModelLabel ?? t("composer.modeAndModel")}
                  </span>
                  <ChevronRight size={12} aria-hidden />
                </button>
                {menu === "hub" && (
                  <div className="composer-menu composer-menu--hub" role="menu" aria-label={t("composer.modeAndModel")}>
                    {hasModes && displayMode && (
                      <button
                        type="button"
                        role="menuitem"
                        className="composer-menu__hub-row"
                        onClick={() => setMenu("mode")}
                      >
                        <span className="composer-menu__hub-name">{t("composer.mode")}</span>
                        <span className="composer-menu__hub-value">{displayModeLabel}</span>
                        <ChevronRight size={13} aria-hidden />
                      </button>
                    )}
                    {hasPermission && (
                      <button
                        type="button"
                        role="menuitem"
                        className="composer-menu__hub-row"
                        onClick={() => setMenu("permission")}
                      >
                        <span className="composer-menu__hub-name">{permissionTitle}</span>
                        <span className="composer-menu__hub-value">{displayPermissionLabel}</span>
                        <ChevronRight size={13} aria-hidden />
                      </button>
                    )}
                    {alwaysApproveSpec && (
                      <button
                        type="button"
                        role="menuitem"
                        className="composer-menu__hub-row"
                        aria-pressed={alwaysApprove}
                        onClick={toggleAlwaysApprove}
                      >
                        <span className="composer-menu__hub-name">
                          {alwaysApproveSpec.onLabel ?? "Always approve"}
                        </span>
                        <span className="composer-menu__hub-value">{alwaysApprove ? t("common.on") : t("common.off")}</span>
                        {alwaysApprove ? <Check size={13} aria-hidden /> : <ChevronRight size={13} aria-hidden />}
                      </button>
                    )}
                    {(hasModels || hasEffort || showKeyPrompt) && (
                      <button
                        type="button"
                        role="menuitem"
                        className="composer-menu__hub-row"
                        onClick={() => setMenu("model")}
                      >
                        <span className="composer-menu__hub-name">{t("composer.model")}</span>
                        <span className="composer-menu__hub-value">
                          {bridgeChipLabel ?? displayModelLabel ?? t("composer.chooseModel")}
                        </span>
                        <ChevronRight size={13} aria-hidden />
                      </button>
                    )}
                    <button
                      type="button"
                      role="menuitem"
                      className="composer-menu__hub-row"
                      onClick={() => setMenu("agent")}
                    >
                      <span className="composer-menu__hub-name">Agent</span>
                      <span className="composer-menu__hub-value">{agent.label}</span>
                      <ChevronRight size={13} aria-hidden />
                    </button>
                  </div>
                )}
            </div>
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
            {/* File permission / sandbox (DeepSeek Harness `sandbox`). Its own
                knob beside the mode chip: 计划 mode still writes files if the
                level allows it, so the two must not be merged. */}
            {hasPermission && (
              <div className="composer-menu-anchor composer-menu-anchor--permission">
                <button
                  className={
                    permissionIsLoose(currentPermission)
                      ? "composer-mode-chip composer-mode-chip--perm composer-mode-chip--perm-loose"
                      : permissionIsReadOnly(currentPermission)
                        ? "composer-mode-chip composer-mode-chip--perm composer-mode-chip--perm-readonly"
                        : "composer-mode-chip composer-mode-chip--perm"
                  }
                  type="button"
                  title={`${permissionTitle}: ${displayPermissionLabel}`}
                  aria-expanded={menu === "permission"}
                  onClick={() => setMenu(menu === "permission" ? null : "permission")}
                >
                  <span className="composer-mode-chip__label">{displayPermissionLabel}</span>
                </button>
                {menu === "permission" && (
                  <div
                    className="composer-menu composer-menu--permission"
                    role="menu"
                    aria-label={permissionTitle}
                  >
                    {permissionOptions.map((option) => (
                      <button
                        key={option.id}
                        className={currentPermission === option.id ? "is-selected" : ""}
                        type="button"
                        role="menuitem"
                        data-mode-tone={
                          permissionIsLoose(option.id)
                            ? "debug"
                            : permissionIsReadOnly(option.id)
                              ? "default"
                              : "build"
                        }
                        title={option.description ?? undefined}
                        onClick={() => {
                          const prev = currentPermission;
                          pinOptions({ permission: option.id });
                          setCurrentPermission(option.id);
                          setMenu(null);
                          void commitUpdate({ permission: option.id }, () => {
                            unpinOption("permission");
                            setCurrentPermission(prev);
                          });
                        }}
                      >
                        <span className="composer-menu__mode-dot" aria-hidden />
                        {option.label}
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
                onClick={toggleAlwaysApprove}
              >
                <span className="composer-mode-chip__label">
                  {alwaysApprove
                    ? (alwaysApproveSpec.onLabel ?? "Always approve")
                    : (alwaysApproveSpec.offLabel ?? "Ask permissions")}
                </span>
              </button>
            )}
          </div>
          <div className="composer__actions">
            {/* ── Model + Effort selector ─────────────────────────────── */}
            {/* Always show when caps exist (even empty for add-key prompt), or has effort */}
            {(hasModels || hasEffort || showKeyPrompt) && (
              <div className="composer-menu-anchor" ref={modelMenuAnchorRef}>
                <button
                  className="composer-select composer-select--model"
                  type="button"
                  title={bridgeChipTitle ?? displayModelTitle}
                  aria-expanded={menu === "model" || menu === "effort"}
                  onClick={() => setMenu(menu === "model" ? null : "model")}
                >
                    <ModelChipLabel
                      text={bridgeChipLabel ?? displayModelLabel ?? "Choose model"}
                      effort={displayEffort != null && hasEffort ? String(displayEffort) : null}
                    />
                </button>

                {menu === "model" && (
                  <div
                    ref={modelMenuRef}
                    className="composer-menu composer-menu--models"
                    role="listbox"
                    aria-label={`${agent.label} models`}
                  >
                    {/* The key prompt renders *above* the rest instead of replacing
                        it. As a ternary it also swallowed the effort block below,
                        and since nothing ever sets `menu = "effort"`, an agent with
                        effort options but no model list lost every way to change
                        effort. */}
                    {!hasModels && showKeyPrompt && (
                      <div className="composer-menu__empty-state">
                        <div className="composer-menu__empty-icon">🔑</div>
                        <div className="composer-menu__empty-title">{t("composer.noApiKeyTitle")}</div>
                        <div className="composer-menu__empty-desc">
                          {t("composer.noApiKeyDesc")}
                        </div>
                        <button
                          className="composer-menu__add-key-btn"
                          type="button"
                          onClick={() => { setMenu(null); setShowProviderDialog(true); }}
                        >
                          {t("composer.addApiKey")}
                        </button>
                      </div>
                    )}
                    {!hasModels && !showKeyPrompt && !hasEffort && (
                      <div className="composer-menu__empty">{t("composer.noSwitchableModels")}</div>
                    )}
                        {hasModels && (
                          <>
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
                              {!modelQuery.trim() && recentModels.length > 0 && (
                                <div className="composer-menu__recent">
                                  <span className="composer-menu__label">{t("composer.recentlyUsed")}</span>
                                  {recentModels.map((entry) => {
                                    const model = caps?.models.find((m) => m.id === entry.modelId);
                                    if (!model) return null;
                                    const provider = modelProviderGroup(model);
                                    const selected =
                                      !bridgeActive && displayModel === entry.modelId;
                                    return (
                                      <button
                                        key={entry.modelId}
                                        className={selected ? "is-selected" : ""}
                                        type="button"
                                        role="option"
                                        aria-selected={selected}
                                        title={modelTooltip(model)}
                                        onClick={() => {
                                          const prev = displayModel;
                                          // Picking a native model drops the cross-use binding.
                                          unbindBridge();
                                          pinOptions({ model: entry.modelId });
                                          setCurrentModel(entry.modelId);
                                          setMenu(null);
                                          void commitUpdate({ model: entry.modelId }, () => {
                                            unpinOption("model");
                                            setCurrentModel(prev);
                                          });
                                        }}
                                      >
                                        <span className="composer-menu__model-name">{modelRowLabel(model)}</span>
                                        <span className="composer-menu__model-recent-hint">
                                          <span className="composer-menu__model-source">{provider}</span>
                                          <span className="composer-menu__model-recent-time">{formatRecentTime(entry.lastUsedAt)}</span>
                                        </span>
                                      </button>
                                    );
                                  })}
                                </div>
                              )}
                              {!modelQuery.trim() && recentModels.length > 0 && (
                                <span className="composer-menu__divider" />
                              )}
                              {modelGroups.length === 0 && crossSections.customSources.length === 0 && (
                                <div className="composer-menu__empty">{modelQuery ? t("composer.noModelMatches", { query: modelQuery }) : t("composer.noVisibleModels")}</div>
                              )}
                              {menuGroups.map(group => (
                                <div className="composer-menu__group" key={group.key}>
                                  <span className="composer-menu__label">{group.label}</span>
                                  {group.rows.map(row => {
                                    const m = row.native;
                                    const shared = row.bridge;
                                    const selected = m ? !bridgeActive && displayModel === m.id : shared != null && ((bridgeSelection?.entryId === shared.entryId && bridgeSelection.modelId === shared.modelId) || currentModel === nativeBridgeModelId(shared.modelId));
                                    return <button key={row.key} className={selected ? "is-selected" : ""} type="button" role="option" aria-selected={selected}
                                      title={[row.label, row.modelId, row.origin].filter(Boolean).join("\n")}
                                      onClick={() => {
                                        if (m) {
                                          const prev = displayModel;
                                          unbindBridge();
                                          pinOptions({ model: m.id });
                                          setCurrentModel(m.id);
                                          setMenu(null);
                                          void commitUpdate({ model: m.id }, () => { unpinOption("model"); setCurrentModel(prev); });
                                        } else if (shared) { void pickBridgeModel(shared); }
                                      }}>
                                      <ModelMenuName label={m ? modelRowLabel(m) : row.label} hint={row.hint} selected={selected} />
                                    </button>;
                                  })}
                                </div>
                              ))}
                              {/* Per-source custom-id rows (binding targets
                                  an entry) — a compact footer, not headers. */}
                              {crossSections.customSources.map((source) => (
                                <div key={source.entryId}>
                                  {/* Source whose model list is not enumerable
                                      (e.g. openrouter) — custom ids only. */}
                                  {!source.hasPresetModels && (
                                    <div className="composer-menu__bridge-empty">
                                      {t("composer.sourceNoPresetList", { label: source.label })}
                                    </div>
                                  )}
                                  {bridgeCustomEntryId === source.entryId ? (
                                    <div className="composer-menu__bridge-custom">
                                      <input
                                        ref={bridgeCustomInputRef}
                                        type="text"
                                        value={bridgeCustomDraft}
                                        placeholder={t("composer.customModelPlaceholder", { sample: source.sampleModelId })}
                                        aria-label={t("composer.customModelAria")}
                                        onChange={(e) => setBridgeCustomDraft(e.target.value)}
                                        onKeyDown={(e) => {
                                          if (e.key === "Enter" && !e.nativeEvent.isComposing) {
                                            e.preventDefault();
                                            e.stopPropagation();
                                            confirmBridgeCustomModel(source.entryId);
                                          } else if (e.key === "Escape") {
                                            // Cancel the input, keep the menu open.
                                            e.stopPropagation();
                                            setBridgeCustomEntryId(null);
                                          }
                                        }}
                                      />
                                    </div>
                                  ) : (
                                    <button
                                      className="composer-menu__bridge-custom-trigger"
                                      type="button"
                                      title={t("composer.customModelTitle")}
                                      onClick={() => {
                                        setBridgeCustomEntryId(source.entryId);
                                        setBridgeCustomDraft("");
                                      }}
                                    >
                                      {t("composer.addCustomModel")}
                                      {crossSections.customSources.length > 1
                                        ? ` · ${source.label}`
                                        : ""}
                                      …
                                    </button>
                                  )}
                                </div>
                              ))}
                              {bridgeNote && (
                                <div className="composer-menu__bridge-note">{bridgeNote}</div>
                              )}
                            </div>

                            {/* ── Configure sources / display at the bottom ── */}
                            <span className="composer-menu__divider" />
                            <button
                              className="composer-menu__add-key-item"
                              type="button"
                              title={t("composer.configureModelsTitle")}
                              onClick={() => { setMenu(null); setShowModelLibrary(true); }}
                            >
                              <Settings2 size={13} aria-hidden /> {t("composer.configureModels")}
                            </button>
                            {usesOpencodeAuth && (
                              <button
                                className="composer-menu__add-key-item"
                                type="button"
                                title={t("composer.addApiKeyTitle")}
                                onClick={() => { setMenu(null); setShowProviderDialog(true); }}
                              >
                                {t("composer.addApiKeyMenu")}
                              </button>
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
                                    {prettyEffortLabel(opt.label, opt.id)}
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
                        {effortRejected && hasModels && (
                          <div className="composer-menu__empty">
                            {t("composer.effortNotAdjustable")}
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
                            {prettyEffortLabel(opt.label, opt.id)}
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
                      // ZCode without a local runtime (desktop install /
                      // PATH zcode / ZCODE_BIN): installing the npm adapter is
                      // a dead end — the engine cannot come from npm.
                      const runtimeGate = status?.runtimeGate ?? null;
                      const canInstall =
                        !ready && (status?.installable ?? false) && !busyInstall;
                      // Grok / Hermes / Cursor: no in-app installer — show setup help instead of a blank gap.
                      const needsManualSetup =
                        !ready &&
                        !canInstall &&
                        !busyInstall &&
                        (candidate.install.manager === "manual" || Boolean(runtimeGate));
                      const manualHint =
                        runtimeGate?.trim() ||
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
                                  : needsManualSetup
                                    ? t("composer.needsLocalInstall")
                                    : status?.status === "incomplete"
                                      ? "needs CLI"
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
                                  `${t("composer.manualInstallHead", { label: candidate.label })}\n\n${manualHint}${
                                    pfLines ? `\n\n${t("composer.envCheckTitle")}\n${pfLines}` : ""
                                  }\n\n${t("composer.manualInstallTail")}`,
                                );
                                void navigator.clipboard?.writeText(manualHint).catch(() => undefined);
                              }}
                            >
                              {t("composer.howToInstall")}
                            </button>
                          )}
                          {!ready && !needsManualSetup && preflightById[candidate.id] && !preflightById[candidate.id]!.passed && (
                            <button
                              className="composer-agent-row__install composer-agent-row__install--manual"
                              type="button"
                              title={t("composer.preflightFailedTitle")}
                              onClick={(event) => {
                                event.stopPropagation();
                                const pf = preflightById[candidate.id];
                                if (!pf) return;
                                setInstallNote(
                                  `${t("composer.envCheckFor", { name: pf.agentName })}\n` +
                                    pf.checks
                                      .map(
                                        (c) =>
                                          `${c.status === "pass" ? "✓" : c.status === "warn" ? "!" : "✗"} ${c.label}: ${c.message}`,
                                      )
                                      .join("\n"),
                                );
                              }}
                            >
                              {t("composer.check")}
                            </button>
                          )}
                          {customAgentIds.has(candidate.id) || candidate.id.startsWith("custom-") || candidate.install.note?.includes("Custom agent") ? (
                            <button
                              className="composer-agent-row__install composer-agent-row__install--manual"
                              type="button"
                              title={t("composer.deleteCustomAgent")}
                              onClick={(event) => {
                                event.stopPropagation();
                                setPendingDeleteAgent({ id: candidate.id, label: candidate.label });
                              }}
                            >
                              {t("common.delete")}
                            </button>
                          ) : null}
                        </div>
                      );
                    })}
                  <button
                    type="button"
                    className="composer-menu__add-custom"
                    onClick={() => setShowCustomAgentDialog(true)}
                  >
                    {t("composer.addCustomAgent")}
                  </button>
                  {installNote && <div className="composer-menu__note">{installNote}</div>}
                </div>
              )}
            </div>

            {/* ── Send / Interrupt (Esc×2 also interrupts) ──
                While the agent is running: non-empty draft → interrupt + send;
                empty draft → interrupt. Right-click toggles Enter shortcut. */}
            <div className="composer-menu-anchor composer-menu-anchor--send">
              <button
                className={
                  isBusy && canCancel && !draft.trim() && imageAttachments.length === 0
                    ? "send-button is-interrupting"
                    : draft.trim() || imageAttachments.length > 0
                      ? "send-button is-ready"
                      : "send-button"
                }
                type="button"
                title={
                  isBusy && (draft.trim() || imageAttachments.length > 0)
                    ? t("composer.interruptAndSendTitle")
                    : isBusy && canCancel
                      ? "Interrupt (or double-press Esc)"
                      : isBusy && !canCancel
                        ? "Agent is busy"
                        : t("composer.sendHint")
                }
                aria-label={
                  isBusy && (draft.trim() || imageAttachments.length > 0)
                    ? "Interrupt turn and send"
                    : isBusy && canCancel
                      ? "Interrupt conversation"
                      : isBusy && !canCancel
                        ? "Agent is busy"
                        : "Send"
                }
                aria-haspopup="menu"
                aria-expanded={menu === "send"}
                disabled={
                  isBusy &&
                  !canCancel &&
                  !draft.trim() &&
                  imageAttachments.length === 0
                }
                onClick={() => {
                  setMenu(null);
                  if (draft.trim() || imageAttachments.length > 0) {
                    submit();
                    return;
                  }
                  if (isBusy && canCancel) onInterrupt();
                }}
                onContextMenu={(event) => {
                  event.preventDefault();
                  const opening = menu !== "send";
                  if (opening) {
                    setScheduleCustom(toDatetimeLocalValue(Date.now() + 30 * 60_000));
                  }
                  setMenu(opening ? "send" : null);
                }}
              >
                {isBusy &&
                canCancel &&
                !draft.trim() &&
                imageAttachments.length === 0 ? (
                  <Square size={12} fill="currentColor" />
                ) : (
                  <SendHorizontal size={15} />
                )}
              </button>
              {menu === "send" && (
                <div
                  className="composer-menu composer-menu--send"
                  role="menu"
                  aria-label={t("composer.sendOptions")}
                >
                  <div className="composer-menu__label">{t("composer.sendShortcut")}</div>
                  <button
                    type="button"
                    role="menuitem"
                    className={!sendOnEnter ? "is-selected" : ""}
                    onClick={() => {
                      setSendOnEnter(false);
                      setMenu(null);
                    }}
                  >
                    {t("composer.ctrlEnterSend")}
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
                    {t("composer.enterSend")}
                  </button>
                  {onScheduleSend && (
                    <>
                      <div className="composer-menu__divider" />
                      <div className="composer-menu__label">{t("composer.scheduleSend")}</div>
                      {(
                        [
                          [5, t("composer.inMinutes", { count: 5 })],
                          [15, t("composer.inMinutes", { count: 15 })],
                          [30, t("composer.inMinutes", { count: 30 })],
                          [60, t("composer.inHours")],
                        ] as const
                      ).map(([minutes, label]) => (
                        <button
                          key={minutes}
                          type="button"
                          role="menuitem"
                          disabled={!draft.trim() && imageAttachments.length === 0}
                          onClick={() => {
                            submitText(draft, Date.now() + minutes * 60_000);
                            setMenu(null);
                          }}
                        >
                          {label}
                        </button>
                      ))}
                      <div className="composer-menu__schedule">
                        <input
                          type="datetime-local"
                          aria-label={t("composer.customSendTime")}
                          min={toDatetimeLocalValue(Date.now() + 60_000)}
                          value={scheduleCustom}
                          onChange={(event) => setScheduleCustom(event.target.value)}
                          onClick={(event) => event.stopPropagation()}
                        />
                        <button
                          type="button"
                          role="menuitem"
                          disabled={
                            (!draft.trim() && imageAttachments.length === 0) ||
                            !scheduleCustom ||
                            Date.parse(scheduleCustom) <= Date.now()
                          }
                          onClick={() => {
                            const at = Date.parse(scheduleCustom);
                            if (!Number.isFinite(at) || at <= Date.now()) return;
                            submitText(draft, at);
                            setMenu(null);
                          }}
                        >
                          {t("composer.sendAtSpecifiedTime")}
                        </button>
                      </div>
                    </>
                  )}
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
      {showCustomAgentDialog && (
        <CustomAgentDialog
          agents={agents}
          customIds={customAgentIds}
          onClose={() => setShowCustomAgentDialog(false)}
          onAdded={(note) => {
            setInstallNote(note);
            setShowCustomAgentDialog(false);
            void refreshCustomIds();
            void onAgentsReload?.();
            void refreshAgentStatuses();
          }}
        />
      )}
      {pendingDeleteAgent && (
        <ConfirmDialog
          title={t("composer.deleteCustomAgent")}
          itemName={pendingDeleteAgent.label}
          description={t("composer.deleteCustomAgentDesc")}
          onCancel={() => setPendingDeleteAgent(null)}
          onConfirm={() => {
            const target = pendingDeleteAgent;
            setPendingDeleteAgent(null);
            void removeCustomAgent(target.id)
              .then(async () => {
                setInstallNote(t("composer.agentDeletedNote", { label: target.label }));
                await onAgentsReload?.();
                void refreshAgentStatuses();
              })
              .catch((e) =>
                setInstallNote(e instanceof Error ? e.message : String(e)),
              );
          }}
        />
      )}
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
      {showModelLibrary && (
        <ModelLibraryDialog
          agents={agents}
          onClose={() => setShowModelLibrary(false)}
          onManageSources={() => { setShowModelLibrary(false); setShowSourcesDialog(true); }}
        />
      )}
      {showSourcesDialog && (
        <CredentialsDialog
          agents={agents}
          onClose={() => {
            setShowSourcesDialog(false);
            // Keys and sources feed the picker; re-read them once the dialog
            // closes so the model menu reflects an added or removed credential.
            void refreshModelSources().catch(() => undefined);
            invalidateBridgeModels();
          }}
        />
      )}
    </footer>
  );
}

function ComposerAttachmentThumb({ path, name }: { path: string; name: string }) {
  const [dataUrl, setDataUrl] = useState<string | null>(null);
  useEffect(() => {
    let cancelled = false;
    if (!isTauriRuntime()) return;
    void readImageDataUrl(path)
      .then((res) => {
        if (!cancelled) setDataUrl(res.dataUrl);
      })
      .catch(() => {
        if (!cancelled) setDataUrl(null);
      });
    return () => {
      cancelled = true;
    };
  }, [path]);
  if (!dataUrl) {
    const initial = (name.replace(/\.[^.]+$/, "").trim()[0] || "?").toUpperCase();
    return (
      <span className="composer__attachment-thumb-fallback" aria-hidden>
        {initial}
      </span>
    );
  }
  return <img className="composer__attachment-thumb" src={dataUrl} alt="" />;
}
