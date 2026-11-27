/**
 * Cross-use bindings store the engine-side model id. Agents that expose the
 * bridge through their own ACP catalog wrap that id in a provider-specific
 * prefix. Keep both forms explicit: UI/card labels may use the engine id, but
 * `session/set_config_option` must receive the agent-native wire id.
 */
export function nativeBridgeModelId(agentId: string, modelId: string): string {
  if (agentId === "deepseek" || agentId === "deepseek-acp") {
    return `marionette::${modelId}`;
  }
  if (agentId === "zcode" || agentId === "zcode-acp") {
    return `marionette\\${modelId}`;
  }
  return `marionette/${modelId}`;
}

const NATIVE_BRIDGE_AGENTS = new Set([
  "grok-build",
  "grok",
  "opencode",
  "deepseek",
  "deepseek-acp",
  "pi",
  "pi-acp",
  "zcode",
  "zcode-acp",
]);

/** Agents whose ACP model catalog contains the injected `marionette` provider. */
export function reportsNativeBridgeModel(agentId: string): boolean {
  return NATIVE_BRIDGE_AGENTS.has(agentId);
}

/** Wire id to use when this cross-use binding is submitted for a prompt. */
export function bridgeSubmissionModelId(agentId: string, modelId: string): string {
  return reportsNativeBridgeModel(agentId)
    ? nativeBridgeModelId(agentId, modelId)
    : modelId;
}

/**
 * Agents that carry the whole engine catalog in their own live model list
 * (opencode / pi / dsh / zcode) switch with a plain `set_config_option`.
 *
 * The rest inject the pick into the *process* (grok owned TOML table, claude
 * env, codex providers/set, antigravity gateway), so only a new process can
 * carry them. Restarting one of the catalog-native agents is actively harmful:
 * the fresh process comes up on the harness default (OpenCode's
 * `opencode/seaglass-alpha-free`) and nothing re-asserts the pick until the
 * next prompt â the chip then reads "Seaglass Alpha Free" no matter how many
 * times the same shared model is clicked (2026-11-27 user report).
 */
const NATIVE_BRIDGE_SWITCH_AGENTS = new Set([
  "opencode",
  "deepseek",
  "deepseek-acp",
  "pi",
  "pi-acp",
  "zcode",
  "zcode-acp",
]);

export function bridgeSwitchNeedsRestart(agentId: string): boolean {
  return !NATIVE_BRIDGE_SWITCH_AGENTS.has(agentId);
}

/**
 * A bridge chip must not cover a different live model with the selected label.
 * Claude/Codex inject indirectly and may keep a native model id, so their live
 * id is not expected to equal the bridge id. OpenCode/DSH/Pi/ZCode/Grok do
 * advertise `marionette…` and therefore can be checked exactly.
 */
export function bridgeBindingIsStale(args: {
  agentId: string;
  binding: { modelId: string } | null | undefined;
  capsLive: boolean;
  currentModel: string | null | undefined;
  models: ReadonlyArray<{ id: string }>;
}): boolean {
  const { agentId, binding, capsLive, currentModel, models } = args;
  if (!binding || !capsLive) return false;

  const expected = nativeBridgeModelId(agentId, binding.modelId);
  const live = currentModel?.trim() || null;
  // A bridge-prefixed live id is always conclusive, regardless of agent shape.
  if (
    live &&
    (live.startsWith("marionette/") ||
      live.startsWith("marionette::") ||
      live.startsWith("marionette\\"))
  ) {
    return live !== expected;
  }
  if (!reportsNativeBridgeModel(agentId)) return false;

  // The provider can be absent entirely when spawn-time injection did not take
  // effect. A live catalog is authoritative in that case.
  if (models.length > 0 && !models.some((model) => model.id === expected)) {
    return true;
  }
  return live != null && live !== expected;
}
