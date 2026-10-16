// Run with Node's test runner and Playwright on the module path; see README.md.
// Settings › Models › Replies (#822, liuweifeng: a reply's model named only
// the vendor's glm-5.3-flash, so pi counted every provider's in one bucket):
// Name the member in replies is Off, says what it does and whom it leaves
// as they are, and On saves memberModel with the rest of the settings kept,
// the page not moved by the click. In English and Chinese.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");
const { watchSettingsReads, settingsDrawn } = require("./reader.cjs");

const assets = path.resolve(__dirname, "../assets");
const words = {
  en: { name: "Name the member in replies", head: "Replies", on: "On", sub: "Claude Code, Claude Desktop and Codex keep the vendor's" },
  zh: { name: "回复里写明成员", head: "回复", on: "开启", sub: "Claude Code、Claude Desktop 和 Codex 仍拿到厂商的名字" },
};

function serve(lang, saved) {
  const settings = { lang, theme: "light", redact: true };
  const state = { agents: [], profiles: [], settings };
  return async (r) => {
    const url = new URL(r.request().url());
    const json = (data) => r.fulfill({ json: data });
    if (url.pathname === "/boot.js") return r.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"${lang}",theme:"light",web:true};` });
    if (url.pathname === "/wails/runtime.js") return r.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json(state);
    if (url.pathname === "/api/settings") {
      if (r.request().method() === "POST") {
        saved.push(JSON.parse(r.request().postData()));
        Object.assign(settings, saved.at(-1));
      }
      return json(settings);
    }
    if (url.pathname === "/api/groups") return json({ models: [], groups: [], pools: [] });
    if (url.pathname === "/api/providers") return json({ providers: [], presets: [], excluded: [], gateway: { running: true, window: true, url: "http://127.0.0.1:3999" } });
    if (url.pathname.startsWith("/api/")) return json({});
    const file = path.join(assets, url.pathname === "/" ? "index.html" : url.pathname);
    const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)];
    await r.fulfill({ body: await fs.readFile(file), contentType });
  };
}

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  for (const lang of ["en", "zh"]) {
    const w = words[lang];
    test(`${engine} ${lang}: replies can name the member`, async (t) => {
      const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
      t.after(() => browser.close());
      const page = await (await browser.newContext({ viewport: { width: 1100, height: 900 } })).newPage();
      page.setDefaultTimeout(5000);
      const errors = [];
      page.on("pageerror", (e) => errors.push(e.message));
      const saved = [];
      await page.route("**/*", serve(lang, saved));
      await watchSettingsReads(page);
      await page.goto("http://magpie.test/?view=settings&tab=models");
      await settingsDrawn(page); // rows found are the ones that stay (#1659)
      const row = page.locator("#replyList #memberModelRow");
      await row.waitFor();
      await row.scrollIntoViewIfNeeded();
      assert.equal(await row.locator(".name").textContent(), w.name);
      assert((await row.locator(".sub").textContent()).includes(w.sub));
      assert.equal(await page.locator("#replyList").evaluate((l) => l.previousElementSibling.textContent.trim()), w.head);
      const on = row.locator(".segs button", { hasText: w.on });
      assert.notEqual(await on.getAttribute("aria-pressed"), "true", "Off at first");

      const before = await page.evaluate(() => [scrollX, scrollY, document.scrollingElement.scrollTop]);
      const b = await on.boundingBox();
      await page.mouse.click(b.x + b.width / 2, b.y + b.height / 2);
      await page.waitForFunction(() => document.querySelector("#memberModelRow"));
      for (let i = 0; i < 50 && !saved.length; i++) await page.waitForTimeout(50);
      assert.equal(saved.length, 1, "saved once");
      assert.equal(saved[0].memberModel, true);
      assert.equal(saved[0].redact, true, "the rest kept");
      assert.equal(saved[0].lang, lang);
      assert.deepEqual(await page.evaluate(() => [scrollX, scrollY, document.scrollingElement.scrollTop]), before, "the click moved nothing");
      assert.deepEqual(errors, []);
    });
  }
}
