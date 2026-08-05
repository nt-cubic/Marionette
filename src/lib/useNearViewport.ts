import { useEffect, useState, type RefObject } from "react";

/**
 * True when `target` is within the scroll root (or viewport) expanded by
 * `rootMargin`. Used to mount heavy subtrees only near the camera.
 */
export function useNearViewport(
  targetRef: RefObject<Element | null>,
  rootRef?: RefObject<Element | null> | null,
  rootMargin = "900px 0px",
  /** false = unload far media by default until IO confirms. */
  initialNear = false
): boolean {
  const [near, setNear] = useState(initialNear);

  useEffect(() => {
    const target = targetRef.current;
    if (!target || typeof IntersectionObserver === "undefined") {
      setNear(true);
      return;
    }
    const root = rootRef?.current ?? null;
    const io = new IntersectionObserver(
      (entries) => {
        const hit = entries.some((e) => e.isIntersecting);
        setNear(hit);
      },
      { root, rootMargin, threshold: 0 }
    );
    io.observe(target);
    return () => io.disconnect();
  }, [targetRef, rootRef, rootMargin]);

  return near;
}
