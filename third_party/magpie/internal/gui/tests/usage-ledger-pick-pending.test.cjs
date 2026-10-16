// Run with Node's test runner and Playwright on the module path; see README.md.
// A filter picked on Usage > Requests is seen taken at once, while its read
// is on its way (#1990, overstarry: on Windows over a long history, the
// agent picked "has no response" and "the selection change does not take
// effect"). The read is held here as a long history holds it: the picker
// says the agent picked at once, the rows shown stay, dimmed and marked
// busy, and when the answer comes they are its rows, undimmed. Picking All
// agents again does the same. A failed read undims them too. Nothing
// moves. English, Chinese (both), Japanese and German, Chromium and
// WebKit; no backend, the API is faked here.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");

const assets = path.resolve(__dirname, "../assets");
const now = Date.now();
const ROWS = [];
for (let i = 0; i < 40; i++) {
  ROWS.push({ t: new Date(now - (60 + i * 60) * 1e3).toISOString(), agent: i % 2 ? "claude" : "codex", agentName: i % 2 ? "Claude Code" : "Codex",
    provider: "relay", providerName: "Relay", model: "gpt-6-sol", in: 1000 + i, out: 100, ms: 900, status: 200, cost: 0.001, priced: true });
}
function answer(q) {
  const rows = ROWS.filter((r) => !q.get("agent") || r.agent === q.get("agent"));
  return {
    period: q.get("period"), rows: rows.slice(0, 100), offset: 0, total: rows.length, calls: rows.length, errors: 0,
    input: rows.reduce((a, r) => a + r.in, 0), output: 100 * rows.length, cost: 0.001 * rows.length,
    agents: [{ id: "claude", name: "Claude Code" }, { id: "codex", name: "Codex" }],
  };
}

function server(lang, held) {
  return async (route) => {
    const req = route.request(), url = new URL(req.url());
    const json = (data) => route.fulfill({ json: data });
    if (url.pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"${lang}",theme:"light",web:false};` });
    if (url.pathname === "/wails/runtime.js") return route.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/gateway/trace") return json({ mine: false, seq: 0, routes: [], totals: { requests: 0, rerouted: 0, errors: 0 }, now: new Date().toISOString() });
    if (url.pathname === "/api/state") return json({ agents: [], profiles: [], settings: { lang, theme: "light" } });
    if (url.pathname === "/api/usage/requests") {
      const q = url.searchParams;
      // a pick's read, held till the test lets it go
      if (held.on) await new Promise((r) => held.waiting.push(r));
      if (held.fail) return route.fulfill({ status: 500, body: "read failed" });
      return json(answer(q));
    }
    if (url.pathname === "/api/usage") return json({ calls: 40, input: 40000, output: 4000, cost: 0.04, bucket: "day", series: [], agents: [], models: [], path: "~/.config/magpie/usage.jsonl" });
    if (url.pathname === "/api/sessions") return json({ sessions: [], dirs: [] });
    if (url.pathname === "/api/providers") return json({ providers: [], presets: [], excluded: [], models: [], gateway: { running: false } });
    if (url.pathname === "/api/groups") return json({ groups: [], models: [] });
    if (url.pathname.startsWith("/api/")) return json({});
    const file = path.join(assets, url.pathname === "/" ? "index.html" : url.pathname);
    const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)];
    await route.fulfill({ body: await fs.readFile(file), contentType });
  };
}

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  for (const lang of ["en", "zh", "zh-TW", "ja", "de"]) {
    test(`${engine} ${lang}: an agent picked on Requests is seen taken while its read is on its way`, async (t) => {
      const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
      t.after(() => browser.close());
      const held = { on: false, fail: false, waiting: [] };
      const release = async (page) => {
        for (let i = 0; i < 60 && !held.waiting.length; i++) await page.waitForTimeout(25);
        assert(held.waiting.length, "the read was asked");
        held.on = false;
        for (const r of held.waiting.splice(0)) r();
      };
      const errors = [];
      const page = await (await browser.newContext({ viewport: { width: 1100, height: 700 }, reducedMotion: "reduce" })).newPage();
      page.setDefaultTimeout(5000);
      page.on("pageerror", (e) => errors.push(e.message));
      await page.route("**/*", server(lang, held));
      await page.goto("http://magpie.test/");
      await page.locator('[data-view="usage"]').first().click();
      await page.locator(":is(#usageTab .opt, .nav-s):visible").nth(1).click();
      await page.locator("#ledWrap .led tbody tr").first().waitFor();
      const all = await page.evaluate(() => t("All agents"));
      const rows = () => page.locator("#ledWrap .led-row").count();
      const pending = () => page.locator("#ledWrap").evaluate((e) => [e.classList.contains("led-pending"), e.getAttribute("aria-busy")]);
      const dim = () => page.locator("#ledWrap > *").first().evaluate((e) => getComputedStyle(e).opacity);
      const spot = () => page.evaluate(() => [document.scrollingElement.scrollTop, ...[...document.querySelectorAll(".view")].map((v) => v.scrollTop)]);
      assert.equal(await rows(), 40);
      assert.equal(await page.locator("#ledAgent").textContent(), all);
      assert.deepEqual(await pending(), [false, null]);

      for (const [pick, name, n] of [["Claude Code", "Claude Code", 20], [all, all, 40]]) {
        held.on = true;
        const was = await spot();
        await page.locator("#ledAgent").click();
        await page.locator(".sess-menu .pm-item", { hasText: pick }).first().click();
        // at once, the read still held: the picker says it, the rows wait dimmed
        assert.equal(await page.locator("#ledAgent").textContent(), name, "the picker says the pick at once");
        assert.equal(await page.locator("#ledAgent").evaluate((e) => e.classList.contains("set")), name !== all);
        assert.deepEqual(await pending(), [true, "true"], "the rows shown wait, marked busy");
        assert(Number(await dim()) < 1, "and dimmed");
        assert.equal(await rows(), n === 20 ? 40 : 20, "the rows shown stay till the answer comes");
        await release(page);
        await page.waitForFunction((n) => document.querySelectorAll("#ledWrap .led-row").length === n, n);
        assert.deepEqual(await pending(), [false, null], "the answer undims them");
        assert.equal(await dim(), "1");
        assert.equal(await page.locator("#ledAgent").textContent(), name);
        assert.deepEqual(await spot(), was, "the pick moved the page");
      }

      // a read that fails undims them too
      held.on = true;
      held.fail = true;
      await page.locator("#ledAgent").click();
      await page.locator(".sess-menu .pm-item", { hasText: "Codex" }).first().click();
      assert.deepEqual(await pending(), [true, "true"]);
      await release(page);
      await page.waitForFunction(() => !document.querySelector("#ledWrap").classList.contains("led-pending"));
      assert.equal(await page.locator("#ledWrap").getAttribute("aria-busy"), null);
      assert.deepEqual(errors, []);
    });
  }
}
