import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { Expand, Plus, Search, SendHorizontal, Shrink, Square } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState, type DragEvent as ReactDragEvent } from "react";
import { expandAcpConfigAttempts, getAcpSupplement, mergeAcpCapabilities } from "../lib/acpSupplements";
import {
  agentVersions,
  getSessionCapabilities,
  installAgent,
  isTauriRuntime,
  listAgentCommands,
  sendAcpPrompt,
  updateAcpSession,
} from "../lib/api";
import { cacheAgentCapabilities, cachedCapabilitiesFor } from "../lib/capabilityCache";
import { ptyCommandsForPatch, ptyProfileToCapabilities } from "../lib/ptyProfiles";
import { ProviderConfigDialog } from "./ProviderConfigDialog";
import { recordModelUsage, getRecentModels } from "../lib/recentModels";
import type {
  AcpEvent,
  AgentCommandStatus,
  AgentConfig,
  AgentVersionInfo,
  CapabilitySnapshot,
  ModelDef,
  SessionComposerPrefs,
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
  onSend: (text: string, droppedPaths?: string[]) => void;
  /** PTY: inject slash commands into the live terminal. */
  onPtyCommand?: (commandLine: string) => void | Promise<void>;
  /** Notify parent when the active model id changes (for provider balance probes). */
  onActiveModelChange?: (modelId: string | null) => void;
  /** Lazy ACP: warm the agent process without blocking the composer. */
  onWarmAgent?: () => void;
  /** Ensure ACP is ready before set_config_option (returns false if failed). */
  onEnsureAgentReady?: () => Promise<boolean>;
  /** Last stream activity (ms) — escalates busy strip when the turn goes quiet. */
  lastActivityAt?: number | null;
};

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
  // Prefer ACP-enriched label (includes generation from description).
  const label = model.label.includes("/")
    ? model.label.slice(model.label.indexOf("/") + 1).trim()
    : model.label;
  const idTail = model.id.includes("/")
    ? model.id.slice(model.id.indexOf("/") + 1)
    : model.id;
  const name = label || idTail || model.id;
  return name.length > 48 ? `${name.slice(0, 46)}…` : name;
}

function shortTriggerLabel(label: string | null, id: string | null): string {
  const raw = label || id || "model";
  const name = raw.includes("/") ? raw.slice(raw.lastIndexOf("/") + 1).trim() : raw;
  return name.length > 22 ? `${name.slice(0, 20)}…` : name;
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
const COMPOSER_HEIGHT_KEY = "agentshell-composer-height";

function readComposerHeight(): number {
  try {
    const raw = window.localStorage.getItem(COMPOSER_HEIGHT_KEY);
    const n = raw ? Number(raw) : NaN;
    if (Number.isFinite(n)) {
      return Math.min(COMPOSER_HEIGHT_MAX, Math.max(COMPOSER_HEIGHT_MIN, n));
    }
  } catch {
    // ignore
  }
  return COMPOSER_HEIGHT_DEFAULT;
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
) {
  if (!prefs) return;
  const model = prefs.preferredModel?.trim();
  const mode = prefs.preferredMode?.trim();
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

export function Composer({
  agent,
  agents,
  currentAgentId,
  sessionId,
  sessionStatus,
  capabilities: capabilitiesProp,
  prefillText = null,
  prefillToken = 0,
  sessionPrefs = null,
  onSessionPrefsChange,
  onAgentChange,
  onInterrupt,
  onSend,
  onPtyCommand,
  onActiveModelChange,
  onWarmAgent,
  onEnsureAgentReady,
  lastActivityAt = null,
}: ComposerProps) {
  /**
   * Offline-first controls: the last catalog this agent advertised, with this
   * dialog's saved model/mode/effort overlaid. Computed once per mount (App
   * remounts Composer whenever dialog identity changes) so the chips are on
   * screen in the first paint instead of appearing after a handshake.
   */
  const initialCaps = useMemo(
    () => (agent.transport === "acp" ? cachedCapabilitiesFor(agent.id, sessionPrefs) : null),
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
  /** True once caps came from a live agent — cached caps must not act as live. */
  const [capsLive, setCapsLive] = useState(false);
  const [menu, setMenu] = useState<"mode" | "model" | "effort" | "agent" | null>(null);
  const [modelQuery, setModelQuery] = useState("");
  const [draft, setDraft] = useState("");
  const [composerHeight, setComposerHeight] = useState(readComposerHeight);
  const [resizingComposer, setResizingComposer] = useState(false);
  const [showProviderDialog, setShowProviderDialog] = useState(false);
  const [flashError, setFlashError] = useState("");
  const [dropActive, setDropActive] = useState(false);
  /** CLI availability per agent — loaded when the agent menu opens. */
  const [agentStatuses, setAgentStatuses] = useState<Record<string, AgentCommandStatus>>({});
  const [agentVersionInfo, setAgentVersionInfo] = useState<Record<string, AgentVersionInfo>>({});
  const [installingAgentId, setInstallingAgentId] = useState<string | null>(null);
  const [installNote, setInstallNote] = useState("");
  const errorTimer = useRef<ReturnType<typeof setTimeout>>();
  const updating = useRef(false);
  const modelSearchRef = useRef<HTMLInputElement>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
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
   *  - `profile`  static PTY profile (nothing to cache)
   *  - `cache`    replay of the offline snapshot (must not count as live)
   */
  const applyCaps = useCallback(
    (
      result: CapabilitySnapshot | null | undefined,
      source: "acp" | "handshake" | "profile" | "cache" = "acp",
    ) => {
      applyCapabilities(result, capSetters);
      if (!result) return;
      // Layered display: snapshot → this dialog's saved choice → the pick the
      // user just made. Without this the chips flash agent-default → saved →
      // picked on every connect and every set_config echo.
      applyPreferredDisplay(result, sessionPrefsRef.current, capSetters);
      applyPinnedDisplay(pinsRef.current, capSetters);
      if (source === "cache") return;
      setCapsLive(true);
      if (source === "profile") return;
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

  const isAcp = agent.transport === "acp";
  const isPty = agent.transport === "pty";

  // PTY fallback only (custom CLIs without protocol).
  useEffect(() => {
    if (!isPty) return;
    const profileCaps = ptyProfileToCapabilities(agent.id);
    loadSeq.current += 1;
    if (profileCaps) {
      applyCaps(profileCaps, "profile");
    } else {
      setCaps(null);
      setCurrentMode(null);
      setCurrentModel(null);
      setCurrentEffort(null);
    }
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [isPty, agent.id, applyCaps]);

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
    // Rebind from this agent's own source: static profile for PTY, last known
    // catalog for ACP. (Runs after the profile effect, so it must re-apply the
    // profile rather than blank it.)
    const rebound = isPty
      ? ptyProfileToCapabilities(agent.id)
      : cachedCapabilitiesFor(agent.id, sessionPrefsRef.current);
    if (rebound) {
      applyCaps(rebound, isPty ? "profile" : "cache");
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
    if (!isAcp) return;
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
  }, [isAcp, agent.id, capabilitiesProp, applyCaps]);

  // ── Fetch capabilities when session is ready (ACP only) ────────────────────
  useEffect(() => {
    if (isPty) return; // handled by static profile effect
    if (!isAcp || !sessionId || sessionId.startsWith("session-empty-")) {
      return;
    }
    // Do not fetch on "starting": handshake not done yet, null would race-wipe UI.
    // waiting/running: session/new completed and caps are stored server-side.
    if (sessionStatus === "waiting" || sessionStatus === "running") {
      loadCapabilities(sessionId);
    }
  }, [isAcp, isPty, sessionId, sessionStatus, loadCapabilities]);

  // ── Apply caps from session/ready (primary path, avoids race with invoke) ──
  useEffect(() => {
    if (!isAcp || !sessionId || sessionId.startsWith("session-empty-") || !isTauriRuntime()) return;

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
  }, [isAcp, sessionId, loadCapabilities]);

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
      if (updating.current || !sessionId) return;
      updating.current = true;
      try {
        if (isPty) {
          const lines = ptyCommandsForPatch(agent.id, patch);
          if (lines.length === 0) {
            flash("This control has no TUI command for this agent");
            revert();
            return;
          }
          if (!onPtyCommand) {
            flash("Terminal not ready — open Raw Terminal first");
            revert();
            return;
          }
          for (const line of lines) {
            await onPtyCommand(line);
          }
          flash(`Sent to TUI: ${lines.join(" · ")}`);
        } else {
          // Lazy ACP: mode/model/effort require a live session.
          if (onEnsureAgentReady) {
            const ready = await onEnsureAgentReady();
            if (!ready) {
              throw new Error("Agent is not connected yet — type or wait for ACP warm-up");
            }
          }
          // Some agents (Grok Build) don't support session/set_config_option —
          // use prompt injection for mode changes instead.
          const sup = getAcpSupplement(agent.id);
          if (typeof patch.mode === "string" && sup?.promptModeCommands) {
            const cmd = sup.promptModeCommands[patch.mode];
            if (!cmd) {
              throw new Error(`No prompt command for mode "${patch.mode}"`);
            }
            await sendAcpPrompt(sessionId, cmd);
            // Persist the mode preference; skip caps reload — agent won't
            // reflect the change in ACP caps, and the local UI state is correct.
            persistPrefs({ preferredMode: patch.mode });
            return;
          }

          const attempts = expandAcpConfigAttempts(agent.id, patch, caps);
          if (attempts.length === 0) {
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
        }
        // Disk SSOT: only fields that changed (parent merges; never wipe siblings).
        const prefsPatch: SessionComposerPrefs = {};
        if (typeof patch.model === "string") prefsPatch.preferredModel = patch.model;
        if (typeof patch.mode === "string") prefsPatch.preferredMode = patch.mode;
        if (typeof patch.thinkingEffort === "number") prefsPatch.preferredEffort = patch.thinkingEffort;
        if (typeof patch.effortId === "string") prefsPatch.preferredEffortId = patch.effortId;
        if (Object.keys(prefsPatch).length > 0) persistPrefs(prefsPatch);
        // Track recently used model
        if (typeof patch.model === "string") {
          recordModelUsage(patch.model);
        }
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
    [sessionId, flash, loadCapabilities, isPty, agent.id, onPtyCommand, caps, onEnsureAgentReady, persistPrefs],
  );

  // Restore preferred model/mode/effort once caps are available for this dialog.
  // Cached caps only paint the chips — pushing set_config at an agent that is
  // not connected would start a process the user never asked for.
  useEffect(() => {
    if (!caps || !capsLive || sessionId.startsWith("session-empty-")) return;
    const key = `${sessionId}:${agent.id}`;
    if (prefsRestoredKey.current === key) return;

    const prefModel = sessionPrefs?.preferredModel?.trim() || null;
    const prefMode = sessionPrefs?.preferredMode?.trim() || null;
    const prefEffortId = sessionPrefs?.preferredEffortId?.trim() || null;
    const prefEffort =
      typeof sessionPrefs?.preferredEffort === "number" && Number.isFinite(sessionPrefs.preferredEffort)
        ? sessionPrefs.preferredEffort
        : null;

    const hasAny = Boolean(prefModel || prefMode || prefEffortId || prefEffort != null);
    if (!hasAny) {
      prefsRestoredKey.current = key;
      return;
    }

    prefsRestoredKey.current = key;

    const modelOk =
      prefModel &&
      (caps.models.length === 0 || caps.models.some((m) => m.id === prefModel));
    const modeOk =
      prefMode &&
      (caps.modes.length === 0 || caps.modes.some((m) => m.id === prefMode));
    const effortIdOk =
      prefEffortId &&
      (caps.effortOptions ?? []).some((o) => o.id === prefEffortId);
    const numericOk =
      prefEffort != null &&
      caps.thinkingEffort != null &&
      prefEffort >= caps.thinkingEffort.min &&
      prefEffort <= caps.thinkingEffort.max;

    // Optimistic UI so the trigger labels update before set_config returns.
    if (modelOk && prefModel) setCurrentModel(prefModel);
    if (modeOk && prefMode) setCurrentMode(prefMode);
    if (effortIdOk && prefEffortId) setCurrentEffortId(prefEffortId);
    if (numericOk && prefEffort != null) setCurrentEffort(prefEffort);

    void (async () => {
      // Only push options that differ from live caps (avoid noisy set_config on every open).
      if (modelOk && prefModel && prefModel !== caps.currentModel) {
        await commitUpdate({ model: prefModel }, () => {
          setCurrentModel(caps.currentModel);
        });
      }
      if (modeOk && prefMode && prefMode !== caps.currentMode) {
        await commitUpdate({ mode: prefMode }, () => {
          setCurrentMode(caps.currentMode);
        });
      }
      // Effort options are per-model. `caps` here still describes the model that
      // was live at session/new, so the check above validated the saved effort
      // against the *wrong* list — a saved "max" survived the switch to a model
      // that never offered it, and the agent rejected it every session with
      // "effort not found: max". Re-check against what the new model advertises.
      const live = modelOk && prefModel && prefModel !== caps.currentModel
        ? ((await getSessionCapabilities(sessionId).catch(() => null)) ?? caps)
        : caps;
      const liveEffortIdOk =
        Boolean(prefEffortId) && (live.effortOptions ?? []).some((o) => o.id === prefEffortId);
      const liveNumericOk =
        prefEffort != null &&
        live.thinkingEffort != null &&
        prefEffort >= live.thinkingEffort.min &&
        prefEffort <= live.thinkingEffort.max;
      if (effortIdOk && !liveEffortIdOk) {
        // Keep it on disk — switching back to a model that has it should restore it.
        setCurrentEffortId(live.currentEffortId);
      }

      if (liveEffortIdOk && prefEffortId && prefEffortId !== live.currentEffortId) {
        await commitUpdate({ effortId: prefEffortId }, () => {
          setCurrentEffortId(live.currentEffortId);
        });
      } else if (
        liveNumericOk &&
        prefEffort != null &&
        (live.currentEffort == null || Math.abs(live.currentEffort - prefEffort) > 1e-6)
      ) {
        await commitUpdate({ thinkingEffort: prefEffort }, () => {
          setCurrentEffort(live.currentEffort);
        });
      }
    })();
    // commitUpdate changes often; restore only when caps identity for this dialog lands.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [caps, capsLive, sessionId, agent.id, sessionPrefs]);

  // ── Send / Interrupt ───────────────────────────────────────────────────────
  // Only a live turn (`running`) blocks send. `starting` must NOT freeze the
  // composer — warm happens in the background; send will wait for ready.
  const isBusy =
    (isAcp && sessionStatus === "running") ||
    (isPty && sessionStatus === "running");
  const isWarming = isAcp && sessionStatus === "starting";
  // Prefer advertised cancel; still offer interrupt while running (Esc×2 / button).
  const canCancel = isBusy && (isPty || (caps?.supportsCancel ?? true));
  const submit = () => {
    // Empty draft is OK when parent has quote-pins (App merges on send).
    if (isBusy) return;
    if (!sessionId || sessionId.startsWith("session-empty-")) return;
    onWarmAgent?.();
    // Only paths the user actually left in the draft — deleting the text should
    // not still trigger a grant prompt for it.
    const dropped = [...droppedPathsRef.current].filter((p) => draft.includes(p));
    onSend(draft, dropped);
    droppedPathsRef.current.clear();
    setDraft("");
  };

  /** Insert dropped file paths into the draft (Tauri exposes absolute `path` on File). */
  const insertDroppedPaths = useCallback((paths: string[]) => {
    const unique = [...new Set(paths.map((p) => p.trim()).filter(Boolean))];
    if (unique.length === 0) return;
    for (const path of unique) droppedPathsRef.current.add(path);
    const chunk = unique.join("\n");
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
      if (!isAcp) return;
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
    [isAcp, onWarmAgent],
  );

  // ── Derived state ──────────────────────────────────────────────────────────
  // ACP: negotiated caps. PTY: static profile so model/mode/effort always show when defined.
  const hasModes = caps != null && caps.modes.length > 1;
  const hasModels = caps != null && caps.models.length > 0;
  const effortOptions = caps?.effortOptions ?? [];
  // ACP: only show effort when the live agent registered the option.
  // Claude omits `effort` entirely for models without supportsEffort
  // (e.g. Haiku after model switch) — inventing it yields:
  //   Unknown config option: effort
  // and a stale Effort menu would crash on thinkingEffort!.default.
  const hasStringEffort =
    effortOptions.length > 0 && (isPty || Boolean(caps?.effortConfigId));
  const hasNumericEffort =
    caps != null &&
    caps.thinkingEffort != null &&
    (isPty || Boolean(caps.effortConfigId));
  const hasEffort = hasStringEffort || hasNumericEffort;
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
    // Two passes: `--version` is local and paints immediately, the registry
    // lookup is network and only fills in "update available" when it lands.
    let live = true;
    void (async () => {
      const local = await agentVersions(false);
      if (live && local.length > 0) {
        setAgentVersionInfo(Object.fromEntries(local.map((v) => [v.id, v])));
      }
      const remote = await agentVersions(true);
      if (live && remote.length > 0) {
        setAgentVersionInfo(Object.fromEntries(remote.map((v) => [v.id, v])));
      }
    })();
    return () => {
      live = false;
    };
  }, [menu, refreshAgentStatuses]);

  const runInstall = useCallback(
    async (agentId: string) => {
      if (installingAgentId) return;
      setInstallingAgentId(agentId);
      setInstallNote(`Installing ${agentId}… (npm global, this can take a minute)`);
      try {
        const result = await installAgent(agentId, true);
        setAgentStatuses((current) => ({ ...current, [agentId]: result.status }));
        setInstallNote(result.message);
        // Pick up anything the install pulled in for the other agents too.
        void refreshAgentStatuses();
      } catch (error) {
        setInstallNote(error instanceof Error ? error.message : String(error));
      } finally {
        setInstallingAgentId(null);
      }
    },
    [installingAgentId, refreshAgentStatuses],
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
      <div
        className={dropActive ? "composer__field is-drop-target" : "composer__field"}
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
        <textarea
          ref={textareaRef}
          aria-label="Prompt composer"
          placeholder={
            isAcp
              ? isWarming && !composingRef.current
                ? "Agent connecting in background… keep typing"
                : "Message the Agent (Ctrl+Enter to send) · Tab cycles mode · drop files for paths"
              : "Message the TUI (Ctrl+Enter) · drop files for paths"
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
          }}
          onCompositionEnd={(event) => {
            composingRef.current = false;
            // Ensure final composed text is stored (some IMEs need this).
            setDraft(event.currentTarget.value);
            warmIfNeeded();
          }}
          onChange={(event) => {
            setDraft(event.target.value);
            // Do not warm on every keypress — freezes IME while ACP handshake runs.
          }}
          onKeyDown={(event) => {
            if (event.key === "Enter" && (event.ctrlKey || event.metaKey)) {
              event.preventDefault();
              if (composingRef.current) return;
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
            <button className="composer-tool" type="button" title="Add files or context">
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
          </div>
          <div className="composer__actions">
            {/* ── Model + Effort selector ─────────────────────────────── */}
            {/* Always show when caps exist (even empty for add-key prompt), or has effort */}
            {(hasModels || hasEffort || (caps && caps.models.length === 0)) && (
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
                    {!hasModels && caps?.models.length === 0 ? (
                      /* ── Empty state: no models / no keys ── */
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
                    ) : (
                      <>
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
                                      title={[m.label || m.id, m.description, m.id].filter(Boolean).join("\n")}
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
                                      <span className="composer-menu__model-id">{m.id}</span>
                                    </button>
                                  ))}
                                </div>
                              ))}
                            </div>

                            {/* ── Add API Key entry at bottom ── */}
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
                        {hasEffort && <span className="composer-menu__divider" />}
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
                      </>
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
                className="composer-select composer-select--agent"
                type="button"
                title={`Agent for this dialog (bound to session · ${agent.id})`}
                aria-expanded={menu === "agent"}
                onClick={() => setMenu(menu === "agent" ? null : "agent")}
              >
                {agent.label}
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
                      const version = agentVersionInfo[candidate.id];
                      // Offer the upgrade even when startup already tried it —
                      // it may have failed, or the release may be newer than
                      // this app launch.
                      const canUpdate =
                        ready && (version?.updateAvailable ?? false) && !busyInstall;
                      return (
                        <div className="composer-agent-row" key={candidate.id}>
                          <button
                            className={agent.id === candidate.id ? "is-selected" : ""}
                            type="button"
                            role="option"
                            aria-selected={agent.id === candidate.id}
                            title={status?.message ?? candidate.command}
                            onClick={() => {
                              setMenu(null);
                              // Always mirror session.agentId (via agent prop), never a floating selection.
                              onAgentChange(candidate.id);
                            }}
                          >
                            <span className="composer-agent-row__name">{candidate.label}</span>
                            {version?.installed && (
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
                            )}
                            {!ready && (
                              <span className="composer-agent-row__tag">
                                {busyInstall
                                  ? "installing…"
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
                                void runInstall(candidate.id);
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
                        </div>
                      );
                    })}
                  {installNote && <div className="composer-menu__note">{installNote}</div>}
                </div>
              )}
            </div>

            {/* ── Send / Interrupt (Esc×2 also interrupts) ── */}
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
                    : "Send"
              }
              aria-label={
                isBusy && canCancel
                  ? "Interrupt conversation"
                  : isBusy && !canCancel
                    ? "Agent is busy"
                    : "Send"
              }
              disabled={isBusy && !canCancel}
              onClick={() => {
                if (isBusy && canCancel) onInterrupt();
                else if (!isBusy) submit();
              }}
            >
              {isBusy && canCancel ? (
                <Square size={12} fill="currentColor" />
              ) : (
                <SendHorizontal size={15} />
              )}
            </button>
          </div>
        </div>
      </div>
      {showProviderDialog && (
        <ProviderConfigDialog
          onClose={() => setShowProviderDialog(false)}
          onSaved={() => {
            setShowProviderDialog(false);
            // Refresh capabilities to pick up new models
            if (sessionId && !sessionId.startsWith("session-empty-")) {
              loadCapabilities(sessionId);
            }
          }}
        />
      )}
    </footer>
  );
}
