// Run with Node's test runner and Playwright on the module path; see README.md.
// The calls an app reported itself, through POST /v1/magpie/usage/requests
// (#1874), are a fourth source on the Requests tab, beside the gateway's and
// the session files': a cell of their own, there only once there are some,
// that lists only them when picked (via=reported, which a link can carry
// too), counted in the total; and each such row says it was reported. In
// every language, Chromium and WebKit, wide and narrow, light and dark; no
// backend, the API is faked.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");
const { reach } = require("./reader.cjs");

const assets = path.resolve(__dirname, "../assets");
const tot = (calls, tokens, cost) => ({ calls, errors: 0, input: tokens * 0.2, output: tokens * 0.1, cache_write: 0, cache_read: tokens * 0.7, cost });
const ROW = { t: new Date().toISOString(), agent: "claude", agentName: "Claude Code", icon: "claudecode-color", provider: "anthropic", providerName: "Claude", model: "claude-sonnet-5", in: 2, out: 600, cache_write: 0, cache_read: 3900, ms: 2380, status: 200, cost: 0.08, priced: true };
const REPORTED = { ...ROW, agent: "my-app", agentName: "my-app", icon: "", rid: "r-1", source: "reported", cost: 0.25 };

function page(q, reported) {
  const via = q.get("via") || "";
  const all = [ROW, ...(reported ? [REPORTED] : [])];
  const rows = via === "reported" ? all.filter((r) => r.source === "reported") : via === "through" ? all.filter((r) => !r.source) : via === "direct" ? [] : all;
  return {
    period: "today", rows, offset: 0, total: rows.length, ...tot(rows.length, 4500 * rows.length, 0.33), reasoning: 0, unpriced: 0,
    bucket: "hour", series: [], by: { provider: [], agent: [], model: [] },
    agents: [{ id: "claude", name: "Claude Code", icon: "claudecode-color" }], providers: [{ id: "anthropic", name: "Claude", icon: "claudecode-color" }],
    through: tot(8, 4e6, 4), direct: tot(12, 6e6, 6), ...(reported ? { reported: tot(5, 1e6, 1.25) } : {}),
  };
}

function server(lang, theme, reported, asked) {
  return async (route) => {
    const req = route.request(), url = new URL(req.url());
    const json = (data) => route.fulfill({ json: data });
    if (url.pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"${lang}",theme:"${theme}",web:false};` });
    if (url.pathname === "/wails/runtime.js") return route.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json({ agents: [], profiles: [], settings: { lang, theme }, fx: { rate: 7.2, at: new Date().toISOString() } });
    if (url.pathname === "/api/usage/requests") { asked.push(url.searchParams); return json(page(url.searchParams, reported)); }
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

// the four cells, and the row's badge, as each language says them
const L = {
  en: { cells: ["all requests", "through magpie", "not through magpie", "reported by apps"], badge: "Reported", reqs: "requests" },
  zh: { cells: ["全部请求", "经过 magpie", "不经过 magpie", "由应用上报"], badge: "上报" },
  "zh-TW": { cells: ["全部請求", "經過 magpie", "不經過 magpie", "由應用程式回報"], badge: "回報" },
  ja: { cells: ["すべてのリクエスト", "magpie 経由", "magpie を経由しない", "アプリからの報告"], badge: "報告" },
  de: { cells: ["alle Anfragen", "über magpie", "nicht über magpie", "von Apps gemeldet"], badge: "Gemeldet" },
};

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  test(engine + ": reported calls are a source of their own", async (t) => {
    assert(["chromium", "webkit"].includes(engine), "BROWSER must be chromium or webkit");
    const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
    t.after(() => browser.close());
    const shots = process.env.ARTIFACT_DIR;
    if (shots) await fs.mkdir(shots, { recursive: true });

    const open = async (lang, { theme = "light", width = 1180, reported = true, query = "" } = {}) => {
      const errors = [], asked = [];
      const context = await browser.newContext({ viewport: { width, height: 760 }, reducedMotion: "reduce" });
      t.after(() => context.close());
      const p = await context.newPage();
      p.setDefaultTimeout(5000);
      p.on("pageerror", (e) => errors.push(e.message));
      await p.route("**/*", server(lang, theme, reported, asked));
      await p.goto("http://magpie.test/" + query);
      if (!query) {
        await p.locator('[data-view="usage"]').first().click();
        await p.locator(":is(#usageTab .opt, .nav-s):visible").nth(1).click();
      }
      await p.locator("#ledVia .blk").first().waitFor();
      return { p, errors, asked };
    };
    const lastAsked = async (asked, want) => {
      for (let i = 0; i < 60 && !(asked.length && want(asked.at(-1))); i++) await new Promise((r) => setTimeout(r, 40));
      assert(asked.length && want(asked.at(-1)), "asked: " + (asked.at(-1) || ""));
    };
    const labels = (p) => p.locator("#ledVia .blk .k").allTextContents();
    const fits = (p) => p.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1 &&
      [...document.querySelectorAll("#ledVia .blk")].every((b) => { const r = b.getBoundingClientRect(); return r.left >= -1 && r.right <= innerWidth + 1 && b.scrollWidth <= b.clientWidth + 1; }));

    for (const lang of Object.keys(L)) {
      const w = L[lang];
      for (const width of [1180, 420]) {
        await t.test(`${lang} at ${width}`, async () => {
          const { p, errors, asked } = await open(lang, { width, theme: width < 600 ? "dark" : "light" });
          assert.deepEqual(await labels(p), w.cells);
          // the total is the three sources together
          const counts = await p.locator("#ledVia .blk .v").evaluateAll((vs) => vs.map((v) => parseInt(v.firstChild.textContent.replace(/\D/g, ""), 10)));
          assert.deepEqual(counts, [25, 8, 12, 5]);
          // in a row at a wide window, stacked at a narrow one, and either way inside it
          const tops = await p.locator("#ledVia .blk").evaluateAll((bs) => bs.map((b) => Math.round(b.getBoundingClientRect().top)));
          if (width > 600) assert.equal(new Set(tops).size, 1, "four in a row: " + tops);
          else assert.equal(new Set(tops).size, 4, "stacked: " + tops);
          assert(await fits(p), "the cells fit and the page doesn't scroll sideways");
          // no native select, and no coloured stripe down a cell's or the badge's side
          assert.equal(await p.locator("#ledVia select, #view-usage select").count(), 0);
          const sides = await p.locator("#ledVia .blk, .led .src.reported").evaluateAll((bs) => bs.map((b) => parseFloat(getComputedStyle(b).borderLeftWidth)));
          assert(sides.every((s) => s <= 1), "no left accent: " + sides);

          // the reported row says so
          const badge = p.locator(".led .src.reported");
          await badge.first().waitFor();
          assert.equal(await badge.count(), 1);
          assert.equal(await badge.textContent(), w.badge);
          assert((await badge.getAttribute("title")).includes("POST /v1/magpie/usage/requests"));
          if (shots) await p.screenshot({ path: path.join(shots, `reported-${engine}-${lang}-${width}.png`) });

          // picking it lists only them, and the total is the way back
          // stacked at a narrow window, the fourth is under the window's
          // foot: the reader scrolls down to it
          await reach(p, p.locator("#ledVia .blk").nth(3));
          await p.locator("#ledVia .blk").nth(3).click();
          await lastAsked(asked, (q) => q.get("via") === "reported");
          await p.locator("#ledVia .blk:nth-child(4).on[aria-pressed=true]").waitFor();
          assert.equal(await p.locator("#ledVia .blk.on").count(), 1);
          // the bar under the picked cell is straight and stops short of its
          // rounded corners (the owner: "在拐角处没有紫色 border")
          const barClear = (n) => p.locator("#ledVia .blk").nth(n).evaluate((b) => {
            const c = getComputedStyle(b), bar = getComputedStyle(b, "::after");
            return c.boxShadow === "none" && bar.content !== "none" && parseFloat(bar.height) === 2 &&
              parseFloat(bar.left) >= parseFloat(c.borderBottomLeftRadius) && parseFloat(bar.right) >= parseFloat(c.borderBottomRightRadius);
          });
          assert(await barClear(3), "the last cell's bar clears its corner");
          assert.deepEqual(await labels(p), w.cells, "the cells stay while one is picked");
          await reach(p, p.locator("#ledVia .blk").nth(0));
          await p.locator("#ledVia .blk").nth(0).click();
          await lastAsked(asked, (q) => !q.has("via"));
          await p.locator("#ledVia .blk:nth-child(1).on").waitFor();
          assert(await barClear(0), "and the first's");
          if (shots) await p.screenshot({ clip: await p.locator("#ledVia").boundingBox(), path: path.join(shots, `via-picked-${engine}-${lang}-${width}.png`) });
          assert.deepEqual(errors, []);
        });
      }
    }

    await t.test("none reported: three cells, as before", async () => {
      const { p, errors } = await open("en", { reported: false });
      assert.deepEqual(await labels(p), L.en.cells.slice(0, 3));
      assert.equal(await p.locator("#ledVia").evaluate((v) => getComputedStyle(v).gridTemplateColumns.split(" ").length), 3);
      assert.equal(await p.locator(".led .src.reported").count(), 0);
      assert.deepEqual(errors, []);
    });

    await t.test("a link with via=reported opens on them", async () => {
      const { p, errors, asked } = await open("en", { query: "?view=usage&tab=requests&via=reported" });
      await lastAsked(asked, (q) => q.get("via") === "reported");
      await p.locator("#ledVia .blk:nth-child(4).on[aria-pressed=true]").waitFor();
      assert.deepEqual(errors, []);
    });
  });
}
