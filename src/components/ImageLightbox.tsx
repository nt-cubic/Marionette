import { useEffect, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { X } from "lucide-react";
import { useT } from "../lib/i18n";
import { usePanZoom } from "../lib/usePanZoom";

type ImageLightboxProps = {
  src: string;
  alt?: string;
  caption?: string;
  children?: ReactNode;
  onClose: () => void;
  onContextMenu?: (event: React.MouseEvent) => void;
};

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
  const t = useT();
  const { dragging, stageStyle, stageProps } = usePanZoom();

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

  return createPortal(
    <div
      className="image-lightbox"
      role="dialog"
      aria-modal="true"
      aria-label={alt || caption || t("lightbox.image")}
      onClick={onClose}
    >
      <button
        type="button"
        className="image-lightbox__close"
        title={t("common.close")}
        aria-label={t("common.close")}
        onClick={(event) => {
          event.stopPropagation();
          onClose();
        }}
      >
        <X size={18} />
      </button>
      <div
        className={dragging ? "image-lightbox__stage is-dragging" : "image-lightbox__stage"}
        style={stageStyle}
        onClick={(event) => event.stopPropagation()}
        onContextMenu={onContextMenu}
        {...stageProps}
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
        <span>{t("lightbox.hint")}</span>
      </p>
    </div>,
    document.body,
  );
}
