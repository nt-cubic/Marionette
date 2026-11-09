import { useEffect, useRef, useState, type CSSProperties, type PointerEvent as ReactPointerEvent } from "react";

const MIN_SCALE = 1;
const MAX_SCALE = 6;

/**
 * Wheel-to-zoom + drag-to-pan for an image stage (shared by ImageLightbox and the seen-image
 * viewer). Spread `stageProps` on the element that holds the picture and apply `stageStyle`.
 * Zoom and pan reset whenever `resetKey` changes (the next image) and on double click.
 */
export function usePanZoom(resetKey?: unknown) {
  const [scale, setScale] = useState(1);
  const [tx, setTx] = useState(0);
  const [ty, setTy] = useState(0);
  const [dragging, setDragging] = useState(false);
  const dragRef = useRef<{ x: number; y: number; tx: number; ty: number } | null>(null);

  useEffect(() => {
    const onWheelNative = (event: WheelEvent) => {
      event.preventDefault();
      const nextFactor = event.deltaY > 0 ? 0.9 : 1.1;
      setScale((current) => Math.min(MAX_SCALE, Math.max(MIN_SCALE, current * nextFactor)));
    };
    window.addEventListener("wheel", onWheelNative, { passive: false });
    return () => window.removeEventListener("wheel", onWheelNative);
  }, []);

  useEffect(() => {
    if (scale === MIN_SCALE) {
      setTx(0);
      setTy(0);
    }
  }, [scale]);

  useEffect(() => {
    setScale(1);
    setTx(0);
    setTy(0);
  }, [resetKey]);

  const onPointerDown = (event: ReactPointerEvent<HTMLElement>) => {
    if (event.button !== 0) return;
    event.currentTarget.setPointerCapture(event.pointerId);
    dragRef.current = { x: event.clientX, y: event.clientY, tx, ty };
    setDragging(true);
  };

  const onPointerMove = (event: ReactPointerEvent<HTMLElement>) => {
    const drag = dragRef.current;
    if (!drag) return;
    setTx(drag.tx + (event.clientX - drag.x));
    setTy(drag.ty + (event.clientY - drag.y));
  };

  const onPointerUp = (event: ReactPointerEvent<HTMLElement>) => {
    try {
      event.currentTarget.releasePointerCapture(event.pointerId);
    } catch {
      /* already released */
    }
    dragRef.current = null;
    setDragging(false);
  };

  const resetView = () => {
    setScale(1);
    setTx(0);
    setTy(0);
  };

  const stageStyle: CSSProperties = { transform: `translate(${tx}px, ${ty}px) scale(${scale})` };
  return {
    scale,
    dragging,
    stageStyle,
    resetView,
    stageProps: {
      onPointerDown,
      onPointerMove,
      onPointerUp,
      onPointerCancel: onPointerUp,
      onDoubleClick: (event: React.MouseEvent) => {
        event.stopPropagation();
        resetView();
      },
    },
  };
}
