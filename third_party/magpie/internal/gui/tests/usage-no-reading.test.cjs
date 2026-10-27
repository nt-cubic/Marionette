// Run with Node's test runner and Playwright on the module path; see README.md.
// A plugin's usage window that leaves out used says nothing of how much of
// it is spent — a 0% it never reported. The card shows what the plugin did
// say (its display text, else "No allowance reported") and no percentage,
// and no meter is drawn from it, rather than "100% left" (or NaN%) from a
// 0 it was never told (#3650): neither as magpie's API sends such a window
// (text, used 0) nor with used left out. A window that did say used:0
// still shows its 0%, and its meter. Chromium and WebKit, every language,
// used and left, at a narrow width; no backend, the API is faked here.
// Contributed by skygunner.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");

const assets = path.resolve(process.env.MAGPIE_ASSETS || path.join(__dirname, "../assets"));
const langs = ["en", "zh", "zh-TW", "ja", "de"];
const none = { en: "No allowance reported", zh: "暂无额度数据", "zh-TW": "暫無額度資料", ja: "利用枠の情報なし", de: "Kein Kontingent gemeldet" };

const quotas = [
  // the reporter's plugin: a count in its display, no used — as the API
  // sends it (text) and as a page may be handed one (used left out)
  { provider: "loomy-plugin", name: "Loomy", user: "spent@example.com", windows: [
    { name: "Credits", used: 0, text: true, display: "0 / 5000" },
    { name: "Bonus", display: "12 / 300" },
    { name: "Gift" },
  ] },
  { provider: "claude", name: "Claude", user: "fine@example.com", windows: [
    { name: "5 hours", used: 40 },
    { name: "Weekly", used: 0 },
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
  test(`${engine}: a window that didn't say used shows its text, not a percentage`, async (t) => {
    const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
    t.after(() => browser.close());
    for (const lang of langs) {
      for (const width of [900, 390]) {
        const page = await browser.newPage({ viewport: { width, height: 800 }, reducedMotion: "reduce" });
        page.setDefaultTimeout(5000);
        const errors = [];
        page.on("pageerror", (e) => errors.push(e.message));
        await page.route("**/*", serve(lang));
        await page.goto("http://magpie.test/?view=usage");
        const at = `${lang} ${width}px`;

        const spent = page.locator(".subscription-card", { hasText: "Loomy" });
        const fine = page.locator(".subscription-card", { hasText: "Claude" });
        await spent.locator(".quota-n").first().waitFor();
        await fine.locator(".quota-n").first().waitFor();
        // as used, then as left: "100% left" was the reporter's reading
        for (const side of ["used", "left"]) {
          const s = await windows(spent);
          assert.equal(s.length, 3, `${at} ${side}: ${JSON.stringify(s)}`);
          assert.match(s[0].n, /^0 \/ 5000$/, `${at} ${side}: the plugin's own count`);
          assert.match(s[1].n, /^12 \/ 300$/, `${at} ${side}: the plugin's own count`);
          assert.equal(s[2].n, none[lang], `${at} ${side}: a window that said nothing`);
          for (const w of s) {
            assert.doesNotMatch(w.n, /NaN|%/, `${at} ${side}: no share from a used never sent: ${w.n}`);
            assert.equal(w.track, false, `${at} ${side}: no meter from a share never sent: ${w.n}`);
          }
          // a window that did say its share keeps it, 0 too
          const f = await windows(fine);
          assert.match(f[0].n, side === "used" ? /40\s?%/ : /60\s?%/, `${at} ${side}: ${f[0].n}`);
          assert.match(f[1].n, side === "used" ? /(^|\D)0\s?%/ : /100\s?%/, `${at} ${side}: a used of 0 that was said: ${f[1].n}`);
          assert.ok(f.every((w) => w.track), `${at} ${side}: a window that said its share has its meter`);
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
