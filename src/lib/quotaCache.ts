import type { EngineQuotaEntry } from "./api";

/**
 * Offline cache of engine subscription quotas (右栏额度卡的引擎行).
 *
 * 懒启动（2026-11-06）配套：引擎不用模型共享就不运行，`engine-state-changed=
 * absent` 也不再把行清空——上次快照保留显示并标注「引擎未运行 · 快照时间」。
 * 配额无密钥、全局单份（非 per-agent），localStorage 单 key 即可
 * （照 capabilityCache.ts 的 version/savedAt/fail-open 形状）。
 */

const KEY = "marionette-quota";
/** Bump when the entry shape changes in a breaking way. */
const VERSION = 1;
/** Forget snapshots nobody refreshed in a week (quotas reset/consumed). */
const MAX_AGE_MS = 7 * 24 * 60 * 60 * 1000;

type CacheEntry = {
  version: number;
  savedAt: number;
  entries: EngineQuotaEntry[];
};

export type CachedEngineQuota = {
  savedAt: number;
  entries: EngineQuotaEntry[];
};

/** Last known engine quota rows (null = never fetched / expired / corrupt). */
export function cachedEngineQuota(): CachedEngineQuota | null {
  try {
    const raw = window.localStorage.getItem(KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as CacheEntry;
    if (!parsed || parsed.version !== VERSION || !Array.isArray(parsed.entries)) return null;
    if (!Number.isFinite(parsed.savedAt) || Date.now() - parsed.savedAt > MAX_AGE_MS) {
      window.localStorage.removeItem(KEY);
      return null;
    }
    return { savedAt: parsed.savedAt, entries: parsed.entries };
  } catch {
    return null;
  }
}

/** Store a fresh snapshot (empty rows are not worth remembering). */
export function cacheEngineQuota(entries: EngineQuotaEntry[]): void {
  if (!entries.length) return;
  const entry: CacheEntry = { version: VERSION, savedAt: Date.now(), entries };
  try {
    window.localStorage.setItem(KEY, JSON.stringify(entry));
  } catch {
    // Quota / private mode: the UI just falls back to live-only quotas.
  }
}
