// Run with Node's test runner and Playwright on the module path; see README.md.
// Usage › Sessions with more agents than fit on one line (#929, liuweifeng):
// the agent filter is a menu beside the model and folder ones, not a strip
// of a segment per agent, which ran past the window and was scrolled
// sideways to reach the last (the owner, 10-28: "这应该是个 Combo Box").
// The page never scrolls sideways, every agent is in the menu, most used
// first, a few letters find one, and one agent in range leaves it hidden.
// English and Chinese, Chromium and WebKit.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");

const assets = path.resolve(__dirname, "../assets");
const names = ["Pi", "WorkBuddy", "DeepSeek Harness", "Claude Code", "Codex", "omp", "OpenCode", "Qoder", "ZCode", "Claude Desktop", "Qoder CN", "Grok Build", "Kimi Code", "Cursor", "Zed"];
const agents = Object.fromEntries(names.map((n, i) => ["a" + i, n]));
const iso = (d) => [d.getFullYear(), d.getMonth() + 1, d.getDate()].map((n) => String(n).padStart(2, "0")).join("-");
const today = iso(new Date());
const days = [{
  date: today,
  usage: names.map((_, i) => ({ agent: "a" + i, cwd: "/work/app" + (i % 2), model: "m" + (i % 3), input: 1000 * (i + 1), output: 100, cache_read: 0, cache_write: 0, cost: 0.1, priced: true })),
  active: [],
}];

function serve(lang, n = names.length) {
  return async (route) => {
    const url = new URL(route.request().url());
    const json = (data) => route.fulfill({ json: data });
    if (url.pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"${lang}",theme:"light",web:false};` });
    if (url.pathname === "/wails/runtime.js") return route.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json({ agents: [], profiles: [], settings: { lang, theme: "light" } });
    if (url.pathname === "/api/sessions/progress") return json({ indexing: false });
    if (url.pathname === "/api/sessions") return json({ sessions: [], dirs: [] });
    if (url.pathname === "/api/sessions/stats") return json({ from: today, to: today, days: days.map((d) => ({ ...d, usage: d.usage.slice(0, n) })), agents: Object.fromEntries(Object.entries(agents).slice(0, n)) });
    if (url.pathname === "/api/groups") return json({ groups: [] });
    if (url.pathname === "/api/usage/quotas") return json([]);
    if (url.pathname.startsWith("/api/")) return json({});
    const file = path.join(assets, url.pathname === "/" ? "index.html" : url.pathname);
    const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)];
    const body = await fs.readFile(file).catch(() => null);
    return body ? route.fulfill({ body, contentType }) : route.fulfill({ status: 404, body: "" });
  };
}

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  for (const [lang, everyAgent] of [["en", "All agents"], ["zh", "全部 Agent"]]) {
    test(`${engine} ${lang}: many agents are a menu that doesn't widen Usage › Sessions`, async (t) => {
      const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
      t.after(() => browser.close());
      const page = await (await browser.newContext({ viewport: { width: 860, height: 700 }, reducedMotion: "reduce" })).newPage();
      page.setDefaultTimeout(8000);
      const errors = [];
      page.on("pageerror", (e) => errors.push(e.message));
      await page.route("**/*", serve(lang));
      await page.addInitScript(() => { localStorage.setItem("magpie.usageTab", "sessions"); localStorage.setItem("magpie.sessRange", "all"); localStorage.setItem("magpie.usageEvery", "0"); });
      await page.goto("http://magpie.test/?view=usage");
      const pick = page.locator("#sessAgent");
      await pick.waitFor({ state: "visible" });
      assert.equal(await pick.evaluate((e) => e.tagName + "." + e.className), "BUTTON.sess-pick", "the agent filter is the same menu button as the model and folder ones");
      assert.equal((await pick.innerText()).trim(), everyAgent);
      assert.equal(await pick.locator(".opt").count(), 0, "no segment per agent is left");
      const settle = () => page.evaluate(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))));
      for (const width of [860, 560]) {
        await page.setViewportSize({ width, height: 700 });
        await settle();
        const sideways = await page.evaluate(() => [document.documentElement, document.body, document.querySelector("main"), document.querySelector("#view-usage"), document.querySelector(".sess-tools")]
          .filter(Boolean).filter((e) => e.scrollWidth > e.clientWidth + 1).map((e) => `${e.tagName}#${e.id} ${e.scrollWidth}>${e.clientWidth}`));
        assert.deepEqual(sideways, [], `${width}px: something scrolls sideways`);
        // the three filters sit on one line, inside the window
        const boxes = await Promise.all(["#sessAgent", "#sessModel", "#sessFolder"].map((id) => page.locator(id).boundingBox()));
        for (const b of boxes) assert(b && b.x >= 0 && b.x + b.width <= width, `${width}px: a filter sits outside the window: ${JSON.stringify(boxes)}`);
        assert(boxes.every((b) => Math.abs(b.y - boxes[0].y) < 1), `${width}px: the filters aren't on one line: ${JSON.stringify(boxes)}`);
      }

      // every agent is in the menu, after "All agents", the most used first
      await pick.click();
      const menu = page.locator(".pop.proto-menu.sess-menu");
      await menu.waitFor();
      const items = menu.locator(".pm-item .pm-name");
      assert.deepEqual(await items.allInnerTexts(), [everyAgent, ...[...names].reverse()]);
      // a few letters find one, and picking it says so on the button
      await menu.locator(".sc-q").fill("zed");
      assert.deepEqual(await items.allInnerTexts(), ["Zed"]);
      await menu.locator(".pm-item").first().click();
      await menu.waitFor({ state: "detached" });
      await page.waitForFunction(() => document.querySelector("#sessAgent span")?.textContent === "Zed");
      assert.match(await pick.getAttribute("class"), /\bset\b/);
      // drawn again, the pick holds, and the menu shows it checked
      await settle();
      assert.equal((await pick.innerText()).trim(), "Zed");
      await pick.click();
      await menu.waitFor();
      assert.equal(await menu.locator('.pm-item[aria-checked="true"] .pm-name').innerText(), "Zed");
      await menu.locator(".pm-item").first().click();
      await page.waitForFunction((all) => document.querySelector("#sessAgent span")?.textContent === all, everyAgent);
      assert.doesNotMatch(await pick.getAttribute("class"), /\bset\b/);
      assert.deepEqual(errors, []);

      // one agent: the filter stays hidden, as the strip was
      await page.unroute("**/*");
      await page.route("**/*", serve(lang, 1));
      await page.reload();
      await page.waitForFunction(() => !document.querySelector("#view-usage").classList.contains("loading") && document.querySelector("#sessStats").childElementCount > 0);
      assert.equal(await pick.isVisible(), false, "one agent in range should leave the agent filter hidden");
    });
  }
}
