// Run with Node's test runner and Playwright on the module path; see README.md.
// Usage's days of the reader's own (#1492, nianlee-official): after Today,
// 7 days, 30 days and All, a Custom opt opens the app's own menu (never a
// native <select>) with a few spans and a month of days. Picking a first day
// then a last asks the backend for "first..last", the opt then names the days,
// days after today can't be picked, a span is a click away, Escape and a click
// outside close it, and nothing scrolls the page. English and Chinese, at a
// phone's width; no backend, the API is faked here.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");

const assets = path.resolve(process.env.MAGPIE_ASSETS || path.resolve(__dirname, "../assets"));
const dayKey = (d) => `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
const back = (n) => { const d = new Date(); d.setHours(12, 0, 0, 0); d.setDate(d.getDate() - n); return d; };

function serve(lang, asked) {
  const settings = { theme: "light", lang, quotaLeft: false, currency: "usd" };
  return async (route) => {
    const url = new URL(route.request().url());
    const json = (data) => route.fulfill({ json: data });
    if (url.pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"${lang}",theme:"light",web:true};` });
    if (url.pathname === "/wails/runtime.js") return route.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json({ agents: [], profiles: [], settings });
    if (url.pathname === "/api/settings") return json(settings);
    if (url.pathname === "/api/providers") return json({ providers: [], presets: [], excluded: [], gateway: { running: true, window: true } });
    if (url.pathname === "/api/usage") {
      const p = url.searchParams.get("period");
      asked.push(p);
      return json({ period: p, calls: 0, errors: 0, input: 0, output: 0, reasoning: 0, unpriced: 0, cost: 0, bucket: "day", series: [], agents: [], models: [], path: "" });
    }
    if (url.pathname === "/api/usage/quotas") return json([]);
    if (url.pathname === "/api/groups") return json({ groups: [], models: [] });
    if (url.pathname === "/api/plugins") return json({ plugins: [] });
    if (url.pathname === "/api/sessions") return json({ sessions: [], dirs: [] });
    if (url.pathname === "/api/sessions/stats") return json({ from: "", to: "", days: [], agents: {} });
    if (url.pathname.startsWith("/api/")) return json({});
    const file = path.join(assets, url.pathname === "/" ? "index.html" : url.pathname);
    const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)];
    try { await route.fulfill({ body: await fs.readFile(file), contentType }); } catch { await route.fulfill({ status: 404, body: "" }); }
  };
}

const words = {
  en: { custom: "Custom", yesterday: "Yesterday", head: "Pick the first day, then the last", next: "Now the last day", none: "No calls on the days picked." },
  zh: { custom: "自定义", yesterday: "昨天", head: "先选第一天，再选最后一天", next: "再选最后一天", none: "所选日期内没有调用。" },
};

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  for (const lang of ["en", "zh"]) {
    const w = words[lang];
    test(`${engine} ${lang}: Usage over days of the reader's own`, async (t) => {
      const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
      let page;
      t.after(async () => {
        if (process.env.ARTIFACT_DIR && page) {
          await fs.mkdir(process.env.ARTIFACT_DIR, { recursive: true });
          await page.screenshot({ path: path.join(process.env.ARTIFACT_DIR, `${engine}-${lang}-usage-range.png`) });
        }
        await browser.close();
      });
      page = await (await browser.newContext({ viewport: { width: 390, height: 760 }, reducedMotion: "reduce" })).newPage();
      page.setDefaultTimeout(5000);
      const errors = [];
      page.on("pageerror", (e) => errors.push(e.message));
      const asked = [];
      await page.route("**/*", serve(lang, asked));
      await page.goto("http://magpie.test/?view=usage");
      const opt = page.locator("#period .range-opt");
      await opt.waitFor();
      await page.waitForFunction(() => !document.querySelector("#period .opt:disabled"));
      assert.equal(await opt.getAttribute("aria-haspopup"), "dialog");
      const scrollY = () => page.evaluate(() => [scrollY, document.querySelector("#view-usage").scrollTop]);
      const y = await scrollY();

      // the menu: the app's own, never a native select, whole on the page
      await opt.click();
      const menu = page.locator(".range-menu");
      await menu.waitFor();
      assert.equal(await menu.locator(".pm-head").textContent(), w.head);
      assert.equal(await page.locator("select").count(), 0);
      const box = await menu.boundingBox();
      assert.ok(box.x >= 0 && box.x + box.width <= 390, `the menu runs off a phone's width: ${JSON.stringify(box)}`);
      assert.deepEqual(await menu.evaluate((m) => [getComputedStyle(m).borderLeftWidth === getComputedStyle(m).borderRightWidth]), [true]);
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true, "the page scrolls sideways");
      // a day after today can't be picked
      const tomorrow = dayKey(back(-1));
      const later = menu.locator(`.rm-day[data-day="${tomorrow}"]`);
      if (await later.count()) assert.equal(await later.isDisabled(), true);
      assert.equal(await menu.locator(".rm-step").last().isDisabled(), true, "no month after this one");
      // Escape closes it
      await page.keyboard.press("Escape");
      await menu.waitFor({ state: "detached" });

      // first day then last, in either order: from the earlier
      const first = back(9), last = back(3);
      await opt.click();
      await menu.waitFor();
      const day = async (d) => {
        for (let i = 0; i < 3 && !(await menu.locator(`.rm-day[data-day="${dayKey(d)}"]`).count()); i++) await menu.locator(".rm-step").first().click();
        await menu.locator(`.rm-day[data-day="${dayKey(d)}"]`).click();
      };
      asked.length = 0;
      await day(last);
      assert.equal(await menu.locator(".pm-head").textContent(), w.next);
      assert.deepEqual(asked, [], "one day clicked asks for nothing yet");
      await day(first);
      await menu.waitFor({ state: "detached" });
      await page.waitForFunction(() => document.querySelector("#period .range-opt.on"));
      const want = dayKey(first) + ".." + dayKey(last);
      assert.ok(asked.includes(want), `asked for ${JSON.stringify(asked)}, want ${want}`);
      assert.equal(await page.locator("#period .opt.on").count(), 1, "one opt is on");
      const label = (await opt.textContent()).trim();
      assert.match(label, /–/, "the opt names the days picked: " + label);
      // the empty page says so of the days picked
      await page.waitForFunction((none) => document.querySelector("#view-usage").textContent.includes(none), w.none);

      // the menu again opens on the days picked, and a span is a click away
      await opt.click();
      await menu.waitFor();
      assert.equal(await menu.locator(".rm-day.end").count() >= 1, true);
      asked.length = 0;
      await menu.locator(".rm-span", { hasText: w.yesterday }).click();
      await menu.waitFor({ state: "detached" });
      await page.waitForFunction(() => !document.querySelector("#period .opt:disabled"));
      assert.deepEqual(asked, [dayKey(back(1)) + ".." + dayKey(back(1))]);
      assert.doesNotMatch((await opt.textContent()).trim(), /–/, "one day is named alone");

      // a click outside closes it, and back to a preset
      await opt.click();
      await menu.waitFor();
      await page.mouse.click(380, 740);
      await menu.waitFor({ state: "detached" });
      await page.locator("#period .opt").first().click();
      await page.waitForFunction(() => !document.querySelector("#period .range-opt.on"));
      assert.equal((await opt.textContent()).trim(), w.custom);
      assert.deepEqual(await scrollY(), y, "a click scrolled the page");
      assert.deepEqual(errors, []);
    });
  }
}
