/**
 * imageSupport — can the current model take images? Three states, never guessed.
 *
 *   yes      the model catalog says it takes images, and the agent does not refuse them
 *   no       the agent said it does not take images (ACP `promptCapabilities.image: false`),
 *            or the model catalog explicitly says text-only
 *   unknown  nobody told us — the UI says nothing and dims nothing
 *
 * Sources, in the order they are consulted (see AGENTS.md「看图能力」):
 *   1. agent level — ACP initialize `agentCapabilities.promptCapabilities.image`
 *      (Rust → `SessionCapabilities.agentPromptImage`)
 *   2. model level — the agent's own `availableModels[]` entry (`SessionCapabilities.modelImages`,
 *      best effort: ACP has no standard field) and, for bridged `marionette/*` models, the
 *      engine model catalog's `modalities.input` (`engine_model_images`)
 *
 * There is deliberately no model-name table in here.
 */

export type ImageSupport = "yes" | "no" | "unknown";

export type ImageSupportInput = {
  /** ACP `promptCapabilities.image`; null/undefined = the agent did not say. */
  agentPromptImage?: boolean | null;
  /** Current model id as the session reports it. */
  modelId?: string | null;
  /** Per-model flags from the agent's own model catalog (id → accepts images). */
  modelImages?: Readonly<Record<string, boolean>> | null;
  /** Per-model flags from the engine catalog (`provider/model` → accepts images). Only entries that have metadata. */
  engineImages?: Readonly<Record<string, boolean>> | null;
};

/** `marionette/p/m`, `marionette::p/m`, `marionette\p/m` → `p/m`; anything else → null. */
export function bridgeCatalogId(modelId: string | null | undefined): string | null {
  const id = modelId?.trim() ?? "";
  const rest = /^marionette(?:::|[/\\])(.+)$/.exec(id)?.[1];
  return rest ? rest.replace(/\\/g, "/") : null;
}

function lookup(map: Readonly<Record<string, boolean>> | null | undefined, id: string): boolean | null {
  if (!map) return null;
  const direct = map[id];
  if (typeof direct === "boolean") return direct;
  const lower = id.toLowerCase();
  for (const [key, value] of Object.entries(map)) {
    if (key.toLowerCase() === lower && typeof value === "boolean") return value;
  }
  return null;
}

export function resolveImageSupport(input: ImageSupportInput): ImageSupport {
  if (input.agentPromptImage === false) return "no";
  const id = input.modelId?.trim() ?? "";
  let model: boolean | null = null;
  if (id) {
    model = lookup(input.modelImages, id);
    if (model == null) {
      const bridged = bridgeCatalogId(id);
      if (bridged) model = lookup(input.engineImages, bridged);
    }
  }
  if (model === false) return "no";
  if (model === true) return "yes";
  return "unknown";
}
