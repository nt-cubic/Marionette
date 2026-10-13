const SIZE_CACHE_LIMIT = 24;

/**
 * Per-transcript measured heights. Switching tabs used to wipe the map,
 * which collapsed the list to 48px estimates and left a restored "bottom"
 * pin stranded near the real top after remeasure.
 */
const sizeCache = new Map<string, Map<number, number>>();

export function sizeMapForKey(key: string): Map<number, number> {
  const existing = sizeCache.get(key);
  if (existing) {
    sizeCache.delete(key);
    sizeCache.set(key, existing);
    return existing;
  }
  const next = new Map<number, number>();
  sizeCache.set(key, next);
  while (sizeCache.size > SIZE_CACHE_LIMIT) {
    const oldest = sizeCache.keys().next().value;
    if (oldest == null) break;
    sizeCache.delete(oldest);
  }
  return next;
}

/** Test helper — not used by the app. */
export function resetVirtualSizeCache() {
  sizeCache.clear();
}
