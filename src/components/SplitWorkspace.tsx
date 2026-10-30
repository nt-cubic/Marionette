import type { ReactNode } from "react";
import type {
  DropKind,
  HitTestInput,
  PaneLeaf,
  RectLike,
  SplitNode,
  SplitWorkspaceState,
} from "../lib/splitWorkspace";
import { isLeaf } from "../lib/splitWorkspace";
import { PaneSplitter } from "./PaneSplitter";
import { SessionPane } from "./SessionPane";
import { SplitDropOverlay } from "./SplitDropOverlay";
import { t } from "../lib/i18n";

function toRect(dom: DOMRect): RectLike {
  return {
    left: dom.left,
    top: dom.top,
    right: dom.right,
    bottom: dom.bottom,
    width: dom.width,
    height: dom.height,
  };
}

function tabXsFrom(tablist: Element): number[] {
  return [...tablist.querySelectorAll(".editor-tab")].map(
    (el) => el.getBoundingClientRect().left,
  );
}

function cssEscape(value: string): string {
  if (typeof CSS !== "undefined" && typeof CSS.escape === "function") return CSS.escape(value);
  return value.replace(/"/g, '\\"');
}

/** Snapshot pane / tab-strip geometry for `hitTestDropZone`. */
export function collectDropGeometry(
  geometry: HitTestInput["geometry"],
  leaves: PaneLeaf[],
): HitTestInput["panes"] {
  if (geometry === "titlebar-plus-center") {
    const leaf = leaves[0];
    if (!leaf) return [];
    const center = document.querySelector(".center-workspace");
    const tablist = document.querySelector(".workspace-titlebar .editor-tabs__tablist");
    if (!center || !tablist) return [];
    return [
      {
        paneId: leaf.id,
        pane: toRect(center.getBoundingClientRect()),
        tabStrip: toRect(tablist.getBoundingClientRect()),
        tabXs: tabXsFrom(tablist),
      },
    ];
  }

  const out: HitTestInput["panes"] = [];
  for (const leaf of leaves) {
    const el = document.querySelector(`[data-pane-id="${cssEscape(leaf.id)}"]`);
    if (!el) continue;
    const tablist = el.querySelector(".editor-tabs__tablist");
    const pane = toRect(el.getBoundingClientRect());
    const tabStrip = tablist
      ? toRect(tablist.getBoundingClientRect())
      : {
          left: pane.left,
          top: pane.top,
          right: pane.right,
          bottom: pane.top,
          width: pane.width,
          height: 0,
        };
    out.push({
      paneId: leaf.id,
      pane,
      tabStrip,
      tabXs: tablist ? tabXsFrom(tablist) : [],
    });
  }
  return out;
}

export function dropKindsEqual(a: DropKind | null, b: DropKind | null): boolean {
  if (a === b) return true;
  if (!a || !b) return false;
  if (a.kind !== b.kind) return false;
  if (a.kind === "split" && b.kind === "split") return a.paneId === b.paneId && a.edge === b.edge;
  if (a.kind === "activate" && b.kind === "activate") return a.paneId === b.paneId;
  if (a.kind === "insert-tab" && b.kind === "insert-tab") {
    return a.paneId === b.paneId && a.index === b.index;
  }
  if (a.kind === "cancel" && b.kind === "cancel") return a.reason === b.reason;
  return true;
}

export function ghostHintForDrop(drop: DropKind | null, opts?: { atMax?: boolean }): string {
  if (!drop) return t("split.releaseNewWindow");
  switch (drop.kind) {
    case "split":
      return drop.edge === "left"
        ? t("split.releaseSplitLeft")
        : drop.edge === "right"
          ? t("split.releaseSplitRight")
          : drop.edge === "top"
            ? t("split.releaseSplitTop")
            : t("split.releaseSplitBottom");
    case "activate":
      return opts?.atMax ? t("split.atMaxDropHere") : t("split.releaseDropHere");
    case "insert-tab":
      return t("split.releaseInsertHere");
    case "detach":
      return t("split.releaseNewWindow");
    case "cancel":
      if (drop.reason === "self-split-last-tab") return t("split.noSelfSplitLastTab");
      if (drop.reason === "too-narrow") return t("split.tooNarrowToSplit");
      if (drop.reason === "back-on-source") return t("split.releaseCancel");
      return t("split.releaseCancel");
  }
}

export function SplitWorkspace({
  state,
  dropPreview,
  dropPanes,
  geometry,
  onFocusPane,
  onResize,
  renderLeaf,
}: {
  state: SplitWorkspaceState;
  dropPreview: DropKind | null;
  dropPanes: HitTestInput["panes"];
  geometry: HitTestInput["geometry"];
  onFocusPane: (paneId: string) => void;
  onResize: (splitId: string, ratio: number) => void;
  renderLeaf: (leaf: PaneLeaf, focused: boolean) => { tabs: ReactNode; body: ReactNode };
}) {
  const render = (node: SplitNode): ReactNode => {
    if (isLeaf(node)) {
      const slot = renderLeaf(node, node.id === state.focusedPaneId);
      return (
        <SessionPane
          leaf={node}
          focused={node.id === state.focusedPaneId}
          onFocus={onFocusPane}
          tabs={slot.tabs}
        >
          {slot.body}
        </SessionPane>
      );
    }
    return (
      <div
        className={`split-branch is-${node.axis}`}
        data-split-id={node.id}
        style={{ ["--split-ratio" as string]: String(node.ratio) }}
      >
        <div className="split-a">{render(node.a)}</div>
        <PaneSplitter axis={node.axis} splitId={node.id} onCommit={onResize} />
        <div className="split-b">{render(node.b)}</div>
      </div>
    );
  };

  return (
    <div className="split-workspace">
      {render(state.root)}
      <SplitDropOverlay preview={dropPreview} panes={dropPanes} geometry={geometry} root={state.root} />
    </div>
  );
}
