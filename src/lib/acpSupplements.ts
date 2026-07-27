import type { CapabilitySnapshot, ModeDef, ModelDef, ThinkingEffort } from "./types";

/**
 * When an ACP agent omits modes/models/effort in session/new, we still want
 * Composer controls. Supplements are agent-specific knowledge of CLI flags /
 * config option ids — not TUI scraping.
 */

export type AcpSupplement = {
  modes?: ModeDef[];
  models?: ModelDef[];
  thinkingEffort?: ThinkingEffort | null;
  /** Discrete effort ids when the agent does not use 0–1 numbers (Claude). */
  effortOptions?: ModeDef[];
  defaultMode?: string;
  defaultModel?: string;
  defaultEffortId?: string;
  /** Extra keys tried for session/set_config_option when setting mode. */
  modeConfigIds?: string[];
  effortConfigIds?: string[];
  modelConfigIds?: string[];
  /**
   * Effort is carried by `session/set_model`'s `_meta.reasoningEffort` rather
   * than a config option, so `effortOptions` here are real even though live
   * negotiation advertises no effort config id. Rust routes these (see
   * `legacy_set_config`); the id stays null on purpose so we never send a
   * fabricated `effort` config option.
   */
  effortViaLegacyModel?: boolean;
  /**
   * Permission auto-approval is a *separate axis* from plan/build modes.
   * Grok exposes it only as the `/always-approve on|off` slash command
   * (verified: off → session/request_permission, on → tools run without).
   * Composer renders a dedicated chip and drives it via session/prompt.
   */
  alwaysApprove?: {
    on: string;
    off: string;
    /** Chip label when enabled. */
    onLabel?: string;
    /** Chip label when disabled. */
    offLabel?: string;
  };
};

const SUPPLEMENTS: Record<string, AcpSupplement> = {
  "grok-build": {
    // Grok answers -32601 to session/set_config_option; its knobs are the
    // pre-v2 RPCs (session/set_mode, session/set_model). Verified against
    // `grok agent stdio` 0.2.104: session/new advertises neither `modes` nor
    // `configOptions`, so every list below has to come from here.
    //
    // plan/build verified *behaviourally*, not by echo: in `plan` Grok refuses
    // a file write and calls exit_plan_mode; in `build` the same prompt writes.
    // Do not trust `current_mode_update` as validation — Grok echoes back any
    // string you send it (`yolo`, `chat`, … all "confirm"), so the echo says
    // nothing about whether a mode exists.
    modes: [
      { id: "plan", label: "Plan" },
      { id: "build", label: "Build" },
    ],
    models: [{ id: "grok-4.5", label: "Grok 4.5" }],
    // Grok publishes these under _meta["x.ai/sessionConfig"] (confusingly with
    // category "mode") and _meta.modelState[].reasoningEfforts.
    effortOptions: [
      { id: "high", label: "High" },
      { id: "medium", label: "Medium" },
      { id: "low", label: "Low" },
    ],
    effortViaLegacyModel: true,
    // Grok sends no current_mode_update at session start, so this default is the
    // only thing the chip can show. It says `build` because a fresh session that
    // is told set_mode("build") emits no change event, while plan/build swaps do.
    defaultMode: "build",
    defaultModel: "grok-4.5",
    // Matches _meta.modelState[].reasoningEfforts[].default for grok-4.5.
    defaultEffortId: "high",
    modelConfigIds: ["model"],
    // Grok's most important day-to-day control. Not an ACP mode — Shift+Tab
    // cycles plan/build; Ctrl+O / this chip toggles always-approve.
    alwaysApprove: {
      on: "/always-approve on",
      off: "/always-approve off",
      onLabel: "Always approve",
      offLabel: "Ask permissions",
    },
  },
  "claude-code": {
    // Wire ids from claude-agent-acp `buildAvailableModes` (name "Manual" still id "default")
    modes: [
      { id: "default", label: "Manual" },
      { id: "acceptEdits", label: "Accept Edits" },
      { id: "plan", label: "Plan Mode" },
      { id: "dontAsk", label: "Don't Ask" },
      { id: "auto", label: "Auto" },
      { id: "bypassPermissions", label: "Bypass Permissions" },
    ],
    // IMPORTANT: do NOT invent effortOptions / effortConfigIds here.
    // claude-agent-acp only registers config id `effort` when the *current model*
    // has supportsEffort + supportedEffortLevels. Inventing "effort" causes:
    //   Unknown config option: effort
    // Live session/new configOptions is the source of truth.
    defaultMode: "default",
    modeConfigIds: ["mode"],
    modelConfigIds: ["model"],
  },
  codex: {
    modes: [
      { id: "default", label: "Default" },
      { id: "suggest", label: "Suggest" },
      { id: "auto-edit", label: "Auto edit" },
      { id: "full-auto", label: "Full auto" },
    ],
    thinkingEffort: { min: 0, max: 1, default: 0.5 },
    modeConfigIds: ["mode", "approval-policy", "approvalPolicy"],
    effortConfigIds: ["reasoning", "reasoning-effort", "effort"],
  },
};

export function getAcpSupplement(agentId: string): AcpSupplement | null {
  return SUPPLEMENTS[agentId] ?? null;
}

/** Merge live ACP negotiation with static supplements (prefer live non-empty lists). */
export function mergeAcpCapabilities(
  agentId: string,
  live: CapabilitySnapshot | null | undefined,
): CapabilitySnapshot | null {
  const sup = getAcpSupplement(agentId);
  if (!live && !sup) return null;

  const base: CapabilitySnapshot = live ?? {
    modes: [],
    models: [],
    thinkingEffort: null,
    effortOptions: [],
    supportsCancel: true,
    currentMode: null,
    currentModel: null,
    currentEffort: null,
    currentEffortId: null,
    modelConfigId: null,
    modeConfigId: null,
    effortConfigId: null,
  };

  if (!sup) {
    return {
      ...base,
      effortOptions: base.effortOptions ?? [],
      currentEffortId: base.currentEffortId ?? null,
    };
  }

  const modes = base.modes.length > 0 ? base.modes : sup.modes ?? [];
  const models = base.models.length > 0 ? base.models : sup.models ?? [];

  // Grok Build: ACP does not support session/set_config_option, but its pre-v2
  // RPCs (session/set_mode, session/set_model) carry mode/effort via `_meta`.
  // The `legacyEffort` path below handles effort; no early return needed.

  // Effort is model-dependent (especially Claude). Never invent an effort control
  // when live negotiation did not advertise one — that produces "Unknown config option".
  const liveHasEffort =
    Boolean(base.effortConfigId) ||
    (base.effortOptions?.length ?? 0) > 0 ||
    base.thinkingEffort != null;
  // Exception: agents whose effort rides on session/set_model have no config id
  // to advertise, so absence proves nothing. Only a supplement that explicitly
  // opts in gets this — a missing id still means "no control" everywhere else.
  const legacyEffort =
    sup.effortViaLegacyModel === true && (sup.effortOptions?.length ?? 0) > 0;

  const effortOptions = legacyEffort
    ? sup.effortOptions ?? []
    : liveHasEffort
      ? (base.effortOptions?.length ?? 0) > 0
        ? base.effortOptions
        : sup.effortOptions ?? []
      : live
        ? []
        : sup.effortOptions ?? [];
  const thinkingEffort = legacyEffort
    ? null // discrete levels only; no 0–1 slider
    : liveHasEffort
      ? (base.thinkingEffort ?? (effortOptions.length > 0 ? null : sup.thinkingEffort ?? null))
      : live
        ? null
        : (sup.thinkingEffort ?? null);
  // Stays null for the legacy transport: there is no config option to name.
  const effortConfigId = legacyEffort
    ? null
    : liveHasEffort
      ? (base.effortConfigId ?? sup.effortConfigIds?.[0] ?? null)
      : live
        ? null
        : (base.effortConfigId ?? sup.effortConfigIds?.[0] ?? null);

  return {
    ...base,
    modes,
    models,
    thinkingEffort,
    effortOptions,
    supportsCancel: base.supportsCancel || true,
    currentMode: base.currentMode ?? sup.defaultMode ?? modes[0]?.id ?? null,
    currentModel: base.currentModel ?? sup.defaultModel ?? models[0]?.id ?? null,
    currentEffort:
      base.currentEffort ??
      (thinkingEffort ? thinkingEffort.default : null),
    currentEffortId: liveHasEffort || legacyEffort
      ? (base.currentEffortId ??
        sup.defaultEffortId ??
        effortOptions[0]?.id ??
        null)
      : null,
    // Keep live config ids; if missing, use first known id for set_config_option attempts
    modeConfigId: base.modeConfigId ?? sup.modeConfigIds?.[0] ?? null,
    modelConfigId: base.modelConfigId ?? sup.modelConfigIds?.[0] ?? null,
    effortConfigId,
  };
}

/**
 * Map a 0–1 UI strength onto the closest discrete effort option id.
 * Used only when the agent exposes string levels (Claude).
 */
export function mapNumericEffortToOptionId(
  n: number,
  options: ModeDef[],
): string | null {
  if (options.length === 0) return null;
  const ids = options.map((o) => o.id.toLowerCase());
  if (n <= 0.2) {
    return options.find((o) => /low|minimal|min/.test(o.id.toLowerCase()))?.id
      ?? options.find((o) => o.id.toLowerCase() === "default")?.id
      ?? options[0].id;
  }
  if (n >= 0.8) {
    return options.find((o) => /max|xhigh|ultra/.test(o.id.toLowerCase()))?.id
      ?? options.find((o) => /high/.test(o.id.toLowerCase()))?.id
      ?? options[options.length - 1].id;
  }
  if (n >= 0.55) {
    return options.find((o) => o.id.toLowerCase() === "high")?.id
      ?? options.find((o) => /medium|mid|default/.test(o.id.toLowerCase()))?.id
      ?? options[Math.floor(options.length / 2)].id;
  }
  // mid-low → default or medium
  if (ids.includes("default")) return options.find((o) => o.id.toLowerCase() === "default")!.id;
  if (ids.includes("medium")) return options.find((o) => o.id.toLowerCase() === "medium")!.id;
  return options[Math.floor(options.length / 2)]?.id ?? options[0].id;
}

/** Expand a UI patch into one or more ACP set_config payloads to try. */
export function expandAcpConfigAttempts(
  agentId: string,
  patch: Record<string, unknown>,
  caps: CapabilitySnapshot | null,
): Record<string, unknown>[] {
  const sup = getAcpSupplement(agentId);
  const attempts: Record<string, unknown>[] = [];

  // Grok: model is launch-time only; effort and mode use legacy set_{model,mode}
  // RPCs routed through Rust. Never try set_config_option for these on Grok.
  if (agentId === "grok-build") {
    if (typeof patch.model === "string") {
      return [];
    }
  }

  if (typeof patch.model === "string") {
    const ids = [
      caps?.modelConfigId,
      ...(sup?.modelConfigIds ?? []),
      "model",
    ].filter(Boolean) as string[];
    for (const id of [...new Set(ids)]) {
      attempts.push({ configId: id, value: patch.model });
    }
  }

  if (typeof patch.mode === "string") {
    const ids = [
      caps?.modeConfigId,
      ...(sup?.modeConfigIds ?? []),
      "mode",
    ].filter(Boolean) as string[];
    for (const id of [...new Set(ids)]) {
      attempts.push({ configId: id, value: patch.mode });
    }
  }

  // Grok-style: no config id exists, so hand Rust the logical knob and let
  // `legacy_set_config` put it on session/set_model.
  if (sup?.effortViaLegacyModel && !caps?.effortConfigId) {
    if (typeof patch.effortId === "string") {
      attempts.push({ effortId: patch.effortId });
    } else if (typeof patch.thinkingEffort === "number") {
      attempts.push({ thinkingEffort: patch.thinkingEffort });
    }
  }

  // Preferred path: explicit string effort id (Claude)
  // Only attempt when the live session actually advertised an effort config.
  if (typeof patch.effortId === "string" && caps?.effortConfigId) {
    attempts.push({ configId: caps.effortConfigId, value: patch.effortId });
  } else if (typeof patch.thinkingEffort === "number" && caps?.effortConfigId) {
    const n = patch.thinkingEffort;
    const effortOptions = caps.effortOptions ?? [];
    if (effortOptions.length > 0) {
      const mapped = mapNumericEffortToOptionId(n, effortOptions);
      if (mapped) {
        attempts.push({ configId: caps.effortConfigId, value: mapped });
      }
    } else {
      // Numeric agents (Grok/Codex-style): try float + low/medium/high aliases
      const level = n <= 0.25 ? "low" : n >= 0.75 ? "high" : "medium";
      attempts.push({ configId: caps.effortConfigId, value: level });
      attempts.push({ configId: caps.effortConfigId, value: n });
    }
  }

  // Passthrough if already configId form
  if (typeof patch.configId === "string" && patch.value != null) {
    attempts.unshift({ configId: patch.configId, value: patch.value });
  }

  return attempts;
}
