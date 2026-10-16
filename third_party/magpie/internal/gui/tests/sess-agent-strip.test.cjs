// Run with Node's test runner and Playwright on the module path; see README.md.
// #929 (yetone/magpie): with a dozen agents in range, Usage → Sessions' agent
// filter grew wider than the window, so the whole page scrolled sideways and
// the last agent's filter was only reachable by dragging the page itself. A
// strip too wide for the window scrolls in itself — the regions strip and
// Connect's five APIs already do — so this one does too, and its options keep
// their own width. English and Chinese, Chromium and WebKit.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");

const assets = path.resolve(__dirname, "../assets");
const at = (min) => new Date(Date.now() - min * 60e3).toISOString();
// names long enough that eleven agents make a strip wider than an 866px window,
// and short enough to fit a wide one, whose pages keep to 1200px (面条 on Discord),
// with room to spare: a strip that fits by a fraction of a pixel fails on a
// glyph's rounding
const NAMES = ["Trae CN Agent", "Kilo Code Agent", "Pi", "WorkBuddy",
  "DeepSeek Harness Agent", "Claude Code", "Codex", "omp", "OpenCode Agent", "Qoder Code", "ZCode Agent"];
const NARROW = { width: 866, height: 800 };
const WIDE = { width: 1600, height: 800 };

const sess = (agent, name, k) => ({
  agent, id: `${agent}-${k}`, cwd: `/work/${agent}/part${k}`, title: `task ${k} on ${name}`, start: at(20), last: at(10),
  models: [{ model: k ? "m-two" : "m-one", input: 1000, output: 100, cache_read: 500, cache_write: 0, cost: 0.5, priced: true }],
  input: 1000, output: 100, cache_read: 500, cache_write: 0, cost: 0.5, unpriced: 0,
  resume: "x", path: `~/.${agent}/x/${k}.jsonl`, name, icon: "generic",
});

function serve(lang, names) {
  const sessions = names.flatMap((name, i) => [0, 1].map((k) => sess(`ag${i}`, name, k)));
  const agents = Object.fromEntries(names.map((name, i) => [`ag${i}`, name]));
  const state = { agents: [], profiles: [], settings: { lang, theme: "light" } };
  return async (r) => {
    const url = new URL(r.request().url());
    const json = (data) => r.fulfill({ json: data });
    if (url.pathname === "/boot.js") return r.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"${lang}",theme:"light",web:false};` });
    if (url.pathname === "/wails/runtime.js") return r.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json(state);
    if (url.pathname === "/api/sessions") return json({ sessions, terminal: true, dirs: [] });
    if (url.pathname === "/api/sessions/stats") {
      const today = new Date().toISOString().slice(0, 10);
      const usage = sessions.map((s) => ({ agent: s.agent, cwd: s.cwd, model: s.models[0].model, input: s.input, output: s.output, cache_read: s.cache_read, cache_write: 0, cost: s.cost, priced: true }));
      return json({ from: today, to: today, days: [{ date: today, usage, active: [] }], agents });
    }
    if (url.pathname === "/api/sessions/overview") return json({ count: sessions.length, median: 5000, p90: 15000, days: [1], top: { tokens: [], cost: [], active: [] } });
    if (url.pathname === "/api/sessions/manage") {
      const ids = Object.keys(agents);
      const want = url.searchParams.get("agent");
      const agent = ids.includes(want) ? want : ids[0];
      return json({
        agents: ids.map((id) => ({ agent: id, count: 2, deletable: true, name: agents[id], icon: "generic" })),
        agent, sessions: sessions.filter((s) => s.agent === agent), terminal: true, trash: [], trashDir: "~/magpie/trash/sessions",
      });
    }
    if (url.pathname === "/api/groups") return json({ groups: [] });
    if (url.pathname === "/api/usage/quotas") return json([]);
    if (url.pathname.startsWith("/api/")) return json({});
    const file = path.join(assets, url.pathname === "/" ? "index.html" : url.pathname);
    const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)];
    const body = await fs.readFile(file).catch(() => null);
    await (body ? r.fulfill({ body, contentType }) : r.fulfill({ status: 404, body: "" }));
  };
}

// a box's own sideways overflow, and whether it scrolls sideways at all
const box = (page, sel) => page.locator(sel).evaluate((el) => ({
  scroll: el.scrollWidth, client: el.clientWidth, overflow: getComputedStyle(el).overflowX,
}));
const inside = (page, sel, outer = "#view-usage") => page.locator(sel).evaluate((el, o) => {
  const o1 = document.querySelector(o).getBoundingClientRect();
  const r = el.getBoundingClientRect();
  return r.left >= o1.left - 1 && r.right <= o1.right + 1;
}, outer);

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  for (const lang of ["en", "zh"]) {
    test(`${engine}: ${lang}: Usage → Sessions' agent strip scrolls in itself, not the page`, async (t) => {
      const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
      const page = await (await browser.newContext({ viewport: NARROW, reducedMotion: "reduce" })).newPage();
      t.after(() => browser.close());
      page.setDefaultTimeout(8000);
      const errors = [];
      page.on("pageerror", (e) => errors.push(e.message));
      await page.route("**/*", serve(lang, NAMES));
      await page.addInitScript(() => {
        localStorage.setItem("magpie.usageTab", "sessions");
        localStorage.setItem("magpie.sessRange", "all");
        // the Usage page's own 5 s refresh would rebuild the strip between a
        // measurement and the click it measures; one read is enough here
        localStorage.setItem("magpie.usageEvery", "0");
      });
      await page.goto("http://magpie.test/?view=usage");
      await page.waitForFunction(() => document.querySelectorAll("#sessAgent .opt").length === 12);

      // #929: eleven agents in range made the strip wider than the window, and
      // the page scrolled sideways instead of the strip
      const view = await box(page, "#view-usage");
      assert.ok(view.scroll <= view.client + 1, `the Usage page scrolls sideways by ${view.scroll - view.client}px with ${NAMES.length} agents in range`);
      const seg = await box(page, "#sessAgent");
      assert.equal(seg.overflow, "auto", "the agent strip doesn't scroll sideways in itself");
      assert.ok(seg.scroll > seg.client + 1, `the strip fits the window at ${seg.client}px, so nothing is being fixed`);

      // every option is reachable: the last one, then the first, by scrolling
      // the strip — none of them is cut off past the strip's own edge
      const opts = page.locator("#sessAgent .opt");
      // a real click where a reader's would land: scroll the option into the
      // strip, then press the middle of it rather than trust a stale handle
      const press = async (which) => {
        for (let tries = 3; tries; tries--) {
          const p = await page.locator("#sessAgent").evaluate((strip, which) => {
            const all = strip.querySelectorAll(".opt");
            const el = which === "first" ? all[0] : all[all.length - 1];
            el.scrollIntoView({ block: "nearest", inline: "nearest" });
            const r = el.getBoundingClientRect();
            const s = strip.getBoundingClientRect();
            return { x: r.left + r.width / 2, y: r.top + r.height / 2, text: el.textContent, cut: r.left < s.left - 1 || r.right > s.right + 1 };
          }, which);
          assert.equal(p.cut, false, `the ${which} agent's filter sits outside the strip and is cut off`);
          await page.mouse.click(p.x, p.y);
          try {
            await page.waitForFunction((text) => document.querySelector("#sessAgent .opt.on")?.textContent === text, p.text, { polling: 100, timeout: 2000 });
            return p.text;
          } catch (e) {
            if (!tries) throw e; // WebKit drops the odd press; a reader would press again
          }
        }
      };
      const first = (await opts.first().innerText()).trim();
      assert.equal(await press("last"), NAMES[NAMES.length - 1], "clicking the last agent's filter didn't choose it");
      assert.equal(await press("first"), first, "clicking the first agent's filter didn't choose it again");

      // the picks beside the strip stay visible and inside the window
      for (const id of ["#sessModel", "#sessFolder"]) {
        assert(await page.locator(id).isVisible(), `${id} is not visible beside the strip`);
        assert(await inside(page, id), `${id} sits outside the window`);
      }
      await page.locator("#sessModel").click();
      await page.locator(".pop.proto-menu.sess-menu").waitFor();
      await page.keyboard.press("Escape");
      await page.locator("#sessFolder").click();
      await page.locator(".pop.proto-menu.sess-menu").waitFor();
      await page.keyboard.press("Escape");
      // no click on the strip left an accent behind; the picks beside it keep
      // their own box border on all four sides
      const borders = await page.evaluate(() => [...document.querySelectorAll("#sessAgent .opt")]
        .map((e) => getComputedStyle(e).borderLeftWidth).filter((w) => w && parseFloat(w) > 0));
      assert.deepEqual(borders, [], "no left-border accent");

      // a window wider than the strip: nothing scrolls sideways, and the
      // options keep their own widths instead of being stretched to fill it
      await page.setViewportSize(WIDE);
      await page.waitForTimeout(120);
      const wideView = await box(page, "#view-usage");
      assert.ok(wideView.scroll <= wideView.client + 1, `a window wider than the strip still scrolls the page sideways by ${wideView.scroll - wideView.client}px`);
      const fit = await box(page, "#sessAgent");
      assert.ok(fit.scroll <= fit.client + 1, `the strip scrolls sideways in a window that fits it (${fit.scroll} in ${fit.client})`);
      const widths = await opts.evaluateAll((els) => els.map((el) => ({ w: el.getBoundingClientRect().width, need: el.scrollWidth })));
      assert.ok(widths.every((o) => o.need <= o.w + 1), "an option was squeezed narrower than its own text");
      assert.ok(new Set(widths.map((o) => Math.round(o.w))).size > 1, "every option is the same width: they were stretched, not laid out by their text");
      assert.deepEqual(errors, []);

      // one agent: the strip stays hidden, as it was before (#929 changed
      // nothing about when the filter shows)
      await page.unroute("**/*");
      await page.route("**/*", serve(lang, NAMES.slice(0, 1)));
      await page.reload();
      await page.waitForFunction(() => document.querySelectorAll("#sessAgent .opt").length > 0);
      assert.equal(await page.locator("#sessAgent").isVisible(), false, "one agent in range should leave the agent filter hidden");
      const one = await box(page, "#view-usage");
      assert.ok(one.scroll <= one.client + 1, "one agent in range still scrolls the page sideways");

      // the other strips are untouched: the Usage page's own tab strip and
      // range strip still don't scroll in themselves, and the regions strip
      // and Connect's still do
      await page.unroute("**/*");
      await page.route("**/*", serve(lang, NAMES));
      await page.goto("http://magpie.test/?view=usage");
      await page.waitForFunction(() => document.querySelectorAll("#sessAgent .opt").length === 12);
      assert.equal((await box(page, "#period")).overflow, "visible", "the Usage page's range strip started scrolling in itself");
      assert.equal((await box(page, "#usageTab")).overflow, "visible", "the Usage page's tab strip started scrolling in itself");
      const others = await page.evaluate(() => {
        const out = {};
        const probe = (classes, host) => {
          const wrap = document.createElement("div");
          if (host) wrap.className = host;
          const el = document.createElement("div");
          el.className = classes;
          el.append(Object.assign(document.createElement("button"), { className: "opt", textContent: "x" }));
          wrap.append(el);
          document.body.append(wrap);
          out[host ? `${classes} in ${host}` : classes] = { overflow: getComputedStyle(el).overflowX, flex: getComputedStyle(el.firstChild).flex };
          wrap.remove();
        };
        probe("segs regions");
        probe("segs", "connect");
        return out;
      });
      assert.equal(others["segs regions"].overflow, "auto", "the regions strip stopped scrolling in itself");
      assert.equal(others["segs regions"].flex, "0 0 auto", "the regions strip's options lost their own width");
      assert.equal(others["segs in connect"].overflow, "auto", "Connect's API strip stopped scrolling in itself");
      assert.equal(others["segs in connect"].flex, "0 0 auto", "Connect's API options lost their own width");
      assert.deepEqual(errors, []);
    });
  }
}