// Run with Node's test runner and Playwright on the module path; see README.md.
// #3950 (墨雨藏锋 / mycf_711): a prepaid wallet (SuperMagic's points) has a
// balance and no total. Its plugin sends the window's balance and unit
// with no used; the usage card says the figure, "6,291 积分 left", with no
// meter and no percentage, whether the meters read used or left, where it
// said "No allowance reported" or a 0% it was never told. A window that
// said its share beside it keeps its percentage and meter. Chromium and
// WebKit, every language, at a window's width and a phone's; no backend,
// the API is faked here.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");

const assets = path.resolve(process.env.MAGPIE_ASSETS || path.join(__dirname, "../assets"));
const left = { en: "6,291 积分 left", zh: "剩余 6,291 积分", "zh-TW": "剩餘 6,291 积分", ja: "残り 6,291 积分", de: "6,291 积分 übrig" };

const quotas = [
  // as magpie's API sends a wallet's window: its balance and unit, used 0
  // and no share (QuotaSpan.Balance)
  { provider: "magicrew-plugin", name: "SuperMagic", user: "wallet@example.com", windows: [
    { name: "Points", used: 0, remaining: 100, balance: 6291, unit: "积分" },
  ] },
  { provider: "claude", name: "Claude", user: "fine@example.com", windows: [
    { name: "5 hours", used: 40 },
  ] },
];

function serve(lang) {
  return async (route) => {
    const url = new URL(route.request().url());
    const json = (data) => route.fulfill({ json: data });
    if (url.pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"${lang}",theme:"light",web:true};` });
    if (url.pathname === "/wails/runtime.js") return route.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json({ agents: [], profiles: [], settings: { lang, theme: "light" } });
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

async function windows(card) {
  return card.locator(".quota").evaluateAll((qs) => qs.map((q) => ({ n: q.querySelector(".quota-n")?.innerText ?? "", track: !!q.querySelector(".quota-track") })));
}

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  test(`${engine}: a wallet's window says its balance, not a percentage`, async (t) => {
    const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
    t.after(() => browser.close());
    for (const lang of Object.keys(left)) {
      for (const width of [900, 390]) {
        const page = await browser.newPage({ viewport: { width, height: 800 }, reducedMotion: "reduce" });
        page.setDefaultTimeout(5000);
        const errors = [];
        page.on("pageerror", (e) => errors.push(e.message));
        await page.route("**/*", serve(lang));
        await page.goto("http://magpie.test/?view=usage");
        const at = `${lang} ${width}px`;
        const wallet = page.locator(".subscription-card", { hasText: "SuperMagic" });
        const fine = page.locator(".subscription-card", { hasText: "Claude" });
        await wallet.locator(".quota-n").first().waitFor();
        await fine.locator(".quota-n").first().waitFor();
        for (const side of ["used", "left"]) {
          const w = await windows(wallet);
          assert.equal(w.length, 1, `${at} ${side}: ${JSON.stringify(w)}`);
          assert.equal(w[0].n, left[lang], `${at} ${side}: the wallet's figure`);
          assert.equal(w[0].track, false, `${at} ${side}: no meter from a share never sent`);
          const f = await windows(fine);
          assert.match(f[0].n, side === "used" ? /40\s?%/ : /60\s?%/, `${at} ${side}: ${f[0].n}`);
          assert.ok(f[0].track, `${at} ${side}: a window that said its share has its meter`);
          if (side === "used") await fine.locator(".quota-n").first().click();
        }
        const overflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
        assert.ok(overflow <= 0, `${at}: the page scrolls sideways by ${overflow}px`);
        assert.deepEqual(errors, [], at);
        await page.close();
      }
    }
  });
}
