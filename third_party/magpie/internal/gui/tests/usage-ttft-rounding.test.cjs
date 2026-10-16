// Run with Node's test runner and Playwright on the module path; see README.md.
// An average first-token time on the Usage page's Requests tab is whole
// milliseconds under a second and a second with one decimal above it (John
// on Discord: the speed trend's tooltip read "258 token/秒 · 首字
// 761.9796610169492 毫秒"). The trend's tooltip, the ranking and the period's
// KPI, in English and Chinese; no backend, the API is faked.
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

const row = (i, model) => ({
  route_id: 900 + i, t: new Date(now - (i + 1) * 60e3).toISOString(), agent: "claude", agentName: "Claude Code", icon: "claudecode-color",
  provider: "zai", providerName: "Z.ai Coding Plan", req: model, model, served: model, in: 1000, out: 258, cache_read: 0, cache_write: 0,
  ms: 1762, ttft_ms: 762, status: 200, rid: "req_" + i, ep: "/v1/messages", cost: 0.01, priced: true,
});
// fast: 59 replies whose first tokens took 44955 ms, 761.949… on average;
// slow: 3 in 3500 ms, 1166.66…; all of them 48455 ms over 62, 781.53…
const FAST = { id: "glm-fast", calls: 59, input: 1, output: 15222, cache_read: 0, cache_write: 0, cost: 0.5, timed: 59, ttft_ms: 44955, decode_ms: 59000, decode_out: 15222 };
const SLOW = { id: "kimi-slow", calls: 3, input: 1, output: 300, cache_read: 0, cache_write: 0, cost: 0.1, timed: 3, ttft_ms: 3500, decode_ms: 30000, decode_out: 300 };
const part = (x) => ({ calls: x.calls, tokens: x.output, cost: x.cost, timed: x.timed, ttft_ms: x.ttft_ms, decode_ms: x.decode_ms, decode_out: x.decode_out });
const sum = (k) => FAST[k] + SLOW[k];
const timing = { timed: sum("timed"), ttft_ms: sum("ttft_ms"), decode_ms: sum("decode_ms"), decode_out: sum("decode_out") };

function ledger(period) {
  return {
    period, rows: [row(0, "glm-fast"), row(1, "kimi-slow")], offset: 0, total: 2, calls: sum("calls"), errors: 0,
    input: sum("input"), output: sum("output"), cache_read: 0, cache_write: 0, cost: sum("cost"), unpriced: 0, ...timing,
    by: { model: [FAST, SLOW] }, bucket: "hour",
    series: [{ label: "", time: hour(0), calls: sum("calls"), input: 2, output: sum("output"), ...timing, by: { model: { "glm-fast": part(FAST), "kimi-slow": part(SLOW) } } }],
    agents: [{ id: "claude", name: "Claude Code", icon: "claudecode-color" }],
  };
}

function server(lang) {
  return async (route) => {
    const url = new URL(route.request().url());
    const json = (data) => route.fulfill({ json: data });
    if (url.pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"${lang}",theme:"light",web:false};` });
    if (url.pathname === "/wails/runtime.js") return route.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json({ agents: [], profiles: [], settings: { lang, theme: "light" } });
    if (url.pathname === "/api/usage/requests") return json(ledger(url.searchParams.get("period")));
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
  en: { speed: "Speed", kpi: "Output speed", first: "first token in 782 ms on average · 62 timed replies", fast: "TTFT 762 ms", slow: "TTFT 1.2 s" },
  zh: { speed: "速度", kpi: "输出速度", first: "首字平均 782 毫秒 · 62 次计时", fast: "首字 762 毫秒", slow: "首字 1.2 秒" },
};
// a long fraction anywhere: 761.949…, 1166.66…
const ragged = /\d[.,]\d{2,} ?(ms|s|毫秒|秒)/;

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  test(engine + ": an average first token in whole milliseconds", async (t) => {
    const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
    t.after(() => browser.close());
    for (const lang of ["en", "zh"]) {
      const w = L[lang];
      await t.test(lang, async () => {
        const errors = [];
        const ctx = await browser.newContext({ viewport: { width: 1100, height: 800 }, reducedMotion: "reduce" });
        const p = await ctx.newPage();
        p.setDefaultTimeout(5000);
        p.on("pageerror", (e) => errors.push(e.message));
        await p.route("**/*", server(lang));
        await p.goto("http://magpie.test/");
        await p.locator('[data-view="usage"]').first().click();
        await p.locator(":is(#usageTab .opt, .nav-s):visible").nth(1).click();
        await p.locator("#ledWrap .led tbody tr").first().waitFor();
        await p.waitForTimeout(300);

        // the period's KPI
        const kpi = p.locator("#ledKpi .blk").filter({ hasText: w.kpi });
        assert.equal((await kpi.locator(".sub").textContent()).trim(), w.first);
        // the ranking: each model's own
        const rank = await p.locator("#ledRank .rk .rk-b").allTextContents();
        assert(rank.some((s) => s.includes(w.fast)) && rank.some((s) => s.includes(w.slow)), "the ranking: " + rank.join(" | "));
        assert(!rank.some((s) => ragged.test(s)), "no long fraction in the ranking: " + rank.join(" | "));

        // the speed trend's tooltip, the report's
        await reader.click(p, p.locator("#ledMetric .opt").filter({ hasText: w.speed }));
        await p.waitForTimeout(150);
        const box = await p.locator("#ledChart rect.col.all").first().boundingBox();
        await p.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
        const tip = p.locator("#ledChart .tip");
        await tip.waitFor();
        const said = await tip.textContent();
        assert(said.includes(w.fast) && said.includes(w.slow), "the tooltip: " + said);
        assert(!ragged.test(said), "no long fraction in the tooltip: " + said);
        assert.deepEqual(errors, []);
        await ctx.close();
      });
    }
  });
}
