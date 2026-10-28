// Run with Node's test runner and Playwright on the module path; see README.md.
// Keys of one account that read the account's balance (KeyBalances'
// sharedBalance, the first key's name) show it once on the Usage page: one
// row, its balance and its curve, with "+2 keys" beside the key's name and
// the keys on hover; the head's sum counts the account once (mimi on
// Discord: a site's keys 1-A031-Pro, 2-S009-Plus and 3-S010-Team-2 each
// showed the account's $8.39 and its own curve). A key with the same amount
// that magpie didn't find to be that account's, and another provider's keys,
// stay each its own row. At 440px nothing is cut or spills. English and
// Chinese; no backend, the API is faked here.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");

const assets = path.resolve(__dirname, "../assets");
const now = Date.now();
const iso = (ms) => new Date(ms).toISOString();
const trend = { points: [{ at: iso(now - 3 * 3600e3), amount: 9.2 }, { at: iso(now - 3600e3), amount: 8.8 }, { at: iso(now - 60e3), amount: 8.39 }] };
const key = (provider, name, user, extra) => ({ provider, name, icon: "openai", user, windows: [], balance: "$8.39", readAt: iso(now), ...extra });
const shared = { sharedBalance: "1-A031-Pro", balanceTrend: trend };
const quotas = () => [
  key("site", "Site", "1-A031-Pro", shared),
  key("site", "Site", "2-S009-Plus", shared),
  key("site", "Site", "3-S010-Team-2", shared),
  // the same amount, not found to be the account's: its own row
  key("site", "Site", "other"),
  // another provider's two keys: each its own
  key("panel", "Panel", "t1"),
  key("panel", "Panel", "t2"),
];

function serve(lang) {
  const settings = { theme: "light", lang, tray: "panel", quotaLeft: false, currency: "usd" };
  return async (route) => {
    const url = new URL(route.request().url());
    const json = (data) => route.fulfill({ json: data });
    if (url.pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"${lang}",theme:"light",web:true};` });
    if (url.pathname === "/wails/runtime.js") return route.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json({ agents: [], profiles: [], settings });
    if (url.pathname === "/api/settings") return json(settings);
    if (url.pathname === "/api/providers") return json({ providers: [], presets: [], excluded: [], gateway: { running: true, window: true } });
    if (url.pathname === "/api/usage/quotas") return json(quotas());
    if (url.pathname === "/api/groups") return json({ groups: [], models: [] });
    if (url.pathname === "/api/plugins") return json({ plugins: [] });
    if (url.pathname.startsWith("/api/")) return json({});
    const file = path.join(assets, url.pathname === "/" ? "index.html" : url.pathname);
    const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)];
    try { await route.fulfill({ body: await fs.readFile(file), contentType }); } catch { await route.fulfill({ status: 404, body: "" }); }
  };
}

const keysOf = "1-A031-Pro, 2-S009-Plus, 3-S010-Team-2";
const words = {
  en: { more: "+2 keys", tip: "One balance for the account these keys share: " + keysOf, sum: "$16.78 in all", why: "The balances added up, each account's once", panelSum: "$16.78 in all", panelWhy: "The 2 keys' balances added up" },
  zh: { more: "+2 个密钥", tip: "这些密钥同属一个账号，余额只显示一次：" + keysOf, sum: "合计 $16.78", why: "余额之和，每个账号只计一次", panelSum: "合计 $16.78", panelWhy: "2 个密钥的余额之和" },
};

const cards = (page) => page.evaluate(() => Object.fromEntries([...document.querySelectorAll("#subscriptionUsage > .subscription-card")].map((c) => [c.dataset.key, {
  users: [...c.querySelectorAll(".subscription-account .user")].filter((u) => u.offsetParent).map((u) => u.title),
  balances: [...c.querySelectorAll(".quota-balance b")].filter((b) => b.offsetParent).length,
  curves: [...c.querySelectorAll(".balance-curve")].filter((b) => b.offsetParent).length,
  shared: [...c.querySelectorAll(".quota-shared")].map((s) => [s.textContent, s.title]),
  sum: c.querySelector(".quota-sum")?.textContent ?? null,
  why: c.querySelector(".quota-sum")?.title ?? null,
  more: c.querySelector(".quota-keys-more")?.textContent ?? null,
}])));

const fits = (page, key) => page.evaluate((key) => {
  const c = document.querySelector(`#subscriptionUsage > [data-key=${key}]`);
  const box = c.getBoundingClientRect();
  const bad = [];
  if (document.documentElement.scrollWidth > innerWidth) bad.push("page scrolls sideways");
  if (c.scrollWidth > c.clientWidth) bad.push("card spills");
  for (const e of c.querySelectorAll(".subscription-head > *, .quota-balance b, .subscription-account .user, .quota-shared")) {
    if (!e.offsetParent) continue;
    const r = e.getBoundingClientRect();
    if (r.left < box.left - 0.5 || r.right > box.right + 0.5) bad.push(`${e.className || e.tagName} past the card`);
    if (e.matches(".quota-sum, .quota-balance b, .quota-shared") && e.scrollWidth > e.clientWidth + 0.5) bad.push(`${e.className || e.tagName} cut: ${e.textContent}`);
  }
  return bad;
}, key);

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  for (const lang of ["en", "zh"]) {
    const w = words[lang];
    test(`${engine} ${lang}: keys of one account show its balance once`, async (t) => {
      const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
      t.after(() => browser.close());
      const context = await browser.newContext({ viewport: { width: 900, height: 700 }, reducedMotion: "reduce" });
      const page = await context.newPage();
      page.setDefaultTimeout(5000);
      const errors = [];
      page.on("pageerror", (e) => errors.push(e.message));
      await page.route("**/*", serve(lang));
      await page.addInitScript(() => { try { if (!sessionStorage.getItem("set")) { localStorage.clear(); localStorage.setItem("magpie.quotaRange", "2d"); sessionStorage.setItem("set", "1"); } } catch {} });
      await page.goto("http://magpie.test/?view=usage");
      await page.waitForSelector("#subscriptionUsage > [data-key=site] .subscription-account .user", { state: "attached" });

      const got = await cards(page);
      // the account's three keys: one row, one balance, one curve
      assert.deepEqual(got.site.users, ["1-A031-Pro", "other"]);
      assert.equal(got.site.balances, 2);
      assert.equal(got.site.curves, 1);
      assert.deepEqual(got.site.shared, [[w.more, w.tip]]);
      // the account counted once in the sum, and the sum says so
      assert.equal(got.site.sum, w.sum);
      assert.equal(got.site.why, w.why);
      assert.equal(got.site.more, null);
      // another provider's keys with the same amount: each its own
      assert.deepEqual(got.panel.users, ["t1", "t2"]);
      assert.equal(got.panel.balances, 2);
      assert.deepEqual(got.panel.shared, []);
      assert.equal(got.panel.sum, w.panelSum);
      assert.equal(got.panel.why, w.panelWhy);

      await page.setViewportSize({ width: 440, height: 700 });
      await page.waitForTimeout(300);
      for (const k of ["site", "panel"]) assert.deepEqual(await fits(page, k), [], `${k} fits at 440px`);
      assert.deepEqual(errors, []);
    });
  }
}
