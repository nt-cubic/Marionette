import { useCallback, useLayoutEffect, useMemo, useRef, useState, type RefObject } from "react";

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

  const sizeMapRef = useRef<Map<number, number>>(new Map());
  const [scrollTop, setScrollTop] = useState(0);
  const [viewportH, setViewportH] = useState(600);
  const [version, setVersion] = useState(0);

  // Drop measured heights when the transcript identity changes.
  useLayoutEffect(() => {
    sizeMapRef.current = new Map();
    setVersion((v) => v + 1);
  }, [resetKey, itemCount > 0 ? Math.floor(itemCount / 50) : 0]);

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

  const { offsets, totalSize } = useMemo(() => {
    void version;
    const offsets: number[] = new Array(itemCount + 1);
    offsets[0] = 0;
    let acc = 0;
    for (let i = 0; i < itemCount; i += 1) {
      acc += getSize(i);
      offsets[i + 1] = acc;
    }
    return { offsets, totalSize: acc };
  }, [getSize, itemCount, version]);

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
      if (offsets[mid + 1] > viewStart) {
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
      if (offsets[mid] < viewEnd) {
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

  // Batch height writes into one setVersion per frame — unbatched measure
  // during streaming used to thrash layout / flicker the WebView.
  const measureRafRef = useRef(0);
  const measureDirtyRef = useRef(false);
  const measureRef = useCallback((index: number, node: HTMLElement | null) => {
    if (!node) return;
    const h = node.getBoundingClientRect().height;
    if (!Number.isFinite(h) || h <= 0) return;
    const prev = sizeMapRef.current.get(index);
    // Ignore sub-pixel noise
    if (prev != null && Math.abs(prev - h) < 2) return;
    sizeMapRef.current.set(index, h);
    measureDirtyRef.current = true;
    if (measureRafRef.current) return;
    measureRafRef.current = requestAnimationFrame(() => {
      measureRafRef.current = 0;
      if (!measureDirtyRef.current) return;
      measureDirtyRef.current = false;
      setVersion((v) => v + 1);
    });
  }, []);

  const paddingTop = offsets[start] ?? 0;
  const paddingBottom = Math.max(0, totalSize - (offsets[end] ?? totalSize));

  const offsetOf = useCallback(
    (index: number) => offsets[Math.max(0, Math.min(index, itemCount))] ?? 0,
    [itemCount, offsets]
  );

  return {
    start,
    end,
    paddingTop,
    paddingBottom,
    totalSize,
    measureRef,
    offsetOf,
    /** Call after stick-to-bottom when sizes may have changed. */
    forceRemeasure: () => setVersion((v) => v + 1),
  };
}
