const STORAGE_KEY = "marionette-recent-models";
const MAX_ENTRIES = 5;

type RecentModelEntry = {
  modelId: string;
  lastUsedAt: number;
};

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
