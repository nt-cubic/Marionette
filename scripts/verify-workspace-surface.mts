import {
  WORKSPACE_CHAT_MIN_PX,
  WORKSPACE_SURFACE_MIN_PX,
  WORKSPACE_SURFACE_STACK_PX,
  browserTabToOpenInWorkspace,
  clampWorkspaceSurfaceWidth,
  documentPathFromSurfaceTabId,
  documentSurfaceTabId,
  shouldStackWorkspace,
  workspaceSurfaceTabIds,
  workspaceSurfaceWidthFromPointer,
} from "../src/lib/workspaceSurface";

function ok(condition: unknown, label: string) {
  if (!condition) throw new Error(label);
  console.log(`  ok  ${label}`);
}

ok(
  shouldStackWorkspace(WORKSPACE_SURFACE_STACK_PX - 1),
  "narrow workspace stacks",
);
ok(
  !shouldStackWorkspace(WORKSPACE_SURFACE_STACK_PX),
  "stack breakpoint is inclusive on wide side",
);
ok(
  clampWorkspaceSurfaceWidth(100, 1200) === WORKSPACE_SURFACE_MIN_PX,
  "surface keeps its minimum width",
);
ok(
  clampWorkspaceSurfaceWidth(1100, 1200) === 1200 - WORKSPACE_CHAT_MIN_PX,
  "chat keeps its minimum width",
);
ok(
  clampWorkspaceSurfaceWidth(620, 1000) === 620,
  "valid dragged width survives",
);
ok(
  workspaceSurfaceWidthFromPointer(
    620,
    { left: 0, right: 1000, width: 1000 },
    "left",
  ) === 620,
  "left surface follows pointer",
);
ok(
  workspaceSurfaceWidthFromPointer(
    380,
    { left: 0, right: 1000, width: 1000 },
    "right",
  ) === 620,
  "right surface follows pointer from the right edge",
);
ok(
  clampWorkspaceSurfaceWidth(620, 0) === WORKSPACE_SURFACE_MIN_PX,
  "unmeasured layout has a safe width",
);
ok(
  browserTabToOpenInWorkspace([
    { id: "b1", display: "background" },
    { id: "b2", display: "workspace" },
    { id: "b3", display: "background" },
  ]) === "b2",
  "workspace tab wins over background tabs",
);
ok(
  browserTabToOpenInWorkspace([{ id: "b1", display: "background" }]) === null,
  "background tab never steals the workspace",
);
ok(
  browserTabToOpenInWorkspace([{ id: "b1", display: "workspace" }], true) ===
    null,
  "test suppression blocks workspace activation",
);
ok(
  documentPathFromSurfaceTabId(documentSurfaceTabId("D:\\项目\\README.md")) ===
    "D:\\项目\\README.md",
  "document surface id round-trips Windows paths",
);
ok(
  documentPathFromSurfaceTabId("memo") === null,
  "non-document surface ids do not parse as document paths",
);
ok(
  workspaceSurfaceTabIds(
    true,
    ["D:\\项目\\README.md", "D:\\项目\\PLAN.md"],
    true,
  ).join("|") ===
    [
      "browser",
      documentSurfaceTabId("D:\\项目\\README.md"),
      documentSurfaceTabId("D:\\项目\\PLAN.md"),
      "memo",
    ].join("|"),
  "surface tabs keep one shared browser/document/memo order",
);

console.log("\nall checks passed");
