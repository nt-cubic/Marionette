/**
 * verify-composer-compact-toolbar — 窄格/窄窗底栏收成贴边同款 hub
 *
 * 分屏把某一格 composer 压窄时，模式/权限/模型/Agent 芯片叠在发送钮上。
 * 贴边窄条已经有 hub（滑杆 + 当前模型名）；普通窗口和分屏过窄时走同一套。
 *
 * 跑法：npx tsx scripts/verify-composer-compact-toolbar.mts
 */

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const composer = readFileSync(join(root, "src/components/Composer.tsx"), "utf8");
const css = readFileSync(join(root, "src/styles/stream-cards.css"), "utf8");
const dock = readFileSync(join(root, "src/styles/dock.css"), "utf8");

let failures = 0;
function check(name: string, ok: boolean, detail = "") {
  if (ok) {
    console.log(`  ok  ${name}`);
  } else {
    failures += 1;
    console.error(`FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
  }
}

check(
  "enter/exit thresholds exist (hysteresis)",
  composer.includes("COMPACT_TOOLBAR_ENTER_PX = 580") &&
    composer.includes("COMPACT_TOOLBAR_EXIT_PX = 640"),
);

check(
  "JS measures this composer, not the window",
  composer.includes("compactByWidth") &&
    composer.includes("el.getBoundingClientRect().width") &&
    composer.includes("new ResizeObserver"),
);

check(
  "docked or compactByWidth sets is-compact",
  composer.includes("const compactToolbar = docked || compactByWidth") &&
    composer.includes('compactToolbar ? "is-compact" : ""'),
);

check(
  "hub is always in the tree (container query first paint)",
  composer.includes("composer-menu-anchor--hub") &&
    !composer.includes("{compactToolbar && (") &&
    composer.includes('className="composer-dock-hub"'),
);

check(
  "chips stay in the tree so menus still open from hub rows",
  composer.includes("composer-mode-chip") &&
    composer.includes("composer-select--model") &&
    composer.includes("composer-select--agent") &&
    !composer.includes("{!compactToolbar && ("),
);

check(
  "container-type on .composer + named container query",
  css.includes("container-type: inline-size") &&
    css.includes("container-name: composer") &&
    css.includes("@container composer (max-width: 580px)"),
);

check(
  "hub hidden until compact; chips hidden in compact",
  css.includes(".composer-menu-anchor--hub") &&
    css.includes("display: none") &&
    css.includes(".composer.is-compact .composer-menu-anchor--hub") &&
    css.includes(".composer.is-compact .composer-mode-chip") &&
    css.includes(".composer.is-compact .composer-select--model") &&
    css.includes(".composer.is-compact .composer-select--agent"),
);

check(
  "dock.css no longer hides chips itself (shared is-compact path)",
  !dock.includes(".dock-mode .composer-mode-chip,") &&
    !dock.includes(".dock-mode .composer-select--model,") &&
    !dock.includes(".dock-mode .composer-select--agent {"),
);

check(
  "container query uses a stronger chip selector than .composer-select",
  css.includes(".composer-select.composer-select--model") &&
    css.includes(".composer-select.composer-select--agent"),
);

if (failures > 0) {
  console.error(`\n${failures} check(s) failed`);
  process.exit(1);
}
console.log("\ncomposer compact toolbar: all checks passed");
