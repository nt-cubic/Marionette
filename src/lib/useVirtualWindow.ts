import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type RefObject } from "react";
import { sizeMapForKey } from "./virtualSizeCache";

const DEFAULT_ESTIMATE = 128;
const DEFAULT_OVERSCAN = 8;

/**
 * Variable-height virtual window over a vertical scroll container.
 * Data stays full-length; only the mount range changes.
 */
export function useVirtualWindow(
  scrollRef: RefObject<HTMLElement | null>,
  itemCount: number,
  opts?: { estimate?: number; overscan?: number; /** Force remeasure when content identity changes */ resetKey?: string }
) {
  const estimate = opts?.estimate ?? DEFAULT_ESTIMATE;
  const overscan = opts?.overscan ?? DEFAULT_OVERSCAN;
  const resetKey = opts?.resetKey ?? "";

  const sizeMapRef = useRef<Map<number, number>>(sizeMapForKey(resetKey));
  const appliedKeyRef = useRef(resetKey);
  if (appliedKeyRef.current !== resetKey) {
    appliedKeyRef.current = resetKey;
    sizeMapRef.current = sizeMapForKey(resetKey);
  }
  const [scrollTop, setScrollTop] = useState(0);
  const [viewportH, setViewportH] = useState(600);
  // 前缀和重算起点：measure 写入的最小下标（itemCount 干净时为 itemCount，
  // 表示只有尾部增长）。全量重算只在初始/reset/forceRemeasure 时发生。
  const [rebuildFrom, setRebuildFrom] = useState(0);

  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (!el) return;

    const onScroll = () => setScrollTop(el.scrollTop);
    const onResize = () => setViewportH(el.clientHeight || 600);

    onScroll();
    onResize();
    el.addEventListener("scroll", onScroll, { passive: true });
    const ro =
      typeof ResizeObserver !== "undefined"
        ? new ResizeObserver(onResize)
        : null;
    ro?.observe(el);
    window.addEventListener("resize", onResize);
    return () => {
      el.removeEventListener("scroll", onScroll);
      ro?.disconnect();
      window.removeEventListener("resize", onResize);
    };
  }, [scrollRef]);

  const getSize = useCallback(
    (index: number) => sizeMapRef.current.get(index) ?? estimate,
    [estimate]
  );

  // 增量前缀和：流式期间 itemCount 持续增长、measure 偶发改写个别下标，
  // 旧实现每次 version bump 都全量重算 O(n)。现在只在「最早变更下标」之后
  // 重算，缓存里 0..from 的前缀直接复用（流式尾部增长 = O(新增)）。
  const offsetsCacheRef = useRef<{ key: string; offsets: number[] } | null>(null);
  const { offsets, totalSize } = useMemo(() => {
    const prev = offsetsCacheRef.current;
    const cached = prev && prev.key === resetKey ? prev : null;
    const baseLen = cached ? cached.offsets.length - 1 : 0;
    const from = Math.min(rebuildFrom, itemCount, baseLen);
    const offsets = (cached ? cached.offsets : [0]).slice(0, from + 1);
    let acc = offsets[offsets.length - 1] ?? 0;
    for (let i = offsets.length - 1; i < itemCount; i += 1) {
      acc += getSize(i);
      offsets[i + 1] = acc;
    }
    offsetsCacheRef.current = { key: resetKey, offsets };
    return { offsets, totalSize: acc };
  }, [getSize, itemCount, rebuildFrom, resetKey]);

  // 重算完成后把起点重置为 itemCount（干净态），额外渲染一次但 O(1)。
  useEffect(() => {
    if (rebuildFrom < itemCount) setRebuildFrom(itemCount);
  }, [itemCount, rebuildFrom]);

  const { start, end } = useMemo(() => {
    if (itemCount === 0) return { start: 0, end: 0 };
    const viewStart = scrollTop;
    const viewEnd = scrollTop + viewportH;

    // Binary search first index whose bottom > viewStart
    let lo = 0;
    let hi = itemCount - 1;
    let first = 0;
    while (lo <= hi) {
      const mid = (lo + hi) >> 1;
      if (offsets[mid + 1]! > viewStart) {
        first = mid;
        hi = mid - 1;
      } else {
        lo = mid + 1;
      }
    }

    lo = first;
    hi = itemCount - 1;
    let last = itemCount - 1;
    while (lo <= hi) {
      const mid = (lo + hi) >> 1;
      if (offsets[mid]! < viewEnd) {
        last = mid;
        lo = mid + 1;
      } else {
        hi = mid - 1;
      }
    }

    const start = Math.max(0, first - overscan);
    const end = Math.min(itemCount, last + 1 + overscan);
    return { start, end };
  }, [itemCount, offsets, overscan, scrollTop, viewportH]);

  // Batch height writes into one rebuild per frame — unbatched measure
  // during streaming used to thrash layout / flicker the WebView.
  const measureRafRef = useRef(0);
  const measureDirtyRef = useRef(false);
  const measureMinChangedRef = useRef(Number.MAX_SAFE_INTEGER);
  const measureRef = useCallback((index: number, node: HTMLElement | null) => {
    if (!node) return;
    const h = node.getBoundingClientRect().height;
    if (!Number.isFinite(h) || h <= 0) return;
    const prev = sizeMapRef.current.get(index);
    // Ignore sub-pixel noise
    if (prev != null && Math.abs(prev - h) < 2) return;
    sizeMapRef.current.set(index, h);
    measureMinChangedRef.current = Math.min(measureMinChangedRef.current, index);
    measureDirtyRef.current = true;
    if (measureRafRef.current) return;
    measureRafRef.current = requestAnimationFrame(() => {
      measureRafRef.current = 0;
      if (!measureDirtyRef.current) return;
      measureDirtyRef.current = false;
      const from = measureMinChangedRef.current;
      measureMinChangedRef.current = Number.MAX_SAFE_INTEGER;
      setRebuildFrom((current) => Math.min(current, from));
    });
  }, []);

  // Cancel a pending measure frame on unmount — a fired callback on a
  // detached list would call setState on an unmounted hook.
  useEffect(() => {
    return () => {
      if (measureRafRef.current) {
        cancelAnimationFrame(measureRafRef.current);
        measureRafRef.current = 0;
      }
    };
  }, []);

  const paddingTop = offsets[start] ?? 0;
  const paddingBottom = Math.max(0, totalSize - (offsets[end] ?? totalSize));

  const offsetOf = useCallback(
    (index: number) => offsets[Math.max(0, Math.min(index, itemCount))] ?? 0,
    [itemCount, offsets]
  );

  const syncFromDom = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;
    setScrollTop(el.scrollTop);
    setViewportH(el.clientHeight || 600);
  }, [scrollRef]);

  return {
    start,
    end,
    paddingTop,
    paddingBottom,
    totalSize,
    measureRef,
    offsetOf,
    /** After a tab restore writes scrollTop, pull it into the window state. */
    syncFromDom,
    /** Call after stick-to-bottom when sizes may have changed (full recompute). */
    forceRemeasure: () => setRebuildFrom(0),
  };
}
