import { listen } from "@tauri-apps/api/event";
import { Expand, Plus, Search, SendHorizontal, Shrink, Square, Target } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { expandAcpConfigAttempts, mergeAcpCapabilities } from "../lib/acpSupplements";
import { getSessionCapabilities, isTauriRuntime, updateAcpSession } from "../lib/api";
import { ptyCommandsForPatch, ptyProfileToCapabilities } from "../lib/ptyProfiles";
import type {
  AcpEvent,
  AgentConfig,
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
  onSend: (text: string) => void;
  /** PTY: inject slash commands into the live terminal. */
  onPtyCommand?: (commandLine: string) => void | Promise<void>;
  /** Notify parent when the active model id changes (for provider balance probes). */
  onActiveModelChange?: (modelId: string | null) => void;
  /** Lazy ACP: warm the agent process without blocking the composer. */
  onWarmAgent?: () => void;
  /** Ensure ACP is ready before set_config_option (returns false if failed). */
  onEnsureAgentReady?: () => Promise<boolean>;
};

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
}: ComposerProps) {
  const [caps, setCaps] = useState<CapabilitySnapshot | null>(null);
  const [currentMode, setCurrentMode] = useState<string | null>(null);
  const [currentModel, setCurrentModel] = useState<string | null>(null);
  const [currentEffort, setCurrentEffort] = useState<number | null>(null);
  const [currentEffortId, setCurrentEffortId] = useState<string | null>(null);
  const [menu, setMenu] = useState<"mode" | "model" | "effort" | null>(null);
  const [modelQuery, setModelQuery] = useState("");
  const [draft, setDraft] = useState("");
  const [expanded, setExpanded] = useState(false);
  const [flashError, setFlashError] = useState("");
  const errorTimer = useRef<ReturnType<typeof setTimeout>>();
  const updating = useRef(false);
  const modelSearchRef = useRef<HTMLInputElement>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  /** IME composition must not re-render placeholder / warm ACP mid-input. */
  const composingRef = useRef(false);
  const warmedThisFocusRef = useRef(false);
  // Ignore stale getSessionCapabilities responses (starting→null overwriting ready).
  const loadSeq = useRef(0);
  const appliedPrefillToken = useRef(0);
  /** Restore disk prefs once per (session, agent) after caps are known. */
  const prefsRestoredKey = useRef("");

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

  // ── Shared capability loader ────────────────────────────────────────────────
  const loadCapabilities = useCallback(
    (id: string) => {
      const seq = ++loadSeq.current;
      getSessionCapabilities(id).then((result) => {
        if (seq !== loadSeq.current) return;
        // Merge ACP negotiation with static supplements (e.g. Grok modes).
        const merged = mergeAcpCapabilities(agent.id, result);
        if (merged) applyCapabilities(merged, capSetters);
      });
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [agent.id],
  );

  const isAcp = agent.transport === "acp";
  const isPty = agent.transport === "pty";

  // PTY fallback only (custom CLIs without protocol).
  useEffect(() => {
    if (!isPty) return;
    const profileCaps = ptyProfileToCapabilities(agent.id);
    loadSeq.current += 1;
    if (profileCaps) {
      applyCapabilities(profileCaps, capSetters);
    } else {
      setCaps(null);
      setCurrentMode(null);
      setCurrentModel(null);
      setCurrentEffort(null);
    }
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [isPty, agent.id]);

  // HARD RULE: wipe composer capability state whenever dialog identity changes.
  // Prevents "OpenCode selected but Claude models still listed".
  useEffect(() => {
    loadSeq.current += 1;
    setCaps(null);
    setCurrentMode(null);
    setCurrentModel(null);
    setCurrentEffort(null);
    setCurrentEffortId(null);
    setMenu(null);
    setModelQuery("");
    // Keep draft/prefill; collapse tall composer when switching dialog/agent.
    setExpanded(false);
    prefsRestoredKey.current = "";
  }, [sessionId, agent.id]);

  // Parent-pushed caps from startAcpSession + supplements for missing mode/effort.
  useEffect(() => {
    if (!isAcp) return;
    if (capabilitiesProp) {
      loadSeq.current += 1;
      const merged = mergeAcpCapabilities(agent.id, capabilitiesProp);
      if (merged) applyCapabilities(merged, capSetters);
    } else {
      // Parent cleared caps (session switch) — already wiped by identity effect.
    }
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [isAcp, agent.id, capabilitiesProp]);

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
          if (merged) applyCapabilities(merged, capSetters);
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
          loadCapabilities(sessionId);
        }
        // Disk SSOT: only fields that changed (parent merges; never wipe siblings).
        const prefsPatch: SessionComposerPrefs = {};
        if (typeof patch.model === "string") prefsPatch.preferredModel = patch.model;
        if (typeof patch.mode === "string") prefsPatch.preferredMode = patch.mode;
        if (typeof patch.thinkingEffort === "number") prefsPatch.preferredEffort = patch.thinkingEffort;
        if (typeof patch.effortId === "string") prefsPatch.preferredEffortId = patch.effortId;
        if (Object.keys(prefsPatch).length > 0) persistPrefs(prefsPatch);
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
  useEffect(() => {
    if (!caps || sessionId.startsWith("session-empty-")) return;
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
      if (effortIdOk && prefEffortId && prefEffortId !== caps.currentEffortId) {
        await commitUpdate({ effortId: prefEffortId }, () => {
          setCurrentEffortId(caps.currentEffortId);
        });
      } else if (
        numericOk &&
        prefEffort != null &&
        (caps.currentEffort == null || Math.abs(caps.currentEffort - prefEffort) > 1e-6)
      ) {
        await commitUpdate({ thinkingEffort: prefEffort }, () => {
          setCurrentEffort(caps.currentEffort);
        });
      }
    })();
    // commitUpdate changes often; restore only when caps identity for this dialog lands.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [caps, sessionId, agent.id, sessionPrefs]);

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
    onSend(draft);
    setDraft("");
  };

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
      setCurrentMode(next.id);
      setMenu(null);
      void commitUpdate({ mode: next.id }, () => setCurrentMode(prev));
      flash(`Mode · ${next.label}`);
    },
    // commitUpdate / flash defined above; displayMode computed later — use caps only
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [caps, currentMode, commitUpdate, flash],
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

  // Reset + focus search when opening model menu
  useEffect(() => {
    if (menu === "model") {
      setModelQuery("");
      requestAnimationFrame(() => modelSearchRef.current?.focus());
    }
  }, [menu]);

  return (
    <footer
      className={
        expanded
          ? "composer is-expanded"
          : isBusy
            ? "composer is-busy"
            : isWarming
              ? "composer is-warming"
              : "composer"
      }
      aria-label="Composer"
    >
      {flashError && <div className="composer__error-toast">{flashError}</div>}
      {(isBusy || isWarming) && (
        <div className="composer__activity" role="status" aria-live="polite">
          <span className="composer__activity-pulse" aria-hidden />
          <span>
            {isBusy
              ? "Agent working — Esc×2 or ■ to interrupt"
              : "Connecting agent in background…"}
          </span>
        </div>
      )}
      <div className="composer__field">
        <button
          className="composer-expand"
          type="button"
          title={expanded ? "Collapse composer" : "Expand composer"}
          aria-label={expanded ? "Collapse composer" : "Expand composer"}
          aria-pressed={expanded}
          onMouseDown={(event) => {
            // Don't steal focus from IME mid-composition; still toggle on click.
            event.preventDefault();
          }}
          onClick={(event) => {
            event.preventDefault();
            event.stopPropagation();
            setExpanded((v) => !v);
            // Keep caret focus after layout change.
            requestAnimationFrame(() => textareaRef.current?.focus());
          }}
        >
          {expanded ? <Shrink size={13} /> : <Expand size={13} />}
        </button>
        <textarea
          ref={textareaRef}
          aria-label="Prompt composer"
          placeholder={
            isAcp
              ? isWarming && !composingRef.current
                ? "Agent connecting in background… keep typing"
                : "Message the Agent (Ctrl+Enter to send)"
              : "Message the TUI (Ctrl+Enter) · model/mode/effort inject slash commands"
          }
          rows={expanded ? 12 : 2}
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
            // Esc: close menus first; if expanded with no menu, collapse height.
            if (event.key === "Escape") {
              if (menu) {
                event.preventDefault();
                event.stopPropagation();
                setMenu(null);
                return;
              }
              if (expanded) {
                event.preventDefault();
                event.stopPropagation();
                setExpanded(false);
              }
            }
          }}
        />
        <div className="composer__toolbar">
          <div className="composer__controls">
            <button className="composer-tool" type="button" title="Add files or context">
              <Plus size={14} />
            </button>
          </div>
          <div className="composer__actions">
            {/* ── Mode selector (only if agent exposes ≥2 modes) ──────── */}
            {hasModes && displayMode && (
              <div className="composer-menu-anchor">
                <button
                  className="composer-select"
                  type="button"
                  title="Execution mode"
                  aria-expanded={menu === "mode"}
                  onClick={() => setMenu(menu === "mode" ? null : "mode")}
                >
                  <Target size={13} />
                  {displayModeLabel}
                </button>
                {menu === "mode" && (
                  <div className="composer-menu" role="menu" aria-label="Execution mode">
                    {caps!.modes.map((m) => (
                      <button
                        key={m.id}
                        className={displayMode === m.id ? "is-selected" : ""}
                        type="button"
                        role="menuitem"
                        onClick={() => {
                          const prev = displayMode;
                          setCurrentMode(m.id);
                          setMenu(null);
                          void commitUpdate(
                            { mode: m.id },
                            () => setCurrentMode(prev),
                          );
                        }}
                      >
                        {m.label}
                      </button>
                    ))}
                  </div>
                )}
              </div>
            )}

            {/* ── Model + Effort selector ─────────────────────────────── */}
            {(hasModels || hasEffort) && (
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
                    {hasModels && (
                      <>
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
                                    setCurrentModel(m.id);
                                    // Never jump to Effort before the agent re-negotiates
                                    // options — Haiku drops effort and a stale menu white-screens.
                                    setMenu(null);
                                    void commitUpdate({ model: m.id }, () => setCurrentModel(prev));
                                  }}
                                >
                                  <span className="composer-menu__model-name">{shortModelName(m)}</span>
                                  <span className="composer-menu__model-id">{m.id}</span>
                                </button>
                              ))}
                            </div>
                          ))}
                        </div>
                        {hasEffort && <span className="composer-menu__divider" />}
                      </>
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
                                  setCurrentEffortId(opt.id);
                                  setMenu(null);
                                  void commitUpdate({ effortId: opt.id }, () =>
                                    setCurrentEffortId(prev),
                                  );
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
                                  setCurrentEffort(preset.value);
                                  setMenu(null);
                                  void commitUpdate(
                                    { thinkingEffort: preset.value },
                                    () => setCurrentEffort(prev),
                                  );
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
                              setCurrentEffortId(opt.id);
                              setMenu(null);
                              void commitUpdate({ effortId: opt.id }, () =>
                                setCurrentEffortId(prev),
                              );
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
                              setCurrentEffort(preset.value);
                              setMenu(null);
                              void commitUpdate(
                                { thinkingEffort: preset.value },
                                () => setCurrentEffort(prev),
                              );
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
            <label
              className="composer-select composer-select--agent"
              title={`Agent for this dialog (bound to session · ${agent.id})`}
            >
              <select
                aria-label="Switch Agent"
                // Always mirror session.agentId (via agent prop), never a floating selection.
                value={agent.id}
                onChange={(event) => {
                  setMenu(null);
                  onAgentChange(event.target.value);
                }}
              >
                {agents
                  .filter((candidate) => candidate.enabled)
                  .map((candidate) => (
                    <option key={candidate.id} value={candidate.id}>
                      {candidate.label}
                    </option>
                  ))}
              </select>
            </label>

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
    </footer>
  );
}
