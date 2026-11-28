/**
 * verify-command-palette - Ctrl K stays searchable without showing the whole
 * settings catalog on first paint, and recent-project discovery stays out of
 * the normal project shelf.
 *
 * Run: npx tsx scripts/verify-command-palette.mts
 */

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const read = (path: string) => readFileSync(join(root, path), "utf8");
const palette = read("src/components/CommandPalette.tsx");
const app = read("src/app/App.tsx");
const shelf = read("src/components/ProjectShelf.tsx");
const guide = read("src/components/GettingStartedGuide.tsx");
const zh = read("src/locales/parts/palette.zh.ts");
const en = read("src/locales/parts/palette.en.ts");

let failures = 0;
function check(name: string, ok: boolean) {
  if (ok) {
    console.log(`  ok  ${name}`);
  } else {
    failures += 1;
    console.error(`FAIL  ${name}`);
  }
}

check("empty-query recent list is capped at six", palette.includes("const RECENT_SHOWN = 6"));
check("actions can opt into the empty-query quick row", palette.includes("primary?: boolean"));
check("secondary actions fold behind one row", palette.includes("showAllActions") && palette.includes('kind: "more"'));
check("expanded/query results are grouped", palette.includes("ACTION_GROUP_ORDER") && palette.includes("groupedActions"));
check("typing searches labels and hints", palette.includes("action.label} ${action.hint"));
check(
  "the five daily shortcuts remain primary",
  ["model-settings", "new-session", "new-chat", "open-memo", "open-usage-analysis"].every((id) => {
    const at = app.indexOf(`id: "${id}"`);
    return at >= 0 && app.slice(at, at + 260).includes("primary: true");
  }),
);
check("project shelf no longer mounts recent discovery", !shelf.includes("RecentProjectDiscovery"));
check("getting-started guide no longer mounts recent discovery", !guide.includes("RecentProjectDiscovery"));
check(
  "both locales carry every new group label",
  [
    "palette.groupQuick",
    "palette.groupCreate",
    "palette.groupWorkspace",
    "palette.groupSettings",
    "palette.groupStaff",
    "palette.groupSystem",
    "palette.showMore",
    "palette.hideMore",
    "palette.moreHint",
  ].every((key) => zh.includes(`"${key}"`) && en.includes(`"${key}"`)),
);

if (failures > 0) {
  console.error(`\ncommand palette: ${failures} check(s) failed`);
  process.exit(1);
}
console.log("\ncommand palette: all checks passed");
