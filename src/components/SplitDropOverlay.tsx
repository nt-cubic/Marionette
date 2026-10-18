import type { DropKind, HitTestInput, RectLike, SplitNode } from "../lib/splitWorkspace";
import {
  axisSlotCount,
  collectLeaves,
  contiguousAxisRoot,
  splitPreviewRect,
  unionRect,
} from "../lib/splitWorkspace";

function bodyRect(
  geometry: HitTestInput["geometry"],
  pane: RectLike,
  tabStrip: RectLike,
): RectLike {
  if (geometry === "titlebar-plus-center") return pane;
  const top = Math.max(pane.top, tabStrip.bottom);
  return {
    left: pane.left,
    right: pane.right,
    top,
    bottom: pane.bottom,
    width: pane.width,
    height: Math.max(0, pane.bottom - top),
  };
}

function overlayStyle(rect: RectLike): React.CSSProperties {
  return {
    left: rect.left,
    top: rect.top,
    width: rect.width,
    height: rect.height,
  };
}

export function SplitDropOverlay({
  preview,
  panes,
  geometry,
  root,
}: {
  preview: DropKind | null;
  panes: HitTestInput["panes"];
  geometry: HitTestInput["geometry"];
  root?: SplitNode;
}) {
  if (!preview) return null;
  if (preview.kind !== "split" && preview.kind !== "activate" && preview.kind !== "insert-tab") {
    return null;
  }
  const hit = panes.find((p) => p.paneId === preview.paneId);
  if (!hit) return null;

  if (preview.kind === "insert-tab") {
    return (
      <div
        className="split-drop-overlay split-drop-overlay--insert"
        style={overlayStyle(hit.tabStrip)}
        aria-hidden
      />
    );
  }

  const body = bodyRect(geometry, hit.pane, hit.tabStrip);
  if (body.width <= 0 || body.height <= 0) return null;

  if (preview.kind === "activate") {
    return (
      <div
        className="split-drop-overlay split-drop-overlay--activate"
        style={overlayStyle(body)}
        aria-hidden
      />
    );
  }

  const axis = preview.edge === "left" || preview.edge === "right" ? "vertical" : "horizontal";
  const top = root ? contiguousAxisRoot(root, preview.paneId, axis) : null;
  const groupIds = top ? new Set(collectLeaves(top).map((leaf) => leaf.id)) : null;
  const groupRects = groupIds
    ? panes
        .filter((p) => groupIds.has(p.paneId))
        .map((p) => bodyRect(geometry, p.pane, p.tabStrip))
        .filter((r) => r.width > 0 && r.height > 0)
    : [body];
  const group = unionRect(groupRects) ?? body;
  const nextSlots = (top ? axisSlotCount(top, axis) : 1) + 1;
  const strip = splitPreviewRect(body, group, preview.edge, nextSlots);

  return (
    <div
      className="split-drop-overlay split-drop-overlay--split"
      style={overlayStyle(strip)}
      aria-hidden
    />
  );
}
