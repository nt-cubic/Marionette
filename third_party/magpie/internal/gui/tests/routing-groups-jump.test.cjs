// Run with Node's test runner and Playwright on the module path; see README.md.
// The routing groups sit at the Routing page's foot, below the stage, the
// story and every request: with many models a long scroll down (shihh on
// Discord). The page's head has a Routing groups beside New group, with a
// down arrow and a tooltip saying where they are; its click brings the
// groups' heading to the top of the view (or the view to its foot) and the
// focus into them, and nothing else moves: the groups stay where they were
// in the page, and a click no reader made scrolls nothing. It takes only
// room the head has spare: named where that fits, its arrow alone (still
// named to a screen reader) where only that does, left out where nothing
// does, and every other part of the head is where it is without it. Every
// language, Chromium and WebKit, wide, a phone's width and a narrow window.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");

const assets = path.resolve(process.env.MAGPIE_ROUTING_ASSETS || path.join(__dirname, "../assets"));
const models = [
  { id: "a/m", name: "m", providerName: "A", icon: "generic" },
  { id: "b/m", name: "m", providerName: "B", icon: "generic" },
];
const info = (ids) => ids.map((id) => ({ id, ready: true }));
const found = { id: "auto-m", name: "Model M", members: ["a/m", "b/m"], auto: true, ready: true, memberInfo: info(["a/m", "b/m"]) };

const words = {
  en: { btn: "Routing groups", tip: "Go to the routing groups, below the requests" },
  zh: { btn: "路由组", tip: "跳到请求下方的路由组" },
  "zh-TW": { btn: "路由組", tip: "跳到請求下方的路由組" },
  ja: { btn: "ルーティンググループ", tip: "リクエストの下にあるルーティンググループへ移動" },
  de: { btn: "Weiterleitungsgruppen", tip: "Zu den Weiterleitungsgruppen unter den Anfragen springen" },
};

// requests enough to put the groups well below the window's fold
const now = Date.now();
const routes = Array.from({ length: 60 }, (_, i) => ({
  id: "r" + i, seq: i + 1, at: new Date(now - i * 60000).toISOString(), agent: "claude", asked: "group/auto-m",
  model: "a/m", provider: "a", status: 200, ms: 900, ttft: 300, in: 100, out: 20, done: true,
}));

function serve(lang, web) {
  const state = { agents: [{ id: "claude", name: "Claude Code", path: "/test/claude", fields: [] }], profiles: [], settings: { lang, theme: "light" } };
  return async (r) => {
    const url = new URL(r.request().url());
    const json = (data) => r.fulfill({ json: data });
    if (url.pathname === "/boot.js") return r.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"${lang}",theme:"light",web:${web}};` });
    if (url.pathname === "/wails/runtime.js") return r.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json(state);
    if (url.pathname === "/api/plugins") return json({ plugins: [] });
    if (url.pathname === "/api/gateway/trace") {
      if (url.searchParams.get("wait")) return new Promise(() => {}); // nothing more comes
      return json({ mine: true, now: new Date().toISOString(), seq: routes.length, totals: { requests: routes.length, rerouted: 0, errors: 0 }, routes });
    }
    if (url.pathname === "/api/gateway/history") return json({ cut: false, days: [], routes: [] });
    if (url.pathname === "/api/groups") return json({ models, pools: [], deciders: [], found: true, groups: [found] });
    if (url.pathname === "/api/providers") return json({ providers: [], presets: [], excluded: [], gateway: { running: true, window: true, url: "http://127.0.0.1:3999" } });
    if (url.pathname.startsWith("/api/")) return json({});
    const file = path.join(assets, url.pathname === "/" ? "index.html" : url.pathname);
    const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)];
    await r.fulfill({ body: await fs.readFile(file), contentType });
  };
}

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  for (const lang of Object.keys(words)) {
    for (const { width, web } of [{ width: 1000, web: true }, { width: 480, web: true }, { width: 640, web: false }]) {
      const w = words[lang];
      test(`${engine} ${lang} ${width}px${web ? " web" : " window"}: Routing groups in the Routing page's head goes to them`, async (t) => {
        const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
        const page = await (await browser.newContext({ viewport: { width, height: 600 }, reducedMotion: "reduce" })).newPage();
        t.after(async () => {
          if (process.env.ARTIFACT_DIR) {
            await fs.mkdir(process.env.ARTIFACT_DIR, { recursive: true });
            await page.screenshot({ path: path.join(process.env.ARTIFACT_DIR, `${engine}-${lang}-${width}${web ? "-web" : ""}-routing-groups-jump.png`) });
          }
          await browser.close();
        });
        page.setDefaultTimeout(5000);
        const errors = [];
        page.on("pageerror", (e) => errors.push(e.message));
        await page.route("**/*", serve(lang, web));
        await page.goto("http://magpie.test/?view=routing");
        await page.locator(".rt-group", { hasText: "Model M" }).waitFor({ state: "attached" });
        await page.locator(".rt-reqs > *").first().waitFor({ state: "attached" });

        const view = page.locator("#view-routing");
        const go = page.locator("#view-routing > .row-head #rtGoGroups");
        await go.waitFor({ state: "attached" });
        await page.waitForFunction(() => document.getElementById("rtGoGroups").dataset.fit);
        await page.waitForTimeout(100); // the fit settles in the frame after a resize
        const fit = await go.getAttribute("data-fit");
        if (width === 1000) assert.equal(fit, "full", "a wide head names it");
        assert.equal(await go.getAttribute("title"), w.tip);
        assert.equal(await go.evaluate((e) => e.tagName), "BUTTON", "a button, no native select");
        assert.equal(await go.locator("svg").count(), 1, "a down arrow");
        // nothing else in the head moves for it, or wraps, at this width (the
        // spacer gives it its room)
        const apart = await page.evaluate(() => {
          const b = document.getElementById("rtGoGroups"), h = b.parentElement;
          const at = () => JSON.stringify([h.getBoundingClientRect().height, ...[...h.children].filter((c) => c !== b && !c.classList.contains("grow")).map((c) => { const r = c.getBoundingClientRect(); return [c.className, Math.round(r.left), Math.round(r.top), Math.round(r.width), Math.round(r.height)]; })]);
          const withIt = at();
          b.style.setProperty("display", "none", "important");
          const without = at();
          b.style.removeProperty("display");
          return { withIt, without, over: h.scrollWidth - h.clientWidth };
        });
        assert.equal(apart.withIt, apart.without, "the head is laid out as without it");
        assert(apart.over <= 0, `nothing overflows the head (${apart.over}px)`);
        const vw = await page.evaluate(() => document.documentElement.scrollWidth - innerWidth);
        assert(vw <= 0, `no sideways scroll (${vw}px)`);
        if (fit === "none") {
          assert.equal(await go.isVisible(), false, "left out where the head has no room");
          assert.deepEqual(errors, []);
          return;
        }
        await go.waitFor();
        // named on screen, or to a screen reader where only the arrow shows
        assert.equal(await page.getByRole("button", { name: w.btn, exact: true }).count(), 1);
        const label = go.locator("span");
        if (fit === "full") assert.equal((await label.textContent()).trim(), w.btn);
        else assert((await label.boundingBox()).width <= 1, "the arrow alone");
        const border = await go.evaluate((e) => { const c = getComputedStyle(e); return c.borderLeftWidth !== c.borderRightWidth || c.borderLeftColor !== c.borderRightColor; });
        assert.equal(border, false, "no left-border accent");
        const hb = await go.boundingBox();
        assert(hb.x >= 0 && hb.x + hb.width <= width && hb.y >= 0, "in sight on arrival");

        // the layout as before: the groups after every request, below the fold
        assert.equal(await page.evaluate(() => {
          const more = document.getElementById("rtMore"), g = more.querySelector(".rt-gsec"), reqs = more.querySelector(".rt-reqs");
          return g.parentElement === more && more.lastElementChild === g && !!(reqs.compareDocumentPosition(g) & Node.DOCUMENT_POSITION_FOLLOWING);
        }), true, "the groups stay at the page's foot, after the requests");
        const vh = await view.evaluate((v) => v.getBoundingClientRect().bottom);
        const gTop = () => page.locator(".rt-gsec").evaluate((e) => e.getBoundingClientRect().top);
        assert(await gTop() > vh, "the groups are below the fold on arrival");
        assert.equal(await view.evaluate((v) => v.scrollTop), 0, "the page opens at its top");

        // a click no reader made (code's own) scrolls nothing
        await go.evaluate((b) => b.click());
        await page.waitForTimeout(300);
        assert.equal(await view.evaluate((v) => v.scrollTop), 0, "an untrusted click doesn't move the page");

        // the reader's click: the groups' heading at the top of the view, the focus in them
        await go.click();
        // at the top of the view, or the view at its foot where the page ends first
        await page.waitForFunction(() => {
          const v = document.getElementById("view-routing"), g = document.querySelector(".rt-gsec");
          const top = g.getBoundingClientRect().top - v.getBoundingClientRect().top;
          return (top >= -1 && top < 40) || (top > 0 && v.scrollTop > 0 && v.scrollTop + v.clientHeight >= v.scrollHeight - 1);
        });
        assert.equal(await page.evaluate(() => document.querySelector(".rt-gsec").contains(document.activeElement)), true, "the focus goes with them");
        const gh = await page.locator(".rt-gsec > .row-head .label").first().boundingBox();
        assert(gh && gh.y >= 0 && gh.y + gh.height <= vh, "the groups' heading is in sight");
        // and stays there: a moment later the view hasn't been pulled away
        const at = await view.evaluate((v) => v.scrollTop);
        await page.waitForTimeout(400);
        assert.equal(await view.evaluate((v) => v.scrollTop), at, "the view stays where the click took it");
        assert.deepEqual(errors, []);
      });
    }
  }
}
