import { useEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { ChevronLeft, ChevronRight, Copy, Crosshair, Folder, X } from "lucide-react";
import { useT } from "../../lib/i18n";
import { formatSeenTime, type SeenImage } from "../../lib/seenImages";
import { usePanZoom } from "../../lib/usePanZoom";
import { revealInFileManager } from "../../lib/api";
import { agentInkStyle } from "../../lib/agentInk";
import { SeenImageImg } from "../SeenImageImg";
import { SeenThumb } from "./SeenThumb";

const REMOTE = /^(?:https?|data|blob):/i;
const INLINE = /^(?:data|blob):/i;
/** Filmstrip budget: with huge sets only mount a moving window of this many thumbs. */
const STRIP_WINDOW = 200;

/**
 * Full-screen pager over every image the conversation has seen: dark veil, zoom/pan
 * stage (usePanZoom), arrow / keyboard paging, a filmstrip of SeenThumbs and the
 * locate / reveal / copy actions. Clicking the veil closes; bars stop the click.
 */
export function SeenImageViewer(props: {
  images: SeenImage[];
  startIndex: number;
  agentId?: string;
  onClose: () => void;
  onLocate: (image: SeenImage) => void;
}): JSX.Element | null {
  const { images, startIndex, agentId, onClose, onLocate } = props;
  const t = useT();
  const [index, setIndex] = useState(startIndex);
  const [copied, setCopied] = useState(false);
  const copyTimer = useRef<number | undefined>(undefined);
  const stripRef = useRef<HTMLDivElement | null>(null);

  const total = images.length;
  // Clamp at render time: `index` in state can go stale when `images` shrinks.
  const current = total === 0 ? 0 : Math.min(Math.max(index, 0), total - 1);
  const image = images[current];
  const { dragging, stageStyle, stageProps } = usePanZoom(image?.key);

  // Filmstrip window around the active image (only for sets over the budget).
  const [winStart, winEnd] = useMemo(() => {
    if (total <= STRIP_WINDOW) return [0, total];
    const start = Math.max(0, Math.min(current - (STRIP_WINDOW >> 1), total - STRIP_WINDOW));
    return [start, start + STRIP_WINDOW];
  }, [total, current]);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.ctrlKey || event.altKey || event.metaKey) return;
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        onClose();
        return;
      }
      if (total === 0) return;
      if (event.key === "ArrowLeft") {
        event.preventDefault();
        event.stopPropagation();
        setIndex((i) => Math.max(0, Math.min(i, total - 1) - 1));
      } else if (event.key === "ArrowRight") {
        event.preventDefault();
        event.stopPropagation();
        setIndex((i) => Math.min(total - 1, Math.min(i, total - 1) + 1));
      } else if ((event.key === "l" || event.key === "L") && image) {
        event.preventDefault();
        event.stopPropagation();
        onLocate(image);
        onClose();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [onClose, onLocate, total, image]);

  // Keep the active filmstrip thumb in view; the element may be missing mid-window shift.
  useEffect(() => {
    stripRef.current?.querySelector("[aria-current]")?.scrollIntoView({ inline: "nearest", block: "nearest" });
  }, [current, winStart, winEnd]);

  useEffect(() => () => window.clearTimeout(copyTimer.current), []);

  if (total === 0 || !image) return null;

  const locate = () => {
    onLocate(image);
    onClose();
  };
  const reveal = () => {
    try {
      // Desktop-only helper; a refusal or a non-Tauri host is not worth a UI state.
      void revealInFileManager(image.src).catch(() => {});
    } catch {
      /* ignore */
    }
  };
  const copySrc = () => {
    navigator.clipboard
      .writeText(image.src)
      .then(() => {
        setCopied(true);
        window.clearTimeout(copyTimer.current);
        copyTimer.current = window.setTimeout(() => setCopied(false), 1400);
      })
      .catch(() => {
        /* clipboard refused — keep the plain label */
      });
  };

  const stop = (event: React.MouseEvent) => event.stopPropagation();

  return createPortal(
    <div
      className="seen-viewer"
      style={agentInkStyle(agentId)}
      role="dialog"
      aria-modal="true"
      aria-label={t("seenImages.viewer.aria")}
      onClick={onClose}
    >
      <header className="seen-viewer__top" onClick={stop}>
        <span className="seen-viewer__n">{current + 1} / {total}</span>
        <span className="seen-viewer__src">
          <i className={image.via === "you" ? "is-you" : "is-tool"} aria-hidden="true" />
          {t(`seenImages.via.${image.via}`)}
        </span>
        <span className="seen-viewer__meta">{formatSeenTime(image.at)}</span>
        <button type="button" className="seen-viewer__x" aria-label={t("common.close")} onClick={onClose}>
          <X size={14} strokeWidth={1.5} aria-hidden="true" />
        </button>
      </header>
      <div className="seen-viewer__main">
        <button
          type="button"
          className="seen-viewer__arrow"
          aria-label={t("seenImages.viewer.prev")}
          disabled={current === 0}
          onClick={(event) => {
            stop(event);
            setIndex(current - 1);
          }}
        >
          <ChevronLeft size={16} strokeWidth={1.5} aria-hidden="true" />
        </button>
        <div className="seen-viewer__center" onClick={stop}>
          <div
            className={dragging ? "seen-viewer__stage is-dragging" : "seen-viewer__stage"}
            style={stageStyle}
            onClick={stop}
            {...stageProps}
          >
            <SeenImageImg src={image.src} eager alt={image.name} className="seen-viewer__img" />
          </div>
          <p className="seen-viewer__cap">
            <span className="seen-viewer__cap-name">{image.name}</span>
            {!REMOTE.test(image.src) && (
              <span className="seen-viewer__cap-path" title={image.src}>
                {image.src}
              </span>
            )}
          </p>
        </div>
        <button
          type="button"
          className="seen-viewer__arrow"
          aria-label={t("seenImages.viewer.next")}
          disabled={current === total - 1}
          onClick={(event) => {
            stop(event);
            setIndex(current + 1);
          }}
        >
          <ChevronRight size={16} strokeWidth={1.5} aria-hidden="true" />
        </button>
      </div>
      <div className="seen-viewer__bot" onClick={stop}>
        <div className="seen-viewer__strip" ref={stripRef}>
          {images.slice(winStart, winEnd).map((item, j) => {
            const i = winStart + j;
            return (
              <SeenThumb
                key={item.key}
                image={item}
                width={72}
                height={54}
                showTime
                active={i === current}
                ariaLabel={t("seenImages.viewer.thumb", { n: i + 1 })}
                onClick={() => setIndex(i)}
              />
            );
          })}
        </div>
        <div className="seen-viewer__acts">
          <button type="button" className="seen-viewer__btn is-main" onClick={locate}>
            <Crosshair size={13} strokeWidth={1.5} aria-hidden="true" />
            {t("seenImages.viewer.locate")}
            <kbd>L</kbd>
          </button>
          {!REMOTE.test(image.src) && (
            <button type="button" className="seen-viewer__btn" onClick={reveal}>
              <Folder size={13} strokeWidth={1.5} aria-hidden="true" />
              {t("seenImages.viewer.reveal")}
            </button>
          )}
          {!INLINE.test(image.src) && (
            <button type="button" className="seen-viewer__btn" onClick={copySrc}>
              <Copy size={13} strokeWidth={1.5} aria-hidden="true" />
              {copied ? t("seenImages.viewer.copied") : t("seenImages.viewer.copyPath")}
            </button>
          )}
          <span className="seen-viewer__keys">{t("seenImages.viewer.keys")}</span>
        </div>
      </div>
    </div>,
    document.body,
  );
}
