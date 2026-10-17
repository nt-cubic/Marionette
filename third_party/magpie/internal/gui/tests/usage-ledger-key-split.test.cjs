// Run with Node's test runner and Playwright on the module path; see README.md.
// The Usage page's Requests by gateway key (MauriceCarlo, #2521): with one
// key in use, the key filter still offers that key and the calls no key
// made, and the chart splits by key as it does by agent or model, the calls
// with none named in the reader's language. A click on a key in the ranking
// lists its calls alone, a click again all of them. A period no key made a
// call in has neither. Every language, both engines, a wide and a narrow
// window; no backend, the API is faked.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");
const reader = require("./reader.cjs");

const assets = path.resolve(__dirname, "../assets");
const now = Date.now();
const H = 3600e3;
const hour = new Date(Math.floor(now / H) * H).toISOString();
const base = { agent: "claude", agentName: "Claude Code", icon: "claudecode-color", provider: "relay", providerName: "Relay", req: "m", model: "m", in: 1000, out: 100, cache_read: 0, cache_write: 0, ms: 2000, status: 200, ep: "/v1/messages", cost: 0.01, priced: true };
const ROWS = [
  { ...base, t: new Date(now - 60e3).toISOString(), rid: "req_desk", callerKeyId: "k1", callerKeyName: "Desk" },
  { ...base, t: new Date(now - 120e3).toISOString(), rid: "req_local" },
];
const share = (id, name, calls) => ({ id, name, icon: "generic", calls, input: calls * 1000, output: calls * 100, cache_read: 0, cache_write: 0, cost: calls * 0.01 });
const part = (calls) => ({ calls, tokens: calls * 1100, cost: calls * 0.01 });

function ledger(q, keyed) {
  const key = q.get("callerKey");
  const rows = ROWS.filter((r) => !key || (key === "none" ? !r.callerKeyId : r.callerKeyId === key)).filter((r) => keyed || !r.callerKeyId);
  return {
    period: q.get("period"), rows, offset: 0, total: rows.length, calls: rows.length, errors: 0,
    input: rows.length * 1000, output: rows.length * 100, cache_read: 0, cache_write: 0, cost: rows.length * 0.01, unpriced: 0,
    by: { model: [share("m", "m", rows.length)], callerKey: keyed ? [share("k1", "Desk", 1), share("none", "No gateway key", 1)] : [share("none", "No gateway key", 1)] },
    bucket: "hour",
    series: [{ label: "", time: hour, calls: rows.length, input: rows.length * 1000, output: rows.length * 100, by: { model: { m: part(rows.length) }, callerKey: keyed ? { k1: part(1), none: part(1) } : { none: part(1) } } }],
    agents: [{ id: "claude", name: "Claude Code", icon: "claudecode-color" }],
    providers: [{ id: "relay", name: "Relay", icon: "generic" }],
    callerKeys: keyed ? [{ id: "k1", callerKeyId: "k1", callerKeyName: "Desk", name: "Desk", icon: "generic", calls: 1 }] : [],
  };
}

function server(lang, asked, keyed) {
  return async (route) => {
    const url = new URL(route.request().url());
    const json = (data) => route.fulfill({ json: data });
    if (url.pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"${lang}",theme:"light",web:false};` });
    if (url.pathname === "/wails/runtime.js") return route.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json({ agents: [], profiles: [], settings: { lang, theme: "light" } });
    if (url.pathname === "/api/usage/requests") { asked.push(url.searchParams); return json(ledger(url.searchParams, keyed)); }
    if (url.pathname === "/api/usage/quotas") return json([]);
    if (url.pathname === "/api/usage") return json({ calls: 0, errors: 0, input: 0, output: 0, reasoning: 0, unpriced: 0, cost: 0, bucket: "day", series: [], agents: [], models: [], path: "" });
    if (url.pathname === "/api/providers") return json({ providers: [], presets: [], excluded: [], models: [], gateway: { running: false } });
    if (url.pathname === "/api/sessions") return json({ sessions: [], dirs: [] });
    if (url.pathname === "/api/sessions/stats") return json({ from: "", to: "", days: [], agents: {} });
    if (url.pathname === "/api/groups") return json({ groups: [], models: [] });
    if (url.pathname.startsWith("/api/")) return json({});
    const file = path.join(assets, url.pathname === "/" ? "index.html" : url.pathname);
    const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)];
    await route.fulfill({ body: await fs.readFile(file), contentType });
  };
}

const L = {
  en: { split: "Gateway key", none: "No gateway key", all: "All gateway keys" },
  zh: { split: "网关密钥", none: "无网关密钥", all: "全部网关密钥" },
  "zh-TW": { split: "閘道金鑰", none: "無閘道金鑰", all: "全部閘道金鑰" },
  ja: { split: "ゲートウェイキー", none: "ゲートウェイキーなし", all: "すべてのゲートウェイキー" },
  de: { split: "Zugangsschlüssel", none: "Ohne Zugangsschlüssel", all: "Alle Zugangsschlüssel" },
};

async function open(browser, lang, width, keyed) {
  const errors = [], asked = [];
  const ctx = await browser.newContext({ viewport: { width, height: 900 }, reducedMotion: "reduce" });
  const p = await ctx.newPage();
  p.setDefaultTimeout(5000);
  p.on("pageerror", (e) => errors.push(e.message));
  await p.route("**/*", server(lang, asked, keyed));
  await p.goto("http://magpie.test/");
  await p.locator('[data-view="usage"]').first().click();
  await p.locator(":is(#usageTab .opt, .nav-s):visible").nth(1).click();
  await p.locator("#ledWrap .led tbody tr").first().waitFor();
  await p.waitForTimeout(300);
  return { ctx, p, errors, asked };
}

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  test(engine + ": requests told apart by gateway key", async (t) => {
    assert(["chromium", "webkit"].includes(engine), "BROWSER must be chromium or webkit");
    const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
    t.after(() => browser.close());
    for (const lang of Object.keys(L)) {
      const w = L[lang];
      for (const width of [1400, 760]) {
        await t.test(`${lang} at ${width}px`, async () => {
          const { ctx, p, errors, asked } = await open(browser, lang, width, true);
          t.after(() => ctx.close());
          // one key in use: the filter is there, with the calls of none
          const key = p.locator("#ledKey");
          assert(await key.isVisible(), "one key's filter shows");
          assert.equal((await key.textContent()).trim(), w.all);
          await reader.click(p, key);
          const items = await p.locator(".sess-menu .pm-item").allTextContents();
          assert(items.some((x) => x.includes("Desk")) && items.some((x) => x.includes(w.none)), "the key and the calls of none: " + items.join(" | "));
          await reader.click(p, p.locator(".sess-menu .pm-item").filter({ hasText: w.none }));
          await p.waitForFunction(() => document.querySelectorAll("#ledWrap .led tbody tr.led-row").length === 1);
          assert.equal(asked.at(-1).get("callerKey"), "none");
          assert.equal((await key.textContent()).trim(), w.none);
          await reader.click(p, key);
          await reader.click(p, p.locator(".sess-menu .pm-item").filter({ hasText: w.all }));
          await p.waitForFunction(() => document.querySelectorAll("#ledWrap .led tbody tr.led-row").length === 2);

          // the chart split by key, the calls of none in the reader's words
          const split = p.locator("#ledSplit .opt").filter({ hasText: w.split });
          await reader.click(p, split);
          await p.waitForTimeout(150);
          const names = await p.locator("#ledRank button.rk .rk-nm").allTextContents();
          assert.deepEqual(names.slice().sort(), ["Desk", w.none].sort(), "the ranking: " + names.join(", "));
          const before = asked.length;
          await reader.click(p, p.locator("#ledRank button.rk").filter({ hasText: "Desk" }));
          await p.waitForFunction(() => document.querySelectorAll("#ledRank button.rk.on").length === 1);
          assert(asked.length > before);
          assert.equal(asked.at(-1).get("callerKey"), "k1");
          assert.equal((await key.textContent()).trim(), "Desk", "the filter says the key picked");
          await reader.click(p, p.locator("#ledRank button.rk.on"));
          await p.waitForFunction(() => !document.querySelector("#ledRank button.rk.on"));
          assert.equal(asked.at(-1).get("callerKey"), null);
          assert.equal(await p.locator("#view-usage").evaluate((v) => v.scrollWidth <= v.clientWidth + 1), true, "the page doesn't scroll sideways");
          assert.deepEqual(errors, []);
        });
      }
      await t.test(lang + ": no key in use, neither", async () => {
        const { ctx, p, errors } = await open(browser, lang, 1400, false);
        t.after(() => ctx.close());
        assert.equal(await p.locator("#ledKey").isVisible(), false);
        assert.equal(await p.locator("#ledSplit .opt").filter({ hasText: w.split }).count(), 0);
        assert.deepEqual(errors, []);
      });
    }
  });
}
