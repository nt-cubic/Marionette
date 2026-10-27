// Run with Node's test runner and Playwright on the module path; see README.md.
// A request being masked (Settings → Privacy) says so on the Routing page
// (fremontdango on Discord: a long Claude Code request sat as "routing…"
// while it was masked): its row says masking while the gateway's route for
// it has the "redact" phase, and routing once that is over and no try is
// out yet. Once answered, the time masking took is the first part of where
// its first try's time went, magpie's. Both engines, a narrow window, every
// language. The API is faked.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");

const assets = path.resolve(__dirname, "../assets");
const now = new Date();
const at = (s) => new Date(now.getTime() - s * 1e3).toISOString();
const key = { id: "deepseek", provider: "deepseek", name: "DeepSeek", kind: "provider", model: "deepseek-flash" };
const net = { own: 2.4, asks: 1, send: 0.6, wait: 900, head: 950 };
const routes = [
  // held, being masked: the route that shows it while it isn't routed
  { id: 203, seq: 203, time: at(20), agent: "claude", model: "deepseek/deepseek-flash", provider: "magpie", phase: "redact", order: [], tries: [], done: false, status: 0 },
  // held, masked, not yet routed
  { id: 202, seq: 202, time: at(40), agent: "claude", model: "deepseek/deepseek-chat", provider: "magpie", order: [], tries: [], done: false, status: 0 },
  // answered, after 2.3 s of masking
  { id: 201, seq: 201, time: at(90), agent: "claude", model: "deepseek/deepseek-flash", provider: "deepseek", order: [key],
    tries: [{ id: key.id, model: "deepseek-flash", start: at(87), done: true, status: 200, ms: 3000, ttft: 1200, net }],
    done: true, status: 200, ms: 5400, ttft: 3500, own: 2310, masking: 2300, sent: 2320, tokens: 900, served: "deepseek-flash" },
];

function serve(lang) {
  const state = { agents: [{ id: "claude", name: "Claude Code", path: "/test/settings.json", fields: [] }], profiles: [], settings: { lang, theme: "light" } };
  return async (route) => {
    const url = new URL(route.request().url());
    const json = (data) => route.fulfill({ json: data });
    if (url.pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"${lang}",theme:"light",web:true};` });
    if (url.pathname === "/wails/runtime.js") return route.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json(state);
    if (url.pathname === "/api/gateway/trace") {
      if (url.searchParams.get("wait")) await new Promise((r) => setTimeout(r, 20e3));
      return json({ mine: true, now: now.toISOString(), seq: 203, totals: { requests: 3, rerouted: 0, errors: 0 }, routes });
    }
    if (url.pathname === "/api/gateway/history") return json({ cut: false, days: [], routes: [] });
    if (url.pathname === "/api/groups") return json({ groups: [], models: [] });
    if (url.pathname === "/api/providers") return json({ providers: [], excluded: [], gateway: { running: true, window: true } });
    if (url.pathname === "/api/plugins") return json({ plugins: [] });
    if (url.pathname === "/api/usage/requests") return json({ period: url.searchParams.get("period"), rows: [], offset: 0, total: 0, calls: 0, errors: 0, input: 0, output: 0, cache_read: 0, cache_write: 0, reasoning: 0, cost: 0, unpriced: 0, agents: [] });
    if (url.pathname === "/api/usage/quotas") return json([]);
    if (url.pathname === "/api/usage") return json({ calls: 0, errors: 0, input: 0, output: 0, cache_read: 0, cache_write: 0, reasoning: 0, unpriced: 0, cost: 0, bucket: "day", series: [], agents: [], models: [], path: "~/.config/magpie/usage.jsonl" });
    if (url.pathname === "/api/sessions") return json({ sessions: [], dirs: [] });
    if (url.pathname === "/api/sessions/stats") return json({ from: "", to: "", days: [], agents: {} });
    if (url.pathname.startsWith("/api/")) return json({});
    const file = path.join(assets, url.pathname === "/" ? "index.html" : url.pathname);
    const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)];
    await route.fulfill({ body: await fs.readFile(file), contentType });
  };
}

const want = {
  en: { masking: "masking…", routing: "routing…", part: "Masking" },
  zh: { masking: "脱敏中…", routing: "路由中…", part: "脱敏" },
  "zh-TW": { masking: "脫敏中…", routing: "路由中…", part: "脫敏" },
  ja: { masking: "マスク中…", routing: "ルーティング中…", part: "マスク" },
  de: { masking: "maskiert…", routing: "leitet weiter…", part: "Maskierung" },
};

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  for (const lang of Object.keys(want)) {
    test(`${engine} ${lang}: a request being masked says so, and its time`, async (t) => {
      const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
      const context = await browser.newContext({ viewport: { width: 440, height: 900 }, reducedMotion: "reduce" });
      const page = await context.newPage();
      page.setDefaultTimeout(5000);
      const errors = [];
      page.on("pageerror", (e) => errors.push(e.message));
      await page.route("**/*", serve(lang));
      t.after(async () => {
        if (process.env.ARTIFACT_DIR) {
          await fs.mkdir(process.env.ARTIFACT_DIR, { recursive: true });
          await page.screenshot({ path: path.join(process.env.ARTIFACT_DIR, `${engine}-${lang}-masking.png`), fullPage: true });
        }
        await browser.close();
      });
      await page.goto("http://magpie.test/?view=routing");
      const rows = page.locator(".rt-req");
      await rows.nth(routes.length - 1).waitFor({ timeout: 15000 });
      const said = await rows.evaluateAll((bs) => bs.map((b) => b.textContent));
      assert.equal(said.length, 3);
      // newest first: the one masked, the one waiting for its route
      assert(said[0].includes(want[lang].masking) && !said[0].includes(want[lang].routing), said[0]);
      assert(said[1].includes(want[lang].routing) && !said[1].includes(want[lang].masking), said[1]);
      assert(!said[2].includes(want[lang].masking), said[2]);

      // the answered one: masking is magpie's first part of its try's time
      await rows.nth(2).click();
      const box = page.locator(".rt-steps li.time .rt-time");
      await box.waitFor();
      const parts = await box.evaluate((b) => [...b.querySelectorAll(".rt-time-parts li")].map((li) => [li.className, li.querySelector(".k").textContent, li.querySelector(".v").textContent]));
      assert.equal(parts[0][0], "magpie");
      assert.equal(parts[0][1], want[lang].part);
      assert.match(parts[0][2], /2[.,]3/);
      assert.equal(parts.filter((p) => p[1] === want[lang].part).length, 1);
      assert.deepEqual(errors, []);
    });
  }
}
