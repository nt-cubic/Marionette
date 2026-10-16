// Run with Node's test runner and Playwright on the module path; see README.md.
// A client magpie doesn't know is counted as one of the agents it does,
// from the Usage page's Agents list (#1958, NorethSea): a bridge that
// starts Codex's app-server under a name of its own
// (hermes-codex-thread-bridge) was a row apart from Codex. Its row's "…"
// offers "Count as an agent…", which lists the agents magpie knows; Codex
// picked, it is saved (POST /api/usage/arrange, count) and the page is
// drawn again from the ledger, the bridge's calls in Codex's row, which
// says it has them. Codex's own "…" counts it on its own again. An agent
// magpie knows (curl) isn't offered to be counted as another. Every
// language, a wide window and a narrow one, Chromium and WebKit; no
// backend, the API is faked.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");
const reader = require("./reader.cjs");

const assets = path.resolve(__dirname, "../assets");
const BRIDGE = "hermes-codex-thread-bridge";

const G = (id, name, calls, tokens, icon) => ({ id, name, icon: icon || "generic", calls, input: tokens - 100, output: 100, cache_read: 0, cache_write: 0, cost: 0 });
const CLIENTS = [{ id: "codex", name: "Codex", icon: "codex" }, { id: "claude-code", name: "Claude Code", icon: "claude" }, { id: "curl", name: "curl", icon: "curl" }, { id: "magpie", name: "magpie", icon: "magpie" }];

// the summary as the backend counts it: the bridge apart, or in Codex's row
function usageOf(counted) {
  const agents = counted
    ? [G("codex", "Codex", 23, 900000 + 13000, "codex"), G("curl", "curl", 4, 40000, "curl")]
    : [G("codex", "Codex", 10, 900000, "codex"), G(BRIDGE, BRIDGE, 13, 13000), G("curl", "curl", 4, 40000, "curl")];
  const total = agents.reduce((n, g) => n + g.input + g.output, 0);
  return {
    calls: 27, errors: 0, input: total - 300, output: 300, reasoning: 0, cache_read: 0, cache_write: 0, unpriced: 0, cost: 0, bucket: "day",
    series: [], path: "~/.config/magpie/usage.jsonl", agents,
    models: [{ ...G("m", "glm-5", 27, total), model: "glm-5" }],
  };
}

function server(lang, saved, posts) {
  return async (route) => {
    const req = route.request();
    const url = new URL(req.url());
    const json = (data) => route.fulfill({ json: data });
    const settings = () => ({ lang, theme: "light", clientAgents: { ...saved.clients } });
    if (url.pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"${lang}",theme:"light",web:false};` });
    if (url.pathname === "/wails/runtime.js") return route.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json({ agents: [], clients: CLIENTS, profiles: [], settings: settings() });
    if (url.pathname === "/api/usage/arrange") {
      const body = JSON.parse(req.postData() || "{}");
      posts.push(body);
      if (body.count) {
        if (body.count.as) saved.clients[body.count.client.toLowerCase()] = body.count.as;
        else delete saved.clients[body.count.client.toLowerCase()];
      }
      return json(settings());
    }
    if (url.pathname === "/api/settings") return json(settings());
    if (url.pathname === "/api/usage/quotas") return json([]);
    if (url.pathname === "/api/usage") return json(usageOf(saved.clients[BRIDGE] === "codex"));
    if (url.pathname === "/api/providers") return json({ providers: [], presets: [], excluded: [], models: [], gateway: { running: false } });
    if (url.pathname === "/api/sessions") return json({ sessions: [], dirs: [] });
    if (url.pathname === "/api/sessions/stats") return json({ from: "", to: "", days: [], agents: {} });
    if (url.pathname === "/api/groups") return json({ groups: [], models: [] });
    if (url.pathname.startsWith("/api/")) return json({});
    const file = path.join(assets, url.pathname === "/" ? "index.html" : url.pathname);
    const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)];
    await route.fulfill({ body: await fs.readFile(file).catch(() => ""), contentType });
  };
}

const L = {
  en: { count: "Count as an agent…", head: "Count this client's calls as", own: `Count ${BRIDGE} on its own`, with: `with ${BRIDGE}`, hide: "Hide from this list" },
  zh: { count: "计入某个 Agent…", head: "将此客户端的调用计入", own: `将 ${BRIDGE} 单独计算`, with: `含 ${BRIDGE}`, hide: "从列表中隐藏" },
  "zh-TW": { count: "計入某個 Agent…", head: "將此用戶端的呼叫計入", own: `將 ${BRIDGE} 單獨計算`, with: `含 ${BRIDGE}`, hide: "從列表中隱藏" },
  ja: { count: "エージェントとして数える…", head: "このクライアントの呼び出しを数える先", own: `${BRIDGE} を別に数える`, with: `${BRIDGE} を含む`, hide: "この一覧から隠す" },
  de: { count: "Als Agent zählen…", head: "Aufrufe dieses Clients zählen als", own: `${BRIDGE} wieder einzeln zählen`, with: `mit ${BRIDGE}`, hide: "In dieser Liste ausblenden" },
};

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  test(engine + ": a client is counted as an agent from the Usage page, and on its own again", async (t) => {
    const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
    t.after(() => browser.close());
    for (const lang of Object.keys(L)) {
      for (const width of [1100, 420]) {
        const w = L[lang];
        await t.test(`${lang} ${width}`, async () => {
          const errors = [], posts = [], saved = { clients: {} };
          const ctx = await browser.newContext({ viewport: { width, height: 760 }, reducedMotion: "reduce" });
          const p = await ctx.newPage();
          p.setDefaultTimeout(5000);
          p.on("pageerror", (e) => errors.push(e.message));
          await p.route("**/*", server(lang, saved, posts));
          await p.goto("http://magpie.test/");
          const nav = p.locator('[data-view="usage"]:visible').first();
          if (await nav.count()) await nav.click();
          else await p.evaluate(() => show("usage"));
          await p.locator("#usageAgents .row.stat").first().waitFor();
          const ids = () => p.locator("#usageAgents .row.stat").evaluateAll((rs) => rs.map((r) => r.dataset.id));
          const row = (id) => p.locator(`#usageAgents .row.stat[data-id="${id}"]`);
          const items = () => p.locator(".row-menu .rm-item .rm-name").allTextContents();
          const openMenu = async (id) => {
            await reader.inView(p, row(id));
            await row(id).hover();
            await row(id).locator(".us-more").click();
            await p.locator(".row-menu").waitFor();
          };
          assert.deepEqual(await ids(), ["codex", BRIDGE, "curl"]);

          // an agent magpie knows isn't offered to be counted as another
          await openMenu("curl");
          assert.deepEqual(await items(), [w.hide]);
          await p.keyboard.press("Escape");

          // the bridge's "…": Count as an agent…, then Codex
          await openMenu(BRIDGE);
          assert.deepEqual(await items(), [w.count, w.hide]);
          await p.locator(".row-menu .rm-item", { hasText: w.count }).click();
          const pick = p.locator(".proto-menu");
          await pick.waitFor();
          assert.equal((await pick.locator(".pm-head").textContent()).trim(), w.head);
          const names = await pick.locator(".pm-item .pm-name").allTextContents();
          assert(names.includes("Codex") && !names.includes("magpie"), "the agents offered: " + names);
          await pick.locator(".pm-item", { hasText: "Codex" }).click();
          await p.waitForFunction((b) => !document.querySelector(`#usageAgents .row.stat[data-id="${b}"]`), BRIDGE);
          assert.deepEqual(posts.at(-1), { count: { client: BRIDGE, as: "codex" } });
          assert.deepEqual(await ids(), ["codex", "curl"]);
          const sub = (await row("codex").locator(".sub").textContent()).trim();
          assert(sub.startsWith(w.with), "Codex's row doesn't say it has the bridge: " + sub);

          // nothing is wider than the page
          const wide = await p.evaluate(() => [document.documentElement.scrollWidth, window.innerWidth]);
          assert(wide[0] <= wide[1] + 1, "no sideways scroll: " + wide);

          // Codex's "…" counts the bridge on its own again
          await openMenu("codex");
          assert.deepEqual(await items(), [w.own, w.hide]);
          await p.locator(".row-menu .rm-item", { hasText: w.own }).click();
          await p.waitForFunction((b) => !!document.querySelector(`#usageAgents .row.stat[data-id="${b}"]`), BRIDGE);
          assert.deepEqual(posts.at(-1), { count: { client: BRIDGE, as: "" } });
          assert.deepEqual(await ids(), ["codex", BRIDGE, "curl"]);
          assert(!(await row("codex").locator(".sub").textContent()).includes(BRIDGE));
          assert.deepEqual(errors, []);
          await ctx.close();
        });
      }
    }
  });
}
