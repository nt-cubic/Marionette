/**
 * Split-workspace pane tree: pure reducers + drop-zone hit test.
 * No React, no Tauri. UI commits on pointerup only.
 */

export const MAX_PANES = 4;
export const MAX_PANES_V1 = 2;
export const EDGE_FRACTION = 0.25;
export const MIN_PANE_WIDTH_PX = 280;
export const MIN_PANE_HEIGHT_PX = 220;
export const MIN_SPLIT_RATIO = 0.2;
export const MAX_SPLIT_RATIO = 0.8;
export const DEFAULT_SPLIT_RATIO = 0.5;
/** Vertical slop around a tab strip: still insert/cancel, not split. */
export const TAB_DETACH_SLOP_PX = 28;

export type SplitAxis = "vertical" | "horizontal";

export type PaneLeaf = {
  type: "leaf";
  id: string;
  tabIds: string[];
  activeTabId: string;
};

export type SplitBranch = {
  type: "split";
  id: string;
  axis: SplitAxis;
  ratio: number;
  a: SplitNode;
  b: SplitNode;
};

export type SplitNode = PaneLeaf | SplitBranch;

export type SplitWorkspaceState = {
  root: SplitNode;
  focusedPaneId: string;
};

export type SplitLayoutV1 = {
  version: 1;
  root: SplitNode;
  focusedPaneId: string;
};

export type SplitMutation = {
  state: SplitWorkspaceState;
  needPlaceholderChat: boolean;
};

export type DropEdge = "left" | "right" | "top" | "bottom";

export type CancelReason =
  | "self-split-last-tab"
  | "too-narrow"
  | "max-panes"
  | "duplicate-detached"
  | "back-on-source"
  | "miss";

export type DropKind =
  | { kind: "split"; paneId: string; edge: DropEdge }
  | { kind: "activate"; paneId: string }
  | { kind: "insert-tab"; paneId: string; index: number }
  | { kind: "detach" }
  | { kind: "cancel"; reason: CancelReason };

export type RectLike = {
  left: number;
  top: number;
  right: number;
  bottom: number;
  width: number;
  height: number;
};

export type HitTestInput = {
  x: number;
  y: number;
  windowW: number;
  windowH: number;
  geometry: "titlebar-plus-center" | "per-pane";
  panes: Array<{
    paneId: string;
    pane: RectLike;
    tabStrip: RectLike;
    tabXs: number[];
  }>;
  source: { sessionId: string; paneId: string | null; tabCountInSource: number };
  leafCount: number;
  allowHorizontal: boolean;
  maxLeaves: number;
  /** Current pane tree. Same-axis row/column span uses this for 1|1|1 too-narrow. */
  root?: SplitNode;
};

export function isLeaf(node: SplitNode): node is PaneLeaf {
  return node.type === "leaf";
}

export function isSplit(node: SplitNode): node is SplitBranch {
  return node.type === "split";
}

export function emptyWorkspace(paneId = "pane-1"): SplitWorkspaceState {
  return {
    root: { type: "leaf", id: paneId, tabIds: [], activeTabId: "" },
    focusedPaneId: paneId,
  };
}

export function migrateFromFlat(
  openSessionIds: string[],
  currentSessionId: string,
): SplitWorkspaceState {
  const tabs = openSessionIds.filter(Boolean);
  const active = tabs.includes(currentSessionId) ? currentSessionId : (tabs[0] ?? "");
  if (!active) return emptyWorkspace();
  const tabIds = tabs.includes(active) ? tabs : [active, ...tabs];
  return {
    root: { type: "leaf", id: "pane-1", tabIds, activeTabId: active },
    focusedPaneId: "pane-1",
  };
}

export function collectLeaves(node: SplitNode): PaneLeaf[] {
  if (node.type === "leaf") return [node];
  return [...collectLeaves(node.a), ...collectLeaves(node.b)];
}

function pathToNode(root: SplitNode, id: string): SplitNode[] | null {
  if (root.id === id) return [root];
  if (root.type === "leaf") return null;
  const viaA = pathToNode(root.a, id);
  if (viaA) return [root, ...viaA];
  const viaB = pathToNode(root.b, id);
  if (viaB) return [root, ...viaB];
  return null;
}

/**
 * Highest ancestor of `paneId` whose path down is only `axis` splits.
 * Used so a third left/right drop equalizes the whole row (1|1|1), not half of one pane.
 */
export function contiguousAxisRoot(root: SplitNode, paneId: string, axis: SplitAxis): SplitNode {
  const path = pathToNode(root, paneId);
  if (!path || path.length === 0) return root;
  let top = path[path.length - 1]!;
  for (let i = path.length - 2; i >= 0; i -= 1) {
    const node = path[i]!;
    if (node.type === "split" && node.axis === axis) top = node;
    else break;
  }
  return top;
}

/** How many columns (vertical) or rows (horizontal) `node` occupies on `axis`. */
export function axisSlotCount(node: SplitNode, axis: SplitAxis): number {
  if (node.type === "leaf") return 1;
  if (node.axis !== axis) return 1;
  return axisSlotCount(node.a, axis) + axisSlotCount(node.b, axis);
}

function equalizeAxis(node: SplitNode, axis: SplitAxis): SplitNode {
  if (node.type !== "split" || node.axis !== axis) return node;
  const aCount = axisSlotCount(node.a, axis);
  const bCount = axisSlotCount(node.b, axis);
  const total = Math.max(1, aCount + bCount);
  const ratio = clampRatio(aCount / total);
  const a = equalizeAxis(node.a, axis);
  const b = equalizeAxis(node.b, axis);
  if (a === node.a && b === node.b && ratio === node.ratio) return node;
  return { ...node, ratio, a, b };
}

export function unionRect(rects: readonly RectLike[]): RectLike | null {
  if (rects.length === 0) return null;
  let left = Infinity;
  let top = Infinity;
  let right = -Infinity;
  let bottom = -Infinity;
  for (const r of rects) {
    left = Math.min(left, r.left);
    top = Math.min(top, r.top);
    right = Math.max(right, r.right);
    bottom = Math.max(bottom, r.bottom);
  }
  return { left, top, right, bottom, width: right - left, height: bottom - top };
}

/** Preview strip for a split drop: outer edge of a row uses 1/(n+1) of the whole row. */
export function splitPreviewRect(
  body: RectLike,
  group: RectLike,
  edge: DropEdge,
  nextSlots: number,
): RectLike {
  const outer =
    (edge === "right" && Math.abs(body.right - group.right) <= 2) ||
    (edge === "left" && Math.abs(body.left - group.left) <= 2) ||
    (edge === "bottom" && Math.abs(body.bottom - group.bottom) <= 2) ||
    (edge === "top" && Math.abs(body.top - group.top) <= 2);
  const box = outer ? group : body;
  const frac = outer ? 1 / Math.max(2, nextSlots) : 0.5;
  if (edge === "left") {
    return { ...box, width: box.width * frac, right: box.left + box.width * frac };
  }
  if (edge === "right") {
    const width = box.width * frac;
    return { ...box, left: box.right - width, width };
  }
  if (edge === "top") {
    return { ...box, height: box.height * frac, bottom: box.top + box.height * frac };
  }
  const height = box.height * frac;
  return { ...box, top: box.bottom - height, height };
}

export function leafCount(state: SplitWorkspaceState): number {
  return collectLeaves(state.root).length;
}

export function findPane(state: SplitWorkspaceState, paneId: string): PaneLeaf | null {
  for (const leaf of collectLeaves(state.root)) {
    if (leaf.id === paneId) return leaf;
  }
  return null;
}

export function paneOfSession(state: SplitWorkspaceState, sessionId: string): PaneLeaf | null {
  if (!sessionId) return null;
  for (const leaf of collectLeaves(state.root)) {
    if (leaf.tabIds.includes(sessionId)) return leaf;
  }
  return null;
}

export function findSplit(state: SplitWorkspaceState, splitId: string): SplitBranch | null {
  const walk = (node: SplitNode): SplitBranch | null => {
    if (node.type === "leaf") return null;
    if (node.id === splitId) return node;
    return walk(node.a) ?? walk(node.b);
  };
  return walk(state.root);
}

/** Pre-order concatenation of every leaf's tab list. */
export function openSessionIds(state: SplitWorkspaceState): string[] {
  const ids: string[] = [];
  for (const leaf of collectLeaves(state.root)) ids.push(...leaf.tabIds);
  return ids;
}

export function focusedSessionId(state: SplitWorkspaceState): string {
  return findPane(state, state.focusedPaneId)?.activeTabId ?? "";
}

export function visibleSessionIds(state: SplitWorkspaceState): string[] {
  const ids: string[] = [];
  for (const leaf of collectLeaves(state.root)) {
    if (leaf.activeTabId) ids.push(leaf.activeTabId);
  }
  return ids;
}

function walkIds(node: SplitNode, into: { pane: number; split: number; ids: Set<string> }) {
  into.ids.add(node.id);
  if (node.type === "leaf") {
    const n = parseKindId("pane", node.id);
    if (n > into.pane) into.pane = n;
    return;
  }
  const n = parseKindId("split", node.id);
  if (n > into.split) into.split = n;
  walkIds(node.a, into);
  walkIds(node.b, into);
}

function parseKindId(kind: "pane" | "split", id: string): number {
  const prefix = `${kind}-`;
  if (!id.startsWith(prefix)) return 0;
  const n = Number(id.slice(prefix.length));
  return Number.isInteger(n) && n > 0 ? n : 0;
}

export function nextId(state: SplitWorkspaceState, kind: "pane" | "split"): string {
  const acc = { pane: 0, split: 0, ids: new Set<string>() };
  walkIds(state.root, acc);
  const n = (kind === "pane" ? acc.pane : acc.split) + 1;
  return `${kind}-${n}`;
}

function clampRatio(ratio: number): number {
  if (!Number.isFinite(ratio)) return DEFAULT_SPLIT_RATIO;
  return Math.min(MAX_SPLIT_RATIO, Math.max(MIN_SPLIT_RATIO, ratio));
}

function nextActiveAfterRemove(tabIds: string[], activeTabId: string, removedId: string): string {
  if (removedId !== activeTabId) {
    return tabIds.includes(activeTabId) && activeTabId !== removedId ? activeTabId : (tabIds.find((id) => id !== removedId) ?? "");
  }
  const idx = tabIds.indexOf(removedId);
  if (idx < 0) return activeTabId;
  if (idx < tabIds.length - 1) return tabIds[idx + 1]!;
  if (idx > 0) return tabIds[idx - 1]!;
  return "";
}

function replaceNode(node: SplitNode, id: string, replacement: SplitNode): SplitNode {
  if (node.id === id) return replacement;
  if (node.type === "leaf") return node;
  const a = replaceNode(node.a, id, replacement);
  const b = replaceNode(node.b, id, replacement);
  if (a === node.a && b === node.b) return node;
  return { ...node, a, b };
}

function mapLeaves(node: SplitNode, fn: (leaf: PaneLeaf) => PaneLeaf): SplitNode {
  if (node.type === "leaf") return fn(node);
  const a = mapLeaves(node.a, fn);
  const b = mapLeaves(node.b, fn);
  if (a === node.a && b === node.b) return node;
  return { ...node, a, b };
}

function removeSessionFromTree(node: SplitNode, sessionId: string): SplitNode {
  return mapLeaves(node, (leaf) => {
    if (!leaf.tabIds.includes(sessionId)) return leaf;
    const tabIds = leaf.tabIds.filter((id) => id !== sessionId);
    return { ...leaf, tabIds, activeTabId: nextActiveAfterRemove(leaf.tabIds, leaf.activeTabId, sessionId) };
  });
}

function collapseNode(node: SplitNode): SplitNode {
  if (node.type === "leaf") return node;
  const a = collapseNode(node.a);
  const b = collapseNode(node.b);
  const aEmpty = a.type === "leaf" && a.tabIds.length === 0;
  const bEmpty = b.type === "leaf" && b.tabIds.length === 0;
  if (aEmpty && !bEmpty) return b;
  if (bEmpty && !aEmpty) return a;
  if (aEmpty && bEmpty) return a;
  if (a === node.a && b === node.b) return node;
  return { ...node, a, b, ratio: clampRatio(node.ratio) };
}

function firstLeafId(node: SplitNode): string {
  return collectLeaves(node)[0]?.id ?? "pane-1";
}

function uniqueTabs(tabIds: string[]): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  for (const id of tabIds) {
    if (!id || seen.has(id)) continue;
    seen.add(id);
    out.push(id);
  }
  return out;
}

function squashOverflow(root: SplitNode, maxLeaves: number): SplitNode {
  const leaves = collectLeaves(root);
  if (leaves.length <= maxLeaves) return root;
  const keepLast = leaves[maxLeaves - 1]!;
  const extra: string[] = [];
  for (const leaf of leaves.slice(maxLeaves)) extra.push(...leaf.tabIds);
  const overflowIds = new Set(leaves.slice(maxLeaves).map((l) => l.id));
  const merged = uniqueTabs([...keepLast.tabIds, ...extra]);
  const withMerge = mapLeaves(root, (leaf) => {
    if (leaf.id === keepLast.id) {
      const active = merged.includes(leaf.activeTabId) ? leaf.activeTabId : (merged[merged.length - 1] ?? "");
      return { ...leaf, tabIds: merged, activeTabId: active };
    }
    if (overflowIds.has(leaf.id)) return { ...leaf, tabIds: [], activeTabId: "" };
    return leaf;
  });
  return collapseNode(withMerge);
}

function dropDuplicateSessions(root: SplitNode): SplitNode {
  const seen = new Set<string>();
  return mapLeaves(root, (leaf) => {
    const tabIds: string[] = [];
    for (const id of leaf.tabIds) {
      if (!id || seen.has(id)) continue;
      seen.add(id);
      tabIds.push(id);
    }
    if (tabIds.length === leaf.tabIds.length && tabIds.every((id, i) => id === leaf.tabIds[i])) {
      const active = tabIds.includes(leaf.activeTabId) ? leaf.activeTabId : (tabIds[0] ?? "");
      return active === leaf.activeTabId ? leaf : { ...leaf, activeTabId: active };
    }
    const active = tabIds.includes(leaf.activeTabId) ? leaf.activeTabId : (tabIds[0] ?? "");
    return { ...leaf, tabIds, activeTabId: active };
  });
}

function clampSplits(node: SplitNode): SplitNode {
  if (node.type === "leaf") return node;
  const a = clampSplits(node.a);
  const b = clampSplits(node.b);
  const ratio = clampRatio(node.ratio);
  if (a === node.a && b === node.b && ratio === node.ratio) return node;
  return { ...node, a, b, ratio };
}

function repairFocused(state: SplitWorkspaceState): SplitWorkspaceState {
  if (findPane(state, state.focusedPaneId)) return state;
  return { ...state, focusedPaneId: firstLeafId(state.root) };
}

function normalize(state: SplitWorkspaceState, maxLeaves = MAX_PANES): SplitWorkspaceState {
  let root = dropDuplicateSessions(state.root);
  const total = openSessionIds({ root, focusedPaneId: state.focusedPaneId }).length;
  root = collapseNode(root);
  if (total === 0) {
    const id = root.type === "leaf" ? root.id : "pane-1";
    root = { type: "leaf", id, tabIds: [], activeTabId: "" };
    return { root, focusedPaneId: id };
  }
  root = squashOverflow(root, maxLeaves);
  root = collapseNode(root);
  root = clampSplits(root);
  root = mapLeaves(root, (leaf) => {
    const tabIds = uniqueTabs(leaf.tabIds);
    const active = tabIds.includes(leaf.activeTabId) ? leaf.activeTabId : (tabIds[0] ?? "");
    if (tabIds === leaf.tabIds && active === leaf.activeTabId) return leaf;
    if (tabIds.length === leaf.tabIds.length && active === leaf.activeTabId && tabIds.every((id, i) => id === leaf.tabIds[i])) {
      return leaf;
    }
    return { ...leaf, tabIds, activeTabId: active };
  });
  return repairFocused({ root, focusedPaneId: state.focusedPaneId });
}

export function assertInvariants(state: SplitWorkspaceState, maxLeaves = MAX_PANES): void {
  const leaves = collectLeaves(state.root);
  if (leaves.length < 1 || leaves.length > maxLeaves) {
    throw new Error(`invariant: leaf count ${leaves.length} not in 1..${maxLeaves}`);
  }
  const ids = new Set<string>();
  const sessions = new Set<string>();
  let tabCount = 0;
  const walk = (node: SplitNode) => {
    if (ids.has(node.id)) throw new Error(`invariant: duplicate node id ${node.id}`);
    ids.add(node.id);
    if (node.type === "leaf") {
      tabCount += node.tabIds.length;
      const unique = uniqueTabs(node.tabIds);
      if (unique.length !== node.tabIds.length) throw new Error(`invariant: duplicate tab in ${node.id}`);
      for (const id of node.tabIds) {
        if (sessions.has(id)) throw new Error(`invariant: duplicate session ${id}`);
        sessions.add(id);
      }
      if (node.tabIds.length === 0) {
        if (node.activeTabId !== "") throw new Error(`invariant: empty leaf ${node.id} has activeTabId`);
      } else if (!node.tabIds.includes(node.activeTabId)) {
        throw new Error(`invariant: activeTabId ${node.activeTabId} missing from ${node.id}`);
      }
      return;
    }
    if (node.ratio < MIN_SPLIT_RATIO || node.ratio > MAX_SPLIT_RATIO) {
      throw new Error(`invariant: ratio ${node.ratio} out of range on ${node.id}`);
    }
    if (node.axis !== "vertical" && node.axis !== "horizontal") {
      throw new Error(`invariant: bad axis on ${node.id}`);
    }
    walk(node.a);
    walk(node.b);
  };
  walk(state.root);
  if (tabCount === 0) {
    if (leaves.length !== 1 || leaves[0]!.tabIds.length !== 0 || leaves[0]!.activeTabId !== "") {
      throw new Error("invariant: empty workspace must be exactly one empty leaf");
    }
    if (state.root.type !== "leaf") throw new Error("invariant: empty workspace root must be a leaf");
  } else {
    for (const leaf of leaves) {
      if (leaf.tabIds.length === 0) throw new Error(`invariant: empty leaf ${leaf.id} while tree has sessions`);
    }
  }
  if (leaves.length === 1 && state.root.type !== "leaf") {
    throw new Error("invariant: single leaf must be the root");
  }
  if (!findPane(state, state.focusedPaneId)) {
    throw new Error(`invariant: focusedPaneId ${state.focusedPaneId} missing`);
  }
}

export function collapseEmpty(state: SplitWorkspaceState): SplitWorkspaceState {
  return normalize(state);
}

export function focusPane(state: SplitWorkspaceState, paneId: string): SplitWorkspaceState {
  if (!findPane(state, paneId)) return state;
  return { ...state, focusedPaneId: paneId };
}

export function resizeSplit(state: SplitWorkspaceState, splitId: string, ratio: number): SplitWorkspaceState {
  const next = clampRatio(ratio);
  const walk = (node: SplitNode): SplitNode => {
    if (node.type === "leaf") return node;
    if (node.id === splitId) return { ...node, ratio: next };
    const a = walk(node.a);
    const b = walk(node.b);
    if (a === node.a && b === node.b) return node;
    return { ...node, a, b };
  };
  const root = walk(state.root);
  return root === state.root ? state : { ...state, root };
}

export function activateTab(
  state: SplitWorkspaceState,
  paneId: string,
  sessionId: string,
): SplitWorkspaceState {
  const leaf = findPane(state, paneId);
  if (!leaf || !leaf.tabIds.includes(sessionId)) return state;
  const root =
    leaf.activeTabId === sessionId
      ? state.root
      : replaceNode(state.root, paneId, { ...leaf, activeTabId: sessionId });
  return { root, focusedPaneId: paneId };
}

export function addTabToPane(
  state: SplitWorkspaceState,
  paneId: string,
  sessionId: string,
  index?: number,
): SplitWorkspaceState {
  if (!sessionId) return state;
  const existing = paneOfSession(state, sessionId);
  if (existing && existing.id !== paneId) {
    return moveTab(state, sessionId, {
      kind: "insert-tab",
      paneId,
      index: index ?? findPane(state, paneId)?.tabIds.length ?? 0,
    });
  }
  const leaf = findPane(state, paneId);
  if (!leaf) return state;
  if (leaf.tabIds.includes(sessionId)) {
    return activateTab(state, paneId, sessionId);
  }
  const at = index == null ? leaf.tabIds.length : Math.max(0, Math.min(index, leaf.tabIds.length));
  const tabIds = [...leaf.tabIds.slice(0, at), sessionId, ...leaf.tabIds.slice(at)];
  const root = replaceNode(state.root, paneId, { ...leaf, tabIds, activeTabId: sessionId });
  return normalize({ root, focusedPaneId: paneId });
}

export function moveTab(
  state: SplitWorkspaceState,
  sessionId: string,
  target: { kind: "activate"; paneId: string } | { kind: "insert-tab"; paneId: string; index: number },
): SplitWorkspaceState {
  const source = paneOfSession(state, sessionId);
  const dest = findPane(state, target.paneId);
  if (!source || !dest) return state;
  if (source.id === dest.id) {
    if (target.kind === "activate") return activateTab(state, dest.id, sessionId);
    const without = source.tabIds.filter((id) => id !== sessionId);
    const at = Math.max(0, Math.min(target.index, without.length));
    const tabIds = [...without.slice(0, at), sessionId, ...without.slice(at)];
    const root = replaceNode(state.root, source.id, { ...source, tabIds, activeTabId: sessionId });
    return normalize({ root, focusedPaneId: source.id });
  }
  const stripped = removeSessionFromTree(state.root, sessionId);
  const destAfter = collectLeaves(stripped).find((l) => l.id === dest.id);
  if (!destAfter) return state;
  const at =
    target.kind === "activate"
      ? destAfter.tabIds.length
      : Math.max(0, Math.min(target.index, destAfter.tabIds.length));
  const tabIds = [...destAfter.tabIds.slice(0, at), sessionId, ...destAfter.tabIds.slice(at)];
  const planted = replaceNode(stripped, dest.id, { ...destAfter, tabIds, activeTabId: sessionId });
  return normalize({ root: planted, focusedPaneId: dest.id });
}

export function splitPane(
  state: SplitWorkspaceState,
  targetPaneId: string,
  edge: DropEdge,
  sessionId: string,
  opts?: { maxLeaves?: number },
): SplitWorkspaceState {
  const maxLeaves = opts?.maxLeaves ?? MAX_PANES;
  const source = paneOfSession(state, sessionId);
  const target = findPane(state, targetPaneId);
  if (!target) return state;
  if (source?.id === target.id && source.tabIds.length === 1) return state;
  if (leafCount(state) >= maxLeaves) {
    return source
      ? moveTab(state, sessionId, { kind: "activate", paneId: targetPaneId })
      : addTabToPane(state, targetPaneId, sessionId);
  }

  const stripped = source ? removeSessionFromTree(state.root, sessionId) : state.root;
  const collapsed = collapseNode(stripped);
  const targetAfter = collectLeaves(collapsed).find((l) => l.id === targetPaneId);
  if (!targetAfter) return normalize({ root: collapsed, focusedPaneId: state.focusedPaneId });

  const mid: SplitWorkspaceState = { root: collapsed, focusedPaneId: state.focusedPaneId };
  const newPaneId = nextId(mid, "pane");
  const newSplitId = nextId(mid, "split");
  const newLeaf: PaneLeaf = { type: "leaf", id: newPaneId, tabIds: [sessionId], activeTabId: sessionId };
  const axis: SplitAxis = edge === "left" || edge === "right" ? "vertical" : "horizontal";
  const newIsA = edge === "left" || edge === "top";
  const branch: SplitBranch = {
    type: "split",
    id: newSplitId,
    axis,
    ratio: DEFAULT_SPLIT_RATIO,
    a: newIsA ? newLeaf : targetAfter,
    b: newIsA ? targetAfter : newLeaf,
  };
  const inserted = replaceNode(collapsed, targetPaneId, branch);
  const top = contiguousAxisRoot(inserted, newPaneId, axis);
  const equalized = equalizeAxis(top, axis);
  const root = top.id === inserted.id ? equalized : replaceNode(inserted, top.id, equalized);
  return normalize({ root, focusedPaneId: newPaneId }, maxLeaves);
}

/**
 * Plant a session that is not (or not only) in the tree — merge-back from a
 * torn-off window. `miss` / `cancel` / `detach` land on the focused pane.
 */
export function adoptExternalSession(
  state: SplitWorkspaceState,
  sessionId: string,
  drop: DropKind,
  opts?: { maxLeaves?: number },
): SplitWorkspaceState {
  if (!sessionId) return state;
  if (drop.kind === "split") {
    return splitPane(state, drop.paneId, drop.edge, sessionId, opts);
  }
  if (drop.kind === "insert-tab") {
    return addTabToPane(state, drop.paneId, sessionId, drop.index);
  }
  if (drop.kind === "activate") {
    return addTabToPane(state, drop.paneId, sessionId);
  }
  const focus = findPane(state, state.focusedPaneId) ?? collectLeaves(state.root)[0];
  if (!focus) return addTabToPane(emptyWorkspace(), "pane-1", sessionId);
  return addTabToPane(state, focus.id, sessionId);
}

/** Physical screen point → CSS client point of a window (inner origin + scale). */
export function clientPointFromScreen(
  phys: { x: number; y: number },
  innerPhys: { x: number; y: number },
  scale: number,
): { x: number; y: number } | null {
  if (!Number.isFinite(phys.x) || !Number.isFinite(phys.y)) return null;
  if (!Number.isFinite(innerPhys.x) || !Number.isFinite(innerPhys.y)) return null;
  if (!Number.isFinite(scale) || scale <= 0) return null;
  return {
    x: (phys.x - innerPhys.x) / scale,
    y: (phys.y - innerPhys.y) / scale,
  };
}

export function closeTab(state: SplitWorkspaceState, sessionId: string): SplitMutation {
  if (!paneOfSession(state, sessionId)) {
    return { state, needPlaceholderChat: false };
  }
  const stripped = removeSessionFromTree(state.root, sessionId);
  const next = normalize({ root: stripped, focusedPaneId: state.focusedPaneId });
  const empty = openSessionIds(next).length === 0;
  return { state: next, needPlaceholderChat: empty };
}

export function popOutTab(state: SplitWorkspaceState, sessionId: string): SplitMutation {
  return closeTab(state, sessionId);
}

export function focusSession(state: SplitWorkspaceState, sessionId: string): SplitWorkspaceState {
  if (!sessionId) return state;
  const leaf = paneOfSession(state, sessionId);
  if (leaf) return activateTab(state, leaf.id, sessionId);
  const focus = findPane(state, state.focusedPaneId) ?? collectLeaves(state.root)[0];
  if (!focus) return addTabToPane(emptyWorkspace(), "pane-1", sessionId);
  return addTabToPane(state, focus.id, sessionId);
}

export function serialize(state: SplitWorkspaceState): SplitLayoutV1 {
  return { version: 1, root: state.root, focusedPaneId: state.focusedPaneId };
}

function parseNode(raw: unknown): SplitNode | null {
  if (!raw || typeof raw !== "object") return null;
  const n = raw as Record<string, unknown>;
  if (n.type === "leaf" && typeof n.id === "string") {
    const tabIds = Array.isArray(n.tabIds) ? n.tabIds.filter((id): id is string => typeof id === "string") : [];
    const activeTabId = typeof n.activeTabId === "string" ? n.activeTabId : "";
    return { type: "leaf", id: n.id, tabIds, activeTabId };
  }
  if (n.type === "split" && typeof n.id === "string") {
    const a = parseNode(n.a);
    const b = parseNode(n.b);
    if (!a || !b) return null;
    const axis = n.axis === "horizontal" ? "horizontal" : n.axis === "vertical" ? "vertical" : null;
    if (!axis) return null;
    const ratio = typeof n.ratio === "number" ? n.ratio : DEFAULT_SPLIT_RATIO;
    return { type: "split", id: n.id, axis, ratio, a, b };
  }
  return null;
}

export function deserialize(
  raw: unknown,
  knownSessionIds: ReadonlySet<string>,
  opts?: { maxLeaves?: number; excludeSessionIds?: ReadonlySet<string> },
): SplitWorkspaceState {
  const maxLeaves = opts?.maxLeaves ?? MAX_PANES;
  const exclude = opts?.excludeSessionIds ?? new Set<string>();
  if (!raw || typeof raw !== "object") return emptyWorkspace();
  const obj = raw as Record<string, unknown>;
  if (obj.version !== 1) return emptyWorkspace();
  const root = parseNode(obj.root);
  if (!root) return emptyWorkspace();
  const filtered = mapLeaves(root, (leaf) => {
    const tabIds = uniqueTabs(leaf.tabIds.filter((id) => knownSessionIds.has(id) && !exclude.has(id)));
    const active = tabIds.includes(leaf.activeTabId) ? leaf.activeTabId : (tabIds[0] ?? "");
    return { ...leaf, tabIds, activeTabId: active };
  });
  const focusedPaneId = typeof obj.focusedPaneId === "string" ? obj.focusedPaneId : firstLeafId(filtered);
  return normalize({ root: filtered, focusedPaneId }, maxLeaves);
}

function pointInRect(x: number, y: number, r: RectLike, vSlop = 0): boolean {
  return x >= r.left && x <= r.right && y >= r.top - vSlop && y <= r.bottom + vSlop;
}

function bodyRect(
  geometry: HitTestInput["geometry"],
  pane: RectLike,
  tabStrip: RectLike,
): RectLike {
  if (geometry === "titlebar-plus-center") return pane;
  const top = Math.max(pane.top, tabStrip.bottom);
  const height = Math.max(0, pane.bottom - top);
  return {
    left: pane.left,
    right: pane.right,
    top,
    bottom: pane.bottom,
    width: pane.width,
    height,
  };
}

function insertIndex(tabXs: number[], x: number): number {
  for (let i = 0; i < tabXs.length; i += 1) {
    if (tabXs[i]! > x) return i;
  }
  return tabXs.length;
}

function pickSplitEdge(
  localX: number,
  localY: number,
  allowHorizontal: boolean,
): DropEdge | null {
  const left = EDGE_FRACTION - localX;
  const right = localX - (1 - EDGE_FRACTION);
  const top = allowHorizontal ? EDGE_FRACTION - localY : -1;
  const bottom = allowHorizontal ? localY - (1 - EDGE_FRACTION) : -1;
  let best: DropEdge | null = null;
  let bestVal = 0;
  const consider = (edge: DropEdge, val: number) => {
    if (val > bestVal) {
      bestVal = val;
      best = edge;
    }
  };
  consider("left", left);
  consider("right", right);
  if (allowHorizontal) {
    consider("top", top);
    consider("bottom", bottom);
  }
  return best;
}

function axisGroupSpan(
  input: HitTestInput,
  paneId: string,
  axis: SplitAxis,
  fallback: RectLike,
): { span: RectLike; slots: number } {
  if (!input.root) return { span: fallback, slots: 1 };
  const top = contiguousAxisRoot(input.root, paneId, axis);
  const slots = axisSlotCount(top, axis);
  const ids = new Set(collectLeaves(top).map((leaf) => leaf.id));
  const rects: RectLike[] = [];
  for (const p of input.panes) {
    if (!ids.has(p.paneId)) continue;
    const body = bodyRect(input.geometry, p.pane, p.tabStrip);
    if (body.width > 0 && body.height > 0) rects.push(body);
  }
  return { span: unionRect(rects) ?? fallback, slots };
}

export function hitTestDropZone(input: HitTestInput): DropKind {
  const { x, y, windowW, windowH } = input;
  if (x < 0 || y < 0 || x > windowW || y > windowH) return { kind: "detach" };

  for (const p of input.panes) {
    if (pointInRect(x, y, p.tabStrip, TAB_DETACH_SLOP_PX)) {
      if (p.paneId === input.source.paneId) {
        return { kind: "cancel", reason: "back-on-source" };
      }
      return { kind: "insert-tab", paneId: p.paneId, index: insertIndex(p.tabXs, x) };
    }
  }

  for (const p of input.panes) {
    const body = bodyRect(input.geometry, p.pane, p.tabStrip);
    if (body.width <= 0 || body.height <= 0) continue;
    if (!pointInRect(x, y, body)) continue;

    const localX = (x - body.left) / body.width;
    const localY = (y - body.top) / body.height;
    const edge = pickSplitEdge(localX, localY, input.allowHorizontal);
    if (!edge) return { kind: "activate", paneId: p.paneId };

    if (p.paneId === input.source.paneId && input.source.tabCountInSource <= 1) {
      return { kind: "cancel", reason: "self-split-last-tab" };
    }
    if (input.leafCount >= input.maxLeaves) {
      return { kind: "activate", paneId: p.paneId };
    }
    const vertical = edge === "left" || edge === "right";
    const axis: SplitAxis = vertical ? "vertical" : "horizontal";
    const minPx = vertical ? MIN_PANE_WIDTH_PX : MIN_PANE_HEIGHT_PX;
    const { span, slots } = axisGroupSpan(input, p.paneId, axis, body);
    const tooNarrow = (vertical ? span.width : span.height) / (slots + 1) < minPx;
    if (tooNarrow) return { kind: "cancel", reason: "too-narrow" };
    return { kind: "split", paneId: p.paneId, edge };
  }

  return { kind: "cancel", reason: "miss" };
}
