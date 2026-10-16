// Run with Node's test runner and Playwright on the module path; see README.md.
// Usage › Requests with Today picked keeps its head still across a refresh
// (@LingYi_Stu on X, v0.1.1233, macOS, Chinese: the refresh button, "5 秒" and
// the cost kept shaking left and right). The year heatmap's cells are square,
// so between its narrowest and widest the page grows taller as it grows wider;
// with the window about as tall as the page, WebKit laid it out at the full
// width under the 8px bar and, on each redraw, 8px narrower for a few frames.
// Every height around the one where the page just fits is tried, each with a
// refresh, and the head's right edge, the interval's place and the view's
// width are read every frame. WebKit showed it, Chromium never did; both
// engines, every language. MAGPIE_ASSETS can point at older assets.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");

const assets = path.resolve(process.env.MAGPIE_ASSETS || path.resolve(__dirname, "../assets"));
const iso = (d) => `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;

// the reporter's page: a few Codex rows, a day of hourly points, a year's heatmap
const now = Date.now();
const R = (i, model, out) => ({ route_id: 900 + i, t: new Date(now - (i + 1) * 60e3).toISOString(), agent: "codex", agentName: "Codex", icon: "codex-color",
  provider: "openai", providerName: "openai", host: "chatgpt.com", ep: "/v1/responses", req: model, model, served: model, effort: "medium",
  in: 4000, out, cache_read: 110000, cache_write: 0, ms: 11000, ttft_ms: 6400, status: 200, rid: "r" + i, cost: 0.00175, priced: true });
const ROWS = [R(0, "gpt-6.1-sol", 240), R(1, "gpt-6.1-sol", 145), R(2, "codex-auto-review", 134), R(3, "gpt-6.1-sol", 93)];
const M = (id, calls, out) => ({ id, calls, input: 1, output: out, cache_read: 0, cache_write: 0, cost: 0.003, timed: calls, ttft_ms: 9000, decode_ms: 6000, decode_out: out });
const part = (x) => ({ calls: x.calls, tokens: x.output, cost: x.cost, timed: x.timed, ttft_ms: x.ttft_ms, decode_ms: x.decode_ms, decode_out: x.decode_out });
function HOURS() {
  const m = new Date(); m.setHours(0, 0, 0, 0);
  const upto = new Date().getHours();
  return Array.from({ length: 24 }, (_, h) => {
    const on = h <= upto && h >= 6, k = on ? (h % 5) + 1 : 0;
    return { label: String(h).padStart(2, "0"), time: new Date(m.getTime() + h * 3600e3).toISOString(), calls: k * 3, input: k * 100, output: k * 500, cache_read: k * 90000, cache_write: 0, cost: k * 0.01,
      timed: k, ttft_ms: k * 900, decode_ms: k * 4000, decode_out: k * 400,
      by: { model: on ? { "gpt-6.1-sol": part(M("gpt-6.1-sol", k * 2, k * 300)), "codex-auto-review": part(M("codex-auto-review", k, k * 200)) } : {} } };
  });
}
let reads = 0;
const LEDGER = (period) => ({
  period, rows: ROWS, offset: 0, total: ROWS.length, calls: ROWS.length, errors: 0, input: 2, output: 612, cache_read: 0, cache_write: 0,
  cost: 0.007 + ++reads * 0.001, unpriced: 0, timed: 4, ttft_ms: 18000, decode_ms: 12000, decode_out: 612,
  by: { model: [M("gpt-6.1-sol", 3, 478), M("codex-auto-review", 1, 134)] }, bucket: "hour", series: HOURS(),
  agents: [{ id: "codex", name: "Codex", icon: "codex-color" }],
});
function HEAT() {
  const to = new Date(), from = new Date(); from.setDate(from.getDate() - 364);
  const days = [];
  for (let i = 0; i < 40; i++) { const d = new Date(); d.setDate(d.getDate() - i); days.push({ date: iso(d), calls: 3 + i, tokens: 1000 * (i + 1), cost: 0.01 * i }); }
  return { from: iso(from), to: iso(to), days };
}

function serve(lang) {
  return async (route) => {
    const url = new URL(route.request().url());
    const json = (data) => route.fulfill({ json: data });
    if (url.pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"${lang}",theme:"light",web:false};` });
    if (url.pathname === "/wails/runtime.js") return route.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json({ agents: [], profiles: [], settings: { lang, theme: "light" } });
    if (url.pathname === "/api/usage/requests") return json(LEDGER(url.searchParams.get("period")));
    if (url.pathname === "/api/usage/heatmap") return json(HEAT());
    if (url.pathname === "/api/usage/quotas") return json([]);
    if (url.pathname === "/api/usage") return json({ calls: 0, errors: 0, input: 0, output: 0, reasoning: 0, unpriced: 0, cost: 0, bucket: "day", series: [], agents: [], models: [], path: "" });
    if (url.pathname === "/api/providers") return json({ providers: [], presets: [], excluded: [], models: [], gateway: { running: false } });
    if (url.pathname === "/api/sessions") return json({ sessions: [], dirs: [] });
    if (url.pathname === "/api/groups") return json({ groups: [], models: [] });
    if (url.pathname.startsWith("/api/")) return json({});
    const file = path.join(assets, url.pathname === "/" ? "index.html" : url.pathname);
    const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)];
    try { await route.fulfill({ body: await fs.readFile(file), contentType }); } catch { await route.fulfill({ status: 404, body: "" }); }
  };
}

// how tall the page's content is, short of the window or not
const CONTENT = () => {
  const v = document.querySelector("#view-usage"), top = v.getBoundingClientRect().top;
  let b = 0;
  for (const k of v.querySelectorAll(":scope > *, :scope > * > *")) { const r = k.getBoundingClientRect(); if (r.height) b = Math.max(b, r.bottom - top + v.scrollTop); }
  return Math.ceil(b + parseFloat(getComputedStyle(v).paddingBottom));
};
// every frame for ~0.6s: where the head ends, where the interval sits, and
// whether the view is laid out wider than it shows
const WATCH = () => new Promise((res) => {
  const v = document.querySelector("#view-usage"), head = document.querySelector(".usage-head"), every = document.querySelector("#usageEvery");
  const seen = new Set();
  let f = 0;
  const tick = () => {
    seen.add(`head ends ${Math.round(head.getBoundingClientRect().right)}, interval at ${Math.round(every.getBoundingClientRect().left)}, view ${v.clientWidth}/${v.scrollWidth}`);
    if (++f < 36) requestAnimationFrame(tick); else res([...seen]);
  };
  requestAnimationFrame(tick);
});

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  for (const lang of ["en", "zh", "zh-TW", "ja", "de"]) {
    test(`${engine} ${lang}: Requests' head keeps still across a refresh with Today picked`, async (t) => {
      const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium", ignoreDefaultArgs: ["--hide-scrollbars"] }));
      t.after(() => browser.close());
      const page = await (await browser.newContext({ viewport: { width: 1044, height: 900 } })).newPage();
      page.setDefaultTimeout(8000);
      const errors = [];
      page.on("pageerror", (e) => errors.push(e.message));
      await page.addInitScript(() => { localStorage.setItem("magpie.navLayout", "top"); localStorage.setItem("magpie.usageTab", "requests"); });
      await page.route("**/*", serve(lang));
      await page.goto("http://magpie.test/?view=usage");
      await page.waitForFunction(() => !document.querySelector("#ledHeat").hidden && document.querySelector("table.led tbody tr") && document.querySelector("#usageEvery").offsetWidth);
      assert.equal(await page.locator("#period .opt").first().evaluate((b) => b.classList.contains("on")), true, "Today is picked");
      assert.equal(await page.evaluate(() => document.querySelector(".usage-head").offsetWidth > 0), true);
      const extra = await page.evaluate(() => innerHeight - document.querySelector("#view-usage").clientHeight);

      const shook = [];
      // 1044 is where it showed at the reporter's zoom; 828 and 936 are narrower windows
      for (const w of [1044, 936, 828]) {
        await page.setViewportSize({ width: w, height: 300 });
        await page.waitForTimeout(150);
        const tall = await page.evaluate(CONTENT);
        await page.setViewportSize({ width: w, height: 3000 });
        await page.waitForTimeout(150);
        const short = await page.evaluate(CONTENT);
        for (let h = Math.min(tall, short) + extra - 2; h <= Math.max(tall, short) + extra + 2; h++) {
          await page.setViewportSize({ width: w, height: h });
          await page.waitForTimeout(120);
          const watch = page.evaluate(WATCH);
          await page.locator("#usageReload").click();
          const seen = await watch;
          if (seen.length > 1) shook.push(`${w}x${h}: ${seen.join(" | ")}`);
        }
      }
      assert.deepEqual(shook, [], "the head moved sideways during a refresh");
      assert.deepEqual(errors, []);
    });
  }
}
