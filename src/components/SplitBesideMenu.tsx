import { useEffect } from "react";
import { createPortal } from "react-dom";
import { Columns2, Rows2, SquareArrowOutUpRight } from "lucide-react";
import { useT } from "../lib/i18n";

export type OpenBesideEdge = "right" | "bottom";

export function SplitBesideMenu({
  x,
  y,
  onPick,
  onClose,
  canSplit = true,
  onPopOut,
}: {
  x: number;
  y: number;
  onPick: (edge: OpenBesideEdge) => void;
  onClose: () => void;
  /** False when there is nothing to split beside (one tab): the split items are disabled. */
  canSplit?: boolean;
  /** Tear the tab off into its own window (the tab row has no button for it). */
  onPopOut?: () => void;
}) {
  const t = useT();
  useEffect(() => {
    const onDown = () => onClose();
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    const timer = window.setTimeout(() => {
      document.addEventListener("mousedown", onDown);
      document.addEventListener("keydown", onKey);
    }, 0);
    return () => {
      window.clearTimeout(timer);
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [onClose]);

  const left = Math.max(8, Math.min(x, window.innerWidth - 188));
  const top = Math.max(8, Math.min(y, window.innerHeight - (onPopOut ? 120 : 88)));

  return createPortal(
    <div
      className="split-beside-menu"
      style={{ left, top }}
      role="menu"
      onMouseDown={(event) => event.stopPropagation()}
    >
      <button
        type="button"
        role="menuitem"
        disabled={!canSplit}
        onClick={() => {
          onPick("right");
          onClose();
        }}
      >
        <Columns2 size={13} aria-hidden />
        {t("split.openRight")}
      </button>
      <button
        type="button"
        role="menuitem"
        disabled={!canSplit}
        onClick={() => {
          onPick("bottom");
          onClose();
        }}
      >
        <Rows2 size={13} aria-hidden />
        {t("split.openBottom")}
      </button>
      {onPopOut && (
        <button
          type="button"
          role="menuitem"
          onClick={() => {
            onPopOut();
            onClose();
          }}
        >
          <SquareArrowOutUpRight size={13} aria-hidden />
          {t("split.openWindow")}
        </button>
      )}
    </div>,
    document.body,
  );
}
