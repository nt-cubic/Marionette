// Run with Node's test runner and Playwright on the module path; see README.md.
// A request sent to a remote magpie for one of its routing groups is
// answered with the member the group routed to (莫 on Discord: group/auto-…
// answered by deepseek/deepseek-v4.1-flash, shown amber as a swap). The
// gateway marks it routed, not swapped: the Routing page's Requests list
// shows the member plain, not amber, with a title saying the remote magpie's
// group routed it there, and so does its story; the Usage page's Requests
// show it muted with the same title. A real swap beside it is still amber.
// Chromium and WebKit, English and Chinese; no backend, the API is faked.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");

const assets = path.resolve(__dirname, "../assets");
const now = new Date();
const day = [now.getFullYear(), now.getMonth() + 1, now.getDate()].map((n) => String(n).padStart(2, "0")).join("-");
const at = (i) => new Date(now.getTime() - (i + 1) * 60e3).toISOString();
const GROUP = "group/auto-deepseek-v4-1-flash", MEMBER = "deepseek/deepseek-v4.1-flash";
const office = { id: "office", provider: "office", name: "Office", kind: "provider", model: GROUP };
const relay = { id: "relay", provider: "relay", name: "Relay", kind: "provider", model: "gpt-6-sol" };
// newest first: the remote group's, a real swap, and a few plain ones (with
// three rows alone the stage above jiggles in WebKit)
const tries = [
  [office, GROUP, MEMBER, { routed: true }],
  [relay, "gpt-6-sol", "gpt-6-luna", { swapped: true }],
  [relay, "gpt-6-sol", "", {}], [relay, "gpt-6-sol", "", {}], [relay, "gpt-6-sol", "", {}], [relay, "gpt-6-sol", "", {}],
];
const routes = tries.map(([key, model, s, mark], i) => ({
  id: 100 - i, seq: 100 - i, time: at(i), agent: "codex", model: key.id + "/" + model, provider: key.id,
  order: [key], tries: [{ id: key.id, model, start: at(i), done: true, status: 200, ms: 900, ...(s ? { served: s } : {}), ...mark }],
  done: true, status: 200, ms: 900, tokens: 1200, ...(s ? { served: s } : {}), ...mark,
}));
const rows = [
  { t: at(0), agent: "codex", agentName: "Codex", icon: "codex-color", provider: "office", providerName: "Office", host: "192.168.1.5:3425", req: "office/" + GROUP, model: GROUP, served: MEMBER, routed: true, in: 900, out: 120, ms: 1320, status: 200, cost: 0, priced: false },
  { t: at(1), agent: "codex", agentName: "Codex", icon: "codex-color", provider: "relay", providerName: "Relay", host: "team", req: "sol", model: "gpt-6-sol", served: "gpt-6-luna", swapped: true, in: 1000, out: 100, ms: 900, status: 200, cost: 0.001, priced: true },
];

function serve(lang) {
  const state = { agents: [{ id: "codex", name: "Codex", path: "/test/config.toml", fields: [] }], profiles: [], settings: { lang, theme: "light" } };
  return async (route) => {
    const url = new URL(route.request().url());
    const json = (data) => route.fulfill({ json: data });
    if (url.pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"${lang}",theme:"light",web:true};` });
    if (url.pathname === "/wails/runtime.js") return route.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json(state);
    if (url.pathname === "/api/gateway/trace") {
      if (url.searchParams.get("wait")) await new Promise((r) => setTimeout(r, 20e3));
      return json({ mine: true, now: now.toISOString(), seq: 1, totals: { requests: 0, rerouted: 0, errors: 0 }, routes: [] });
    }
    if (url.pathname === "/api/gateway/history") {
      const d = url.searchParams.get("day");
      return json({ cut: false, days: [{ day, requests: routes.length }], routes: d ? routes : [] });
    }
    if (url.pathname === "/api/usage/requests") {
      return json({ period: url.searchParams.get("period"), rows, offset: 0, total: rows.length, calls: rows.length, errors: 0, input: 1900, output: 220, cost: 0.001, unpriced: 1,
        agents: [{ id: "codex", name: "Codex", icon: "codex-color" }] });
    }
    if (url.pathname === "/api/usage/quotas") return json([]);
    if (url.pathname === "/api/usage") return json({ calls: 2, errors: 0, input: 1900, output: 220, cost: 0.001, bucket: "day", series: [], agents: [], models: [] });
    if (url.pathname === "/api/sessions") return json({ sessions: [], dirs: [] });
    if (url.pathname === "/api/sessions/stats") return json({ from: "", to: "", days: [], agents: {} });
    if (url.pathname === "/api/plugins") return json({ plugins: [] });
    if (url.pathname === "/api/groups") return json({ groups: [], models: [] });
    if (url.pathname === "/api/providers") return json({ providers: [], gateway: { running: true, window: true } });
    if (url.pathname.startsWith("/api/")) return json({});
    const file = path.join(assets, url.pathname === "/" ? "index.html" : url.pathname);
    const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)];
    await route.fulfill({ body: await fs.readFile(file), contentType });
  };
}

const want = {
  en: { tag: "served " + MEMBER, swap: "served gpt-6-luna", why: new RegExp(`^${GROUP} is a routing group of the remote magpie, and it routed the request to ${MEMBER}: the group picking one of its models, not the vendor swapping the model\\.$`) },
  zh: { tag: "实际 " + MEMBER, swap: "实际 gpt-6-luna", why: new RegExp(`^${GROUP} 是远程 magpie 的路由组，请求被路由给 ${MEMBER}：这是路由组选择了组内模型，并非服务商替换了模型。$`) },
};

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  for (const lang of ["en", "zh"]) {
    test(`${engine} ${lang}: a remote magpie's group routing to a member is not a swap`, async (t) => {
      const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
      const context = await browser.newContext({ viewport: { width: 1180, height: 760 }, reducedMotion: "reduce" });
      const page = await context.newPage();
      page.setDefaultTimeout(5000);
      const errors = [];
      page.on("pageerror", (e) => errors.push(e.message));
      await page.route("**/*", serve(lang));
      t.after(async () => {
        if (process.env.ARTIFACT_DIR) {
          await fs.mkdir(process.env.ARTIFACT_DIR, { recursive: true });
          await page.screenshot({ path: path.join(process.env.ARTIFACT_DIR, `${engine}-${lang}-remote-group.png`), fullPage: true });
        }
        await browser.close();
      });

      // the Routing page's Requests: the member plain, the swap amber
      await page.goto("http://magpie.test/?view=routing");
      // the date bar's day (the By request / By session buttons are rt-day too)
      await page.locator(".rt-days .rt-day").nth(1).click();
      await page.locator(".rt-req").nth(routes.length - 1).waitFor();
      const marks = await page.locator(".rt-req").evaluateAll((rs) => rs.map((r) => [r.querySelector(".to .swap")?.textContent || "", r.querySelector(".to .routed")?.textContent || ""]));
      assert.deepEqual(marks, [["", want[lang].tag], [want[lang].swap, ""], ["", ""], ["", ""], ["", ""], ["", ""]]);
      const look = await page.locator(".rt-req .to .routed").evaluate((e) => {
        const row = e.closest(".rt-req").getBoundingClientRect(), b = e.getBoundingClientRect(), cs = getComputedStyle(e);
        const swap = getComputedStyle(document.querySelector(".rt-req .to .swap"));
        return { inRow: b.left >= row.left && b.right <= row.right + 0.5 && b.width > 0, color: cs.color, bg: cs.backgroundColor, swapColor: swap.color, border: cs.borderLeftWidth, title: e.title };
      });
      assert(look.inRow, JSON.stringify(look));
      assert.notEqual(look.color, look.swapColor, "the member is shown amber");
      assert.equal(look.bg, "rgba(0, 0, 0, 0)");
      assert.equal(look.border, "0px");
      assert.match(look.title, want[lang].why);

      // its story says so, with no swap in it; the click moves nothing
      const row = page.locator(".rt-req").nth(0);
      // measure once the page holds still: the day's rows arriving grow the
      // page above the list, and the day button's click is kept in place by
      // scrolling the view, which can land after the last row is in
      const was = await row.evaluate(async (e) => {
        let last = NaN;
        for (let i = 0; i < 60; i++) {
          await new Promise((r) => requestAnimationFrame(r));
          const top = e.getBoundingClientRect().top;
          if (top === last) return top;
          last = top;
        }
        return last;
      });
      const scrolled = await page.evaluate(() => [scrollX, scrollY, document.scrollingElement.scrollTop]);
      await row.click();
      await page.waitForTimeout(600);
      assert(Math.abs((await row.evaluate((e) => e.getBoundingClientRect().top)) - was) <= 1, "picking the request moved the page");
      assert.deepEqual(await page.evaluate(() => [scrollX, scrollY, document.scrollingElement.scrollTop]), scrolled);
      assert.equal(await page.locator(".rt-steps li.swap").count(), 0);
      const said = await page.locator(".rt-steps li").evaluateAll((ls) => ls.map((l) => l.textContent));
      assert(said.some((s) => want[lang].why.test(s)), JSON.stringify(said));
      await page.locator(".rt-req").nth(1).click();
      await page.waitForTimeout(300);
      assert.equal(await page.locator(".rt-steps li.swap").count(), 1);

      // the Usage page's Requests: muted, with the same title; the swap amber
      await page.goto("http://magpie.test/");
      await page.locator('nav [data-view="usage"], [data-view="usage"]').first().click();
      await page.locator(":is(#usageTab .opt, .nav-s):visible").nth(1).click();
      await page.locator("#ledWrap .led tbody tr").nth(1).waitFor();
      const cells = await page.locator("#ledWrap .led tbody tr").evaluateAll((trs) => trs.map((tr) => {
        const s = [...tr.querySelectorAll("span")].find((e) => e.textContent === "deepseek/deepseek-v4.1-flash" || e.textContent === "gpt-6-luna");
        return s ? [s.textContent, s.className, s.title] : null;
      }));
      assert.equal(cells[0][0], MEMBER);
      assert(!cells[0][1].split(" ").includes("swap") && cells[0][1].split(" ").includes("muted"), cells[0][1]);
      assert.match(cells[0][2], want[lang].why);
      assert.equal(cells[1][1], "swap");
      assert.deepEqual(errors, []);
    });
  }
}
