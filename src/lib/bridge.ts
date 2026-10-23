import {
  bridgeListModels,
  type BridgeModel,
  type BridgeModelSource,
  type BridgeModelsResult,
} from "./api";

/**
 * Cross-use (跨用) model catalog cache for the Composer picker.
 *
 * The list comes from Marionette-side credentials, not from the agent's ACP
 * advertisement, so it does not need to be re-fetched per dialog switch. A
 * module-level per-agent cache keeps the picker cheap; the credentials page
 * calls {@link invalidateBridgeModels} after a key changes so the next mount
 * sees fresh data. Stale data in an already-open dialog is accepted (M1).
 */

type CacheEntry = {
  models: BridgeModel[];
  sources: BridgeModelSource[] | null;
  note: string | null;
  savedAt: number;
};

const cache = new Map<string, CacheEntry>();

/** Cross-use catalog for one agent (cached after the first call). */
export async function getBridgeModels(agentId: string): Promise<BridgeModelsResult> {
  const hit = cache.get(agentId);
  if (hit && Date.now() - hit.savedAt < 15_000) return { models: hit.models, sources: hit.sources, note: hit.note };
  const result = await bridgeListModels(agentId);
  cache.set(agentId, {
    models: result.models,
    sources: result.sources ?? null,
    note: result.note,
    savedAt: Date.now(),
  });
  return result;
}

/** Drop cached catalogs (all agents, or one) — called after credential changes. */
export function invalidateBridgeModels(agentId?: string): void {
  if (agentId) cache.delete(agentId);
  else cache.clear();
}

/**
 * Backend preset catalog for manual key entry. Fixed set (M1) — the Rust side
 * owns the endpoint mapping, this only feeds the settings dropdown.
 */
export const BRIDGE_PRESETS = [
  { id: "deepseek", label: "DeepSeek" },
  { id: "opencode-go", label: "OpenCode Go" },
  { id: "glm", label: "GLM" },
  { id: "kimi", label: "Kimi" },
  { id: "openrouter", label: "OpenRouter" },
  { id: "xai", label: "xAI" },
] as const;

/** Human label for a preset id (falls back to the raw id). */
export function bridgePresetLabel(preset: string): string {
  return BRIDGE_PRESETS.find((p) => p.id === preset)?.label ?? preset;
}

/** Shared-model group header in the model picker: 「共享 · OPENCODE GO」. */
export function bridgeGroupTitle(preset: string): string {
  return `共享 · ${bridgePresetLabel(preset).toUpperCase()}`;
}

/** Same header for a source descriptor (label comes from the entry itself). */
export function bridgeSourceGroupTitle(label: string): string {
  return `共享 · ${label.toUpperCase()}`;
}

/** Drop the group-title prefix so section headers show only the source name. */
export function stripBridgeGroupPrefix(title: string): string {
  return title.replace(/^(?:共享|跨用) · /, "");
}

/**
 * Short provider labels for engine (`provider/model`) ids — used to split the
 * engine source into per-provider menu groups.
 * Subscription families read as subscriptions; unmapped prefixes show raw.
 */
const ENGINE_PROVIDER_LABELS: Record<string, string> = {
  grok: "Grok 订阅",
  codex: "ChatGPT",
  claude: "Claude 订阅",
  gemini: "Gemini 订阅",
  copilot: "Copilot",
  "opencode-go": "OpenCode Go",
  deepseek: "DeepSeek",
  glm: "GLM",
  zhipu: "GLM",
  moonshot: "Kimi",
  kimi: "Kimi",
  openrouter: "OpenRouter",
  xai: "xAI",
};

/** Friendly label for an engine provider prefix (falls back to the raw id). */
export function engineProviderLabel(provider: string): string {
  return ENGINE_PROVIDER_LABELS[provider] ?? provider;
}

/**
 * Normalized model name for cross-source dedup: engine ids are
 * `provider/model` — compare by the model part so `opencode-go/glm-5.3`
 * (engine) and `glm-5.3` (direct key) collapse to one menu entry.
 */
export function crossUseModelKey(entryId: string, modelId: string): string {
  if (entryId === "engine" && modelId.includes("/")) {
    return modelId.slice(modelId.indexOf("/") + 1);
  }
  return modelId;
}

/**
 * Group descriptors the picker builds its cross-use groups from.
 *
 * Newer backends send one source per usable credential entry (key-plan and
 * scanned references alike), so a group survives even when its model list is
 * empty — those sources are custom-id-only (e.g. openrouter). Older backends
 * do not send `sources`; derive one per preset from the models themselves,
 * which matches the pre-sources grouping exactly.
 */
export function resolveBridgeSources(result: BridgeModelsResult): BridgeModelSource[] {
  if (result.sources && result.sources.length > 0) return result.sources;
  const byPreset = new Map<string, BridgeModelSource>();
  for (const m of result.models) {
    if (!byPreset.has(m.preset)) {
      byPreset.set(m.preset, {
        entryId: m.entryId,
        preset: m.preset,
        label: bridgePresetLabel(m.preset),
        fromScan: false,
      });
    }
  }
  return [...byPreset.values()];
}

/**
 * Preset id for one source entry — sources first, then any catalog model of
 * the same entry. Used to label custom model ids the catalog cannot resolve
 * (restore path + inline custom-model confirm); "" when nothing matches.
 */
export function bridgePresetFor(result: BridgeModelsResult, entryId: string): string {
  const source = result.sources?.find((s) => s.entryId === entryId);
  if (source) return source.preset;
  return result.models.find((m) => m.entryId === entryId)?.preset ?? "";
}
