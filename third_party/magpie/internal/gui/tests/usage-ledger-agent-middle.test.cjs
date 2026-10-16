// Run with Node's test runner and Playwright on the module path; see README.md.
// The Requests tab's Agent cell (#2237, duanckham): the agent's name sits on
// the row's middle line, as the time and the requested model beside it do,
// not raised by its icon. An inline-flex box with no text in its first item
// takes the icon's foot as its baseline, which sat the icon on the line's
// baseline and lifted the name ~2px over its neighbours. In every language,
// Chromium and WebKit, wide and narrow, light and dark; no backend, the API
// is faked.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");

const assets = path.resolve(__dirname, "../assets");
const now = Date.now();
const ROW = { t: new Date(now).toISOString(), agent: "codex", agentName: "Codex", icon: "codex-color", provider: "openai", providerName: "OpenAI", req: "group/nerd", model: "gpt-5.5", in: 2, out: 600, cache_write: 0, cache_read: 3900, ms: 2380, status: 200, cost: 0.08, priced: true };
const ROWS = [
  { ...ROW, route_id: "r-1" },
  { ...ROW, t: new Date(now - 60000).toISOString(), agent: "claude", agentName: "Claude Code", icon: "claudecode-color", req: "claude-sonnet-5" },
  // magpie's own request carries a badge after the name (#1839)
  { ...ROW, t: new Date(now - 120000).toISOString(), agent: "magpie", agentName: "magpie", icon: "", req: "" },
];

function server(lang, theme) {
  return async (route) => {
    const req = route.request(), url = new URL(req.url());
    const json = (data) => route.fulfill({ json: data });
    if (url.pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"${lang}",theme:"${theme}",web:false};` });
    if (url.pathname === "/wails/runtime.js") return route.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json({ agents: [], profiles: [], settings: { lang, theme }, fx: { rate: 7.2, at: new Date().toISOString() } });
    if (url.pathname === "/api/usage/requests") return json({
      period: "today", rows: ROWS, offset: 0, total: ROWS.length, calls: 3, errors: 0, input: 6, output: 1800, cache_write: 0, cache_read: 11700, cost: 0.24, reasoning: 0, unpriced: 0,
      bucket: "hour", series: [], by: { provider: [], agent: [], model: [] },
      agents: [{ id: "codex", name: "Codex", icon: "codex-color" }], providers: [{ id: "openai", name: "OpenAI", icon: "openai" }],
    });
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

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  test(engine + ": the Agent cell's name is on the row's middle line", async (t) => {
    assert(["chromium", "webkit"].includes(engine), "BROWSER must be chromium or webkit");
    const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
    t.after(() => browser.close());
    for (const lang of ["en", "zh", "zh-TW", "ja", "de"]) {
      for (const width of [1100, 420]) {
        await t.test(`${lang} at ${width}`, async () => {
          const errors = [];
          const context = await browser.newContext({ viewport: { width, height: 760 }, reducedMotion: "reduce" });
          t.after(() => context.close());
          const p = await context.newPage();
          p.setDefaultTimeout(5000);
          p.on("pageerror", (e) => errors.push(e.message));
          await p.route("**/*", server(lang, width < 600 ? "dark" : "light"));
          await p.goto("http://magpie.test/?view=usage&tab=requests");
          await p.locator("table.led tbody tr.led-row").nth(2).waitFor();
          // each row: the middle of the name's text, of the time's and of the
          // requested model's, and of the cell itself
          const rows = await p.locator("table.led tbody tr.led-row").evaluateAll((trs) => trs.map((tr) => {
            const mid = (n) => { const r = n.getBoundingClientRect(); return (r.top + r.bottom) / 2; };
            const text = (n) => { const r = document.createRange(); r.selectNodeContents(n); return mid(r); };
            const cells = tr.children, name = cells[1].querySelector(".who > span:not(.ic)");
            const st = tr.querySelector(".st");
            return { cell: mid(cells[1]), name: text(name), time: text(cells[0]), req: text(cells[2]), icon: mid(cells[1].querySelector(".ic")), status: st ? text(st) : null };
          }));
          for (const r of rows) {
            assert(Math.abs(r.name - r.time) <= 1, `the name is level with the time: ${JSON.stringify(r)}`);
            assert(Math.abs(r.name - r.req) <= 1, `the name is level with the requested model: ${JSON.stringify(r)}`);
            assert(Math.abs(r.name - r.cell) <= 1, `the name is in the cell's middle: ${JSON.stringify(r)}`);
            assert(Math.abs(r.icon - r.name) <= 1, `the icon is level with the name: ${JSON.stringify(r)}`);
            if (r.status !== null) assert(Math.abs(r.status - r.time) <= 1, `the status is level with the time: ${JSON.stringify(r)}`);
          }
          assert.equal(await p.locator("#view-usage select").count(), 0);
          assert.equal(await p.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1), true, "the page doesn't scroll sideways");
          assert.deepEqual(errors, []);
        });
      }
    }
  });
}
