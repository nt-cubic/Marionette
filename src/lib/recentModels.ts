const STORAGE_KEY = "agentshell-recent-models";
const MAX_ENTRIES = 5;

type RecentModelEntry = {
  modelId: string;
  lastUsedAt: number;
};

/** Record a model usage (push to front, deduplicate, trim to MAX_ENTRIES). */
export function recordModelUsage(modelId: string): void {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    const entries: RecentModelEntry[] = raw ? JSON.parse(raw) : [];
    const filtered = entries.filter((e) => e.modelId !== modelId);
    filtered.unshift({ modelId, lastUsedAt: Date.now() });
    localStorage.setItem(STORAGE_KEY, JSON.stringify(filtered.slice(0, MAX_ENTRIES)));
  } catch {
    // localStorage might be disabled or full
  }
}

/** Get recent models that are still in the valid set, sorted by recency. */
export function getRecentModels(
  validModelIds: ReadonlySet<string>,
): { modelId: string; lastUsedAt: number }[] {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return [];
    const entries: RecentModelEntry[] = JSON.parse(raw);
    return entries.filter((e) => validModelIds.has(e.modelId));
  } catch {
    return [];
  }
}
