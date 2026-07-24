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
};

const SUPPLEMENTS: Record<string, AcpSupplement> = {
  "grok-build": {
    // From `grok --permission-mode` help
    modes: [
      { id: "default", label: "Default" },
      { id: "plan", label: "Plan" },
      { id: "acceptEdits", label: "Accept edits" },
      { id: "auto", label: "Auto" },
      { id: "dontAsk", label: "Don't ask" },
      { id: "bypassPermissions", label: "Bypass permissions" },
    ],
    models: [{ id: "grok-4.5", label: "Grok 4.5" }],
    thinkingEffort: { min: 0, max: 1, default: 0.5 },
    defaultMode: "default",
    defaultModel: "grok-4.5",
    modeConfigIds: ["permission-mode", "permissionMode", "mode", "permission_mode"],
    effortConfigIds: ["reasoning-effort", "reasoningEffort", "effort", "thought_level", "thinking"],
    modelConfigIds: ["model"],
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

  // Effort is model-dependent (especially Claude). Never invent an effort control
  // when live negotiation did not advertise one — that produces "Unknown config option".
  const liveHasEffort =
    Boolean(base.effortConfigId) ||
    (base.effortOptions?.length ?? 0) > 0 ||
    base.thinkingEffort != null;
  const effortOptions = liveHasEffort
    ? (base.effortOptions?.length ?? 0) > 0
      ? base.effortOptions
      : sup.effortOptions ?? []
    : live
      ? []
      : sup.effortOptions ?? [];
  const thinkingEffort = liveHasEffort
    ? (base.thinkingEffort ?? (effortOptions.length > 0 ? null : sup.thinkingEffort ?? null))
    : live
      ? null
      : (sup.thinkingEffort ?? null);
  const effortConfigId = liveHasEffort
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
    currentEffortId: liveHasEffort
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
