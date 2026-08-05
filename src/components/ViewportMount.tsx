import { useRef, type ReactNode, type RefObject } from "react";
import { useNearViewport } from "../lib/useNearViewport";

type ViewportMountProps = {
  /** Scroll container (event-list). Null = viewport. */
  rootRef?: RefObject<Element | null> | null;
  /** Min height while unloaded so scroll position stays stable-ish. */
  minHeight?: number;
  /** Extra rootMargin for pre-mount (Twitter-style overscan). */
  rootMargin?: string;
  /** Lightweight stand-in when far from the camera. */
  placeholder?: ReactNode;
  children: ReactNode;
  className?: string;
};

/**
 * Mounts children only when near the scroll camera.
 * Far rows keep a cheap placeholder (media / markdown not in the tree).
 */
export function ViewportMount({
  rootRef = null,
  minHeight = 72,
  rootMargin = "1000px 0px",
  placeholder,
  children,
  className,
}: ViewportMountProps) {
  const ref = useRef<HTMLDivElement>(null);
  const near = useNearViewport(ref, rootRef, rootMargin);

  return (
    <div
      ref={ref}
      className={className}
      style={near ? undefined : { minHeight }}
      data-viewport={near ? "near" : "far"}
    >
      {near
        ? children
        : placeholder ?? (
            <div className="viewport-mount__shell" aria-hidden>
              …
            </div>
          )}
    </div>
  );
}
