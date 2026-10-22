/**
 * The collapsed 72×72 orb: an accent-gradient ball, draggable through its
 * whole surface (`data-tauri-drag-region`). A click that did not turn into a
 * drag expands the panel; badges (worst pending notice, capture-processing
 * count, transient toast) sit on top with pointer-events disabled so they
 * never block the drag surface.
 */

import { useCallback, useRef } from "react";
import type { OrbNoticeKind } from "../lib/orbEvents";

export const NOTICE_BADGES: Record<OrbNoticeKind, { icon: string; className: string }> = {
  reply: { icon: "!", className: "orb-ball__badge--reply" },
  permission: { icon: "🔑", className: "orb-ball__badge--permission" },
  error: { icon: "⚠", className: "orb-ball__badge--error" },
};

/** error > permission > reply — the single worst notice owns the badge slot. */
const NOTICE_PRIORITY: OrbNoticeKind[] = ["error", "permission", "reply"];

export function worstNoticeKind(kinds: OrbNoticeKind[]): OrbNoticeKind | null {
  for (const kind of NOTICE_PRIORITY) {
    if (kinds.includes(kind)) return kind;
  }
  return null;
}

export type OrbBallProps = {
  worstNotice: OrbNoticeKind | null;
  processingCount: number;
  /** Transient chip text (已送入收集箱 ✓ / failure line) shown under the badge slot. */
  toastText: string | null;
  toastTone: "ok" | "warn";
  dragHover: boolean;
  onActivate: () => void;
};

/** Pointer travel tolerated before a press stops counting as a click. */
const CLICK_SLOP_PX = 6;

export function OrbBall({
  worstNotice,
  processingCount,
  toastText,
  toastTone,
  dragHover,
  onActivate,
}: OrbBallProps) {
  const pressRef = useRef<{ x: number; y: number } | null>(null);

  const onPointerDown = useCallback((e: React.PointerEvent<HTMLDivElement>) => {
    pressRef.current = { x: e.clientX, y: e.clientY };
  }, []);

  const onClick = useCallback(
    (e: React.MouseEvent<HTMLDivElement>) => {
      const press = pressRef.current;
      pressRef.current = null;
      if (!press) return;
      const dx = e.clientX - press.x;
      const dy = e.clientY - press.y;
      if (Math.abs(dx) > CLICK_SLOP_PX || Math.abs(dy) > CLICK_SLOP_PX) return;
      e.stopPropagation();
      onActivate();
    },
    [onActivate]
  );

  const badge = worstNotice ? NOTICE_BADGES[worstNotice] : null;

  return (
    <div
      className={`orb-ball${dragHover ? " orb-ball--hover" : ""}`}
      data-tauri-drag-region
      onPointerDown={onPointerDown}
      onClick={onClick}
      role="button"
      aria-label="Marionette 悬浮球"
      tabIndex={-1}
    >
      <span className="orb-ball__mark" />
      {badge ? (
        <span className={`orb-ball__badge ${badge.className}`} aria-label="有提醒">
          {badge.icon}
        </span>
      ) : null}
      {processingCount > 0 ? (
        <span className="orb-ball__count" aria-label="正在收集">
          {processingCount > 99 ? "99+" : processingCount}
        </span>
      ) : null}
      {toastText ? (
        <span className={`orb-ball__toast orb-ball__toast--${toastTone}`}>{toastText}</span>
      ) : null}
    </div>
  );
}
