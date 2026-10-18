import { useRef } from "react";
import {
  DEFAULT_SPLIT_RATIO,
  MAX_SPLIT_RATIO,
  MIN_SPLIT_RATIO,
  type SplitAxis,
} from "../lib/splitWorkspace";

function clampDragRatio(ratio: number): number {
  if (!Number.isFinite(ratio)) return DEFAULT_SPLIT_RATIO;
  return Math.min(MAX_SPLIT_RATIO, Math.max(MIN_SPLIT_RATIO, ratio));
}

export function PaneSplitter({
  axis,
  splitId,
  onCommit,
}: {
  axis: SplitAxis;
  splitId: string;
  onCommit: (splitId: string, ratio: number) => void;
}) {
  const dragRef = useRef<{
    pointerId: number;
    startRatio: number;
    branch: HTMLElement;
  } | null>(null);

  const onPointerDown = (event: React.PointerEvent<HTMLButtonElement>) => {
    if (event.button !== 0) return;
    const branch = event.currentTarget.parentElement;
    if (!branch) return;
    event.preventDefault();
    event.stopPropagation();
    const start = Number.parseFloat(getComputedStyle(branch).getPropertyValue("--split-ratio")) || 0.5;
    dragRef.current = { pointerId: event.pointerId, startRatio: start, branch };
    event.currentTarget.classList.add("is-dragging");
    document.body.classList.add("is-pane-splitting");
    document.body.classList.toggle("is-pane-splitting-y", axis === "horizontal");
    try {
      event.currentTarget.setPointerCapture(event.pointerId);
    } catch {
      /* optional */
    }
  };

  const onPointerMove = (event: React.PointerEvent<HTMLButtonElement>) => {
    const drag = dragRef.current;
    if (!drag || drag.pointerId !== event.pointerId) return;
    const rect = drag.branch.getBoundingClientRect();
    const raw =
      axis === "vertical"
        ? (event.clientX - rect.left) / Math.max(1, rect.width)
        : (event.clientY - rect.top) / Math.max(1, rect.height);
    const next = clampDragRatio(raw);
    drag.branch.style.setProperty("--split-ratio", String(next));
  };

  const finish = (event: React.PointerEvent<HTMLButtonElement>, commit: boolean) => {
    const drag = dragRef.current;
    if (!drag || drag.pointerId !== event.pointerId) return;
    dragRef.current = null;
    event.currentTarget.classList.remove("is-dragging");
    document.body.classList.remove("is-pane-splitting", "is-pane-splitting-y");
    try {
      event.currentTarget.releasePointerCapture(event.pointerId);
    } catch {
      /* ignore */
    }
    if (!commit) {
      drag.branch.style.setProperty("--split-ratio", String(drag.startRatio));
      return;
    }
    const raw = Number.parseFloat(drag.branch.style.getPropertyValue("--split-ratio"));
    onCommit(splitId, Number.isFinite(raw) ? raw : drag.startRatio);
  };

  return (
    <button
      type="button"
      className={`pane-splitter pane-splitter--${axis}`}
      aria-label={axis === "vertical" ? "Resize panes horizontally" : "Resize panes vertically"}
      title="Drag to resize"
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={(e) => finish(e, true)}
      onPointerCancel={(e) => finish(e, false)}
    />
  );
}
