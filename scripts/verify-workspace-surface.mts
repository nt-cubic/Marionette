import {
  WORKSPACE_CHAT_MIN_PX,
  WORKSPACE_SURFACE_MIN_PX,
  WORKSPACE_SURFACE_STACK_PX,
  browserTabToOpenInWorkspace,
  clampWorkspaceSurfaceWidth,
  shouldStackWorkspace,
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

console.log("\nall checks passed");
