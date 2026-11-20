import {
  browserTabToOpenInWorkspace,
  documentSurfaceTabId,
} from "../src/lib/workspaceSurface";
import { documentTabPath } from "../src/lib/splitWorkspace";

function ok(condition: unknown, label: string) {
  if (!condition) throw new Error(label);
  console.log(`  ok  ${label}`);
}

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
  documentTabPath(documentSurfaceTabId("D:\\项目\\README.md")) ===
    "D:\\项目\\README.md",
  "document surface id round-trips Windows paths",
);
ok(
  documentTabPath("memo") === null,
  "non-document surface ids do not parse as document paths",
);

console.log("\nall checks passed");
