import {
  useEffect,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";
import { X } from "lucide-react";

type ImageLightboxProps = {
  src: string;
  alt?: string;
  caption?: string;
  children?: ReactNode;
  onClose: () => void;
  onContextMenu?: (event: React.MouseEvent) => void;
};

const MIN_SCALE = 1;
const MAX_SCALE = 6;

/**
 * Weibo / Twitter-style viewer: dark veil, picture in the middle, drag to pan,
 * wheel to zoom, Esc / click the black / × to close.
 */
export function ImageLightbox({
  src,
  alt,
  caption,
  children,
  onClose,
  onContextMenu,
}: ImageLightboxProps) {
  const [scale, setScale] = useState(1);
  const [tx, setTx] = useState(0);
  const [ty, setTy] = useState(0);
  const [dragging, setDragging] = useState(false);
  const dragRef = useRef<{
    x: number;
    y: number;
    tx: number;
    ty: number;
  } | null>(null);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      onClose();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [onClose]);

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

  const onPointerDown = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (event.button !== 0) return;
    event.currentTarget.setPointerCapture(event.pointerId);
    dragRef.current = { x: event.clientX, y: event.clientY, tx, ty };
    setDragging(true);
  };

  const onPointerMove = (event: ReactPointerEvent<HTMLDivElement>) => {
    const drag = dragRef.current;
    if (!drag) return;
    setTx(drag.tx + (event.clientX - drag.x));
    setTy(drag.ty + (event.clientY - drag.y));
  };

  const onPointerUp = (event: ReactPointerEvent<HTMLDivElement>) => {
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

  return createPortal(
    <div
      className="image-lightbox"
      role="dialog"
      aria-modal="true"
      aria-label={alt || caption || "图片"}
      onClick={onClose}
    >
      <button
        type="button"
        className="image-lightbox__close"
        title="关闭"
        aria-label="关闭"
        onClick={(event) => {
          event.stopPropagation();
          onClose();
        }}
      >
        <X size={18} />
      </button>
      <div
        className={dragging ? "image-lightbox__stage is-dragging" : "image-lightbox__stage"}
        style={{ transform: `translate(${tx}px, ${ty}px) scale(${scale})` }}
        onClick={(event) => event.stopPropagation()}
        onDoubleClick={(event) => {
          event.stopPropagation();
          resetView();
        }}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onPointerCancel={onPointerUp}
        onContextMenu={onContextMenu}
      >
        <img
          src={src}
          alt={alt ?? ""}
          className="image-lightbox__img"
          draggable={false}
        />
        {children}
      </div>
      <p className="image-lightbox__hint" onClick={(event) => event.stopPropagation()}>
        {caption ? <span className="image-lightbox__caption">{caption}</span> : null}
        <span>拖动移动 · 滚轮缩放 · Esc 关闭</span>
      </p>
    </div>,
    document.body,
  );
}
