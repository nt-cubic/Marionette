/**
 * UI restore snapshot: split layout persist / migrate / exclude detached.
 *
 * Run: npx tsx scripts/verify-ui-restore.mts
 */
import assert from "node:assert/strict";
import {
  parseUiRestore,
  pickRestoredSession,
  restoreSplitFromSnapshot,
  type UiRestoreSnapshot,
} from "../src/lib/uiRestore.ts";
import {
  MAX_PANES,
  MAX_PANES_V1,
  assertInvariants,
  collectLeaves,
  focusSession,
  focusedSessionId,
  leafCount,
  migrateFromFlat,
  openSessionIds,
  paneOfSession,
  serialize,
  splitPane,
} from "../src/lib/splitWorkspace.ts";
import type { Session } from "../src/lib/types.ts";

let passed = 0;
function check(name: string, fn: () => void) {
  fn();
  passed += 1;
  console.log(`  ok  ${name}`);
}

const row = (id: string, projectId = "p"): Session => ({
  id,
  projectId,
  agentId: "opencode",
  label: id,
  cwd: "D:\\x",
  status: "waiting",
  processId: null,
  startedAt: "1",
  lastActiveAt: "1",
  transcriptPath: "",
  handoffPath: "",
  viewMode: "clean",
});

console.log("ui-restore split layout");

check("parseUiRestore keeps flat fields and version-1 tree", () => {
  const split = splitPane(migrateFromFlat(["A", "B"], "A"), "pane-1", "right", "B");
  const parsed = parseUiRestore({
    sessionId: "B",
    projectId: "p",
    openSessionIds: ["A", "B"],
    updatedAt: 9,
    splitLayout: serialize(split),
  });
  assert.ok(parsed);
  assert.equal(parsed?.sessionId, "B");
  assert.equal(parsed?.splitLayout?.version, 1);
  assert.equal(parsed?.openSessionIds.length, 2);
});

check("parseUiRestore drops non-v1 splitLayout", () => {
  const parsed = parseUiRestore({
    sessionId: "A",
    projectId: "p",
    openSessionIds: ["A"],
    splitLayout: { version: 2, root: {}, focusedPaneId: "x" },
  });
  assert.ok(parsed);
  assert.equal(parsed?.splitLayout, undefined);
});

check("old snapshot without splitLayout migrates to a single leaf", () => {
  const saved: UiRestoreSnapshot = {
    sessionId: "B",
    projectId: "p",
    openSessionIds: ["A", "B", "C"],
    updatedAt: 1,
  };
  const tree = restoreSplitFromSnapshot(saved, new Set(["A", "B", "C"]), {
    maxLeaves: MAX_PANES_V1,
  });
  assertInvariants(tree, MAX_PANES_V1);
  assert.equal(tree.root.type, "leaf");
  assert.deepEqual(openSessionIds(tree), ["A", "B", "C"]);
  assert.equal(focusedSessionId(tree), "B");
});

check("two-pane snapshot restores both leaves and focused pane", () => {
  const split = splitPane(migrateFromFlat(["A", "B"], "A"), "pane-1", "right", "B");
  const saved: UiRestoreSnapshot = {
    sessionId: "B",
    projectId: "p",
    openSessionIds: ["A", "B"],
    updatedAt: 1,
    splitLayout: serialize(split),
  };
  const tree = restoreSplitFromSnapshot(saved, new Set(["A", "B"]), {
    maxLeaves: MAX_PANES_V1,
  });
  assertInvariants(tree, MAX_PANES_V1);
  assert.equal(leafCount(tree), 2);
  const leaves = collectLeaves(tree.root);
  assert.deepEqual(leaves.map((l) => l.tabIds), [["A"], ["B"]]);
  assert.equal(focusedSessionId(tree), "B");
});

check("excludeSessionIds drops a live detached tab and collapses", () => {
  const split = splitPane(migrateFromFlat(["A", "B"], "A"), "pane-1", "right", "B");
  const saved: UiRestoreSnapshot = {
    sessionId: "B",
    projectId: "p",
    openSessionIds: ["A", "B"],
    updatedAt: 1,
    splitLayout: serialize(split),
  };
  const tree = restoreSplitFromSnapshot(saved, new Set(["A", "B"]), {
    maxLeaves: MAX_PANES_V1,
    excludeSessionIds: new Set(["B"]),
  });
  assertInvariants(tree, MAX_PANES_V1);
  assert.equal(tree.root.type, "leaf");
  assert.deepEqual(openSessionIds(tree), ["A"]);
  assert.equal(focusedSessionId(tree), "A");
});

check("detached URL ignores saved splitLayout", () => {
  const split = splitPane(migrateFromFlat(["A", "B"], "A"), "pane-1", "right", "B");
  const saved: UiRestoreSnapshot = {
    sessionId: "A",
    projectId: "p",
    openSessionIds: ["A", "B"],
    updatedAt: 1,
    splitLayout: serialize(split),
  };
  const tree = restoreSplitFromSnapshot(saved, new Set(["A", "B"]), {
    detachedSessionId: "B",
    maxLeaves: MAX_PANES_V1,
  });
  assert.equal(tree.root.type, "leaf");
  if (tree.root.type !== "leaf") return;
  assert.deepEqual(tree.root.tabIds, ["B"]);
  assert.equal(tree.root.activeTabId, "B");
});

check("unknown session ids are dropped from the tree", () => {
  const split = splitPane(migrateFromFlat(["A", "B"], "A"), "pane-1", "right", "B");
  const saved: UiRestoreSnapshot = {
    sessionId: "B",
    projectId: "p",
    openSessionIds: ["A", "B"],
    updatedAt: 1,
    splitLayout: serialize(split),
  };
  const tree = restoreSplitFromSnapshot(saved, new Set(["A"]), {
    maxLeaves: MAX_PANES_V1,
  });
  assert.equal(tree.root.type, "leaf");
  assert.deepEqual(openSessionIds(tree), ["A"]);
});

check("pickRestoredSession returns tree focus after excluding the saved sessionId", () => {
  const split = splitPane(migrateFromFlat(["A", "B"], "A"), "pane-1", "right", "B");
  const restored = pickRestoredSession([row("A"), row("B")], {
    snapshot: {
      sessionId: "B",
      projectId: "p",
      openSessionIds: ["A", "B"],
      updatedAt: 1,
      splitLayout: serialize(split),
    },
    excludeSessionIds: new Set(["B"]),
    maxLeaves: MAX_PANES_V1,
  });
  assert.ok(restored);
  assert.equal(restored?.session.id, "A");
  assert.deepEqual(restored?.openSessionIds, ["A"]);
  assert.equal(leafCount(restored!.splitState), 1);
});

check("pickRestoredSession detached URL wins over a two-pane snapshot", () => {
  const split = splitPane(migrateFromFlat(["A", "B"], "A"), "pane-1", "right", "B");
  const restored = pickRestoredSession([row("A"), row("B")], {
    detachedSessionId: "A",
    snapshot: {
      sessionId: "B",
      projectId: "p",
      openSessionIds: ["A", "B"],
      updatedAt: 1,
      splitLayout: serialize(split),
    },
    maxLeaves: MAX_PANES_V1,
  });
  assert.ok(restored);
  assert.equal(restored?.session.id, "A");
  assert.deepEqual(restored?.openSessionIds, ["A"]);
  assert.equal(restored?.splitState.root.type, "leaf");
});

check("empty snapshot leaves the workspace empty", () => {
  const restored = pickRestoredSession([row("A")], { snapshot: null });
  assert.equal(restored, null);
});

check("four-pane snapshot restores with maxLeaves=4", () => {
  let split = splitPane(migrateFromFlat(["A", "B", "C", "D"], "A"), "pane-1", "right", "B");
  split = splitPane(split, paneOfSession(split, "B")!.id, "bottom", "C");
  split = splitPane(split, paneOfSession(split, "A")!.id, "bottom", "D");
  split = focusSession(split, "C");
  const saved: UiRestoreSnapshot = {
    sessionId: "C",
    projectId: "p",
    openSessionIds: ["A", "B", "C", "D"],
    updatedAt: 1,
    splitLayout: serialize(split),
  };
  const tree = restoreSplitFromSnapshot(saved, new Set(["A", "B", "C", "D"]), {
    maxLeaves: MAX_PANES,
  });
  assertInvariants(tree, MAX_PANES);
  assert.equal(leafCount(tree), 4);
  assert.equal(focusedSessionId(tree), "C");
});

console.log(`\n${passed} checks passed`);
