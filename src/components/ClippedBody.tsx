import { useEffect, useRef, useState, type ReactNode } from "react";

type ClippedBodyProps = {
  children: ReactNode;
  /** Fixed max height of the clip box (px). Vertical overflow scrolls. */
  maxHeight?: number;
  className?: string;
};

/**
 * Fixed-size clip box for tool / thinking bodies.
 * Click the details summary to open; stays within card width × maxHeight.
 * Vertical overflow: mouse-wheel scroll. Horizontal: hard-clip + fade.
 */
export function ClippedBody({ children, maxHeight = 220, className }: ClippedBodyProps) {
  const [overflows, setOverflows] = useState(true);
  const contentRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const el = contentRef.current;
    if (!el) return;

    const measure = () => {
      const heightOverflow = el.scrollHeight > el.clientHeight + 1;
      const widthOverflow = el.scrollWidth > el.clientWidth + 1;
      setOverflows(heightOverflow || widthOverflow);
    };

    const raf = requestAnimationFrame(measure);
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    if (el.firstElementChild) ro.observe(el.firstElementChild);
    return () => {
      cancelAnimationFrame(raf);
      ro.disconnect();
    };
  }, [children, maxHeight]);

  return (
    <div
      className={[
        "clipped-body",
        "clipped-body--hard",
        overflows ? "is-clipped" : "",
        className ?? "",
      ]
        .filter(Boolean)
        .join(" ")}
    >
      <div
        ref={contentRef}
        className="clipped-body__content"
        style={{ maxHeight }}
      >
        {children}
      </div>
      {overflows && (
        <>
          <div className="clipped-body__fade clipped-body__fade--bottom" aria-hidden />
          <div className="clipped-body__fade clipped-body__fade--right" aria-hidden />
        </>
      )}
    </div>
  );
}
