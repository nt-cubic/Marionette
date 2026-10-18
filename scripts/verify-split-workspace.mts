/**
 * Split-workspace pane tree + drop-zone hit test.
 *
 * Run: npx tsx scripts/verify-split-workspace.mts
 */
import assert from "node:assert/strict";
import {
  MAX_PANES,
  MAX_PANES_V1,
  MIN_PANE_HEIGHT_PX,
  MIN_PANE_WIDTH_PX,
  TAB_DETACH_SLOP_PX,
  activateTab,
  addTabToPane,
  adoptExternalSession,
  assertInvariants,
  clientPointFromScreen,
  closeTab,
  collectLeaves,
  deserialize,
  emptyWorkspace,
  findPane,
  focusedSessionId,
  focusSession,
  hitTestDropZone,
  leafCount,
  migrateFromFlat,
  moveTab,
  nextId,
  openSessionIds,
  paneOfSession,
  popOutTab,
  serialize,
  splitPane,
  splitPreviewRect,
  visibleSessionIds,
  type HitTestInput,
  type RectLike,
  type SplitWorkspaceState,
} from "../src/lib/splitWorkspace.ts";

let passed = 0;
function check(name: string, fn: () => void) {
  fn();
  passed += 1;
  console.log(`  ok  ${name}`);
}

function expectTree(state: SplitWorkspaceState, maxLeaves = MAX_PANES) {
  assertInvariants(state, maxLeaves);
  return state;
}

function rect(left: number, top: number, width: number, height: number): RectLike {
  return { left, top, width, height, right: left + width, bottom: top + height };
}

function hit(over: Partial<HitTestInput> & Pick<HitTestInput, "x" | "y" | "panes">) {
  return hitTestDropZone({
    windowW: 1200,
    windowH: 800,
    geometry: "titlebar-plus-center",
    source: { sessionId: "B", paneId: "pane-1", tabCountInSource: 2 },
    leafCount: 1,
    allowHorizontal: false,
    maxLeaves: MAX_PANES_V1,
    ...over,
  });
}

console.log("split-workspace model");

check("migrate empty workspace", () => {
  const s = expectTree(migrateFromFlat([], ""));
  assert.equal(s.root.type, "leaf");
  if (s.root.type !== "leaf") return;
  assert.deepEqual(s.root.tabIds, []);
  assert.equal(s.root.activeTabId, "");
  assert.equal(s.focusedPaneId, "pane-1");
  assert.equal(nextId(s, "pane"), "pane-2");
  assert.equal(nextId(s, "split"), "split-1");
});

check("migrate two tabs keeps order and active", () => {
  const s = expectTree(migrateFromFlat(["A", "B"], "A"));
  assert.equal(s.root.type, "leaf");
  if (s.root.type !== "leaf") return;
  assert.deepEqual(s.root.tabIds, ["A", "B"]);
  assert.equal(s.root.activeTabId, "A");
  assert.deepEqual(openSessionIds(s), ["A", "B"]);
  assert.deepEqual(visibleSessionIds(s), ["A"]);
});

check("example 1: split B to the right of a single leaf", () => {
  const before = migrateFromFlat(["A", "B"], "A");
  const after = expectTree(splitPane(before, "pane-1", "right", "B"));
  assert.equal(after.root.type, "split");
  if (after.root.type !== "split") return;
  assert.equal(after.root.id, "split-1");
  assert.equal(after.root.axis, "vertical");
  assert.equal(after.root.ratio, 0.5);
  assert.equal(after.root.a.type, "leaf");
  assert.equal(after.root.b.type, "leaf");
  if (after.root.a.type !== "leaf" || after.root.b.type !== "leaf") return;
  assert.equal(after.root.a.id, "pane-1");
  assert.deepEqual(after.root.a.tabIds, ["A"]);
  assert.equal(after.root.a.activeTabId, "A");
  assert.equal(after.root.b.id, "pane-2");
  assert.deepEqual(after.root.b.tabIds, ["B"]);
  assert.equal(after.root.b.activeTabId, "B");
  assert.equal(after.focusedPaneId, "pane-2");
  assert.deepEqual(openSessionIds(after), ["A", "B"]);
  assert.deepEqual(visibleSessionIds(after), ["A", "B"]);
  assert.equal(focusedSessionId(after), "B");
  assert.equal(nextId(after, "pane"), "pane-3");
  assert.equal(nextId(after, "split"), "split-2");
});

check("example 2: activate B onto the other pane", () => {
  const split = splitPane(migrateFromFlat(["A", "B"], "A"), "pane-1", "right", "B");
  const withC = addTabToPane(split, "pane-1", "C");
  // pane-1 [A,C] / pane-2 [B]; move B is already on pane-2. Rebuild like the spec:
  // before: pane-1 [A,B] active=A / pane-2 [C] — start from two tabs plus a third.
  const two = splitPane(migrateFromFlat(["A", "B", "C"], "A"), "pane-1", "right", "C");
  const before = moveTab(two, "B", { kind: "activate", paneId: "pane-1" });
  // now pane-1 [A,B] / pane-2 [C]; spec wants pane-1 [A,B] active A, pane-2 [C]
  const reset = activateTab(before, "pane-1", "A");
  const after = expectTree(moveTab(reset, "B", { kind: "activate", paneId: "pane-2" }));
  const p1 = findPane(after, "pane-1");
  const p2 = findPane(after, "pane-2");
  assert.deepEqual(p1?.tabIds, ["A"]);
  assert.equal(p1?.activeTabId, "A");
  assert.deepEqual(p2?.tabIds, ["C", "B"]);
  assert.equal(p2?.activeTabId, "B");
  assert.equal(after.focusedPaneId, "pane-2");
  void withC;
});

check("example 3: close last tab of one pane collapses to the other", () => {
  const split = splitPane(migrateFromFlat(["A", "B"], "A"), "pane-1", "right", "B");
  const result = closeTab(split, "A");
  expectTree(result.state);
  assert.equal(result.needPlaceholderChat, false);
  assert.equal(result.state.root.type, "leaf");
  if (result.state.root.type !== "leaf") return;
  assert.equal(result.state.root.id, "pane-2");
  assert.deepEqual(result.state.root.tabIds, ["B"]);
  assert.equal(result.state.focusedPaneId, "pane-2");
});

check("example 4: close last tab of the whole tree asks for a placeholder", () => {
  const result = closeTab(migrateFromFlat(["A"], "A"), "A");
  expectTree(result.state);
  assert.equal(result.needPlaceholderChat, true);
  assert.equal(result.state.root.type, "leaf");
  if (result.state.root.type !== "leaf") return;
  assert.deepEqual(result.state.root.tabIds, []);
  assert.equal(result.state.root.activeTabId, "");
});

check("popOutTab matches closeTab tree shape", () => {
  const split = splitPane(migrateFromFlat(["A", "B"], "A"), "pane-1", "right", "B");
  const closed = closeTab(split, "B");
  const popped = popOutTab(split, "B");
  assert.deepEqual(popped.state, closed.state);
  assert.equal(popped.needPlaceholderChat, closed.needPlaceholderChat);
});

check("example 5: third split at maxLeaves=2 degrades to activate", () => {
  const two = splitPane(migrateFromFlat(["A", "B", "C"], "A"), "pane-1", "right", "B", {
    maxLeaves: MAX_PANES_V1,
  });
  assert.equal(leafCount(two), 2);
  const after = expectTree(
    splitPane(two, "pane-1", "right", "C", { maxLeaves: MAX_PANES_V1 }),
    MAX_PANES_V1,
  );
  assert.equal(leafCount(after), 2);
  assert.ok(findPane(after, "pane-1")?.tabIds.includes("C"));
  assert.equal(findPane(after, "pane-1")?.activeTabId, "C");
  assert.equal(after.focusedPaneId, "pane-1");
  assert.equal(paneOfSession(after, "B")?.id, "pane-2");
});

check("self-split of a pane's last tab is a no-op", () => {
  const one = migrateFromFlat(["A"], "A");
  const after = splitPane(one, "pane-1", "right", "A");
  assert.deepEqual(after, one);
});

check("closing a non-active tab keeps the active", () => {
  const s = migrateFromFlat(["A", "B", "C"], "B");
  const result = closeTab(s, "A");
  expectTree(result.state);
  if (result.state.root.type !== "leaf") throw new Error("expected leaf");
  assert.deepEqual(result.state.root.tabIds, ["B", "C"]);
  assert.equal(result.state.root.activeTabId, "B");
});

check("closing the active tab picks the right neighbor", () => {
  const s = migrateFromFlat(["A", "B", "C"], "B");
  const result = closeTab(s, "B");
  if (result.state.root.type !== "leaf") throw new Error("expected leaf");
  assert.deepEqual(result.state.root.tabIds, ["A", "C"]);
  assert.equal(result.state.root.activeTabId, "C");
});

check("focusSession inserts into the focused pane when missing", () => {
  const split = splitPane(migrateFromFlat(["A", "B"], "A"), "pane-1", "right", "B");
  const after = expectTree(focusSession(split, "Z"));
  assert.equal(paneOfSession(after, "Z")?.id, "pane-2");
  assert.equal(focusedSessionId(after), "Z");
});

check("addTabToPane on empty workspace fills the placeholder leaf", () => {
  const after = expectTree(addTabToPane(emptyWorkspace(), "pane-1", "N"));
  if (after.root.type !== "leaf") throw new Error("expected leaf");
  assert.deepEqual(after.root.tabIds, ["N"]);
  assert.equal(after.root.activeTabId, "N");
});

check("nextId reads the tree, not a module counter", () => {
  const raw = {
    version: 1 as const,
    focusedPaneId: "pane-5",
    root: { type: "leaf" as const, id: "pane-5", tabIds: ["A"], activeTabId: "A" },
  };
  const s = deserialize(raw, new Set(["A"]));
  assert.equal(nextId(s, "pane"), "pane-6");
  assert.equal(nextId(s, "split"), "split-1");
});

check("deserialize drops unknown ids and excluded detached ids", () => {
  const split = splitPane(migrateFromFlat(["A", "B", "C"], "A"), "pane-1", "right", "B");
  const raw = serialize(split);
  const s = expectTree(
    deserialize(raw, new Set(["A", "C"]), { excludeSessionIds: new Set(["C"]) }),
  );
  assert.equal(s.root.type, "leaf");
  if (s.root.type !== "leaf") return;
  assert.deepEqual(s.root.tabIds, ["A"]);
  assert.ok(!openSessionIds(s).includes("B"));
  assert.ok(!openSessionIds(s).includes("C"));
});

check("deserialize maxLeaves=2 flattens a 4-leaf tree without dropping tabs", () => {
  let s = migrateFromFlat(["A", "B", "C", "D"], "A");
  s = splitPane(s, "pane-1", "right", "B", { maxLeaves: 4 });
  s = splitPane(s, "pane-1", "right", "C", { maxLeaves: 4 });
  s = splitPane(s, "pane-2", "right", "D", { maxLeaves: 4 });
  assert.equal(leafCount(s), 4);
  const flat = expectTree(deserialize(serialize(s), new Set(["A", "B", "C", "D"]), { maxLeaves: 2 }), 2);
  assert.equal(leafCount(flat), 2);
  const ids = openSessionIds(flat).sort();
  assert.deepEqual(ids, ["A", "B", "C", "D"]);
});

check("splitPane of a session not in the tree plants a new leaf", () => {
  const s = migrateFromFlat(["A"], "A");
  const after = expectTree(splitPane(s, "pane-1", "right", "D"));
  assert.equal(leafCount(after), 2);
  assert.equal(paneOfSession(after, "A")?.id, "pane-1");
  assert.equal(paneOfSession(after, "D")?.id, "pane-2");
  assert.equal(focusedSessionId(after), "D");
});

check("adoptExternalSession split right from a torn-off session", () => {
  const s = migrateFromFlat(["A"], "A");
  const after = expectTree(
    adoptExternalSession(s, "B", { kind: "split", paneId: "pane-1", edge: "right" }, { maxLeaves: 2 }),
  );
  assert.equal(leafCount(after), 2);
  assert.deepEqual(collectLeaves(after.root).map((l) => l.tabIds), [["A"], ["B"]]);
  assert.equal(focusedSessionId(after), "B");
});

check("adoptExternalSession activate lands on the named pane", () => {
  const split = splitPane(migrateFromFlat(["A", "C"], "A"), "pane-1", "right", "C");
  const after = expectTree(
    adoptExternalSession(split, "B", { kind: "activate", paneId: "pane-2" }, { maxLeaves: 2 }),
  );
  assert.equal(paneOfSession(after, "B")?.id, "pane-2");
  assert.equal(findPane(after, "pane-2")?.activeTabId, "B");
  assert.equal(focusedSessionId(after), "B");
});

check("adoptExternalSession miss/cancel lands on the focused pane", () => {
  const split = splitPane(migrateFromFlat(["A", "C"], "A"), "pane-1", "right", "C");
  const focused = { ...split, focusedPaneId: "pane-1" };
  const after = expectTree(
    adoptExternalSession(focused, "B", { kind: "cancel", reason: "miss" }, { maxLeaves: 2 }),
  );
  assert.equal(paneOfSession(after, "B")?.id, "pane-1");
  assert.equal(focusedSessionId(after), "B");
});

check("adoptExternalSession at maxLeaves degrades split to activate", () => {
  const split = splitPane(migrateFromFlat(["A", "C"], "A"), "pane-1", "right", "C");
  const after = expectTree(
    adoptExternalSession(
      split,
      "B",
      { kind: "split", paneId: "pane-1", edge: "right" },
      { maxLeaves: 2 },
    ),
    2,
  );
  assert.equal(leafCount(after), 2);
  assert.equal(paneOfSession(after, "B")?.id, "pane-1");
  assert.ok(openSessionIds(after).includes("A"));
  assert.ok(openSessionIds(after).includes("C"));
});

check("clientPointFromScreen converts physical to CSS client", () => {
  const pt = clientPointFromScreen({ x: 300, y: 180 }, { x: 100, y: 80 }, 2);
  assert.deepEqual(pt, { x: 100, y: 50 });
});

check("clientPointFromScreen rejects bad scale", () => {
  assert.equal(clientPointFromScreen({ x: 1, y: 1 }, { x: 0, y: 0 }, 0), null);
  assert.equal(clientPointFromScreen({ x: 1, y: 1 }, { x: 0, y: 0 }, Number.NaN), null);
});

console.log("split-workspace hit test");

const TITLEBAR = rect(240, 0, 960, 34);
const CENTER = rect(240, 34, 680, 766);
const INSPECTOR = rect(920, 34, 280, 766);

check("single-leaf: right edge of center splits", () => {
  const kind = hit({
    x: CENTER.left + CENTER.width * 0.9,
    y: 400,
    panes: [{ paneId: "pane-1", pane: CENTER, tabStrip: TITLEBAR, tabXs: [260, 360] }],
  });
  assert.deepEqual(kind, { kind: "split", paneId: "pane-1", edge: "right" });
});

check("single-leaf: center of center is activate (no-op for same pane)", () => {
  const kind = hit({
    x: CENTER.left + CENTER.width * 0.5,
    y: 400,
    panes: [{ paneId: "pane-1", pane: CENTER, tabStrip: TITLEBAR, tabXs: [260, 360] }],
  });
  assert.deepEqual(kind, { kind: "activate", paneId: "pane-1" });
});

check("single-leaf: titlebar (including over inspector) is back-on-source", () => {
  const kind = hit({
    x: 1000,
    y: 16,
    panes: [{ paneId: "pane-1", pane: CENTER, tabStrip: TITLEBAR, tabXs: [260, 360] }],
  });
  assert.deepEqual(kind, { kind: "cancel", reason: "back-on-source" });
});

check("single-leaf: inspector body is miss, not split or detach", () => {
  const kind = hit({
    x: INSPECTOR.left + 40,
    y: 400,
    panes: [{ paneId: "pane-1", pane: CENTER, tabStrip: TITLEBAR, tabXs: [260, 360] }],
  });
  assert.deepEqual(kind, { kind: "cancel", reason: "miss" });
});

check("single-leaf: 28px under the titlebar is still the strip, not split", () => {
  const y = TITLEBAR.bottom + TAB_DETACH_SLOP_PX - 1;
  assert.ok(y > CENTER.top);
  const kind = hit({
    x: CENTER.left + CENTER.width * 0.9,
    y,
    panes: [{ paneId: "pane-1", pane: CENTER, tabStrip: TITLEBAR, tabXs: [260, 360] }],
  });
  assert.deepEqual(kind, { kind: "cancel", reason: "back-on-source" });
});

check("outside the OS window is detach", () => {
  const kind = hit({
    x: -4,
    y: 200,
    panes: [{ paneId: "pane-1", pane: CENTER, tabStrip: TITLEBAR, tabXs: [260, 360] }],
  });
  assert.deepEqual(kind, { kind: "detach" });
});

check("self-split last tab on the right edge is cancelled", () => {
  const kind = hit({
    x: CENTER.left + CENTER.width * 0.9,
    y: 400,
    source: { sessionId: "A", paneId: "pane-1", tabCountInSource: 1 },
    panes: [{ paneId: "pane-1", pane: CENTER, tabStrip: TITLEBAR, tabXs: [260] }],
  });
  assert.deepEqual(kind, { kind: "cancel", reason: "self-split-last-tab" });
});

check("too-narrow center refuses a split", () => {
  const narrow = rect(240, 34, 2 * MIN_PANE_WIDTH_PX - 40, 766);
  const kind = hit({
    x: narrow.left + narrow.width * 0.9,
    y: 400,
    panes: [{ paneId: "pane-1", pane: narrow, tabStrip: TITLEBAR, tabXs: [260, 360] }],
  });
  assert.deepEqual(kind, { kind: "cancel", reason: "too-narrow" });
});

check("per-pane: drop on the other strip inserts", () => {
  const leftPane = rect(240, 34, 400, 766);
  const rightPane = rect(648, 34, 400, 766);
  const leftStrip = rect(240, 34, 400, 28);
  const rightStrip = rect(648, 34, 400, 28);
  const kind = hit({
    geometry: "per-pane",
    leafCount: 2,
    x: 700,
    y: 46,
    source: { sessionId: "B", paneId: "pane-1", tabCountInSource: 2 },
    panes: [
      { paneId: "pane-1", pane: leftPane, tabStrip: leftStrip, tabXs: [260, 360] },
      { paneId: "pane-2", pane: rightPane, tabStrip: rightStrip, tabXs: [660, 760] },
    ],
  });
  assert.equal(kind.kind, "insert-tab");
  if (kind.kind !== "insert-tab") return;
  assert.equal(kind.paneId, "pane-2");
  assert.equal(kind.index, 1);
});

check("per-pane: maxLeaves reached degrades edge drop to activate", () => {
  const leftPane = rect(240, 34, 400, 766);
  const rightPane = rect(648, 34, 400, 766);
  const leftStrip = rect(240, 34, 400, 28);
  const rightStrip = rect(648, 34, 400, 28);
  const kind = hit({
    geometry: "per-pane",
    leafCount: 2,
    maxLeaves: 2,
    x: rightPane.left + rightPane.width * 0.9,
    y: 400,
    source: { sessionId: "A", paneId: "pane-1", tabCountInSource: 2 },
    panes: [
      { paneId: "pane-1", pane: leftPane, tabStrip: leftStrip, tabXs: [260, 360] },
      { paneId: "pane-2", pane: rightPane, tabStrip: rightStrip, tabXs: [660] },
    ],
  });
  assert.deepEqual(kind, { kind: "activate", paneId: "pane-2" });
});

check("source strip is always cancel, never insert (v1 no reorder)", () => {
  const kind = hit({
    x: 300,
    y: 16,
    panes: [{ paneId: "pane-1", pane: CENTER, tabStrip: TITLEBAR, tabXs: [260, 360] }],
  });
  assert.deepEqual(kind, { kind: "cancel", reason: "back-on-source" });
});

check("1|1|1: second right split equalizes three full-height columns", () => {
  let s = expectTree(splitPane(migrateFromFlat(["A", "B", "C"], "A"), "pane-1", "right", "B"));
  s = expectTree(splitPane(s, paneOfSession(s, "B")!.id, "right", "C"));
  assert.equal(leafCount(s), 3);
  assert.equal(s.root.type, "split");
  if (s.root.type !== "split") return;
  assert.equal(s.root.axis, "vertical");
  assert.ok(Math.abs(s.root.ratio - 1 / 3) < 1e-9);
  assert.equal(s.root.a.type, "leaf");
  assert.equal(s.root.b.type, "split");
  if (s.root.a.type !== "leaf" || s.root.b.type !== "split") return;
  assert.equal(s.root.b.axis, "vertical");
  assert.ok(Math.abs(s.root.b.ratio - 0.5) < 1e-9);
  assert.deepEqual(s.root.a.tabIds, ["A"]);
  if (s.root.b.a.type !== "leaf" || s.root.b.b.type !== "leaf") return;
  assert.deepEqual(s.root.b.a.tabIds, ["B"]);
  assert.deepEqual(s.root.b.b.tabIds, ["C"]);
  assert.equal(focusedSessionId(s), "C");
});

check("1|2 does not steal the left column when stacking the right pane", () => {
  let s = expectTree(splitPane(migrateFromFlat(["A", "B", "C"], "A"), "pane-1", "right", "B"));
  s = expectTree(splitPane(s, paneOfSession(s, "B")!.id, "bottom", "C"));
  assert.equal(s.root.type, "split");
  if (s.root.type !== "split") return;
  assert.equal(s.root.axis, "vertical");
  assert.ok(Math.abs(s.root.ratio - 0.5) < 1e-9);
});

check("T-layout 1|2: right then bottom on the right pane", () => {
  let s = expectTree(splitPane(migrateFromFlat(["A", "B", "C"], "A"), "pane-1", "right", "B"));
  const rightId = paneOfSession(s, "B")?.id;
  assert.ok(rightId);
  s = expectTree(splitPane(s, rightId!, "bottom", "C"));
  assert.equal(leafCount(s), 3);
  assert.equal(s.root.type, "split");
  if (s.root.type !== "split") return;
  assert.equal(s.root.axis, "vertical");
  assert.equal(s.root.a.type, "leaf");
  assert.equal(s.root.b.type, "split");
  if (s.root.b.type !== "split") return;
  assert.equal(s.root.b.axis, "horizontal");
  if (s.root.a.type !== "leaf") return;
  assert.deepEqual(s.root.a.tabIds, ["A"]);
  if (s.root.b.a.type !== "leaf" || s.root.b.b.type !== "leaf") return;
  assert.deepEqual(s.root.b.a.tabIds, ["B"]);
  assert.deepEqual(s.root.b.b.tabIds, ["C"]);
  assert.equal(focusedSessionId(s), "C");
});

check("2x2 then fifth leaf degrades to activate", () => {
  let s = splitPane(migrateFromFlat(["A", "B", "C", "D", "E"], "A"), "pane-1", "right", "B");
  s = splitPane(s, paneOfSession(s, "B")!.id, "bottom", "C");
  s = splitPane(s, paneOfSession(s, "A")!.id, "bottom", "D");
  s = expectTree(s);
  assert.equal(leafCount(s), 4);
  const fifth = expectTree(splitPane(s, paneOfSession(s, "A")!.id, "right", "E"));
  assert.equal(leafCount(fifth), 4);
  assert.ok(paneOfSession(fifth, "E")?.tabIds.includes("E"));
  assert.equal(paneOfSession(fifth, "E")?.id, paneOfSession(fifth, "A")?.id);
  assert.equal(fifth.focusedPaneId, paneOfSession(fifth, "E")?.id);
});

check("allowHorizontal top edge of center splits", () => {
  const kind = hit({
    allowHorizontal: true,
    maxLeaves: MAX_PANES,
    x: CENTER.left + CENTER.width * 0.5,
    y: CENTER.top + CENTER.height * 0.08,
    panes: [{ paneId: "pane-1", pane: CENTER, tabStrip: TITLEBAR, tabXs: [260, 360] }],
  });
  assert.deepEqual(kind, { kind: "split", paneId: "pane-1", edge: "top" });
});

check("allowHorizontal false ignores top/bottom and activates", () => {
  const kind = hit({
    allowHorizontal: false,
    x: CENTER.left + CENTER.width * 0.5,
    y: CENTER.top + CENTER.height * 0.08,
    panes: [{ paneId: "pane-1", pane: CENTER, tabStrip: TITLEBAR, tabXs: [260, 360] }],
  });
  assert.deepEqual(kind, { kind: "activate", paneId: "pane-1" });
});

check("too-short pane refuses a horizontal split", () => {
  const short = rect(240, 34, 680, 2 * MIN_PANE_HEIGHT_PX - 40);
  const kind = hit({
    allowHorizontal: true,
    maxLeaves: MAX_PANES,
    x: short.left + short.width * 0.5,
    y: short.top + short.height * 0.92,
    panes: [{ paneId: "pane-1", pane: short, tabStrip: TITLEBAR, tabXs: [260, 360] }],
  });
  assert.deepEqual(kind, { kind: "cancel", reason: "too-narrow" });
});

check("per-pane: wide two-column row allows a third column", () => {
  const two = splitPane(migrateFromFlat(["A", "B", "C"], "A"), "pane-1", "right", "B");
  const leftPane = rect(240, 34, 450, 766);
  const rightPane = rect(696, 34, 450, 766);
  const kind = hit({
    geometry: "per-pane",
    leafCount: 2,
    maxLeaves: MAX_PANES,
    allowHorizontal: true,
    root: two.root,
    x: rightPane.left + rightPane.width * 0.9,
    y: 400,
    source: { sessionId: "C", paneId: "pane-1", tabCountInSource: 2 },
    panes: [
      { paneId: "pane-1", pane: leftPane, tabStrip: rect(240, 34, 450, 28), tabXs: [260, 360] },
      { paneId: "pane-2", pane: rightPane, tabStrip: rect(696, 34, 450, 28), tabXs: [720] },
    ],
  });
  assert.deepEqual(kind, { kind: "split", paneId: "pane-2", edge: "right" });
});

check("per-pane: short two-column row still refuses a third column", () => {
  const two = splitPane(migrateFromFlat(["A", "B", "C"], "A"), "pane-1", "right", "B");
  const leftPane = rect(240, 34, 390, 766);
  const rightPane = rect(636, 34, 390, 766);
  const kind = hit({
    geometry: "per-pane",
    leafCount: 2,
    maxLeaves: MAX_PANES,
    allowHorizontal: true,
    root: two.root,
    x: rightPane.left + rightPane.width * 0.9,
    y: 400,
    source: { sessionId: "C", paneId: "pane-1", tabCountInSource: 2 },
    panes: [
      { paneId: "pane-1", pane: leftPane, tabStrip: rect(240, 34, 390, 28), tabXs: [260, 360] },
      { paneId: "pane-2", pane: rightPane, tabStrip: rect(636, 34, 390, 28), tabXs: [660] },
    ],
  });
  assert.deepEqual(kind, { kind: "cancel", reason: "too-narrow" });
});

check("splitPreviewRect outer third-column uses 1/3 of the row", () => {
  const body = rect(696, 62, 450, 738);
  const group = rect(240, 62, 906, 738);
  const preview = splitPreviewRect(body, group, "right", 3);
  assert.ok(Math.abs(preview.width - 906 / 3) < 1e-6);
  assert.ok(Math.abs(preview.left - (group.right - 906 / 3)) < 1e-6);
});

check("open-beside only-tab of the target pane is a no-op", () => {
  const before = migrateFromFlat(["A"], "A");
  const after = splitPane(before, "pane-1", "right", "A");
  assert.deepEqual(after, before);
});

console.log(`\n${passed} checks passed`);
