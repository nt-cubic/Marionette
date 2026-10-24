// Run with Node's test runner and Playwright on the module path; see README.md.
// The Requests tab's chart can be columns or lines (molvqingtai on Discord):
// a pair of icons beside the metrics switches it, columns by default, and the
// choice is kept per viewer, so a reload finds lines still picked and drawn.
// Lines cover every metric: tokens, cost and requests stacked as the columns
// are (the top line the total), speeds each at their own with the point's
// own dashed. The tooltip and the ranking's emphasis work over them, the
// empty period says so, a click moves nothing, a kept value that isn't
// "line" or a read that throws is columns. Every language, light and dark,
// wide and narrow; no backend, the API is faked.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");

const assets = path.resolve(__dirname, "../assets");
const midnight = new Date();
midnight.setHours(0, 0, 0, 0);

// two models over a day by the hour, with timed replies for the speeds
const MODELS = [
  { id: "claude-sonnet-5", hours: [6, 20], per: { calls: 3, tokens: 900000, cost: 0.9, decode_out: 3000, decode_ms: 40000 } },
  { id: "gpt-6-luna", hours: [10, 23], per: { calls: 5, tokens: 300000, cost: 0.2, decode_out: 5000, decode_ms: 25000 } },
];
const toks = (n) => ({ input: n * 0.01, output: n * 0.005, cache_write: n * 0.05, cache_read: n * 0.935 });
const SERIES = Array.from({ length: 24 }, (_, h) => {
  const model = {}, p = { calls: 0, tokens: 0, cost: 0, decode_out: 0, decode_ms: 0, decode_n: 0 };
  for (const m of MODELS) {
    if (h < m.hours[0] || h > m.hours[1]) continue;
    model[m.id] = { ...m.per, decode_n: m.per.calls };
    for (const k in p) p[k] += model[m.id][k];
  }
  return { label: String(h).padStart(2, "0"), time: new Date(midnight.getTime() + h * 3600e3).toISOString(), errors: 0, ...p, ...toks(p.tokens), by: { model, provider: {}, agent: {} } };
});
const sum = (k, of = SERIES) => of.reduce((a, p) => a + (p[k] || 0), 0);
const shareOf = (m) => {
  const pts = SERIES.map((p) => p.by.model[m.id]).filter(Boolean);
  return { id: m.id, name: m.id, icon: "", calls: sum("calls", pts), errors: 0, ...toks(sum("tokens", pts)), cost: sum("cost", pts), decode_out: sum("decode_out", pts), decode_ms: sum("decode_ms", pts), decode_n: sum("decode_n", pts) };
};

function ledgerPage(empty) {
  // empty: requests listed, but none of them counted in the chart's hours
  const series = empty ? SERIES.map((p) => ({ ...p, calls: 0, tokens: 0, cost: 0, decode_out: 0, decode_ms: 0, decode_n: 0, ...toks(0), by: { model: {}, provider: {}, agent: {} } })) : SERIES;
  return {
    period: "today", rows: [], offset: 0, total: 1, bucket: "hour", series,
    calls: sum("calls", series), errors: 0, ...toks(sum("tokens", series)), reasoning: 0, cost: sum("cost", series), unpriced: 0,
    decode_out: sum("decode_out", series), decode_ms: sum("decode_ms", series), decode_n: sum("decode_n", series),
    by: { model: empty ? [] : MODELS.map(shareOf), provider: [], agent: [] }, agents: [], providers: [],
  };
}

function serve(lang, theme, empty) {
  return async (route) => {
    const url = new URL(route.request().url());
    const json = (data) => route.fulfill({ json: data });
    if (url.pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"${lang}",theme:"${theme}",web:false};` });
    if (url.pathname === "/wails/runtime.js") return route.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json({ agents: [], profiles: [], settings: { lang, theme }, fx: { rate: 7.2, at: new Date().toISOString() } });
    if (url.pathname === "/api/usage/requests") return json(ledgerPage(empty));
    if (url.pathname === "/api/usage/quotas") return json([]);
    if (url.pathname === "/api/usage") return json({ calls: 1, errors: 0, input: 1, output: 1, cache_read: 0, cache_write: 0, reasoning: 0, unpriced: 0, cost: 1, bucket: "day", series: [], agents: [], models: [] });
    if (url.pathname === "/api/sessions") return json({ sessions: [], dirs: [] });
    if (url.pathname === "/api/sessions/stats") return json({ from: "", to: "", days: [], agents: {} });
    if (url.pathname === "/api/groups") return json({ groups: [], models: [] });
    if (url.pathname === "/api/plugins") return json({ plugins: [] });
    if (url.pathname === "/api/providers") return json({ providers: [], gateway: { running: true, window: true } });
    if (url.pathname.startsWith("/api/")) return json({});
    const file = path.join(assets, url.pathname === "/" ? "index.html" : url.pathname);
    const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)];
    await route.fulfill({ body: await fs.readFile(file), contentType });
  };
}

const L = {
  en: { bar: "Bar chart", line: "Line chart", group: "Chart style" },
  zh: { bar: "柱状图", line: "折线图", group: "图表样式" },
  "zh-TW": { bar: "長條圖", line: "折線圖", group: "圖表樣式" },
  ja: { bar: "棒グラフ", line: "折れ線グラフ", group: "グラフの種類" },
  de: { bar: "Balkendiagramm", line: "Liniendiagramm", group: "Diagrammart" },
};

// what the chart is drawn as, and the switch's state
const drawn = (p) => p.evaluate(() => {
  const c = document.querySelector("#ledChart");
  const lines = [...c.querySelectorAll("g.ln")];
  return {
    cols: c.querySelectorAll("rect.col, line.stem").length,
    lines: lines.map((x) => x.dataset.k),
    dashed: c.querySelectorAll("g.ln .line.all").length,
    bands: c.querySelectorAll("g.ln .band").length,
    none: c.querySelector(".none")?.textContent || "",
    pressed: [...document.querySelectorAll("#ledStyle .opt")].map((b) => b.getAttribute("aria-pressed")),
    on: document.querySelector("#ledStyle .opt.on")?.dataset.style || "",
  };
});

// the top of a stacked line chart, at the hour every model has a part in:
// the highest line ends where the column's top did, the total's height
const topAt = (p, h) => p.evaluate((h) => {
  // the points a line passes through: each command's last x y
  const at = [...document.querySelectorAll("#ledChart g.ln .line")].map((x) => x.getAttribute("d").split(/[MLC]/).filter(Boolean).map((s) => s.split(" ").map(Number).slice(-2))[h]);
  return at.reduce((a, b) => (b[1] < a[1] ? b : a));
}, h);
// the top of the column whose middle is at x
const colTop = (p, x) => p.evaluate((x) => Math.min(...[...document.querySelectorAll("#ledChart rect.col")]
  .filter((r) => Math.abs(+r.getAttribute("x") + +r.getAttribute("width") / 2 - x) < 1).map((r) => +r.getAttribute("y"))), x);

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  test(engine + ": the Requests tab's chart switches between columns and lines, and keeps the choice", async (t) => {
    const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
    t.after(() => browser.close());
    const shots = process.env.ARTIFACT_DIR;
    if (shots) await fs.mkdir(shots, { recursive: true });

    const open = async (ctx, lang, theme, empty) => {
      const p = await ctx.newPage();
      const errors = [];
      p.setDefaultTimeout(5000);
      p.on("pageerror", (e) => errors.push(e.message));
      await p.route("**/*", serve(lang, theme, empty));
      await p.goto("http://magpie.test/");
      await p.locator('[data-view="usage"]').first().click();
      await p.locator(":is(#usageTab .opt, .nav-s):visible").nth(1).click();
      await p.locator("#ledStyle .opt").first().waitFor();
      await p.locator(empty ? "#ledChart .none" : "#ledChart svg").waitFor();
      return { p, errors };
    };

    for (const [lang, w] of Object.entries(L)) {
      for (const theme of ["light", "dark"]) {
        for (const width of [1180, 420]) {
          await t.test(`${lang} ${theme} ${width}`, async () => {
            // tall enough that the chart and its ranking are in sight without
            // a scroll, which the page's own scroll guard would take back
            const ctx = await browser.newContext({ viewport: { width, height: width < 600 ? 1300 : 900 }, reducedMotion: "reduce" });
            try {
              let { p, errors } = await open(ctx, lang, theme);
              // columns until the reader picks lines
              let d = await drawn(p);
              assert.equal(d.on, "bars", "columns by default");
              assert.deepEqual(d.pressed, ["true", "false"]);
              assert(d.cols > 0 && d.lines.length === 0, "columns drawn: " + JSON.stringify(d));
              // said in the reader's language, beside the metrics
              const names = await p.locator("#ledStyle .opt").evaluateAll((bs) => bs.map((b) => [b.title, b.getAttribute("aria-label")]));
              assert.deepEqual(names, [[w.bar, w.bar], [w.line, w.line]]);
              assert.equal(await p.locator("#ledStyle").getAttribute("aria-label"), w.group);
              const row = await p.evaluate(() => {
                const m = document.querySelector("#ledMetric").getBoundingClientRect(), s = document.querySelector("#ledStyle").getBoundingClientRect();
                const card = document.querySelector(".led-trend").getBoundingClientRect();
                return { gap: s.left - m.right, same: Math.abs(s.top - m.top) < 1 || s.top >= m.bottom, inside: s.left >= card.left && s.right <= card.right + 0.5, sideways: document.documentElement.scrollWidth > innerWidth + 1, border: getComputedStyle(document.querySelector("#ledStyle .opt")).borderLeftWidth };
              });
              assert(row.inside && row.same && !row.sideways, "the switch sits in the chart's header: " + JSON.stringify(row));
              assert.equal(row.border, "0px");
              if (shots && width === 1180) await p.locator(".led-trend").screenshot({ path: path.join(shots, `chart-bars-${engine}-${lang}-${theme}.png`) });

              // a click moves nothing
              await p.evaluate(() => scrollTo(0, 40));
              const y0 = await p.evaluate(() => scrollY);
              await p.locator("#ledStyle .opt").nth(1).click();
              await p.locator("#ledChart g.ln").first().waitFor();
              assert.equal(await p.evaluate(() => scrollY), y0, "the click scrolled the page");

              // reloaded, lines are still picked and drawn
              await p.reload();
              await p.locator('[data-view="usage"]').first().click();
              await p.locator(":is(#usageTab .opt, .nav-s):visible").nth(1).click();
              await p.locator("#ledChart svg").waitFor();
              d = await drawn(p);
              assert.equal(d.on, "line", "lines kept across a reload");
              assert.deepEqual(d.pressed, ["false", "true"]);
              assert.equal(d.cols, 0, "no columns left under the lines");
              assert.deepEqual(d.lines.sort(), MODELS.map((m) => m.id).sort(), "a line for each model");
              assert.equal(d.bands, 2, "stacked: a band under each");

              // stacked: the highest line at 15:00 is where the total's
              // column top was, for tokens, cost and requests
              for (let i = 0; i < 3; i++) {
                await p.locator("#ledMetric .opt").nth(i).click();
                await p.locator("#ledChart g.ln").first().waitFor();
                const top = await topAt(p, 15);
                await p.locator("#ledStyle .opt").nth(0).click();
                await p.locator("#ledChart rect.col").first().waitFor();
                const col = await colTop(p, top[0]);
                assert(Math.abs(top[1] - col) < 1, `metric ${i}: the top line at ${top[1]}, the column\'s top at ${col}`);
                await p.locator("#ledStyle .opt").nth(1).click();
                await p.locator("#ledChart g.ln").first().waitFor();
              }

              // speeds: each model's own line, over the point's own dashed
              await p.locator("#ledMetric .opt").nth(3).click();
              await p.locator("#ledChart g.ln").first().waitFor();
              d = await drawn(p);
              assert.equal(d.dashed, 1, "the point's own speed, dashed");
              assert.equal(d.bands, 0, "speeds aren't stacked");
              assert.equal(d.lines.length, 3);
              // a model's line breaks where it had no speed: luna starts at 10:00
              const luna = await p.locator('#ledChart g.ln[data-k="gpt-6-luna"] .line').getAttribute("d");
              assert.equal(luna.split("M").length - 1, 1, "one run: " + luna);
              assert.equal(luna.split(/[MLC]/).filter(Boolean).length, 14, "10:00 to 23:00");
              assert.equal(luna.split("C").length - 1, 13, "drawn as curves between its points: " + luna);
              // the curves bend between points but never past them: no line
              // reaches over its highest point or under its lowest
              const past = await p.evaluate(() => [...document.querySelectorAll("#ledChart g.ln .line")].map((x) => {
                const ys = x.getAttribute("d").split(/[MLC]/).filter(Boolean).map((s) => +s.split(" ").at(-1));
                const b = x.getBBox();
                return [b.y - Math.min(...ys), Math.max(...ys) - (b.y + b.height)];
              }).filter(([a, b]) => a < -0.5 || b < -0.5));
              assert.deepEqual(past, [], "a curve overshoots its points");

              // the tooltip over the lines, and the ranking's emphasis
              await p.locator("#ledMetric .opt").nth(0).click();
              await p.locator("#ledChart g.ln").first().waitFor();
              await p.evaluate(() => scrollTo(0, 0));
              const box = await p.locator("#ledChart svg").boundingBox();
              await p.mouse.move(box.x + box.width * 0.62, box.y + box.height / 2);
              await p.locator("#ledChart .tip").waitFor();
              const tip = await p.locator("#ledChart .tip").textContent();
              assert(tip.includes("claude-sonnet-5") && tip.includes("gpt-6-luna"), "tooltip: " + tip);
              await p.locator("#ledRank .rk").first().hover();
              const faded = await p.evaluate(() => [...document.querySelectorAll("#ledChart g.ln")].map((x) => [x.dataset.k, x.style.opacity]));
              assert.equal(faded.filter(([, o]) => o === "0.22" || o === ".22").length, 1, "the other line fades: " + JSON.stringify(faded));
              await p.mouse.move(1, 1);
              if (shots && width === 1180) await p.locator(".led-trend").screenshot({ path: path.join(shots, `chart-line-${engine}-${lang}-${theme}.png`) });
              assert.deepEqual(errors, []);
              await p.close();

              // nothing in the period: the empty line, no lines, no error
              ({ p, errors } = await open(ctx, lang, theme, true));
              d = await drawn(p);
              assert.equal(d.on, "line");
              assert.equal(d.lines.length, 0);
              assert(d.none, "the empty period is said");
              assert.deepEqual(errors, []);
              await p.close();
            } finally {
              await ctx.close();
            }
          });
        }
      }
    }

    await t.test("a kept value that isn't line, or a read that throws, is columns", async () => {
      for (const init of [
        () => localStorage.setItem("magpie.ledChart", "pie"),
        () => { const get = Storage.prototype.getItem; Storage.prototype.getItem = function (k) { if (k === "magpie.ledChart") throw new Error("denied"); return get.call(this, k); }; },
      ]) {
        const ctx = await browser.newContext({ viewport: { width: 1180, height: 760 }, reducedMotion: "reduce" });
        await ctx.addInitScript(init);
        const { p, errors } = await open(ctx, "en", "light");
        const d = await drawn(p);
        assert.equal(d.on, "bars");
        assert(d.cols > 0 && d.lines.length === 0);
        assert.deepEqual(errors, []);
        await ctx.close();
      }
    });
  });
}
