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
    // plan / execute verified *behaviourally*: in `plan` Grok refuses a file
    // write and calls exit_plan_mode; after approve it emits currentModeId
    // `default` and the same prompt writes. Grok 1.0.89 renamed the execute
    // mode from `build` → `default` (`unknown mode id: build`). Keep the chip
    // label "Build". Old disk prefs / OpenCode handoff still say `build` —
    // `normalizeAgentModeId` maps it. Do not trust current_mode_update as
    // validation of invented ids; it does confirm the live execute id.
    modes: [
      { id: "plan", label: "Plan" },
      { id: "default", label: "Build" },
    ],
    models: [{ id: "grok-4.5", label: "Grok 4.5" }],
    // Grok advertises the *current model's* levels under
    // `models.availableModels[]._meta.reasoningEfforts` — a custom catalog entry
    // can list all seven tiers (none … max), grok-4.7 lists four. The four
    // below are only a fallback for a harness build that advertises none, so
    // `mergeAcpCapabilities` lets the live list win. `max` is here because
    // bridged models (marionette/*) and grok config.toml [model.*] entries
    // carry no _meta menu at all — without it those models lose their top tier
    // even when the upstream accepts it (the Rust side passes unknown levels
    // through verbatim for menu-less models).
    effortOptions: [
      { id: "max", label: "Max" },
      { id: "high", label: "High" },
      { id: "medium", label: "Medium" },
      { id: "low", label: "Low" },
    ],
    effortViaLegacyModel: true,
    // Grok sends no current_mode_update at session start, so this default is the
    // only thing the chip can show. Execute id is `default` (Grok 1.0.89).
    defaultMode: "default",
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
    // Wire ids from @agentclientprotocol/codex-acp AgentMode (not the old
    // approval-only labels). Full access = danger-full-access + approval never.
    modes: [
      { id: "read-only", label: "Read-only" },
      { id: "agent", label: "Agent" },
      { id: "agent-full-access", label: "Agent (full access)" },
    ],
    defaultMode: "agent",
    // Codex ACP 2.x often encodes effort in the model id (`gpt-5.6-luna[xhigh]`)
    // and, when the catalog is empty, advertises no `reasoning_effort` option.
    // Rust peels the suffix; this list fills the chip. A live
    // `reasoning_effort` config still wins (see mergeAcpCapabilities).
    effortOptions: [
      { id: "max", label: "Max" },
      { id: "xhigh", label: "Extra High" },
      { id: "high", label: "High" },
      { id: "medium", label: "Medium" },
      { id: "low", label: "Low" },
    ],
    effortViaLegacyModel: true,
    defaultEffortId: "medium",
    // Only `mode` maps to approval+sandbox preset. Do not send agent-full-access
    // as approval-policy — that id expects never|on-request|… and will fail.
    modeConfigIds: ["mode"],
  },
};

export function getAcpSupplement(agentId: string): AcpSupplement | null {
  return SUPPLEMENTS[agentId] ?? null;
}

/**
 * Map legacy / display mode ids onto the wire ids each harness accepts.
 * Codex once advertised approval-only labels (suggest/full-auto); live
 * codex-acp uses read-only | agent | agent-full-access.
 */
export function normalizeAgentModeId(agentId: string, modeId: string): string {
  const key = modeId.trim().toLowerCase();
  if (agentId === "grok-build" || agentId === "grok") {
    // Grok 1.0.89 execute mode is `default`. Older CLIs and Marionette disk
    // prefs / OpenCode handoff still say `build`.
    if (key === "build") return "default";
    return modeId;
  }
  if (agentId !== "codex" && agentId !== "codex-acp") return modeId;
  const legacy: Record<string, string> = {
    default: "agent",
    suggest: "read-only",
    "auto-edit": "agent",
    autoedit: "agent",
    "full-auto": "agent-full-access",
    fullauto: "agent-full-access",
    "full-access": "agent-full-access",
    "full access": "agent-full-access",
    "agent (full access)": "agent-full-access",
    "read only": "read-only",
    readonly: "read-only",
  };
  return legacy[key] ?? modeId;
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
    permissionOptions: [],
    permissionConfigId: null,
    currentPermission: null,
    permissionLabel: null,
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
    sup.effortViaLegacyModel === true &&
    (sup.effortOptions?.length ?? 0) > 0 &&
    !base.effortConfigId;

  const effortOptions = legacyEffort
    ? (base.effortOptions?.length ?? 0) > 0
      ? base.effortOptions
      : sup.effortOptions ?? []
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
      ?? options[0]!.id;
  }
  if (n >= 0.8) {
    return options.find((o) => /max|xhigh|ultra/.test(o.id.toLowerCase()))?.id
      ?? options.find((o) => /high/.test(o.id.toLowerCase()))?.id
      ?? options[options.length - 1]!.id;
  }
  if (n >= 0.55) {
    return options.find((o) => o.id.toLowerCase() === "high")?.id
      ?? options.find((o) => /medium|mid|default/.test(o.id.toLowerCase()))?.id
      ?? options[Math.floor(options.length / 2)]!.id;
  }
  // mid-low → default or medium
  if (ids.includes("default")) return options.find((o) => o.id.toLowerCase() === "default")!.id;
  if (ids.includes("medium")) return options.find((o) => o.id.toLowerCase() === "medium")!.id;
  return options[Math.floor(options.length / 2)]?.id ?? options[0]!.id;
}

/**
 * Decide whether Composer must (re)assert a model so the agent republishes
 * model-dependent options such as Effort.
 *
 * OpenCode's `session/new` can already sit on e.g. `opencode-go/deepseek-v4.1-flash`
 * while omitting the `effort` config option (same class of bug as
 * anomalyco/opencode#31961 in Zed). Marionette used to skip `set_config` when
 * the preferred model already matched `currentModel`, so Effort never appeared
 * until the user switched away and back. Re-asserting the same model forces a
 * full `configOptions` echo.
 *
 * Returns null when nothing needs to be sent.
 */
export function modelAssertForEffortRefresh(
  caps: Pick<
    CapabilitySnapshot,
    "currentModel" | "effortConfigId" | "effortOptions"
  >,
  preferredModel?: string | null,
): { model: string; reason: "switch" | "republish" } | null {
  const pref = preferredModel?.trim() || null;
  const current = caps.currentModel?.trim() || null;
  const hasEffort =
    Boolean(caps.effortConfigId) || (caps.effortOptions?.length ?? 0) > 0;

  if (pref && pref !== current) {
    return { model: pref, reason: "switch" };
  }

  const target = pref || current;
  if (target && !hasEffort) {
    return { model: target, reason: "republish" };
  }

  return null;
}

/**
 * A saved effort id outlives the model it was picked for: "max" comes from grok
 * or a codex model, then the user switches to an opencode / pi model whose live
 * `thought`/`effort` menu stops at `high`. Pushing "max" gets
 * `-32602 effort not found: max`, the whole pre-prompt sync fails, and the chip
 * keeps advertising a level the agent never applied (2026-11-27 doctor line).
 *
 * Clamp to the advertised menu: exact id, then case-insensitive, then the same
 * rung on a known scale (max/high/medium/low), then whatever the agent already
 * reports, then the first option. An EMPTY menu means the agent advertised no
 * levels at all (Claude / Codex / menu-less bridge models) - the value is sent
 * verbatim there, because those harnesses validate it themselves.
 */
export function clampEffortIdToLiveMenu(
  effortId: string,
  caps: Pick<CapabilitySnapshot, "effortOptions" | "currentEffortId"> | null,
): string {
  const options = caps?.effortOptions ?? [];
  if (options.length === 0) return effortId;
  const wanted = effortId.trim();
  const exact = options.find((option) => option.id === wanted);
  if (exact) return exact.id;
  const lower = wanted.toLowerCase();
  const ci = options.find((option) => option.id.trim().toLowerCase() === lower);
  if (ci) return ci.id;

  const scale = ["none", "off", "minimal", "low", "medium", "high", "xhigh", "max"];
  const target = scale.indexOf(lower);
  if (target >= 0) {
    let best: { id: string; distance: number } | null = null;
    for (const option of options) {
      const rank = scale.indexOf(option.id.trim().toLowerCase());
      if (rank < 0) continue;
      const distance = Math.abs(rank - target);
      // Ties go to the stronger level, which is what a user asking for a
      // top tier means when the exact rung is missing.
      if (!best || distance < best.distance) best = { id: option.id, distance };
      else if (distance === best.distance && rank > scale.indexOf(best.id.trim().toLowerCase())) {
        best = { id: option.id, distance };
      }
    }
    if (best) return best.id;
  }

  const current = caps?.currentEffortId?.trim();
  if (current) {
    const held = options.find((option) => option.id === current);
    if (held) return held.id;
  }
  return options[0]?.id ?? effortId;
}

/** Expand a UI patch into one or more ACP set_config payloads to try. */
export function expandAcpConfigAttempts(
  agentId: string,
  patch: Record<string, unknown>,
  caps: CapabilitySnapshot | null,
): Record<string, unknown>[] {
  const sup = getAcpSupplement(agentId);
  const attempts: Record<string, unknown>[] = [];

  // Grok: there is no config id for model; hand Rust the logical knob and let
  // `update_session` fall back from set_config_option to session/set_model.
  if (agentId === "grok-build") {
    if (typeof patch.model === "string") {
      return [{ configId: "model", value: patch.model }];
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
    const modeValue = normalizeAgentModeId(agentId, patch.mode);
    const ids = [
      caps?.modeConfigId,
      ...(sup?.modeConfigIds ?? []),
      "mode",
    ].filter(Boolean) as string[];
    for (const id of [...new Set(ids)]) {
      attempts.push({ configId: id, value: modeValue });
    }
  }

  // File permission / sandbox. Only the id the agent actually advertised is ever
  // sent: unlike model/mode there is no "model" default worth guessing, and a
  // fabricated id would come back -32602 with the user staring at a chip that
  // flipped for nothing. No permission option advertised → no attempt → the
  // Composer reports it instead of pretending the level changed.
  if (typeof patch.permission === "string" && caps?.permissionConfigId) {
    attempts.push({ configId: caps.permissionConfigId, value: patch.permission });
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
    attempts.push({
      configId: caps.effortConfigId,
      value: clampEffortIdToLiveMenu(patch.effortId, caps),
    });
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
