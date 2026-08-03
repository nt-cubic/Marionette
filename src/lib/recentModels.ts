const STORAGE_KEY = "marionette-recent-models";
const MAX_ENTRIES = 5;

type RecentModelEntry = {
  modelId: string;
  lastUsedAt: number;
};

const DEFAULTS_KEY = "marionette-last-defaults";

/** Per-agent last-used Composer chips — applied as defaults for new sessions. */
export type LastUsedDefaults = {
  modelId: string | null;
  modeId: string | null;
  effortId: string | null;
  effort: number | null;
  /** Grok `/always-approve` (and similar). */
  alwaysApprove: boolean | null;
  lastUsedAt: number;
};

type DefaultsMap = Record<string, LastUsedDefaults>;

/**
 * Remember what the user last used for an agent (model / mode / effort). Each
 * patch only overrides the fields it carries; absent fields keep their old
 * value so a model switch does not wipe a saved effort.
 */
export function recordLastUsedDefaults(
  agentId: string,
  patch: {
    modelId?: string | null;
    modeId?: string | null;
    effortId?: string | null;
    effort?: number | null;
    alwaysApprove?: boolean | null;
  },
): void {
  try {
    const raw = localStorage.getItem(DEFAULTS_KEY);
    const parsed: unknown = raw ? JSON.parse(raw) : {};
    const map: DefaultsMap =
      parsed && typeof parsed === "object" ? (parsed as DefaultsMap) : {};
    const prev: Partial<LastUsedDefaults> = map[agentId] ?? {};
    map[agentId] = {
      modelId: patch.modelId !== undefined ? patch.modelId : prev.modelId ?? null,
      modeId: patch.modeId !== undefined ? patch.modeId : prev.modeId ?? null,
      effortId: patch.effortId !== undefined ? patch.effortId : prev.effortId ?? null,
      effort: patch.effort !== undefined ? patch.effort : prev.effort ?? null,
      alwaysApprove:
        patch.alwaysApprove !== undefined
          ? patch.alwaysApprove
          : prev.alwaysApprove ?? null,
      lastUsedAt: Date.now(),
    };
    localStorage.setItem(DEFAULTS_KEY, JSON.stringify(map));
  } catch {
    // localStorage might be disabled or full
  }
}

/** Last-used chips for one agent, or null when nothing was recorded yet. */
export function getLastUsedDefaults(agentId: string): LastUsedDefaults | null {
  try {
    const raw = localStorage.getItem(DEFAULTS_KEY);
    if (!raw) return null;
    const parsed: unknown = JSON.parse(raw);
    if (!parsed || typeof parsed !== "object") return null;
    const rec = (parsed as DefaultsMap)[agentId];
    if (!rec || typeof rec !== "object") return null;
    return {
      modelId: typeof rec.modelId === "string" ? rec.modelId : null,
      modeId: typeof rec.modeId === "string" ? rec.modeId : null,
      effortId: typeof rec.effortId === "string" ? rec.effortId : null,
      effort: typeof rec.effort === "number" ? rec.effort : null,
      alwaysApprove: typeof rec.alwaysApprove === "boolean" ? rec.alwaysApprove : null,
      lastUsedAt: typeof rec.lastUsedAt === "number" ? rec.lastUsedAt : 0,
    };
  } catch {
    return null;
  }
}

/** Record a model usage (push to front, deduplicate, trim to MAX_ENTRIES). */
export function recordModelUsage(modelId: string): void {
  try {
    const raw =
      localStorage.getItem(STORAGE_KEY) ?? localStorage.getItem("agentshell-recent-models");
    const parsed: unknown = raw ? JSON.parse(raw) : [];
    // Start fresh on a corrupt value instead of throwing into the catch below,
    // which would leave the bad entry in place and stop recording for good.
    const entries: RecentModelEntry[] = Array.isArray(parsed) ? parsed : [];
    const filtered = entries.filter((e) => e?.modelId !== modelId);
    filtered.unshift({ modelId, lastUsedAt: Date.now() });
    localStorage.setItem(STORAGE_KEY, JSON.stringify(filtered.slice(0, MAX_ENTRIES)));
  } catch {
    // localStorage might be disabled or full
  }
}

/**
 * Recent models that still exist in the agent's current catalogue, newest first.
 *
 * Sorted explicitly rather than trusting the stored order: the array is written
 * front-first, but a hand-edited or older-format `localStorage` value would
 * otherwise render in whatever order it happened to have.
 */
export function getRecentModels(
  validModelIds: ReadonlySet<string>,
): RecentModelEntry[] {
  try {
    const raw =
      localStorage.getItem(STORAGE_KEY) ?? localStorage.getItem("agentshell-recent-models");
    if (!raw) return [];
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed
      .filter(
        (e): e is RecentModelEntry =>
          typeof e?.modelId === "string" &&
          typeof e?.lastUsedAt === "number" &&
          validModelIds.has(e.modelId),
      )
      .sort((a, b) => b.lastUsedAt - a.lastUsedAt)
      .slice(0, MAX_ENTRIES);
  } catch {
    return [];
  }
}
