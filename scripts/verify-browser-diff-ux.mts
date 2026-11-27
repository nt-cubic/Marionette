/** Focused regression for browser find / viewport and diff presentation. */
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  buildDiffDisplay,
  diffLineText,
  findDiffMatches,
  squashDiffText,
} from "../src/lib/diffPresentation.ts";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const root = path.join(HERE, "..");
const browser = readFileSync(path.join(root, "src/components/BrowserPage.tsx"), "utf8");
const browserApi = readFileSync(path.join(root, "src/lib/browserTabs.ts"), "utf8");
const diffPage = readFileSync(path.join(root, "src/components/DiffPage.tsx"), "utf8");

const diff = "diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1,2 +1,2 @@\n-hello world\n+hello  world\n context\n+new line\n";

const rows = buildDiffDisplay(diff, { ignoreWhitespace: true });
assert.ok(rows.lines.length >= 6, "diff display lost rows");
assert.equal(squashDiffText("-hello world"), squashDiffText("+hello  world"));
assert.ok(
  rows.lines.some((row) => row.line.type === "del" && row.wsIgnored) &&
    rows.lines.some((row) => row.line.type === "add" && row.wsIgnored),
  "whitespace-only pairs were not marked",
);
assert.equal(diffLineText({ type: "ctx", raw: " hello" } as never), "hello");

const matches = findDiffMatches(rows.lines, "HELLO");
assert.equal(matches.length, 2, "diff find should match both body rows");
assert.ok(matches.every((match) => rows.lines[match.row]?.line.type !== "hunk"));
assert.equal(findDiffMatches(rows.lines, "a/a.txt").length, 0, "diff find must skip meta rows");

assert.ok(browser.includes("browserFind("), "BrowserPage is not wired to browserFind");
assert.ok(browser.includes("browserViewportSet("), "BrowserPage is not wired to viewport emulation");
assert.ok(browser.includes('event.key !== "f"'), "browser find shortcut missing");
assert.ok(
  browserApi.includes("invoke<BrowserFindResult>(\"browser_find\""),
  "browser find IPC wrapper missing",
);
assert.ok(diffPage.includes('event.key !== "f"'), "diff Ctrl+F capture missing");
assert.ok(diffPage.includes("marionette-diff-ignore-whitespace"), "ignore-whitespace preference missing");
assert.ok(diffPage.includes("marionette-diff-colorblind"), "colorblind preference missing");

console.log("verify-browser-diff-ux: OK");
