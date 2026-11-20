import assert from "node:assert/strict";
import { openInBrowserWorkspace, setBrowserLinkOpener } from "../src/lib/openBrowserLink";
import { splitLinkSegments } from "../src/lib/linkTargets";

const url = "http://127.0.0.1:8138/Prototypes/DialogueLab/";
assert.equal(await openInBrowserWorkspace(url), false, "no browser host allows the caller to fall back");
const opened: string[] = [];
setBrowserLinkOpener(async (value) => { opened.push(value); });
assert.equal(await openInBrowserWorkspace(url), true);
assert.equal(await openInBrowserWorkspace("https://example.com/path?a=1"), true);
assert.deepEqual(opened, [url, "https://example.com/path?a=1"]);
assert.equal(await openInBrowserWorkspace("file:///C:/test.md"), false);
assert.equal(await openInBrowserWorkspace("javascript:alert(1)"), false);
assert.ok(splitLinkSegments(`打开 ${url}`).some((item) => item.type === "link" && item.target.raw === url));
setBrowserLinkOpener(async () => { throw new Error("open failed"); });
await assert.rejects(openInBrowserWorkspace(url), /open failed/);
setBrowserLinkOpener(null);
assert.equal(await openInBrowserWorkspace(url), false);
console.log("browser link checks passed");
