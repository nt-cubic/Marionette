// Run with Node's test runner and Playwright on the module path; see README.md.
// The Usage page's Requests tab has a Model filter beside its Provider
// filter (OHHHHHH05 on X: "为用量面板增加“按模型筛选”，与现有的供应商筛选并列").
// It is the app's own menu, the models the other filters keep with their
// tokens; a pick asks for that model's requests, gateway and session-log
// rows alike, and the page's totals and source cells are then the model's.
// All models asks for every one again, and a click on a model in the chart's
// ranking shows in the filter too. Every language, wide and narrow, in
// Chromium and WebKit; no backend, the API is faked.
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

// glm-5.3 through the gateway; kimi-k3 once through it and once read from
// Claude Code's own session file
const ROWS = [
  { model: "glm-5.3", source: "", provider: "zhipu", providerName: "Zhipu", in: 1000, out: 500 },
  { model: "kimi-k3", source: "", provider: "moonshot", providerName: "Moonshot", in: 3000, out: 700 },
  { model: "kimi-k3", source: "log", provider: "moonshot", providerName: "Moonshot", in: 2000, out: 300 },
].map((x, i) => ({
  route_id: x.source ? 0 : 900 + i, t: new Date(now - (i + 1) * 60e3).toISOString(), agent: "claude", agentName: "Claude Code", icon: "claudecode-color",
  provider: x.provider, providerName: x.providerName, req: x.model, model: x.model, served: x.model, in: x.in, out: x.out, cache_read: 100, cache_write: 10,
  ms: 2000, status: 200, rid: "req_" + i, ep: "/v1/messages", cost: 0.01, priced: true, source: x.source,
}));
const totals = (rows) => ({
  calls: rows.length, errors: 0, input: rows.reduce((n, r) => n + r.in, 0), output: rows.reduce((n, r) => n + r.out, 0),
  cache_read: rows.reduce((n, r) => n + r.cache_read, 0), cache_write: rows.reduce((n, r) => n + r.cache_write, 0), cost: rows.length * 0.01, unpriced: 0,
});
const share = (id) => ({ id, ...totals(ROWS.filter((r) => r.model === id)) });

function ledger(q) {
  const model = q.get("model");
  const rows = ROWS.filter((r) => !model || r.model === model);
  const tot = totals(rows);
  return {
    period: q.get("period"), rows, offset: 0, total: rows.length, ...tot,
    through: totals(rows.filter((r) => !r.source)), direct: totals(rows.filter((r) => r.source === "log")),
    // the models' shares are of the rows without the model's pick
    by: { model: [share("kimi-k3"), share("glm-5.3")] },
    bucket: "hour",
    series: [{ label: "", time: hour(0), ...tot, by: { model: Object.fromEntries(["kimi-k3", "glm-5.3"].map((m) => { const s = share(m); return [m, { calls: s.calls, tokens: s.input + s.output, cost: s.cost }]; })) } }],
    agents: [{ id: "claude", name: "Claude Code", icon: "claudecode-color" }],
    providers: [{ id: "zhipu", name: "Zhipu" }, { id: "moonshot", name: "Moonshot" }],
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

const LANGS = ["en", "zh", "zh-TW", "ja", "de"];
const WIDTHS = [1100, 420];

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  test(engine + ": a Model filter beside the Provider filter", async (t) => {
    const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
    t.after(() => browser.close());
    for (const lang of LANGS) for (const width of WIDTHS) {
      await t.test(lang + " " + width, async () => {
        const errors = [], asked = [];
        const ctx = await browser.newContext({ viewport: { width, height: 800 }, reducedMotion: "reduce" });
        const p = await ctx.newPage();
        p.setDefaultTimeout(5000);
        p.setDefaultNavigationTimeout(60000);
        p.on("pageerror", (e) => errors.push(e.message));
        await p.route("**/*", server(lang, asked));
        await p.goto("http://magpie.test/");
        await p.locator('[data-view="usage"]').first().click();
        await p.locator(":is(#usageTab .opt, .nav-s):visible").nth(1).click();
        await p.locator("#ledWrap .led tbody tr").first().waitFor();
        await p.waitForTimeout(300);
        const w = await p.evaluate(() => ({ all: t("All models"), head: t("Model"), provider: t("All providers"), tokens: t("{n} tokens", { n: fmtN(6220) }) }));
        if (lang !== "en") assert.notEqual(w.all, "All models", "All models is in " + lang);

        // beside the provider's filter, in the same row, the page not wider than the window
        const pick = p.locator("#ledModel");
        await pick.waitFor();
        assert.equal((await pick.textContent()).trim(), w.all);
        assert.equal((await p.locator("#ledProvider").textContent()).trim(), w.provider);
        assert.equal(await pick.evaluate((b) => b.previousElementSibling.id === "ledProviderMore" && b.parentElement === document.querySelector("#ledProvider").parentElement), true);
        const box = await pick.boundingBox();
        assert(box.x >= 0 && box.x + box.width <= width, "the filter in the window: " + JSON.stringify(box));
        assert.equal(await p.evaluate(() => document.scrollingElement.scrollWidth <= innerWidth), true, "no sideways scroll");

        // its menu: the app's, with each model's tokens
        await reader.click(p, pick);
        const menu = p.locator(".sess-menu");
        await menu.waitFor();
        assert.equal(await p.locator("select").count(), 0);
        assert.equal((await menu.locator(".pm-head").textContent()).trim(), w.head);
        assert.deepEqual(await menu.locator(".pm-item .pm-name").allTextContents(), [w.all, "kimi-k3", "glm-5.3"]);
        assert.equal((await menu.locator(".pm-item").nth(1).locator(".pm-note").textContent()).trim(), w.tokens);

        // kimi-k3: its requests, the gateway's and the session file's
        const before = asked.length;
        await reader.click(p, menu.locator(".pm-item").nth(1));
        await p.waitForFunction((n) => document.querySelectorAll("#ledWrap .led tbody tr.led-row").length === 2, before);
        let q = asked.at(-1);
        assert(asked.length > before);
        assert.equal(q.get("model"), "kimi-k3");
        assert.equal(q.get("q"), null);
        assert.equal((await pick.textContent()).trim(), "kimi-k3");
        assert.equal(await pick.evaluate((b) => b.classList.contains("set")), true);
        const sum = await p.locator("#ledSum").textContent();
        const said = await p.evaluate(() => [t("{n} in", { n: fmtN(5000) }), t("{n} out", { n: fmtN(1000) }), t("{n} cache read", { n: fmtN(200) }), t("{n} cache write", { n: fmtN(20) })]);
        for (const x of said) assert(sum.includes(x), "the model's " + x + " in the sum: " + sum);
        assert(await p.locator("#ledWrap .led tbody tr.led-row").evaluateAll((rs) => rs.every((r) => r.textContent.includes("kimi-k3"))));
        // the source cells: one through magpie, one not
        const cells = await p.locator("#ledVia .blk .v").allTextContents();
        assert.equal(cells.length, 3);
        assert.deepEqual(cells.map((c) => parseInt(c, 10)), [2, 1, 1]);

        // All models: every one again
        await reader.click(p, pick);
        await menu.waitFor();
        await reader.click(p, menu.locator(".pm-item").first());
        await p.waitForFunction(() => document.querySelectorAll("#ledWrap .led tbody tr.led-row").length === 3);
        q = asked.at(-1);
        assert.equal(q.get("model"), null);
        assert.equal((await pick.textContent()).trim(), w.all);

        // a model clicked in the chart's ranking is the filter's pick too
        if (width > 700) {
          await reader.click(p, p.locator("#ledRank button.rk").filter({ hasText: "glm-5.3" }));
          await p.waitForFunction(() => document.querySelectorAll("#ledWrap .led tbody tr.led-row").length === 1);
          assert.equal(asked.at(-1).get("model"), "glm-5.3");
          assert.equal((await pick.textContent()).trim(), "glm-5.3");
        }
        assert.deepEqual(errors, []);
        await ctx.close();
      });
    }
  });
}
