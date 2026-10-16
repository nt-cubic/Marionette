// Run with Node's test runner and Playwright on the module path; see README.md.
// The Usage page's cards are arranged as the Agents page's rows are (the
// owner: 用量页面能否支持供应商排序？像 agents 页面一样): a card's logo is
// its handle, with a grip by it only while the pointer is on the card; it
// is dragged across the grid, or moved with Alt+arrows, and the order is
// saved (usage/arrange) and drawn again from settings. The tray panel's
// Usage tab follows the same order. No click moves the page. Chromium and
// WebKit, English and Chinese; no backend, the API is faked here.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");

const assets = path.resolve(__dirname, "../assets");
const win = (name, used) => ({ name, used, resetSecs: 3 * 3600 });
const quotas = [
  { provider: "codex", name: "Codex", icon: "openai", user: "a@x.dev", plan: "plus", windows: [win("5h", 40), win("Weekly", 20)] },
  { provider: "codex", name: "Codex", icon: "openai", user: "b@x.dev", plan: "pro", windows: [win("5h", 10)] },
  { provider: "claude", name: "Claude", icon: "claude", plan: "max", windows: [win("5h", 70)] },
  { provider: "kimi", name: "Kimi", icon: "kimi", windows: [win("Weekly", 55)] },
  { provider: "zai", name: "Z.ai", icon: "zai", windows: [win("5h", 5)] },
];

function serve(lang, panel, settings, saved) {
  return async (route) => {
    const url = new URL(route.request().url());
    const json = (data) => route.fulfill({ json: data });
    if (url.pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"${lang}",theme:"light",web:${!panel}};` });
    if (url.pathname === "/wails/runtime.js") return route.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json({ agents: [], profiles: [], settings });
    if (url.pathname === "/api/settings") return json(settings);
    if (url.pathname === "/api/usage/arrange") {
      const { order } = route.request().postDataJSON();
      saved.push(order);
      settings.usageOrder = order;
      return json(settings);
    }
    if (url.pathname === "/api/providers") return json({ providers: [], presets: [], excluded: [], gateway: { running: true, window: true } });
    if (url.pathname === "/api/usage/quotas") return json(quotas);
    if (url.pathname === "/api/groups") return json({ groups: [], models: [] });
    if (url.pathname === "/api/plugins") return json({ plugins: [] });
    if (url.pathname.startsWith("/api/")) return json({});
    const file = path.join(assets, url.pathname === "/" ? "index.html" : url.pathname);
    const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)];
    try { await route.fulfill({ body: await fs.readFile(file), contentType }); } catch { await route.fulfill({ status: 404, body: "" }); }
  };
}

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  for (const lang of ["en", "zh"]) {
    test(`${engine} ${lang}: the Usage page's cards are dragged into an order that holds`, async (t) => {
      const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
      const pages = [];
      t.after(async () => {
        if (process.env.ARTIFACT_DIR) {
          await fs.mkdir(process.env.ARTIFACT_DIR, { recursive: true });
          for (const [name, p] of pages) await p.screenshot({ path: path.join(process.env.ARTIFACT_DIR, `${engine}-${lang}-usage-arrange-${name}.png`) });
        }
        await browser.close();
      });
      const settings = { theme: "light", lang, tray: "panel", currency: "usd" };
      const saved = [], errors = [];
      const open = async (name, url, viewport) => {
        const page = await (await browser.newContext({ viewport })).newPage();
        pages.push([name, page]);
        page.setDefaultTimeout(5000);
        page.on("pageerror", (e) => errors.push(e.message));
        await page.route("**/*", serve(lang, url.includes("mode=panel"), settings, saved));
        await page.goto(url);
        return page;
      };
      const page = await open("usage", "http://magpie.test/?view=usage", { width: 1100, height: 760 });
      const cards = page.locator("#subscriptionUsage > .subscription-card");
      const order = () => cards.evaluateAll((cs) => cs.map((c) => c.dataset.key));
      const handle = (key) => page.locator(`#subscriptionUsage > [data-key="${key}"] .us-handle`);
      await handle("kimi").waitFor();
      assert.deepEqual(await order(), ["codex", "claude", "kimi", "zai"], "magpie's own order first");

      // the grip only while the pointer is on the card
      const grip = (key) => handle(key).evaluate((h) => getComputedStyle(h, "::before").opacity);
      await page.mouse.move(5, 5);
      await page.waitForTimeout(250);
      assert.equal(await grip("kimi"), "0", "no grip without the pointer");
      await page.locator('#subscriptionUsage > [data-key="kimi"] .subscription-head b').hover();
      await page.waitForTimeout(250);
      assert.equal(await grip("kimi"), "1", "a grip on hover");

      // dragged across the grid onto the first card's place
      const scrollY = () => page.evaluate(() => [document.scrollingElement.scrollTop, ...[...document.querySelectorAll("*")].filter((e) => e.scrollTop).map((e) => e.scrollTop)].join());
      const before = await scrollY();
      const h = await handle("kimi").boundingBox(), to = await cards.first().boundingBox();
      await page.mouse.move(h.x + h.width / 2, h.y + h.height / 2);
      await page.mouse.down();
      for (let i = 1; i <= 12; i++) await page.mouse.move(h.x + (to.x + 30 - h.x) * i / 12, h.y + (to.y + 30 - h.y) * i / 12);
      await page.waitForTimeout(250);
      assert.equal(await page.locator("#subscriptionUsage.sorting > .dragging").count(), 1, "the card is held while dragged");
      await page.mouse.up();
      await page.waitForTimeout(250);
      assert.deepEqual(await order(), ["kimi", "codex", "claude", "zai"]);
      assert.deepEqual(saved.at(-1), ["kimi", "codex", "claude", "zai"], "the order is saved");
      assert.equal(await scrollY(), before, "the drag doesn't move the page");

      // Alt+arrows from the keyboard, the focus kept on the card moved
      await handle("claude").focus();
      await page.keyboard.press("Alt+ArrowRight");
      await page.waitForTimeout(150);
      assert.deepEqual(await order(), ["kimi", "codex", "zai", "claude"]);
      assert.equal(await page.evaluate(() => document.activeElement?.closest(".subscription-card")?.dataset.key), "claude");
      await page.keyboard.press("Alt+ArrowUp");
      await page.waitForTimeout(150);
      assert.deepEqual(await order(), ["kimi", "codex", "claude", "zai"]);

      // a click on the handle doesn't move anything
      await handle("zai").click();
      await page.waitForTimeout(150);
      assert.deepEqual(await order(), ["kimi", "codex", "claude", "zai"]);
      assert.equal(await scrollY(), before, "no click moves the page");

      // drawn again from settings, and the tray panel follows it
      await page.reload();
      await handle("kimi").waitFor();
      assert.deepEqual(await order(), ["kimi", "codex", "claude", "zai"]);
      const panel = await open("panel", "http://magpie.test/?mode=panel", { width: 400, height: 680 });
      await panel.locator('#ptabs [data-ptab="usage"]').click();
      await panel.locator("#panelQuota .pq-gn").first().waitFor();
      assert.deepEqual(await panel.locator("#panelQuota .pq-gn").allTextContents(), ["Kimi", "Codex", "Claude", "Z.ai"]);
      assert.deepEqual(errors, []);
    });
  }
}
