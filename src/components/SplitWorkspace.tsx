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
  if (!drop) return "松开以打开新窗口";
  switch (drop.kind) {
    case "split":
      return drop.edge === "left"
        ? "松开以在左侧分屏"
        : drop.edge === "right"
          ? "松开以在右侧分屏"
          : drop.edge === "top"
            ? "松开以在上方分屏"
            : "松开以在下方分屏";
    case "activate":
      return opts?.atMax ? "已达上限，放入此格" : "松开以放入此格";
    case "insert-tab":
      return "松开以插入此格";
    case "detach":
      return "松开以打开新窗口";
    case "cancel":
      if (drop.reason === "self-split-last-tab") return "不能用唯一标签拆自己";
      if (drop.reason === "too-narrow") return "此格太小，无法再拆";
      if (drop.reason === "back-on-source") return "松开以取消";
      return "松开以取消";
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
