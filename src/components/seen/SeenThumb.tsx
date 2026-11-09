import type { CSSProperties } from "react";
import { SeenImageImg } from "../SeenImageImg";
import { formatClock, type SeenImage } from "../../lib/seenImages";
import { useT } from "../../lib/i18n";

/**
 * One seen-image thumbnail. The 2px bar on its left edge is the source: theme green for
 * an image the user attached, the surrounding `--agent-ink` for one a tool produced
 * (set `style={agentInkStyle(agentId)}` on an ancestor). Rail section and viewer
 * filmstrip both use this, so the two always look the same.
 */
export function SeenThumb({
  image,
  width,
  height,
  showTime = false,
  active = false,
  dim = false,
  title,
  ariaLabel,
  onClick,
}: {
  image: SeenImage;
  width: number;
  height: number;
  /** HH:MM badge in the corner (grid / filmstrip; not on the tiny default row). */
  showTime?: boolean;
  active?: boolean;
  /** Grey + faded: the current model cannot take images. */
  dim?: boolean;
  title?: string;
  ariaLabel?: string;
  onClick?: () => void;
}) {
  const t = useT();
  const style: CSSProperties = { width, height };
  const cls = ["seen-th", active ? "is-active" : "", dim ? "is-dim" : "", `is-${image.via === "you" ? "you" : "tool"}`]
    .filter(Boolean)
    .join(" ");
  const body = (
    <>
      <SeenImageImg src={image.src} className="seen-th__img" />
      <i className="seen-th__tick" aria-hidden="true" />
      {showTime && <em className="seen-th__time">{formatClock(image.at)}</em>}
    </>
  );
  return onClick ? (
    <button
      type="button"
      className={cls}
      style={style}
      title={title ?? image.name}
      aria-label={ariaLabel ?? t("seenImages.open", { name: image.name })}
      aria-current={active || undefined}
      onClick={onClick}
    >
      {body}
    </button>
  ) : (
    <span className={cls} style={style} title={title ?? image.name}>
      {body}
    </span>
  );
}
