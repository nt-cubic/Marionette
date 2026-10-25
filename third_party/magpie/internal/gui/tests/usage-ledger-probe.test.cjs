// Run with Node's test runner and Playwright on the module path; see README.md.
// A request magpie sent a provider on its own (the editor's Test, Detect
// APIs, a model list: neiko and gys619 on Discord) is a row of the Requests
// tab, with a badge naming what it was for and saying it is counted in no
// total. In every language, Chromium and WebKit, wide and narrow, light and
// dark; no backend, the API is faked.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");

const assets = path.resolve(__dirname, "../assets");
const tot = (calls, tokens, cost) => ({ calls, errors: 0, input: tokens * 0.2, output: tokens * 0.1, cache_write: 0, cache_read: tokens * 0.7, cost });
const ROW = { t: new Date().toISOString(), agent: "claude", agentName: "Claude Code", icon: "claudecode-color", provider: "anthropic", providerName: "Claude", model: "claude-sonnet-5", in: 2, out: 600, cache_write: 0, cache_read: 3900, ms: 2380, status: 200, cost: 0.08, priced: true };
const PROBE = { ...ROW, agent: "magpie", agentName: "magpie", icon: "", provider: "relay", providerName: "Relay", model: "gpt-9", in: 0, out: 0, cache_read: 0, ms: 120, status: 401, cost: 0, priced: false, source: "probe", kind: "provider_test", endpoint: "/v1/chat/completions" };

function page(q, reported) {
  const rows = [ROW, ...(reported ? [PROBE] : [])];
  return {
    period: "today", rows, offset: 0, total: rows.length, ...tot(1, 4500, 0.08), reasoning: 0, unpriced: 0,
    bucket: "hour", series: [], by: { provider: [], agent: [], model: [] },
    agents: [{ id: "claude", name: "Claude Code", icon: "claudecode-color" }], providers: [{ id: "anthropic", name: "Claude", icon: "claudecode-color" }],
    through: tot(1, 4500, 0.08), direct: tot(0, 0, 0),
  };
}

function server(lang, theme, reported, asked) {
  return async (route) => {
    const req = route.request(), url = new URL(req.url());
    const json = (data) => route.fulfill({ json: data });
    if (url.pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"${lang}",theme:"${theme}",web:false};` });
    if (url.pathname === "/wails/runtime.js") return route.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json({ agents: [], profiles: [], settings: { lang, theme }, fx: { rate: 7.2, at: new Date().toISOString() } });
    if (url.pathname === "/api/usage/requests") { asked.push(url.searchParams); return json(page(url.searchParams, reported)); }
    if (url.pathname === "/api/usage/quotas") return json([]);
    if (url.pathname === "/api/usage") return json({ calls: 1, errors: 0, input: 1, output: 1, cache_read: 0, cache_write: 0, reasoning: 0, unpriced: 0, cost: 1, bucket: "day", series: [], agents: [], models: [], path: "~/.config/magpie/usage.jsonl" });
    if (url.pathname === "/api/sessions") return json({ sessions: [], dirs: [] });
    if (url.pathname === "/api/sessions/stats") return json({ from: "", to: "", days: [], agents: {} });
    if (url.pathname === "/api/groups") return json({ groups: [], models: [] });
    if (url.pathname.startsWith("/api/")) return json({});
    const file = path.join(assets, url.pathname === "/" ? "index.html" : url.pathname);
    const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)];
    await route.fulfill({ body: await fs.readFile(file), contentType });
  };
}

// the badge, as each language names the editor's Test, and a word of the
// tooltip that says it is counted in nothing
const L = {
  en: { badge: "Test", why: "not counted in any total" },
  zh: { badge: "测试", why: "不计入任何汇总" },
  "zh-TW": { badge: "測試", why: "不計入任何彙總" },
  ja: { badge: "テスト", why: "どの合計にも数えません" },
  de: { badge: "Testen", why: "zählt in keiner Summe" },
};

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  test(engine + ": magpie's own test is a row of its own", async (t) => {
    assert(["chromium", "webkit"].includes(engine), "BROWSER must be chromium or webkit");
    const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
    t.after(() => browser.close());
    const shots = process.env.ARTIFACT_DIR;
    if (shots) await fs.mkdir(shots, { recursive: true });

    const open = async (lang, { theme = "light", width = 1180, reported = true } = {}) => {
      const errors = [], asked = [];
      const context = await browser.newContext({ viewport: { width, height: 760 }, reducedMotion: "reduce" });
      t.after(() => context.close());
      const p = await context.newPage();
      p.setDefaultTimeout(5000);
      p.on("pageerror", (e) => errors.push(e.message));
      await p.route("**/*", server(lang, theme, reported, asked));
      await p.goto("http://magpie.test/");
      await p.locator('[data-view="usage"]').first().click();
      await p.locator(":is(#usageTab .opt, .nav-s):visible").nth(1).click();
      await p.locator("#ledVia .blk").first().waitFor();
      return { p, errors };
    };

    for (const lang of Object.keys(L)) {
      const w = L[lang];
      for (const width of [1180, 420]) {
        await t.test(`${lang} at ${width}`, async () => {
          const { p, errors } = await open(lang, { width, theme: width < 600 ? "dark" : "light" });
          const badge = p.locator(".led .src.probe");
          await badge.first().waitFor();
          assert.equal(await badge.count(), 1);
          assert.equal(await badge.textContent(), w.badge);
          assert((await badge.getAttribute("title")).includes(w.why), await badge.getAttribute("title"));
          // the gateway's row has none, and the probe's own badge says it isn't counted
          assert.equal(await p.locator(".led .src.reported").count(), 0);
          const own = p.locator(".led tr:has(.src.probe) .own-badge");
          assert.equal(await own.count(), 1, "the row is magpie's own");
          assert((await own.getAttribute("title")).includes(w.why), "and its badge says it counts nowhere");
          // no coloured stripe down its side, and the page doesn't scroll sideways
          assert(parseFloat(await badge.evaluate((b) => getComputedStyle(b).borderLeftWidth)) <= 1);
          assert(await p.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1), "no sideways scroll");
          if (shots) await p.screenshot({ path: path.join(shots, `probe-${engine}-${lang}-${width}.png`) });
          assert.deepEqual(errors, []);
        });
      }
    }

    await t.test("no probe: no badge", async () => {
      const { p, errors } = await open("en", { reported: false });
      await p.locator(".led tr", { hasText: "claude-sonnet-5" }).first().waitFor();
      assert.equal(await p.locator(".led .src.probe").count(), 0);
      assert.deepEqual(errors, []);
    });
  });
}
