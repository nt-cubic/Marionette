import { useLayoutEffect, useRef, useState, type ReactNode } from "react";

type ExpandableBodyProps = {
  children: ReactNode;
  /** Height (px) a closed body keeps; only taller content gets the toggle. */
  maxHeight?: number;
};

/**
 * Long pasted messages fold to a few lines under a soft fade, with 展开全文 /
 * 收起. The toggle only appears when the content really overflows, and the
 * virtual list re-measures the row through its own ResizeObserver.
 */
export function ExpandableBody({ children, maxHeight = 200 }: ExpandableBodyProps) {
  const innerRef = useRef<HTMLDivElement>(null);
  const [overflows, setOverflows] = useState(false);
  const [open, setOpen] = useState(false);

  useLayoutEffect(() => {
    const el = innerRef.current;
    if (!el) return;
    const measure = () => setOverflows(el.scrollHeight > maxHeight + 24);
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, [maxHeight]);

  const folded = overflows && !open;
  return (
    <div className={`expandable-body${folded ? " is-folded" : ""}`}>
      <div className="expandable-body__inner" style={folded ? { maxHeight } : undefined}>
        <div ref={innerRef}>{children}</div>
      </div>
      {overflows && (
        <button type="button" className="expandable-body__toggle" onClick={() => setOpen((value) => !value)}>
          {open ? "收起" : "展开全文"}
        </button>
      )}
    </div>
  );
}
