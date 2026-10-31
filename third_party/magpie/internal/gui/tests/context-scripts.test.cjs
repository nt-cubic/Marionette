// Run with Node's test runner and Playwright on the module path; see README.md.
// A request whose User-Agent names only an HTTP library or an SDK (curl,
// Bun, python-httpx) is no agent (theforeveriris on X: each had a
// scorecard of its own). /api/context puts them on one card, "scripts",
// which the Context tab names Scripts and under it the libraries, the
// most used first; the Sessions filter names it so too. In every language,
// in both engines, at a phone's width and a desk's, nothing wider than the
// page.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");

const assets = path.resolve(__dirname, "../assets");
const now = Date.now();
const ago = (m) => new Date(now - m * 60000).toISOString();
const card = (agent, requests, sessions, more = {}) => ({
  agent, requests, sessions, errors: 0, calls: requests, median: 3000, p90: 6000, baseline: 1200, window: 200000, cache: 0, growth: 0, peakFill: 0, compacts: 0,
  shares: { system: 0.4, chat: 0.6 }, mcp: 0, score: 65,
  scores: [{ key: "cache", points: 0, most: 35 }, { key: "lean", points: 25, most: 25 }, { key: "pace", points: 20, most: 20 }, { key: "reliable", points: 20, most: 20 }],
  tags: [], models: ["deepseek-v4"], latestId: 9, latestTime: ago(3), ...more,
});
const point = (id, m) => ({ id, time: ago(m), tokens: 3000 });
const context = {
  days: 7,
  agents: [
    card("scripts", 300, 37, { clients: ["python-httpx", "Bun", "curl", "OpenAI"] }),
    card("codex", 20, 1),
  ],
  sessions: [
    { agent: "scripts", key: "s-httpx", model: "deepseek-v4", first: ago(9), last: ago(3), requests: 2, peak: 3000, window: 200000, latestId: 9, points: [point(8, 9), point(9, 3)] },
    { agent: "codex", key: "s-codex", title: "Port the parser", model: "gpt-5.6", first: ago(20), last: ago(5), requests: 2, peak: 3000, window: 200000, latestId: 7, points: [point(6, 20), point(7, 5)] },
  ],
};

function serve(lang) {
  const state = { agents: [{ id: "codex", name: "Codex", path: "/test/codex", fields: [] }], profiles: [], settings: { lang, theme: "light" } };
  return async (r) => {
    const url = new URL(r.request().url());
    const json = (data) => r.fulfill({ json: data });
    if (url.pathname === "/boot.js") return r.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"${lang}",theme:"light",web:true};` });
    if (url.pathname === "/wails/runtime.js") return r.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json(state);
    if (url.pathname === "/api/tune") return json({ days: 7, agents: [] });
    if (url.pathname === "/api/context") return json(context);
    if (url.pathname === "/api/gateway/trace") {
      if (url.searchParams.get("wait")) return new Promise(() => {});
      return json({ mine: true, now: new Date().toISOString(), seq: 0, totals: { requests: 0, rerouted: 0, errors: 0 }, routes: [] });
    }
    if (url.pathname === "/api/providers") return json({ providers: [], presets: [], excluded: [], gateway: { running: true, window: true, url: "http://127.0.0.1:3999" } });
    if (url.pathname === "/api/usage/quotas") return json([]);
    if (url.pathname === "/api/groups") return json({ groups: [], models: [] });
    if (url.pathname === "/api/plugins") return json({ plugins: [] });
    if (url.pathname.startsWith("/api/")) return json({});
    const file = path.join(assets, url.pathname === "/" ? "index.html" : url.pathname);
    const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)];
    await r.fulfill({ body: await fs.readFile(file).catch(() => ""), contentType, status: 200 });
  };
}

const name = { en: "Scripts", zh: "脚本", "zh-TW": "腳本", ja: "スクリプト", de: "Skripte" };
const why = {
  en: "Requests whose User-Agent names only an HTTP library or an SDK, not an agent: python-httpx, Bun, curl, OpenAI",
  zh: "请求的 User-Agent 只写了 HTTP 库或 SDK、没有写是哪个 agent：python-httpx, Bun, curl, OpenAI",
  "zh-TW": "請求的 User-Agent 只寫了 HTTP 函式庫或 SDK、沒有寫是哪個 agent：python-httpx, Bun, curl, OpenAI",
  ja: "User-Agent に HTTP ライブラリか SDK の名前しかなく、エージェントを名乗らないリクエスト：python-httpx, Bun, curl, OpenAI",
  de: "Anfragen, deren User-Agent nur eine HTTP-Bibliothek oder ein SDK nennt, keinen Agenten: python-httpx, Bun, curl, OpenAI",
};

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  test(`${engine}: scripts are one card, named Scripts, not an agent each`, async (t) => {
    const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
    const errors = [];
    t.after(async () => {
      if (errors.length) console.log(errors);
      await browser.close();
    });
    for (const lang of Object.keys(name)) for (const [width, height] of [[360, 760], [1100, 760]]) {
      await t.test(`${lang}, ${width}x${height}`, async () => {
        const ctx = await browser.newContext({ viewport: { width, height }, reducedMotion: "reduce" });
        await ctx.addInitScript(() => { try { localStorage.setItem("magpie.usageTab", "context"); } catch {} });
        const page = await ctx.newPage();
        page.setDefaultTimeout(5000);
        page.on("pageerror", (e) => errors.push(e.message));
        await page.route("**/*", serve(lang));
        await page.goto("http://magpie.test/?view=usage");
        const cards = page.locator(".ctx-agents:not(.ctx-tunes) .ctx-agent");
        await cards.nth(1).waitFor();
        assert.equal(await cards.count(), 2);
        const s = cards.nth(0);
        assert.equal(await s.locator(".ctx-who b").textContent(), name[lang]);
        assert.equal(await s.locator(".ctx-who").getAttribute("title"), why[lang]);
        assert.match(await s.locator(".ctx-who-sub").textContent(), /^python-httpx, Bun, curl \+1 · /);
        // an agent's card names no clients; its name comes with the state,
        // which can answer after the history
        await cards.nth(1).locator(".ctx-who b", { hasText: /^Codex$/ }).waitFor();
        assert.doesNotMatch(await cards.nth(1).locator(".ctx-who-sub").textContent(), /,/);
        // the Sessions filter names the scripts' sessions as the card does
        assert.equal(await page.locator(".ctx-sess-head button.opt").filter({ hasText: name[lang] }).count(), 1);

        const over = await page.evaluate(() => {
          const out = [];
          const view = document.querySelector("#view-usage");
          if (view.scrollWidth > view.clientWidth + 1) out.push("page " + view.scrollWidth + ">" + view.clientWidth);
          const c = document.querySelector(".ctx-agent");
          const b = c.getBoundingClientRect();
          for (const e of c.querySelectorAll(".ctx-agent-head *")) {
            const r = e.getBoundingClientRect();
            if (r.width && (r.right > b.right + 1 || r.left < b.left - 1)) out.push((e.className.baseVal ?? e.className) + " " + Math.round(r.left) + "–" + Math.round(r.right) + " of " + Math.round(b.left) + "–" + Math.round(b.right));
          }
          return out;
        });
        assert.deepEqual(over, []);
        await ctx.close();
      });
    }
    assert.deepEqual(errors, []);
  });
}
