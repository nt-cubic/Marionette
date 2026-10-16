// Run with Node's test runner and Playwright on the module path; see README.md.
// A curve made larger on a click goes back on the next, and its card goes
// back with it (#2059, mengyianx: 剩余额度走势 opened then closed left the
// allowance's box as tall as when it was open, a blank under it). The Usage
// page packs its cards into columns, each card stretched to the rows it was
// given and the last in a column held down to the tallest; a card that only
// shrinks inside that box never changed size itself, so nothing packed the
// page again. Checked for an allowance's curve and a balance's, at 1100 and
// 420 wide, in every language, in Chromium and WebKit; no backend, the API
// is faked here.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");
const { reach } = require("./reader.cjs");

const assets = process.env.MAGPIE_ZOOM_ASSETS || path.resolve(__dirname, "../assets");
const H = 3600e3;
const iso = (ms) => new Date(ms).toISOString();

function fixtures(now) {
  const rw = now + 3 * 24 * H;
  const quotas = [
    { provider: "codex", name: "Codex", icon: "openai", plan: "Plus", user: "a@x.com", windows: [
      { name: "5 hours", used: 40, resetsAt: iso(now + 2 * H) },
      { name: "Weekly", used: 30, resetsAt: iso(rw) },
    ] },
    { provider: "kimi", name: "Kimi", icon: "kimi-color", windows: [{ name: "Weekly", used: 10, resetsAt: iso(rw) }] },
    { provider: "deepseek", name: "DeepSeek", icon: "deepseek", balance: "$12.40", balanceTrend: {
      currency: "$", perDay: 1.2, points: [0, 1, 2, 3, 4].map((d) => ({ at: iso(now - (4 - d) * 24 * H), amount: 17 - d * 1.15 })),
    } },
  ];
  const week = [];
  for (let h = 90; h >= 0; h -= 6) week.push({ at: iso(now - h * H), left: 70 + h / 9, start: iso(rw - 168 * H), resetsAt: iso(rw) });
  const history = [{ provider: "codex", user: "a@x.com", lines: [{ name: "Weekly", points: week }] }];
  return { quotas, history };
}

function serve(lang, data) {
  const settings = { theme: "light", lang, tray: "panel", quotaLeft: false, currency: "usd" };
  return async (route) => {
    const url = new URL(route.request().url());
    const json = (body) => route.fulfill({ json: body });
    if (url.pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"${lang}",theme:"light",web:true};` });
    if (url.pathname === "/wails/runtime.js") return route.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json({ agents: [], profiles: [], settings });
    if (url.pathname === "/api/settings") return json(settings);
    if (url.pathname === "/api/providers") return json({ providers: [], presets: [], excluded: [], gateway: { running: true, window: true } });
    if (url.pathname === "/api/usage/quotas") return json(data.quotas);
    if (url.pathname === "/api/usage/quotas/history") return json(data.history);
    if (url.pathname === "/api/usage") return json({ calls: 0, input: 0, output: 0, cost: 0, agents: [], accounts: [], models: [], series: [], days: [] });
    if (url.pathname === "/api/groups") return json({ groups: [], models: [] });
    if (url.pathname === "/api/plugins") return json({ plugins: [] });
    if (url.pathname.startsWith("/api/")) return json({});
    const file = path.join(assets, url.pathname === "/" ? "index.html" : url.pathname);
    const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)];
    try { await route.fulfill({ body: await fs.readFile(file), contentType }); } catch { await route.fulfill({ status: 404, body: "" }); }
  };
}

// the cards' heights and the list's, once the page has settled: two frames
// with nothing moving
async function settled(page) {
  return page.evaluate(() => new Promise((done) => {
    const read = () => JSON.stringify({
      list: Math.round(document.querySelector("#subscriptionUsage").getBoundingClientRect().height),
      cards: [...document.querySelectorAll("#subscriptionUsage > .subscription-card")].map((c) => [c.dataset.key, Math.round(c.getBoundingClientRect().height)]),
    });
    let last = read(), still = 0;
    const tick = () => {
      const now = read();
      if (now === last && ++still >= 4) return done(JSON.parse(now));
      if (now !== last) { last = now; still = 0; }
      requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
  }));
}

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  for (const lang of ["en", "zh", "zh-TW", "ja", "de"]) {
    for (const width of [1100, 420]) {
      test(`${engine} ${lang} ${width}: a curve opened then closed leaves its card as tall as before`, async (t) => {
        const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
        t.after(() => browser.close());
        const page = await (await browser.newContext({ viewport: { width, height: 760 } })).newPage();
        page.setDefaultTimeout(5000);
        const errors = [];
        page.on("pageerror", (e) => errors.push(e.message));
        await page.route("**/*", serve(lang, fixtures(Date.now())));
        await page.goto("http://magpie.test/?view=usage");
        for (const [key, cls] of [["codex", ".quota-curve:not(.balance-curve)"], ["deepseek", ".balance-curve"]]) {
          const card = page.locator(`.subscription-card[data-key="${key}"]`);
          const curve = card.locator(cls).first();
          await curve.waitFor();
          // at 420 the balance's card is past the window's foot: the reader
          // wheels down to it
          await reach(page, curve.locator(".qc-plot"));
          const before = await settled(page);
          const scroll = await page.locator("#view-usage").evaluate((v) => v.scrollTop);
          await curve.locator(".qc-plot").click();
          assert.equal(await curve.evaluate((e) => e.classList.contains("big")), true);
          const open = await settled(page);
          const h = (s) => s.cards.find(([k]) => k === key)[1];
          assert.ok(h(open) > h(before) + 50, `${key}: the card grows with its curve: ${h(before)} → ${h(open)}`);
          await curve.locator(".qc-plot").click();
          assert.equal(await curve.evaluate((e) => e.classList.contains("big")), false);
          const closed = await settled(page);
          assert.deepEqual(closed, before, `${key}: closed again, every card and the list are as tall as before it opened`);
          assert.equal(await page.locator("#view-usage").evaluate((v) => v.scrollTop), scroll, "the clicks don't scroll");
        }
        assert.deepEqual(errors, []);
      });
    }
  }
}
