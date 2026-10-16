// Run with Node's test runner and Playwright on the module path; see README.md.
// The Routing page's own frame loop runs only while the page is the one in
// sight. With Agents selected — #view-routing.hidden, the window drawing
// frames, the tab in sight — the loop must stop asking for frames: the
// function frame that drew the page every frame until now asked for the
// next one whatever the page was showing, so the magpies were posed (and
// the loop kept awake) forever while nobody looked at Routing. In sight
// again the view is picked and the loop resumes. What the poll lists came
// in meanwhile is another test's business (routing-flood); this one counts
// the loop's own callbacks.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");
const engines = process.env.BROWSER ? [process.env.BROWSER] : ["webkit"];

const assets = path.resolve(__dirname, "../assets");
const routing = path.join(assets, "plugins/routing/client.js");
const iso = (ms) => new Date(ms).toISOString();
const acct = (n) => ({ id: `acct-${n}`, provider: `prov${n}`, name: `Provider ${n}`, who: `user${n}@example.com`, kind: "account", model: `model-${n}`, known: true, used: 10 * n, plan: "Pro" });
const agentsOf = ["codex", "claude", "opencode", "kimi"];

function req(id) {
  const t0 = Date.now() - 500;
  const order = [acct(id % 5), acct((id + 1) % 5)];
  const tries = order.slice(0, 1).map((w) => ({ id: w.id, model: w.model, start: iso(t0), done: true, ms: 300, status: 200 }));
  return { id, seq: id, time: iso(t0), agent: agentsOf[id % agentsOf.length], model: order[0].model, provider: order[0].provider, order, tries, done: true, status: 200, ms: 400, tokens: 900 };
}

function serve(lang, feed = [req(1)]) {
  const state = { agents: agentsOf.map((id) => ({ id, name: id[0].toUpperCase() + id.slice(1), path: `/test/${id}`, fields: [] })), profiles: [], settings: { lang, theme: "light" } };
  return async (route) => {
    const url = new URL(route.request().url());
    const json = (data) => route.fulfill({ json: data });
    if (url.pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"${lang}",theme:"light",web:true};` });
    if (url.pathname === "/wails/runtime.js") return route.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json(state);
    if (url.pathname === "/api/plugins") return json({ plugins: [] });
    if (url.pathname === "/api/gateway/trace") return json({ mine: true, now: new Date().toISOString(), seq: url.searchParams.get("after") || 0, totals: { requests: 0, rerouted: 0, errors: 0 }, routes: url.searchParams.get("wait") ? [] : feed });
    if (url.pathname === "/api/gateway/history") return json({ cut: false, days: [], routes: [] });
    if (url.pathname === "/api/groups") return json({ models: [], groups: [], pools: [] });
    if (url.pathname === "/api/providers") return json({ providers: [], excluded: [], gateway: { running: true, window: true, url: "http://127.0.0.1:3999" } });
    if (url.pathname.startsWith("/api/")) return json({});
    const file = path.join(assets, url.pathname === "/" ? "index.html" : url.pathname);
    const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)];
    await route.fulfill({ body: await fs.readFile(file), contentType });
  };
}

// counts the callbacks of the loop's own frame, whatever name the minifier
// would give it: the function that asks for itself again
const init = () => {
  const raf = window.requestAnimationFrame.bind(window);
  window.__frames = 0;
  window.__steps = 0;
  window.__packets = 0;
  window.__hidden = false;
  Object.defineProperty(document, "hidden", { configurable: true, get: () => window.__hidden });
  window.__visibility = (hidden) => { window.__hidden = hidden; document.dispatchEvent(new Event("visibilitychange")); };
  new MutationObserver((records) => {
    for (const record of records) for (const node of record.addedNodes) {
      if (node.nodeType === 1 && node.matches(".pkt")) window.__packets++;
    }
  }).observe(document, { childList: true, subtree: true });
  window.__seenFrame = false;
  // marked between frames, and the next frame awaited, so the one already
  // asked for when the mark falls is out of the count before it settles
  window.__mark = () => { window.__frames = 0; return new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))); };
  window.__count = (ms) => new Promise((r) => setTimeout(() => r(window.__frames), ms));
  window.requestAnimationFrame = (cb) => {
    const self = typeof cb === "function" && /requestAnimationFrame\s*\(\s*([A-Za-z_$][\w$]*)/.test(cb.toString()) && new RegExp(`\\b${RegExp.$1}\\s*\\(`).test(cb.toString());
    if (typeof cb === "function" && (cb.name === "frame" || self)) window.__seenFrame = true;
    return raf((ts) => { if (cb.name === "step") window.__steps++; if (typeof cb === "function" && (cb.name === "frame" || self)) window.__frames++; cb(ts); });
  };
};

const byName = (src) => /(?:function\s+frame\s*\(|(?:const|let|var)\s+frame\s*=\s*(?:function|\(?\s*[\w,\s]*\)?\s*=>)|frame[:\s]*\(?\s*[\w,\s]*\)?\s*(?::[^=]+)?=>)/.test(src);
const selfRaf = (src) => { const m = src.match(/requestAnimationFrame\s*\(\s*([A-Za-z_$][\w$]*)\s*\)/); return m ? new RegExp(`function\\s+${m[1]}\\s*\\(`).test(src) : false; };

test("the loop's frame is still there to be counted, and asks for itself", async () => {
  const src = await fs.readFile(routing, "utf8");
  assert(byName(src) || selfRaf(src), "routing/client.js must still keep its frame loop in a function of its own, or the test below counts nothing");
});

for (const engine of engines) for (const lang of ["en", "zh"]) {
  test(`${engine} ${lang}: with Agents selected the Routing loop asks for no more frames, and picking Routing again resumes it`, async (t) => {
    const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
    const context = await browser.newContext({ viewport: { width: 1100, height: 900 } });
    await context.addInitScript(init);
    const page = await context.newPage();
    page.setDefaultTimeout(5000);
    const errors = [];
    page.on("pageerror", (e) => errors.push(e.message));
    await page.route("**/*", serve(lang));
    t.after(async () => {
      if (process.env.ARTIFACT_DIR) {
        await fs.mkdir(process.env.ARTIFACT_DIR, { recursive: true });
        await page.screenshot({ path: path.join(process.env.ARTIFACT_DIR, `${engine}-${lang}-idle-frame.png`) });
      }
      await browser.close();
    });
    await page.goto("http://magpie.test/?view=routing");
    await page.locator(".rt-req").first().waitFor();
    // Routing in sight: the loop is running, and it is this loop we count
    await page.waitForFunction(() => window.__seenFrame, null, { timeout: 5000 });
    await page.evaluate(() => window.__mark());
    const flying = await page.evaluate(() => window.__count(600));
    assert(flying > 10, `the loop must run while Routing is in sight (${flying} frames in 600ms)`);
    assert(errors.length === 0, errors.join("\n"));

    // Agents picked: #view-routing.hidden, the tab in sight, frames drawn
    await page.locator('#nav button[data-view="agents"]').click();
    assert.equal(await page.evaluate(() => document.querySelector("#view-routing").hidden), true, "Agents selected must hide the Routing view");
    await page.evaluate(() => window.__mark());
    const idle = await page.evaluate(() => window.__count(1200));
    // the frame already asked for when Agents was picked still runs, and
    // finds the page out of sight: it is the last one
    assert(idle <= 1, `${idle} frames of the Routing loop were drawn while Agents was shown, and it kept asking for more`);
    // and it stays stopped, rather than waking on a later request
    await page.evaluate(() => window.__mark());
    assert.equal(await page.evaluate(() => window.__count(1200)), 0, "the Routing loop came back with Agents still selected");

    // Routing picked again: the loop resumes, and the page is drawn
    await page.locator('#nav button[data-view="routing"]').click();
    await page.evaluate(() => window.__mark());
    const again = await page.evaluate(() => window.__count(600));
    assert(again > 10, `picking Routing again must resume the loop (${again} frames in 600ms)`);
    assert.equal(await page.evaluate(() => document.querySelectorAll(".rt-req").length >= 1), true, "the page is drawn again");
    assert(errors.length === 0, errors.join("\n"));
    // A document visibility event must stop the loop even when the engine
    // would suspend its last scheduled frame before it can observe hidden.
    await page.evaluate(() => window.__visibility(true));
    await page.evaluate(() => window.__mark());
    assert.equal(await page.evaluate(() => window.__count(600)), 0);
    await page.evaluate(() => window.__visibility(false));
    await page.evaluate(() => window.__mark());
    assert(await page.evaluate(() => window.__count(600)) > 10);
    assert.equal(errors.length, 0, errors.join("\n"));
  });
}

const hiddenWays = {
  "another view": {
    hide: page => page.locator('#nav button[data-view="agents"]').click(),
    show: page => page.locator('#nav button[data-view="routing"]').click(),
  },
  "document visibility": {
    hide: page => page.evaluate(() => window.__visibility(true)),
    show: page => page.evaluate(() => window.__visibility(false)),
  },
};

async function routingPage(t, engine, lang, feed, view = "routing") {
  const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
  t.after(() => browser.close());
  const context = await browser.newContext({ viewport: { width: 1100, height: 900 } });
  await context.addInitScript(init);
  const page = await context.newPage();
  page.setDefaultTimeout(5000);
  const errors = [];
  page.on("pageerror", e => errors.push(e.message));
  await page.route("**/*", serve(lang, feed));
  await page.goto(`http://magpie.test/?view=${view}`);
  await page.waitForFunction(() => document.querySelectorAll(".rt-req").length > 0);
  return { page, errors };
}

for (const engine of engines) {
  for (const lang of ["en", "zh"]) {
    for (const [way, how] of Object.entries(hiddenWays)) {
      test(`${engine} ${lang}: hiding a running replay through ${way} ends it and stops its frames`, async t => {
        const { page, errors } = await routingPage(t, engine, lang, [1, 2, 3, 4, 5].map(req));
        await page.getByRole("button", { name: lang === "zh" ? "全部重放" : "Replay them all", exact: true }).click();
        await page.waitForFunction(() => !document.querySelector(".rt-replay").hidden && document.querySelectorAll(".pkt").length > 0);
        await how.hide(page);
        // Check the bar's own hidden flag, not visibility inherited from Agents.
        await page.waitForFunction(() => document.querySelector(".rt-replay").hidden && document.querySelectorAll(".pkt").length === 0);
        await page.evaluate(() => { window.__steps = 0; });
        await page.waitForTimeout(300);
        assert(await page.evaluate(() => window.__steps) <= 1, "replay kept scheduling frames after it was hidden");
        await how.show(page);
        assert(await page.evaluate(() => document.querySelector(".rt-replay").hidden), "showing Routing restarted the old replay");
        assert.equal(errors.length, 0, errors.join("\n"));
      });
    }
    test(`${engine} ${lang}: each live request plays once when Routing resumes`, async t => {
      const r = req(1);
      r.done = false;
      r.status = 0;
      r.tries.forEach(tr => { tr.done = false; tr.status = 0; });
      const { page, errors } = await routingPage(t, engine, lang, [r], "agents");
      const showOnce = async show => {
        await page.evaluate(() => { window.__packets = window.__frames = 0; });
        await show(page);
        await page.waitForFunction(() => window.__frames >= 2);
        assert.equal(await page.evaluate(() => window.__packets), 1, "resume played a live request twice");
      };
      await showOnce(hiddenWays["another view"].show);
      for (const how of Object.values(hiddenWays)) {
        await how.hide(page);
        await page.evaluate(() => window.__mark());
        await showOnce(how.show);
      }
      assert.equal(errors.length, 0, errors.join("\n"));
    });
  }
}
