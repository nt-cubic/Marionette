// Run with Node's test runner and Playwright on the module path; see README.md.
// The provider editor's Context window field offers 400K and 512K beside
// 128K, 200K, 256K and 1M, one click each, the one clicked lit (yuexian7 on
// Discord). Every language, at a wide and a narrow window, with no
// sideways scroll.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");

const assets = path.resolve(__dirname, "../assets");

function serve(lang, posts) {
  const state = { agents: [], profiles: [], settings: { lang, theme: "light" } };
  const models = [{ id: "model-1", name: "Model 1", on: true, efforts: [], images: false }];
  const provider = { id: "relay", name: "Relay", icon: "generic", chat: "https://relay.example/v1", responses: "", anthropic: "", models, agents: [], key: { set: true, masked: "sk-…1234" }, ready: true };
  const providers = { providers: [provider], presets: [], excluded: [], gateway: { running: true, window: true } };
  return async (route) => {
    const url = new URL(route.request().url());
    const json = (data) => route.fulfill({ json: data });
    if (url.pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:${JSON.stringify(lang)},theme:"light",web:true};` });
    if (url.pathname === "/wails/runtime.js") return route.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json(state);
    if (url.pathname === "/api/providers") return json(providers);
    if (url.pathname === "/api/groups") return json({ groups: [] });
    if (url.pathname.startsWith("/api/provider/")) {
      posts.push({ path: url.pathname, body: route.request().postDataJSON() });
      return json(providers);
    }
    if (url.pathname.startsWith("/api/")) return json({});
    const file = path.join(assets, url.pathname === "/" ? "index.html" : url.pathname);
    await route.fulfill({ body: await fs.readFile(file), contentType: { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)] });
  };
}


for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  for (const width of [1100, 420]) {
    for (const lang of ["en", "zh", "zh-TW", "ja", "de"]) {
      test(`${engine} ${width} ${lang}: the Context window offers 400K and 512K`, async (t) => {
        const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: process.env.PLAYWRIGHT_CHANNEL || "chromium" }));
        const page = await (await browser.newContext({ viewport: { width, height: 900 }, reducedMotion: "reduce" })).newPage();
        t.after(async () => {
          if (process.env.ARTIFACT_DIR) {
            await fs.mkdir(process.env.ARTIFACT_DIR, { recursive: true });
            await page.screenshot({ path: path.join(process.env.ARTIFACT_DIR, `${engine}-${width}-${lang}-context-picks.png`) });
          }
          await browser.close();
        });
        page.setDefaultTimeout(5000);
        const errors = [], posts = [];
        page.on("pageerror", (e) => errors.push(e.message));
        await page.route("**/*", serve(lang, posts));
        await page.goto("http://magpie.test/?view=providers");
        await page.locator(".row.provider").click();
        // the Context window is under Advanced
        { const adv = page.locator(".editor details.advanced"); await adv.waitFor({ state: "attached" }); if (!(await adv.evaluate((d) => d.open))) await adv.locator(":scope > summary").click(); }

        const picks = page.locator(".cxpicks").first();
        assert.deepEqual(await picks.locator(".cxpick").allTextContents(), ["128K", "200K", "256K", "400K", "512K", "1M"]);
        for (const [label, value] of [["400K", "400k"], ["512K", "512k"]]) {
          await picks.getByRole("button", { name: label, exact: true }).click();
          assert.deepEqual(await picks.locator(".cxpick.on").allTextContents(), [label]);
          assert.equal(await picks.evaluate((r) => r.parentElement.querySelector("input")?.value ?? r.previousElementSibling?.value), value);
        }
        assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true, "no sideways scroll");
        assert.deepEqual(errors, []);
      });
    }
  }
}
