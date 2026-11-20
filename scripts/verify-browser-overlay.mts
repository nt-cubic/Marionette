import assert from "node:assert/strict";
import { browserHostCovered } from "../src/lib/browserOverlay";

type Rect = { left: number; top: number; right: number; bottom: number };
const rect = (left: number, top: number, right: number, bottom: number): Rect => ({ left, top, right, bottom });
const host = { getBoundingClientRect: () => rect(100, 100, 700, 600) } as HTMLElement;
function overlay(bounds: Rect, options: { modal?: boolean; hidden?: boolean } = {}) {
  return {
    getBoundingClientRect: () => bounds,
    getClientRects: () => options.hidden ? [] : [bounds],
    getAttribute: (name: string) => name === "aria-modal" && options.modal ? "true" : null,
  };
}
let overlays: ReturnType<typeof overlay>[] = [];
let queriedSelector = "";
Object.assign(globalThis, {
  document: { querySelectorAll: (selector: string) => { queriedSelector = selector; return overlays; } },
  getComputedStyle: () => ({ visibility: "visible" }),
});

assert.equal(browserHostCovered(host), false);
assert.match(queriedSelector, /\.project-dialog-backdrop/, "legacy modal backdrops are observed even without aria-modal");
assert.match(queriedSelector, /\.permission-dialog-backdrop/);
overlays = [overlay(rect(0, 0, 1000, 800))];
assert.equal(browserHostCovered(host), true, "a full-window backdrop hides the browser even if its dialog panel is elsewhere");
overlays = [overlay(rect(0, 0, 50, 50), { modal: true })];
assert.equal(browserHostCovered(host), true, "modal backdrop covers the browser even if its panel does not");
overlays = [overlay(rect(500, 150, 800, 300))];
assert.equal(browserHostCovered(host), true, "overlapping menu or non-modal dialog");
overlays = [overlay(rect(0, 0, 50, 50))];
assert.equal(browserHostCovered(host), false, "unrelated popover does not hide the browser");
overlays = [overlay(rect(100, 100, 200, 200), { hidden: true })];
assert.equal(browserHostCovered(host), false, "hidden overlays do not block the browser");
overlays = [overlay(rect(0, 0, 50, 50), { modal: true }), overlay(rect(500, 150, 800, 300))];
overlays.shift();
assert.equal(browserHostCovered(host), true, "remaining overlay keeps the surface hidden");
overlays.pop();
assert.equal(browserHostCovered(host), false, "surface can return when all overlays close");
console.log("browser overlay checks passed");
