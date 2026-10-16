// Run with Node's test runner and Playwright on the module path; see README.md.
// Usage says, per model, how many of its replies magpie ended because the
// model was stuck in a loop (#1858, err_type "reply_loop"): beside "{n}
// failed" in the Overview's model rows and in the Requests ranking, the
// ranking's "Other" adding up those it holds. A model with none says
// nothing of it. Nothing runs past the window at 360 or 1100 wide, in
// every language, Chromium and WebKit; the API is faked.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");

const assets = path.resolve(process.env.MAGPIE_ASSETS || path.resolve(__dirname, "../assets"));
const looped = { en: "{n} looped", zh: "{n} 次陷入循环", "zh-TW": "{n} 次陷入迴圈", ja: "{n} 件はループ", de: "{n} in einer Schleife" };

const share = (id, calls, errors, loops) => ({ id, calls, errors, loops, input: 1000 * calls, output: 50 * calls, cache_read: 0, cache_write: 0, cost: 0, unpriced: calls });
// kimi looped twice of 3 failures; glm failed once, not for looping; and
// past the 7 shown, two more that looped once each, in "Other"
const BY = [share("kimi-k3", 40, 3, 2), share("glm-5", 30, 1, 0), ...["a", "b", "c", "d", "e"].map((x, i) => share("m-" + x, 20 - i, 0, 0)),
  share("tail-1", 2, 1, 1), share("tail-2", 1, 1, 1)];

function serve(lang) {
  const settings = { theme: "light", lang, quotaLeft: false, currency: "usd" };
  return async (route) => {
    const url = new URL(route.request().url());
    const json = (data) => route.fulfill({ json: data });
    if (url.pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"${lang}",theme:"light",web:false};` });
    if (url.pathname === "/wails/runtime.js") return route.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json({ agents: [], profiles: [], settings });
    if (url.pathname === "/api/settings") return json(settings);
    if (url.pathname === "/api/providers") return json({ providers: [], presets: [], excluded: [], gateway: { running: true, window: true } });
    if (url.pathname === "/api/usage") {
      return json({
        period: url.searchParams.get("period"), calls: 70, errors: 4, loops: 2, input: 70000, output: 3500, cache_read: 0, reasoning: 0, unpriced: 70, cost: 0, bucket: "hour",
        series: [{ t: new Date().toISOString(), input: 70000, output: 3500, calls: 70, cost: 0 }],
        agents: [{ id: "claude", name: "Claude Code", calls: 70, errors: 4, loops: 2, input: 70000, output: 3500, cost: 0 }],
        models: [
          { id: "p/kimi-k3", provider: "p", model: "kimi-k3", name: "kimi-k3", calls: 40, errors: 3, loops: 2, input: 40000, output: 2000, cost: 0 },
          { id: "p/glm-5", provider: "p", model: "glm-5", name: "glm-5", calls: 30, errors: 1, input: 30000, output: 1500, cost: 0 },
        ], path: "",
      });
    }
    if (url.pathname === "/api/usage/requests") {
      return json({ period: url.searchParams.get("period"), rows: [], offset: 0, total: 113, calls: 113, errors: 7, loops: 4, input: 113000, output: 5650, cache_read: 0, cache_write: 0, cost: 0, unpriced: 113,
        by: { model: BY }, bucket: "hour", series: [], agents: [] });
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

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  for (const lang of ["en", "zh", "zh-TW", "ja", "de"]) {
    for (const width of [360, 1100]) {
      test(`${engine} ${lang} ${width}: a model says how many of its replies looped`, async (t) => {
        const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
        t.after(() => browser.close());
        const errors = [];
        const n = (k) => looped[lang].replace("{n}", k);
        const open = async (tab) => {
          const page = await (await browser.newContext({ viewport: { width, height: 760 }, reducedMotion: "reduce" })).newPage();
          page.setDefaultTimeout(8000);
          page.on("pageerror", (e) => errors.push(e.message));
          await page.route("**/*", serve(lang));
          await page.addInitScript((tab) => localStorage.setItem("magpie.usageTab", tab), tab);
          await page.goto("http://magpie.test/?view=usage");
          return page;
        };

        // Overview: kimi's row says 2 looped beside 3 failed; glm's says none
        let page = await open("usage");
        const rows = page.locator("#usageModels .row");
        await rows.first().waitFor();
        const kimi = await page.locator('#usageModels .row[data-id="p/kimi-k3"] .sub').textContent();
        const glm = await page.locator('#usageModels .row[data-id="p/glm-5"] .sub').textContent();
        assert(kimi?.includes(n(2)), `kimi's row: ${kimi}`);
        assert(glm && !glm.includes(n(0)) && !glm.includes(n(1)), `glm's row: ${glm}`);
        assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), "the page scrolls sideways");

        // Requests: the ranking says it of kimi, not of glm, and Other adds
        // up its two
        page = await open("requests");
        const rank = page.locator("#ledRank .rk-loop");
        await rank.first().waitFor();
        const loops = await rank.allTextContents();
        assert.deepEqual(loops, [n(2), n(2)], "kimi's 2, and Other's 1 + 1");
        const rk = await page.locator("#ledRank > *").allTextContents();
        assert(!rk.find((x) => x.includes("glm-5")).includes(n(0)));
        assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), "the page scrolls sideways");
        assert.deepEqual(errors, []);
      });
    }
  }
}
