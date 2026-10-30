import { useEffect } from "react";
import { createPortal } from "react-dom";
import { Columns2, Rows2 } from "lucide-react";
import { useT } from "../lib/i18n";

export type OpenBesideEdge = "right" | "bottom";

export function SplitBesideMenu({
  x,
  y,
  onPick,
  onClose,
}: {
  x: number;
  y: number;
  onPick: (edge: OpenBesideEdge) => void;
  onClose: () => void;
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
  const top = Math.max(8, Math.min(y, window.innerHeight - 88));

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
        onClick={() => {
          onPick("bottom");
          onClose();
        }}
      >
        <Rows2 size={13} aria-hidden />
        {t("split.openBottom")}
      </button>
    </div>,
    document.body,
  );
}
