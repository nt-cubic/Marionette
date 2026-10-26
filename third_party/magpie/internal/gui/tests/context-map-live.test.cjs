// Run with Node's test runner and Playwright on the module path; see README.md.
// The context map as a live request grows (after flohoeller's context map
// on X): the counts count on to their new values as the tiles glide, a
// tile pointed at keeps its tip, told again with what it holds now, a
// tool's gathered results say how many, and the small files gathered say
// the folder they share. With motion reduced the counts are simply there.
// In every language, at a desk and a phone's width.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");

const assets = path.resolve(__dirname, "../assets");
const now = Date.now();
const ago = (m) => new Date(now - m * 60000).toISOString();

const tiny = ["auth.ts", "billing.ts", "refund.ts", "users.ts", "orders.ts", "carts.ts"]
  .map((f) => ({ name: "/work/app/src/api/" + f, tag: "read", tokens: 60 }));
const prompt = (grow) => ({
  window: 60000, tokens: 40000 + grow, counted: !grow,
  parts: [
    { kind: "system", tokens: 6000, items: [{ name: "prompt", tokens: 6000 }] },
    { kind: "tools", tokens: 8000, items: [{ name: "exec_command", tokens: 8000 }] },
    { kind: "files", tokens: 14000 + grow, items: [{ name: "/work/app/payment.ts", tag: "read", tokens: 9000 + grow }, { name: "/work/app/checkout.spec.ts", tag: "read", tokens: 4640 }, ...tiny] },
    { kind: "results", tokens: 9000, items: [{ name: "exec_command", tag: "result", n: 20, tokens: 9000 }] },
    { kind: "chat", tokens: 3000, items: [{ name: "turn", tag: "turns", n: 6, tokens: 3000 }] },
  ],
});
const context = (days) => ({
  days,
  agents: [{ agent: "codex", requests: 1, sessions: 1, errors: 0, calls: 1, median: 40000, p90: 40000, baseline: 20000, window: 60000, cache: 0.5, growth: 0, peakFill: 0.2, compacts: 0,
    shares: { system: 0.15, tools: 0.2, files: 0.35, results: 0.22, chat: 0.08 }, mcp: 0, score: 80,
    scores: [{ key: "cache", points: 20, most: 35 }], tags: [], models: ["gpt-5.6"], latestId: 19, latestTime: ago(4) }],
  sessions: [{ agent: "codex", key: "01a117b1-aecd-76e2-9b2f-3c1d", title: "Fix checkout", model: "gpt-5.6", first: ago(12), last: ago(4), requests: 1, peak: 40000, window: 60000,
    latest: prompt(0), latestId: 19, points: [{ id: 19, time: ago(4), tokens: 40000 }] }],
});

function serve(lang) {
  const state = { agents: [{ id: "codex", name: "Codex", path: "/test/codex", fields: [] }], profiles: [], settings: { lang, theme: "light" } };
  return async (r) => {
    const url = new URL(r.request().url());
    const json = (data) => r.fulfill({ json: data });
    if (url.pathname === "/boot.js") return r.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"${lang}",theme:"light",web:true};` });
    if (url.pathname === "/wails/runtime.js") return r.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json(state);
    if (url.pathname === "/api/context") return json(context(+url.searchParams.get("days")));
    if (url.pathname === "/api/gateway/trace") {
      if (url.searchParams.get("wait")) return new Promise(() => {});
      return json({ mine: true, now: new Date().toISOString(), seq: 0, totals: { requests: 0, rerouted: 0, errors: 0 }, routes: [] });
    }
    if (url.pathname === "/api/providers") return json({ providers: [], presets: [], excluded: [], gateway: { running: true, window: true, url: "http://127.0.0.1:3999" } });
    if (url.pathname === "/api/usage/quotas") return json([]);
    if (url.pathname === "/api/groups") return json({ groups: [], models: [] });
    if (url.pathname === "/api/plugins") return json({ plugins: [] });
    if (url.pathname.startsWith("/api/")) return json({});
    const file = path.join(assets, url.pathname === "/" ? "index.html" : url.pathname);
    const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)];
    await r.fulfill({ body: await fs.readFile(file).catch(() => ""), contentType, status: 200 });
  };
}

const words = {
  en: { files: "6 files" },
  zh: { files: "6 个文件" },
  "zh-TW": { files: "6 個檔案" },
  ja: { files: "6 ファイル" },
  de: { files: "6 Dateien" },
};

async function open(browser, lang, width, reduced) {
  const page = await (await browser.newContext({ viewport: { width, height: 1600 }, reducedMotion: reduced ? "reduce" : "no-preference" })).newPage();
  page.setDefaultTimeout(5000);
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.route("**/*", serve(lang));
  await page.addInitScript(() => { try { localStorage.clear(); localStorage.setItem("magpie.usageTab", "context"); } catch {} });
  await page.goto("http://magpie.test/?view=usage");
  const row = page.locator(".ctx-sess-row", { hasText: "Fix checkout" });
  await row.waitFor();
  await row.click();
  const card = page.locator(".ctx-sess.open .ctx-card");
  await card.locator(".ctx-map .ctx-tile.free").waitFor();
  // the tiles in and the card open before anything is pointed at
  await page.waitForFunction(() => !document.getAnimations().some((a) => a.playState === "running" && a.effect?.target?.closest?.(".ctx-sess.open")));
  return { page, card, errors };
}

// still waits for a tile to stop moving (the list's scroll, the tiles in)
const still = (loc) => loc.evaluate((n) => new Promise((done) => {
  let last = "", same = 0;
  const go = () => {
    const r = n.getBoundingClientRect(), at = [r.left, r.top, r.width, r.height].map(Math.round).join();
    same = at === last ? same + 1 : 0;
    last = at;
    if (same >= 10) done();
    else requestAnimationFrame(go);
  };
  go();
}));

// grow draws the same request further on, live: payment.ts read again
const grow = (page, by) => page.evaluate((by) => {
  const c = document.querySelector(".ctx-sess.open .ctx-card");
  window.__seen = [];
  const b = c.querySelector(".ctx-big b");
  const look = () => { window.__seen.push(b.textContent); if (window.__seen.length < 90) requestAnimationFrame(look); };
  c.ctxUpdate({ id: 19, agent: "codex", done: false, prompt: window.__prompt(by) }, {});
  look();
}, by);

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  for (const lang of Object.keys(words)) {
    for (const width of [1100, 420]) {
      test(`${engine} ${lang} ${width}px: the map as a live request grows`, async (t) => {
        const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
        t.after(() => browser.close());
        const { page, card, errors } = await open(browser, lang, width);
        await page.evaluate(`window.__prompt = ${prompt.toString().replace("tiny", JSON.stringify(tiny))}`);
        const map = card.locator(".ctx-map");

        // a tool's results gathered say how many; the small files their
        // folder, where the map has room for the files one by one (a phone's
        // narrow map draws them as one tile)
        assert.match(await map.locator(".ctx-tile.k-results .n").innerText(), /exec_command ×20/);
        const apart = (await map.locator(".ctx-tile.k-files").count()) > 1;
        if (apart) {
          const folder = map.locator(".ctx-tile.k-files", { hasText: "api/" });
          assert.equal(await folder.count(), 1, "the small files don't name their folder");
          assert.match(await folder.locator(".v").textContent(), new RegExp(words[lang].files));
        }

        // payment.ts pointed at, and read again while the tip is open
        const tile = map.locator(apart ? ".ctx-tile.k-files:has-text('payment.ts')" : ".ctx-tile.k-files");
        await tile.scrollIntoViewIfNeeded();
        await still(tile);
        await tile.hover();
        const tip = card.locator(".ctx-tip");
        await tip.waitFor({ state: "visible" });
        assert.match(await tip.innerText(), apart ? /9K/ : /14K/);
        // the list above can still settle once the tip is open: pointed at again
        await still(tile);
        await tile.hover();
        await still(tile);
        await grow(page, 8000);
        // the count counts on: past values between 40K and 48K, ending there
        await page.waitForFunction(() => window.__seen.length >= 90);
        const seen = await page.evaluate(() => window.__seen);
        const k = seen.map((s) => parseFloat(s));
        assert.equal(seen.at(-1), "48K");
        assert.ok(new Set(seen).size >= 4, `the count jumped: ${[...new Set(seen)]}`);
        assert.ok(k.every((x, i) => x >= 40 && x <= 48 && (i === 0 || x >= k[i - 1])), `the count went back or out: ${seen}`);
        // the tip stayed open, saying what the tile holds now, over it
        assert.ok(await tip.isVisible(), "the tip closed as the request grew");
        // the narrow map's Files tile can part as payment.ts grows: the tip
        // is then the tile's under the pointer
        assert.match(await tip.innerText(), apart ? /17K/ : /17K|22K/);
        if (apart) {
          assert.match(await tile.locator(".v").textContent(), /^17K/);
          const [tb, pb] = [await tip.boundingBox(), await tile.boundingBox()];
          assert.ok(tb.x < pb.x + pb.width && tb.x + tb.width > pb.x, "the tip was left behind its tile");
        }
        assert.deepEqual(errors, []);
      });
    }
  }

  test(`${engine}: with motion reduced the counts are simply there`, async (t) => {
    const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
    t.after(() => browser.close());
    const { page, card, errors } = await open(browser, "en", 1100, true);
    await page.evaluate(`window.__prompt = ${prompt.toString().replace("tiny", JSON.stringify(tiny))}`);
    await grow(page, 8000);
    await page.waitForFunction(() => window.__seen.length >= 90);
    assert.deepEqual([...new Set(await page.evaluate(() => window.__seen))], ["48K"]);
    assert.equal(await card.locator(".ctx-map .ctx-tile.k-files", { hasText: "payment.ts" }).locator(".v").textContent(), "17K · Files");
    assert.deepEqual(errors, []);
  });
}
