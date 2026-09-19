import { useLayoutEffect, useRef, useState, type ReactNode, type UIEvent } from "react";

type ClipEdges = { top: boolean; bottom: boolean; right: boolean };

const CLIP_NONE: ClipEdges = { top: false, bottom: false, right: false };
const NEAR_BOTTOM_PX = 24;

type ClippedBodyProps = {
  children: ReactNode;
  /** Fixed max height of the clip box (px). Vertical overflow scrolls. */
  maxHeight?: number;
  className?: string;
  /**
   * Follow the latest (bottom) content as the body grows, so a streaming tail
   * stays visible. Following stops as soon as the user scrolls away from the
   * bottom — reading back through a live body must not be interrupted — and
   * resumes once they return to the bottom.
   */
  stickToBottom?: boolean;
};

function sameClip(a: ClipEdges, b: ClipEdges): boolean {
  return a.top === b.top && a.bottom === b.bottom && a.right === b.right;
}

function atBottomOf(el: HTMLElement): boolean {
  return el.scrollHeight - el.scrollTop - el.clientHeight <= NEAR_BOTTOM_PX;
}

/**
 * Fixed-size clip box for tool / thinking bodies.
 * Click the details summary to open; stays within card width × maxHeight.
 * Vertical overflow: mouse-wheel scroll. Horizontal: hard-clip + fade.
 */
export function ClippedBody({
  children,
  maxHeight = 220,
  className,
  stickToBottom = false,
}: ClippedBodyProps) {
  const [clip, setClip] = useState<ClipEdges>(CLIP_NONE);
  const contentRef = useRef<HTMLDivElement>(null);
  const pinnedRef = useRef(true);
  const stickRef = useRef(stickToBottom);
  stickRef.current = stickToBottom;

  useLayoutEffect(() => {
    const el = contentRef.current;
    if (!el) return;

    const measure = () => {
      if (stickRef.current && pinnedRef.current && el.scrollHeight > el.clientHeight) {
        el.scrollTop = el.scrollHeight;
      }
      const heightOverflow = el.scrollHeight > el.clientHeight + 1;
      const widthOverflow = el.scrollWidth > el.clientWidth + 1;
      const next: ClipEdges = {
        top: heightOverflow && el.scrollTop > 1,
        bottom: heightOverflow && !atBottomOf(el),
        right: widthOverflow,
      };
      if (stickRef.current) {
        pinnedRef.current = !heightOverflow || atBottomOf(el);
      }
      setClip((prev) => (sameClip(prev, next) ? prev : next));
    };

    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    if (el.firstElementChild) ro.observe(el.firstElementChild);
    return () => {
      ro.disconnect();
    };
  }, [children, maxHeight, stickToBottom]);

  const onScroll = (event: UIEvent<HTMLDivElement>) => {
    const el = event.currentTarget;
    const heightOverflow = el.scrollHeight > el.clientHeight + 1;
    const widthOverflow = el.scrollWidth > el.clientWidth + 1;
    if (stickRef.current) {
      pinnedRef.current = !heightOverflow || atBottomOf(el);
    }
    const next: ClipEdges = {
      top: heightOverflow && el.scrollTop > 1,
      bottom: heightOverflow && !atBottomOf(el),
      right: widthOverflow,
    };
    setClip((prev) => (sameClip(prev, next) ? prev : next));
  };

  const overflows = clip.top || clip.bottom || clip.right;

  return (
    <div
      className={[
        "clipped-body",
        "clipped-body--hard",
        overflows ? "is-clipped" : "",
        stickToBottom ? "clipped-body--stick-bottom" : "",
        className ?? "",
      ]
        .filter(Boolean)
        .join(" ")}
    >
      <div
        ref={contentRef}
        className="clipped-body__content custom-scrollbar scrollbar-autohide"
        style={{ maxHeight }}
        onScroll={onScroll}
      >
        {children}
      </div>
      {clip.top && <div className="clipped-body__fade clipped-body__fade--top" aria-hidden />}
      {clip.bottom && <div className="clipped-body__fade clipped-body__fade--bottom" aria-hidden />}
      {clip.right && <div className="clipped-body__fade clipped-body__fade--right" aria-hidden />}
    </div>
  );
}
