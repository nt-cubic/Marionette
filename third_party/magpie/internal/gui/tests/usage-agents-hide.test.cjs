// Run with Node's test runner and Playwright on the module path; see README.md.
// The Usage page's Agents list hides the clients a reader doesn't care
// about (ikunbots on X: Go-http-client, AsyncOpenAI, curl, python-httpx and
// magpie beside Pi and their own agent). A row's actions (its "…" button,
// or a right-click) hide it from the list; the totals and the chart still
// count it. A quiet line under the list says how many are hidden and opens
// them, each with Unhide. What is hidden is saved (POST /api/usage/arrange,
// agentsHidden) and stays hidden when the page is opened again. No click
// moves the page. Every language, a wide window, a narrow one and magpie
// web on a phone; no backend, the API is faked.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");
const reader = require("./reader.cjs");

const assets = path.resolve(__dirname, "../assets");

const G = (id, name, tokens, icon) => ({ id, name, icon: icon || "generic", calls: 10, input: tokens - 100, output: 100, cache_read: 0, cache_write: 0, cost: 0 });
const AGENTS = [
  G("pi", "Pi", 900000, "pi"),
  G("novelcraft", "novelcraft", 500000),
  G("curl", "curl", 40000),
  G("Go-http-client", "Go-http-client", 30000),
  G("python-httpx", "python-httpx", 20000),
  G("AsyncOpenAI", "AsyncOpenAI", 10000),
];
const TOTAL = AGENTS.reduce((n, g) => n + g.input + g.output, 0);
const USAGE = {
  calls: 60, errors: 0, input: TOTAL - 600, output: 600, reasoning: 0, cache_read: 0, cache_write: 0, unpriced: 0, cost: 0, bucket: "day",
  series: [],
  path: "~/.config/magpie/usage.jsonl",
  agents: AGENTS,
  models: [{ ...G("m", "glm-5", TOTAL), model: "glm-5" }],
};

function server(lang, web, saved, posts) {
  return async (route) => {
    const req = route.request();
    const url = new URL(req.url());
    const json = (data) => route.fulfill({ json: data });
    const settings = () => ({ lang, theme: "light", usageAgentsHidden: saved.hidden });
    if (url.pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"${lang}",theme:"light",web:${web}};` });
    if (url.pathname === "/wails/runtime.js") return route.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    // the clients magpie knows, curl among them, as /api/state lists them:
    // the others it doesn't know can be counted as one of them (#1958)
    if (url.pathname === "/api/state") return json({ agents: [], clients: [{ id: "pi", name: "Pi", icon: "pi" }, { id: "curl", name: "curl", icon: "curl" }], profiles: [], settings: settings(), fx: { rate: 7.2, at: new Date().toISOString() } });
    if (url.pathname === "/api/usage/arrange") {
      const body = JSON.parse(req.postData() || "{}");
      posts.push(body);
      if (body.agentsHidden) saved.hidden = body.agentsHidden;
      return json(settings());
    }
    if (url.pathname === "/api/settings") return json(settings());
    if (url.pathname === "/api/usage/quotas") return json([]);
    if (url.pathname === "/api/usage") return json(USAGE);
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
  en: { hide: "Hide from this list", n: (n) => `${n} hidden`, unhide: "Unhide" },
  zh: { hide: "从列表中隐藏", n: (n) => `已隐藏 ${n} 个`, unhide: "取消隐藏" },
  "zh-TW": { hide: "從列表中隱藏", n: (n) => `已隱藏 ${n} 個`, unhide: "取消隱藏" },
  ja: { hide: "この一覧から隠す", n: (n) => `${n} 件非表示`, unhide: "再表示" },
  de: { hide: "In dieser Liste ausblenden", n: (n) => `${n} ausgeblendet`, unhide: "Einblenden" },
};
const SIZES = [{ width: 1200, web: false }, { width: 560, web: false }, { width: 390, web: true }];

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  test(engine + ": the Usage page's Agents list hides a client, and brings it back", async (t) => {
    assert(["chromium", "webkit"].includes(engine), "BROWSER must be chromium or webkit");
    const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
    t.after(() => browser.close());
    for (const lang of Object.keys(L)) {
      for (const { width, web } of SIZES) {
        const w = L[lang];
        await t.test(`${lang} ${width}${web ? " web" : ""}`, async () => {
          const errors = [];
          // python-httpx was hidden before: it stays hidden
          const saved = { hidden: ["python-httpx"] }, posts = [];
          const ctx = await browser.newContext({ viewport: { width, height: 700 }, reducedMotion: "reduce", hasTouch: web, isMobile: web && engine === "chromium" });
          const open = async () => {
            const p = await ctx.newPage();
            p.setDefaultTimeout(5000);
            p.on("pageerror", (e) => errors.push(e.message));
            await p.route("**/*", server(lang, web, saved, posts));
            await p.goto("http://magpie.test/");
            const nav = p.locator('[data-view="usage"]:visible').first();
            if (await nav.count()) await nav.click();
            else await p.evaluate(() => show("usage"));
            await p.locator("#usageAgents .row.stat").first().waitFor();
            return p;
          };
          const ids = (p, sel) => p.locator(sel + " .row.stat").evaluateAll((rs) => rs.map((r) => r.dataset.id));
          const tokens = (p) => p.locator("#stats .kpi b").first().textContent();
          const top = (p) => p.locator("#view-usage").evaluate((v) => v.scrollTop);

          let p = await open();
          const allTokens = await tokens(p);
          assert.deepEqual(await ids(p, "#usageAgents"), ["pi", "novelcraft", "curl", "Go-http-client", "AsyncOpenAI"]);
          const line = p.locator("#usageAgentsHid .us-hidline");
          assert.equal((await line.textContent()).trim(), w.n(1));
          assert.equal(await p.locator("#usageAgentsHid .list").isHidden(), true, "the hidden ones wait under their line");

          // a row's "…" opens its menu; Hide takes it out of the list
          const row = (id) => p.locator(`#usageAgents .row.stat[data-id="${id}"]`);
          // brought into sight by the reader's wheel, then hovered: its "…" shows
          await reader.inView(p, row("curl"));
          if (!web) await row("curl").hover();
          const more = row("curl").locator(".us-more");
          // shown once the hover's fade is done
          await more.evaluate((b) => new Promise((ok, no) => { const end = Date.now() + 2000; (function f() { getComputedStyle(b).opacity === "1" ? ok() : Date.now() > end ? no(new Error("the hovered row's actions don't show")) : requestAnimationFrame(f); })(); }));
          let at = await top(p);
          await more.click();
          const item = p.locator(".row-menu .rm-item");
          assert.deepEqual(await item.allTextContents(), [w.hide]);
          await item.click();
          await p.waitForFunction(() => !document.querySelector('#usageAgents .row.stat[data-id="curl"]'));
          assert.deepEqual(posts.at(-1), { agentsHidden: ["python-httpx", "curl"] });
          assert.equal((await line.textContent()).trim(), w.n(2));
          assert.equal(await top(p), at, "hiding a row doesn't move the page");

          // and a right-click opens the same menu
          if (!web) {
            await reader.inView(p, row("Go-http-client"));
            at = await top(p);
            await row("Go-http-client").click({ button: "right" });
            await p.locator(".row-menu .rm-item", { hasText: w.hide }).click();
            await p.waitForFunction(() => !document.querySelector('#usageAgents .row.stat[data-id="Go-http-client"]'));
          } else {
            await reader.inView(p, row("Go-http-client"));
            at = await top(p);
            await row("Go-http-client").locator(".us-more").click();
            await p.locator(".row-menu .rm-item", { hasText: w.hide }).click();
            await p.waitForFunction(() => !document.querySelector('#usageAgents .row.stat[data-id="Go-http-client"]'));
          }
          assert.deepEqual(saved.hidden, ["python-httpx", "curl", "Go-http-client"]);
          assert.equal((await line.textContent()).trim(), w.n(3));
          assert.equal(await top(p), at, "hiding a row doesn't move the page");
          assert.equal(await tokens(p), allTokens, "the totals still count the hidden ones");
          assert.deepEqual(await ids(p, "#usageAgents"), ["pi", "novelcraft", "AsyncOpenAI"]);

          // the line opens them, the page left where it is
          await reader.inView(p, line);
          at = await top(p);
          await line.click();
          assert.equal(await line.getAttribute("aria-expanded"), "true");
          assert.deepEqual(await ids(p, "#usageAgentsHid"), ["curl", "Go-http-client", "python-httpx"]);
          assert.deepEqual((await p.locator("#usageAgentsHid .us-unhide").allTextContents()).map((s) => s.trim()), [w.unhide, w.unhide, w.unhide]);
          assert.equal(await top(p), at, "opening the hidden ones doesn't move the page");

          // Unhide puts one back where it was in the list; the line clicked
          // under stays where it was on the screen as the row goes in above
          const back = p.locator('#usageAgentsHid .row.stat[data-id="curl"] .us-unhide');
          await reader.inView(p, back);
          const lineAt = () => line.evaluate((e) => e.getBoundingClientRect().top);
          const was = await lineAt();
          await back.click();
          await p.waitForFunction(() => !!document.querySelector('#usageAgents .row.stat[data-id="curl"]'));
          await p.waitForTimeout(150);
          assert.deepEqual(posts.at(-1), { agentsHidden: ["python-httpx", "Go-http-client"] });
          assert.deepEqual(await ids(p, "#usageAgents"), ["pi", "novelcraft", "curl", "AsyncOpenAI"]);
          assert.deepEqual(await ids(p, "#usageAgentsHid"), ["Go-http-client", "python-httpx"]);
          assert.equal((await line.textContent()).trim(), w.n(2));
          // within the pixel the page's hold leaves it
          const now = await lineAt();
          assert(Math.abs(now - was) < 1.5, `unhiding a row leaves the hidden line where it was: ${was} → ${now}`);

          // nothing is wider than the page
          const wide = await p.evaluate(() => { const v = document.querySelector("#view-usage"); return [v.scrollWidth, v.clientWidth, document.documentElement.scrollWidth, window.innerWidth]; });
          assert(wide[0] <= wide[1] + 1 && wide[2] <= wide[3] + 1, "no sideways scroll: " + wide);
          const out = await p.evaluate(() => [...document.querySelectorAll("#usageAgents .row.stat, #usageAgentsHid .row.stat")].filter((r) => {
            const box = r.getBoundingClientRect();
            return [...r.querySelectorAll(".us-more, .us-unhide")].some((b) => { const x = b.getBoundingClientRect(); return x.right > box.right + 0.5 || x.left < box.left - 0.5; });
          }).map((r) => r.dataset.id));
          assert.deepEqual(out, [], "every row's action stays inside its row");

          // opened again, the hidden ones are still hidden
          await p.close();
          p = await open();
          assert.deepEqual(await ids(p, "#usageAgents"), ["pi", "novelcraft", "curl", "AsyncOpenAI"]);
          assert.equal((await p.locator("#usageAgentsHid .us-hidline").textContent()).trim(), w.n(2));
          assert.equal(await tokens(p), allTokens);

          // all of them back, the line goes
          await reader.click(p, p.locator("#usageAgentsHid .us-hidline"));
          for (const id of ["Go-http-client", "python-httpx"]) {
            const b = p.locator(`#usageAgentsHid .row.stat[data-id="${id}"] .us-unhide`);
            await reader.inView(p, b);
            await b.click();
            await p.waitForFunction((id) => !!document.querySelector(`#usageAgents .row.stat[data-id="${id}"]`), id);
          }
          assert.deepEqual(saved.hidden, []);
          assert.equal(await p.locator("#usageAgentsHid").isHidden(), true);
          assert.equal(await p.locator("#usageAgents .row.stat").count(), AGENTS.length);
          assert.deepEqual(errors, []);
          await ctx.close();
        });
      }
    }
  });
}
