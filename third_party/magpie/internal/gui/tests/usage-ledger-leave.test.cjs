// Run with Node's test runner and Playwright on the module path; see README.md.
// What a click filtered the Requests by goes when the reader leaves them
// (TJHHHH on Discord): a click on Codex in the ranking split by agent, a
// click on a chart's day, a filter brought from elsewhere (the URL, a
// route from Routing) is for that visit. Off to another page or another
// Usage tab and back, the Requests are of all agents again. An agent picked
// in its own menu stays, as a choice made in a menu does. In every
// language, Chromium and WebKit; no backend, the API is faked.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");
const { click } = require("./reader.cjs");

const assets = path.resolve(__dirname, "../assets");
const now = Date.now();
const ROW = { t: new Date(now).toISOString(), agent: "codex", agentName: "Codex", icon: "codex-color", provider: "openai", providerName: "OpenAI", req: "gpt-5.5", model: "gpt-5.5", in: 2, out: 600, cache_write: 0, cache_read: 0, ms: 2380, status: 200, cost: 0.08, priced: true };
const ROWS = [ROW, { ...ROW, t: new Date(now - 60000).toISOString(), agent: "claude", agentName: "Claude Code", icon: "claudecode-color", req: "claude-sonnet-5", model: "claude-sonnet-5" }];
const AGENTS = [{ id: "codex", name: "Codex", icon: "codex-color" }, { id: "claude", name: "Claude Code", icon: "claudecode-color" }];
const by = (k) => ({ id: k.id, name: k.name, icon: k.icon, calls: 1, input: 2, output: 600, cache_read: 0, cache_write: 0, cost: 0.08 });

function server(lang, asked) {
  return async (route) => {
    const req = route.request(), url = new URL(req.url());
    const json = (data) => route.fulfill({ json: data });
    if (url.pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"${lang}",theme:"light",web:false};` });
    if (url.pathname === "/wails/runtime.js") return route.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json({ agents: [], profiles: [], settings: { lang, theme: "light" }, fx: { rate: 7.2, at: new Date().toISOString() } });
    if (url.pathname === "/api/usage/requests") {
      asked.push(url.searchParams);
      const agent = url.searchParams.get("agent") || "";
      const rows = ROWS.filter((r) => !agent || r.agent === agent);
      return json({
        period: "today", rows, offset: 0, total: rows.length, calls: rows.length, errors: 0, input: 4, output: 1200, cache_write: 0, cache_read: 0, cost: 0.16, reasoning: 0, unpriced: 0,
        bucket: "hour", series: [], by: { provider: [], agent: AGENTS.map(by), model: [] },
        agents: AGENTS, providers: [{ id: "openai", name: "OpenAI", icon: "openai" }],
      });
    }
    if (url.pathname === "/api/usage/quotas") return json([]);
    if (url.pathname === "/api/usage") return json({ calls: 1, errors: 0, input: 1, output: 1, cache_read: 0, cache_write: 0, reasoning: 0, unpriced: 0, cost: 1, bucket: "day", series: [], agents: [], models: [], path: "~/.config/magpie/usage.jsonl" });
    if (url.pathname === "/api/sessions") return json({ sessions: [], dirs: [] });
    if (url.pathname === "/api/sessions/stats") return json({ from: "", to: "", days: [], agents: {} });
    if (url.pathname === "/api/groups") return json({ groups: [], models: [] });
    if (url.pathname.startsWith("/api/")) return json({});
    const file = path.join(assets, url.pathname === "/" ? "index.html" : url.pathname);
    const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)];
    await route.fulfill({ body: await fs.readFile(file), contentType });
  };
}

// the newest ask for the Requests satisfies want
async function lastAsked(p, asked, want, what) {
  for (let i = 0; i < 100; i++) {
    const q = asked[asked.length - 1];
    if (q && want(q)) return;
    await p.waitForTimeout(50);
  }
  assert.fail(what + ": last asked " + String(asked[asked.length - 1]));
}

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  test(engine + ": a filter a click set on the Requests goes when they are left", async (t) => {
    assert(["chromium", "webkit"].includes(engine), "BROWSER must be chromium or webkit");
    const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
    t.after(() => browser.close());
    for (const lang of ["en", "zh", "zh-TW", "ja", "de"]) {
      const open = async (q = "") => {
        const asked = [], errors = [];
        const context = await browser.newContext({ viewport: { width: 1100, height: 760 }, reducedMotion: "reduce" });
        t.after(() => context.close());
        const p = await context.newPage();
        p.setDefaultTimeout(5000);
        p.on("pageerror", (e) => errors.push(e.message));
        await p.route("**/*", server(lang, asked));
        await p.goto("http://magpie.test/?view=usage&tab=requests" + q);
        await p.locator("table.led tbody tr.led-row").first().waitFor();
        return { p, asked, errors };
      };
      // the Usage tabs are in the sidebar under the page's name, or on the
      // page itself where the sidebar has none
      const tabAt = (p, id) => p.evaluate((id) => USAGE_TABS.findIndex(([x]) => x === id), id).then((i) => p.locator(":is(#usageTab .opt, .nav-s):visible").nth(i));
      // away to the Agents page and back to the Usage page's Requests
      const awayByPage = async (p) => {
        await p.locator('#nav button[data-view="agents"]').click();
        await p.locator("#view-agents").waitFor();
        await p.locator('#nav button[data-view="usage"]').click();
      };
      // away to the Overview tab and back
      const awayByTab = async (p) => {
        await (await tabAt(p, "usage")).click();
        await (await tabAt(p, "requests")).click();
      };
      const allAgents = async (p, asked, what) => {
        await lastAsked(p, asked, (q) => !q.has("agent") && !q.has("route"), what);
        await p.locator("table.led tbody tr.led-row").nth(1).waitFor();
        assert.equal(await p.locator("#ledAgent").evaluate((b) => b.classList.contains("set")), false, what + ": the agent menu says all agents");
      };

      for (const [how, away] of [["another page", awayByPage], ["another tab", awayByTab]]) {
        await t.test(`${lang}: a ranking click on Codex goes on leaving for ${how}`, async () => {
          const { p, asked, errors } = await open();
          await click(p, p.locator("#ledSplit .opt").nth(3));
          await click(p, p.locator("#ledRank .rk", { hasText: "Codex" }));
          await lastAsked(p, asked, (q) => q.get("agent") === "codex", "the click lists Codex's requests");
          await away(p);
          await allAgents(p, asked, "back from " + how);
          assert.equal(await p.locator("#view-usage select").count(), 0);
          assert.deepEqual(errors, []);
        });
      }

      await t.test(`${lang}: Codex picked in the agent menu stays`, async () => {
        const { p, asked, errors } = await open();
        await click(p, p.locator("#ledAgent"));
        await p.locator(".sess-menu .pm-item", { hasText: "Codex" }).click();
        await lastAsked(p, asked, (q) => q.get("agent") === "codex", "the pick lists Codex's requests");
        await awayByPage(p);
        await lastAsked(p, asked, (q) => q.get("agent") === "codex", "the pick is still there back on the page");
        await awayByTab(p);
        await lastAsked(p, asked, (q) => q.get("agent") === "codex", "the pick is still there back on the tab");
        assert.equal(await p.locator("#ledAgent").evaluate((b) => b.classList.contains("set")), true);
        // a ranking click on the one picked doesn't make the pick a loan
        await click(p, p.locator("#ledSplit .opt").nth(3));
        await awayByPage(p);
        await lastAsked(p, asked, (q) => q.get("agent") === "codex", "the pick outlives a change of split");
        assert.deepEqual(errors, []);
      });

      await t.test(`${lang}: an agent brought in the address goes on leaving`, async () => {
        const { p, asked, errors } = await open("&agent=codex");
        await lastAsked(p, asked, (q) => q.get("agent") === "codex", "the address lists Codex's requests");
        await awayByPage(p);
        await allAgents(p, asked, "back from another page");
        assert.deepEqual(errors, []);
      });

      await t.test(`${lang}: a route from Routing goes on leaving, and what it set aside with it`, async () => {
        const { p, asked, errors } = await open();
        await click(p, p.locator("#ledSplit .opt").nth(3));
        await click(p, p.locator("#ledRank .rk", { hasText: "Codex" }));
        await lastAsked(p, asked, (q) => q.get("agent") === "codex", "the click lists Codex's requests");
        await p.evaluate(() => window.openUsageRoute({ id: 123, time: new Date().toISOString(), model: "gpt-5.5" }));
        await lastAsked(p, asked, (q) => q.get("route") === "123", "the route is listed");
        await awayByPage(p);
        await allAgents(p, asked, "back from another page");
        assert.equal(await p.locator("#ledRouteClear").isVisible().catch(() => false), false, "no route is left to clear");
        assert.deepEqual(errors, []);
      });
    }
  });
}
