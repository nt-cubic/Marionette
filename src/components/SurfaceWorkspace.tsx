import {
  useEffect,
  useRef,
  useState,
  type PointerEvent,
  type ReactNode,
} from "react";
import { PanelRightClose } from "lucide-react";
import { useT } from "../lib/i18n";
import {
  WORKSPACE_SURFACE_DEFAULT_RATIO,
  clampWorkspaceSurfaceWidth,
  shouldStackWorkspace,
  workspaceSurfaceWidthFromPointer,
  type WorkspaceSurfaceKind,
} from "../lib/workspaceSurface";

const SURFACE_STORAGE_KEY = "marionette-workspace-surface-width";

/**
 * Conversation + content surface, deliberately independent of the session
 * split tree. Browser is its first surface. The chat node is the existing
 * SessionView + Composer; this component owns layout only.
 */
export function SurfaceWorkspace({
  kind,
  surface,
  chat,
  chatTitle,
  surfaceSide = "left",
  onCloseSurface,
}: {
  kind: WorkspaceSurfaceKind;
  surface: ReactNode;
  chat: ReactNode;
  chatTitle: string;
  surfaceSide?: "left" | "right";
  onCloseSurface: () => void;
}) {
  const t = useT();
  const rowRef = useRef<HTMLDivElement>(null);
  const draggingRef = useRef(false);
  const movedRef = useRef(false);
  const widthRef = useRef<number | null>(null);
  const [totalWidth, setTotalWidth] = useState(0);
  const [surfaceWidth, setSurfaceWidth] = useState<number | null>(() => {
    const stored = Number(localStorage.getItem(SURFACE_STORAGE_KEY));
    return Number.isFinite(stored) && stored > 0 ? stored : null;
  });
  widthRef.current = surfaceWidth;
  const stacked = shouldStackWorkspace(totalWidth);
  const effectiveWidth =
    totalWidth > 0
      ? clampWorkspaceSurfaceWidth(
          surfaceWidth ?? totalWidth * WORKSPACE_SURFACE_DEFAULT_RATIO,
          totalWidth,
        )
      : null;

  useEffect(() => {
    const row = rowRef.current;
    if (!row || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver((entries) => {
      const width = entries[0]?.contentRect.width ?? 0;
      setTotalWidth((current) =>
        Math.abs(current - width) < 0.5 ? current : width,
      );
    });
    observer.observe(row);
    return () => observer.disconnect();
  }, []);

  useEffect(
    () => () => document.body.classList.remove("is-surface-resizing"),
    [],
  );

  useEffect(() => {
    if (!stacked) return;
    draggingRef.current = false;
    movedRef.current = false;
    document.body.classList.remove("is-surface-resizing");
  }, [stacked]);

  const onSeamDown = (event: PointerEvent<HTMLDivElement>) => {
    if (stacked || event.button !== 0) return;
    draggingRef.current = true;
    movedRef.current = false;
    event.currentTarget.setPointerCapture(event.pointerId);
    document.body.classList.add("is-surface-resizing");
    event.preventDefault();
  };

  const onSeamMove = (event: PointerEvent<HTMLDivElement>) => {
    if (!draggingRef.current || stacked) return;
    const rect = rowRef.current?.getBoundingClientRect();
    if (!rect) return;
    const next = workspaceSurfaceWidthFromPointer(
      event.clientX,
      rect,
      surfaceSide,
    );
    if (widthRef.current == null || Math.abs(next - widthRef.current) >= 1) {
      movedRef.current = true;
    }
    widthRef.current = next;
    setSurfaceWidth(next);
  };

  const finishDrag = (event: PointerEvent<HTMLDivElement>) => {
    if (!draggingRef.current) return;
    draggingRef.current = false;
    document.body.classList.remove("is-surface-resizing");
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
    if (movedRef.current && widthRef.current != null) {
      localStorage.setItem(
        SURFACE_STORAGE_KEY,
        String(Math.round(widthRef.current)),
      );
    }
  };

  const surfaceNode = (
    <div
      className="workspace-surface__surface"
      style={
        !stacked && effectiveWidth != null
          ? { width: `${effectiveWidth}px` }
          : undefined
      }
    >
      {surface}
    </div>
  );
  const chatNode = (
    <div className="workspace-surface__chat">
      <div className="workspace-surface__chat-head">
        <div className="workspace-surface__chat-title">
          <span>{t("browser.surfaceAgentPane")}</span>
          <b>{chatTitle}</b>
        </div>
        <button
          type="button"
          className="workspace-surface__close"
          title={t("browser.surfaceClose")}
          aria-label={t("browser.surfaceClose")}
          onClick={onCloseSurface}
        >
          <PanelRightClose size={14} />
          <span>{t("browser.surfaceClose")}</span>
        </button>
      </div>
      <div className="workspace-surface__chat-body">{chat}</div>
    </div>
  );
  const seam = (
    <div
      className="workspace-surface__seam"
      role="separator"
      aria-label={t("browser.surfaceResize")}
      aria-orientation={stacked ? "horizontal" : "vertical"}
      onPointerDown={onSeamDown}
      onPointerMove={onSeamMove}
      onPointerUp={finishDrag}
      onPointerCancel={finishDrag}
    />
  );

  return (
    <div
      ref={rowRef}
      className={`workspace-surface-layout is-${surfaceSide}${stacked ? " is-stacked" : ""}${effectiveWidth != null ? " is-measured" : ""}`}
      data-surface-kind={kind}
    >
      {surfaceSide === "left" ? (
        <>
          {surfaceNode}
          {seam}
          {chatNode}
        </>
      ) : (
        <>
          {chatNode}
          {seam}
          {surfaceNode}
        </>
      )}
    </div>
  );
}
