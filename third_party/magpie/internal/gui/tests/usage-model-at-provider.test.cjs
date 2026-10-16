// Run with Node's test runner and Playwright on the module path; see README.md.
// The Usage page's Requests tab ranks one model at each provider it went to
// (inaction on Discord: "相同模型在不同提供商的速度情况"). The chart's split
// has "Model · provider": glm-5.3 at Zhipu and at ZCode are two rows of the
// ranking, each with its own speed and first token, the fastest first by
// Speed; a click lists that provider's requests of that model, a click again
// all of them, and the page doesn't move. English and Chinese; no backend,
// the API is faked.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");
const reader = require("./reader.cjs");

const assets = path.resolve(__dirname, "../assets");
const now = Date.now();
const H = 3600e3;
const hour = (i) => new Date(Math.floor(now / H) * H - i * H).toISOString();

const row = (i, provider, providerName) => ({
  route_id: 900 + i, t: new Date(now - (i + 1) * 60e3).toISOString(), agent: "claude", agentName: "Claude Code", icon: "claudecode-color",
  provider, providerName, req: "glm-5.3", model: "glm-5.3", served: "glm-5.3", in: 1000, out: 500, cache_read: 0, cache_write: 0,
  ms: 2000, ttft_ms: 300, status: 200, rid: "req_" + i, ep: "/v1/messages", cost: 0.01, priced: true,
});
// Zhipu: 2 replies, 300 ms to the first token, 100 tok/s; ZCode: 4 replies,
// 1200 ms, 25 tok/s. More requests went to ZCode, so by requests it leads.
const ZHIPU = { id: "zhipu/glm-5.3", name: "glm-5.3 · Zhipu", icon: "zhipu-color", calls: 2, input: 1, output: 1000, cache_read: 0, cache_write: 0, cost: 0.1, timed: 2, ttft_ms: 600, decode_ms: 10000, decode_out: 1000 };
const ZCODE = { id: "zcode/glm-5.3", name: "glm-5.3 · ZCode", icon: "zcode", calls: 4, input: 1, output: 2000, cache_read: 0, cache_write: 0, cost: 0.2, timed: 4, ttft_ms: 4800, decode_ms: 80000, decode_out: 2000 };
const MODEL = { id: "glm-5.3", calls: 6, input: 2, output: 3000, cache_read: 0, cache_write: 0, cost: 0.3, timed: 6, ttft_ms: 5400, decode_ms: 90000, decode_out: 3000 };
const part = (x) => ({ calls: x.calls, tokens: x.output, cost: x.cost, timed: x.timed, ttft_ms: x.ttft_ms, decode_ms: x.decode_ms, decode_out: x.decode_out });
const timing = { timed: 6, ttft_ms: 5400, decode_ms: 90000, decode_out: 3000 };

function ledger(q) {
  const provider = q.get("provider"), model = q.get("model");
  const rows = [row(0, "zhipu", "Zhipu"), row(1, "zcode", "ZCode")].filter((r) => (!provider || r.provider === provider) && (!model || r.model === model));
  return {
    period: q.get("period"), rows, offset: 0, total: rows.length, calls: 6, errors: 0,
    input: 2, output: 3000, cache_read: 0, cache_write: 0, cost: 0.3, unpriced: 0, ...timing,
    by: {
      model: [MODEL], modelAt: [ZCODE, ZHIPU],
      provider: [{ ...ZCODE, id: "zcode", name: "ZCode" }, { ...ZHIPU, id: "zhipu", name: "Zhipu" }],
    },
    bucket: "hour",
    series: [{
      label: "", time: hour(0), calls: 6, input: 2, output: 3000, ...timing,
      by: { model: { "glm-5.3": part(MODEL) }, modelAt: { "zhipu/glm-5.3": part(ZHIPU), "zcode/glm-5.3": part(ZCODE) } },
    }],
    agents: [{ id: "claude", name: "Claude Code", icon: "claudecode-color" }],
    providers: [{ id: "zhipu", name: "Zhipu", icon: "zhipu-color" }, { id: "zcode", name: "ZCode", icon: "zcode" }],
  };
}

function server(lang, asked) {
  return async (route) => {
    const url = new URL(route.request().url());
    const json = (data) => route.fulfill({ json: data });
    if (url.pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"${lang}",theme:"light",web:false};` });
    if (url.pathname === "/wails/runtime.js") return route.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json({ agents: [], profiles: [], settings: { lang, theme: "light" } });
    if (url.pathname === "/api/usage/requests") { asked.push(url.searchParams); return json(ledger(url.searchParams)); }
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
  en: { split: "Model · provider", speed: "Speed", fast: "TTFT 300 ms", slow: "TTFT 1.2 s" },
  zh: { split: "模型 · 供应商", speed: "速度", fast: "首字 300 毫秒", slow: "首字 1.2 秒" },
};

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  test(engine + ": one model ranked at each provider", async (t) => {
    const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
    t.after(() => browser.close());
    for (const lang of ["en", "zh"]) {
      const w = L[lang];
      await t.test(lang, async () => {
        const errors = [], asked = [];
        const ctx = await browser.newContext({ viewport: { width: 1100, height: 800 }, reducedMotion: "reduce" });
        const p = await ctx.newPage();
        p.setDefaultTimeout(5000);
        p.on("pageerror", (e) => errors.push(e.message));
        await p.route("**/*", server(lang, asked));
        await p.goto("http://magpie.test/");
        await p.locator('[data-view="usage"]').first().click();
        await p.locator(":is(#usageTab .opt, .nav-s):visible").nth(1).click();
        await p.locator("#ledWrap .led tbody tr").first().waitFor();
        await p.waitForTimeout(300);

        await reader.click(p, p.locator("#ledSplit .opt").filter({ hasText: w.split }));
        await reader.click(p, p.locator("#ledMetric .opt").filter({ hasText: w.speed }));
        await p.waitForTimeout(150);
        const rows = p.locator("#ledRank button.rk");
        assert.equal(await rows.count(), 2);
        // by speed the fastest first, though fewer requests went to it
        const names = await p.locator("#ledRank button.rk .rk-nm").allTextContents();
        assert.deepEqual(names, ["glm-5.3 · Zhipu", "glm-5.3 · ZCode"]);
        const said = await p.locator("#ledRank button.rk .rk-b").allTextContents();
        assert(said[0].includes(w.fast) && said[1].includes(w.slow), "each its own first token: " + said.join(" | "));
        // the provider's icon beside each
        assert.deepEqual(await p.locator("#ledRank button.rk .rk-a .ic").evaluateAll((x) => x.map((e) => e.dataset.icon)), ["zhipu-color", "zcode"]);

        // a click lists ZCode's glm-5.3 only; the page stays where it was
        const y = await p.evaluate(() => [document.scrollingElement.scrollTop, document.querySelector("#ledRank").scrollTop]);
        const before = asked.length;
        await reader.click(p, rows.nth(1));
        await p.waitForFunction((n) => document.querySelectorAll("#ledRank button.rk.on").length === 1, before);
        const q = asked.at(-1);
        assert(asked.length > before);
        assert.equal(q.get("provider"), "zcode");
        assert.equal(q.get("model"), "glm-5.3");
        assert.equal(await p.locator("#ledRank button.rk.on .rk-nm").textContent(), "glm-5.3 · ZCode");
        assert.deepEqual(await p.evaluate(() => [document.scrollingElement.scrollTop, document.querySelector("#ledRank").scrollTop]), y);

        // again: all of them
        await reader.click(p, p.locator("#ledRank button.rk.on"));
        await p.waitForFunction(() => !document.querySelector("#ledRank button.rk.on"));
        assert.equal(asked.at(-1).get("provider"), null);
        assert.equal(asked.at(-1).get("model"), null);
        assert.deepEqual(errors, []);
        await ctx.close();
      });
    }
  });
}
