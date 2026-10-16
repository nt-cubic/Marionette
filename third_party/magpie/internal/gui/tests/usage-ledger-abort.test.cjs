// Run with Node's test runner and Playwright on the module path; see README.md.
// Usage › Requests over a long history (Vantiboo1ean, ~750k requests): a read
// of the ledger can take seconds, and every filter picked, page turned or
// refresh pressed sent one more beside it. The answers no longer wanted were
// dropped when they came, but their requests kept running, and a browser
// sends only a few at a time to one host: the newest waited behind them and
// the page spun on. Now a read the reader moved on from is cancelled (the
// page's and the heatmap's), and the page asked again while it is read (the
// refresh button, the timer) waits for that read instead of sending another.
// Checked in Chromium and WebKit, wide and narrow; no backend, every answer
// held until the test lets it go. MAGPIE_ASSETS selects another assets
// directory to check the same cases against the code before the fix.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");
const { inView } = require("./reader.cjs");

const assets = process.env.MAGPIE_ASSETS || path.resolve(__dirname, "../assets");
const now = Date.now();
const ROW = (req) => ({ t: new Date(now - 60e3).toISOString(), agent: "codex", agentName: "Codex", icon: "codex-color", provider: "relay", providerName: "Relay", host: "team", req, model: "gpt-6-sol", in: 100, out: 10, ms: 900, status: 200, cost: 0.01, priced: true });

function server(lang, seen, held) {
  return async (route) => {
    const req = route.request(), url = new URL(req.url());
    const json = (data) => route.fulfill({ json: data }).catch(() => {}); // a cancelled request has no one to answer
    if (url.pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"${lang}",theme:"light",web:false};` });
    if (url.pathname === "/wails/runtime.js") return route.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json({ agents: [], profiles: [], settings: { lang, theme: "light" }, fx: { rate: 7.2, at: new Date().toISOString() } });
    if (url.pathname === "/api/usage/requests" || url.pathname === "/api/usage/heatmap") {
      const kind = url.pathname.endsWith("heatmap") ? "heat" : "page", q = url.searchParams.get("q") || "";
      seen.push({ kind, q, url: req.url() });
      await new Promise((go) => held.push(go));
      if (kind === "heat") return json({ from: "", to: "", days: [] });
      return json({ period: url.searchParams.get("period"), rows: [ROW("asked-" + (q || "none"))], offset: 0, total: 1, calls: 1, errors: 0, input: 100, output: 10, cache_read: 0, cache_write: 0, reasoning: 0, cost: 0.01, unpriced: 0, bucket: "day", series: [], agents: [], providers: [] });
    }
    if (url.pathname === "/api/usage/quotas") return json([]);
    if (url.pathname === "/api/usage") return json({ calls: 1, errors: 0, input: 100, output: 10, cache_read: 0, cache_write: 0, reasoning: 0, unpriced: 0, cost: 0.01, bucket: "day", series: [], agents: [], models: [], path: "~/.config/magpie/usage.jsonl" });
    if (url.pathname === "/api/gateway/trace") return json({ mine: false, seq: 0, routes: [], totals: { requests: 0, rerouted: 0, errors: 0 }, now: new Date().toISOString() });
    if (url.pathname === "/api/sessions") return json({ sessions: [], dirs: [] });
    if (url.pathname === "/api/sessions/stats") return json({ from: "", to: "", days: [], agents: {} });
    if (url.pathname === "/api/groups") return json({ groups: [], models: [] });
    if (url.pathname.startsWith("/api/")) return json({});
    const file = path.join(assets, url.pathname === "/" ? "index.html" : url.pathname);
    const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)];
    await route.fulfill({ body: await fs.readFile(file), contentType });
  };
}

const settle = async (page, seen, n) => {
  for (let i = 0; i < 100 && seen.length < n; i++) await page.waitForTimeout(20);
};

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  for (const width of [1180, 560]) {
    test(`${engine} ${width}px: a read the reader moved on from is cancelled, and a refresh waits for the one on its way`, async (t) => {
      const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
      t.after(() => browser.close());
      const errors = [], seen = [], held = [], failed = [];
      const page = await (await browser.newContext({ viewport: { width, height: 760 }, reducedMotion: "reduce" })).newPage();
      page.setDefaultTimeout(8000);
      page.on("pageerror", (e) => errors.push(e.message));
      page.on("requestfailed", (r) => { if (/\/api\/usage\/(requests|heatmap)\?/.test(r.url())) failed.push(r.url()); });
      await page.route("**/*", server("en", seen, held));
      await page.goto("http://magpie.test/");
      await page.locator('[data-view="usage"]').first().click();
      await page.locator(":is(#usageTab .opt, .nav-s):visible").nth(1).click();
      // the tab's opening reads, let go so the page is drawn
      await settle(page, seen, 2);
      held.splice(0).forEach((go) => go());
      await page.locator("#ledWrap .led tbody tr").first().waitFor();

      // the reader narrows the list three times, each read still on its way
      const box = page.locator("#ledQ");
      await inView(page, box);
      for (const q of ["a", "ab", "abc"]) {
        const n = seen.length;
        await box.fill(q);
        await settle(page, seen, n + 2);
      }
      const asked = (kind, q) => seen.filter((s) => s.kind === kind && s.q === q);
      for (const q of ["a", "ab", "abc"]) assert.equal(asked("page", q).length, 1, `one read of the page for "${q}"`);
      await page.waitForTimeout(150);
      for (const q of ["a", "ab"]) {
        for (const kind of ["page", "heat"]) {
          for (const s of asked(kind, q)) assert(failed.includes(s.url), `the ${kind} read for "${q}" was left running after "abc" was asked: ${JSON.stringify(failed)}`);
        }
      }
      for (const s of asked("page", "abc")) assert(!failed.includes(s.url), "the read wanted was cancelled");

      // Refresh pressed while that read is on its way: no second read (the
      // button turns until the read it waits for is done)
      const before = asked("page", "abc").length;
      await page.locator("#usageReload").click();
      await page.waitForTimeout(250);
      assert.equal(asked("page", "abc").length, before, "a refresh sent another read of the page beside the one on its way");

      // the answers come; the page shows the newest, and no error is said
      held.splice(0).forEach((go) => go());
      await page.waitForFunction(() => /asked-abc/.test(document.querySelector("#ledWrap")?.textContent || ""), null, { timeout: 4000 }).catch(async () => {
        throw new Error("the newest answer was not drawn: " + (await page.locator("#ledWrap").innerText()).slice(0, 400));
      });
      await page.waitForFunction(() => !document.querySelector("#usageReload").classList.contains("busy"), null, { timeout: 4000 });
      const text = await page.locator("body").innerText();
      assert(!/abort/i.test(text), "a cancelled read was said as an error");
      assert.equal(await page.locator("#ledWrap .led tbody tr", { hasText: "asked-ab" }).filter({ hasNotText: "asked-abc" }).count(), 0);
      // a refresh after the read is done reads afresh
      const n = seen.length;
      await page.locator("#usageReload").click();
      await settle(page, seen, n + 1);
      assert(asked("page", "abc").length > before, "a refresh with nothing on its way did not read");
      held.splice(0).forEach((go) => go());
      await page.waitForTimeout(150);
      assert.deepEqual(errors, []);
    });
  }
}
