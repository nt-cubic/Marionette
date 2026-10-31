// Run with Node's test runner and Playwright on the module path; see README.md.
// An allowance card on Usage → Overview can be hidden from it
// (whqtian_07012 on Discord: StepFun's plan card wanted, its balance card
// not). A right-click on a card, or a long press on a touch screen, opens
// its menu: Hide from Overview, and on a plan's card showing its key's
// balance, Hide its balance from Overview, which leaves the plan. Each is
// kept by provider, account and kind (POST /api/usage/arrange,
// cardsHidden) and stays hidden when the page is opened again. A quiet
// line under the cards says how many are hidden and opens them, each with
// Unhide, and Show all. No click moves the page. Every language, a wide
// window, a narrow one and magpie web on a phone; no backend, the API is
// faked.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { isDeepStrictEqual } = require("node:util");
const { chromium, webkit } = require("playwright");
const reader = require("./reader.cjs");

const assets = path.resolve(__dirname, "../assets");
const win = (name, used) => ({ name, used, resetSecs: 3 * 3600 });
const QUOTAS = [
  { provider: "claude", name: "Claude", icon: "claude-color", plan: "max", windows: [win("5h", 70)] },
  // one StepFun key: its Step Plan and its balance, drawn as one card
  { provider: "stepfun", name: "StepFun", icon: "stepfun-color", plan: "Step Plan", windows: [win("5h", 30)] },
  { provider: "stepfun", name: "StepFun", icon: "stepfun-color", windows: [], balance: "¥42.00" },
  // a second StepFun provider with a balance alone: a card of its own
  { provider: "stepfun-2", name: "StepFun Work", icon: "stepfun-color", windows: [], balance: "¥7.50" },
];

function server(lang, web, saved, posts) {
  return async (route) => {
    const req = route.request();
    const url = new URL(req.url());
    const json = (data) => route.fulfill({ json: data });
    const settings = () => ({ lang, theme: "light", usageCardsHidden: saved.hidden });
    if (url.pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"${lang}",theme:"light",web:${web}};` });
    if (url.pathname === "/wails/runtime.js") return route.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json({ agents: [], clients: [], profiles: [], settings: settings(), fx: { rate: 7.2, at: new Date().toISOString() } });
    if (url.pathname === "/api/usage/arrange") {
      const body = JSON.parse(req.postData() || "{}");
      posts.push(body);
      if (body.cardsHidden) saved.hidden = body.cardsHidden;
      return json(settings());
    }
    if (url.pathname === "/api/settings") return json(settings());
    if (url.pathname === "/api/usage/quotas") return json(QUOTAS);
    if (url.pathname === "/api/usage") return json({ calls: 0, errors: 0, input: 0, output: 0, reasoning: 0, cache_read: 0, cache_write: 0, unpriced: 0, cost: 0, bucket: "day", series: [], path: "~/.config/magpie/usage.jsonl", agents: [], models: [] });
    if (url.pathname === "/api/providers") return json({ providers: [], presets: [], excluded: [], models: [], gateway: { running: false } });
    if (url.pathname === "/api/sessions") return json({ sessions: [], dirs: [] });
    if (url.pathname === "/api/sessions/stats") return json({ from: "", to: "", days: [], agents: {} });
    if (url.pathname === "/api/groups") return json({ groups: [], models: [] });
    if (url.pathname.startsWith("/api/")) return json({});
    const file = path.join(assets, url.pathname === "/" ? "index.html" : url.pathname);
    const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)];
    const body = await fs.readFile(file).catch(() => null);
    await (body ? route.fulfill({ body, contentType }) : route.fulfill({ status: 404, body: "" }));
  };
}

const L = {
  en: { hide: "Hide from Overview", bal: "Hide its balance from Overview", n: (n) => `${n} hidden`, unhide: "Unhide", all: "Show all", balance: "Balance" },
  zh: { hide: "从概览中隐藏", bal: "从概览中隐藏它的余额", n: (n) => `已隐藏 ${n} 个`, unhide: "取消隐藏", all: "全部显示", balance: "余额" },
  "zh-TW": { hide: "從概覽中隱藏", bal: "從概覽中隱藏它的餘額", n: (n) => `已隱藏 ${n} 個`, unhide: "取消隱藏", all: "全部顯示", balance: "餘額" },
  ja: { hide: "概要から隠す", bal: "残高を概要から隠す", n: (n) => `${n} 件非表示`, unhide: "再表示", all: "すべて表示", balance: "残高" },
  de: { hide: "In der Übersicht ausblenden", bal: "Sein Guthaben in der Übersicht ausblenden", n: (n) => `${n} ausgeblendet`, unhide: "Einblenden", all: "Alle anzeigen", balance: "Guthaben" },
};
const SIZES = [{ width: 1200, web: false }, { width: 560, web: false }, { width: 390, web: true }];

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  test(engine + ": an allowance card is hidden from the Overview, and brought back", async (t) => {
    assert(["chromium", "webkit"].includes(engine), "BROWSER must be chromium or webkit");
    const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
    t.after(() => browser.close());
    for (const lang of Object.keys(L)) {
      for (const { width, web } of SIZES) {
        const w = L[lang];
        await t.test(`${lang} ${width}${web ? " web" : ""}`, async () => {
          const errors = [];
          const saved = { hidden: [] }, posts = [];
          const ctx = await browser.newContext({ viewport: { width, height: 700 }, reducedMotion: "reduce", hasTouch: web, isMobile: web && engine === "chromium" });
          const open = async () => {
            const p = await ctx.newPage();
            p.setDefaultTimeout(5000);
            p.on("pageerror", (e) => errors.push(e.message));
            await p.route("**/*", server(lang, web, saved, posts));
            await p.goto("http://magpie.test/?view=usage");
            // the first load of the whole app, on a busy machine
            await p.locator("#subscriptionUsage .subscription-card").first().waitFor({ timeout: 20000 });
            return p;
          };
          const cards = (p) => p.locator("#subscriptionUsage > .subscription-card").evaluateAll((cs) => cs.map((c) => c.dataset.provider));
          const card = (p, id) => p.locator(`#subscriptionUsage > .subscription-card[data-provider="${id}"]`);
          const top = (p) => p.locator("#view-usage").evaluate((v) => v.scrollTop);
          const items = (p) => p.locator(".row-menu .rm-item").allTextContents();
          // a long press, as a finger makes it: the pointer down, held, up
          const longPress = (loc) => loc.evaluate((c) => new Promise((done) => {
            const box = c.querySelector(".subscription-head").getBoundingClientRect();
            const at = { pointerType: "touch", isPrimary: true, pointerId: 7, bubbles: true, clientX: box.left + box.width / 2, clientY: box.top + box.height / 2 };
            c.querySelector(".subscription-head b").dispatchEvent(new PointerEvent("pointerdown", at));
            setTimeout(() => { c.querySelector(".subscription-head b").dispatchEvent(new PointerEvent("pointerup", at)); done(); }, 700);
          }));

          // the save reaches the fake server after the page has drawn it
          const savedAs = async (want) => {
            for (const end = Date.now() + 5000; !isDeepStrictEqual(saved.hidden, want) && Date.now() < end;) await new Promise((ok) => setTimeout(ok, 20));
            assert.deepEqual(saved.hidden, want);
          };

          let p = await open();
          assert.deepEqual(await cards(p), ["claude", "stepfun", "stepfun-2"]);
          assert.match(await card(p, "stepfun").textContent(), /42\.00/, "the plan's card shows its key's balance");
          assert.equal(await p.locator("#quotaHid").isHidden(), true, "nothing hidden, no line");

          // the balance-only card: its menu has Hide from Overview alone
          await reader.inView(p, card(p, "stepfun-2"));
          let at = await top(p);
          if (web) await longPress(card(p, "stepfun-2"));
          else await card(p, "stepfun-2").locator(".subscription-head").click({ button: "right" });
          await p.locator(".row-menu").waitFor();
          assert.deepEqual(await items(p), [w.hide]);
          await p.locator(".row-menu .rm-item").click();
          await p.waitForFunction(() => !document.querySelector('#subscriptionUsage > [data-provider="stepfun-2"]'));
          await savedAs(["stepfun-2|balance"]);
          assert.deepEqual(posts.at(-1), { cardsHidden: ["stepfun-2|balance"] });
          const line = p.locator("#quotaHid .us-hidline");
          assert.equal((await line.textContent()).trim(), w.n(1));
          assert.equal(await top(p), at, "hiding a card doesn't move the page");

          // the plan's card with its balance: the balance alone goes
          await reader.inView(p, card(p, "stepfun"));
          at = await top(p);
          await card(p, "stepfun").locator(".subscription-head").click({ button: "right" });
          assert.deepEqual(await items(p), [w.hide, w.bal]);
          await p.locator(".row-menu .rm-item", { hasText: w.bal }).click();
          await p.waitForFunction(() => !/42\.00/.test(document.querySelector('#subscriptionUsage > [data-provider="stepfun"]').textContent));
          await savedAs(["stepfun-2|balance", "stepfun|balance"]);
          assert.deepEqual(await cards(p), ["claude", "stepfun"], "the plan stays");
          assert.match(await card(p, "stepfun").textContent(), /Step Plan/);
          assert.equal((await line.textContent()).trim(), w.n(2));
          assert.equal(await top(p), at, "hiding a balance doesn't move the page");

          // the line opens them, each named with its kind, and Show all
          await reader.inView(p, line);
          at = await top(p);
          await line.click();
          assert.equal(await line.getAttribute("aria-expanded"), "true");
          const hidIds = () => p.locator("#quotaHid .row.stat").evaluateAll((rs) => rs.map((r) => r.dataset.id));
          assert.deepEqual(await hidIds(), ["stepfun|balance", "stepfun-2|balance"]);
          assert.match(await p.locator("#quotaHid .row.stat").first().textContent(), new RegExp(w.balance));
          assert.deepEqual((await p.locator("#quotaHid .us-unhide").allTextContents()).map((s) => s.trim()), [w.unhide, w.unhide]);
          assert.equal((await p.locator("#quotaHid .us-showall").textContent()).trim(), w.all);
          assert.equal(await top(p), at, "opening the hidden ones doesn't move the page");

          // Unhide puts one card back
          const back = p.locator('#quotaHid .row.stat[data-id="stepfun-2|balance"] .us-unhide');
          await reader.inView(p, back);
          at = await top(p);
          await back.click();
          await p.waitForFunction(() => !!document.querySelector('#subscriptionUsage > [data-provider="stepfun-2"]'));
          await savedAs(["stepfun|balance"]);
          assert.deepEqual(posts.at(-1), { cardsHidden: ["stepfun|balance"] });
          assert.deepEqual(await cards(p), ["claude", "stepfun", "stepfun-2"]);
          assert.equal((await line.textContent()).trim(), w.n(1));
          assert.equal(await p.locator("#quotaHid .us-showall").isHidden(), true, "Show all is for more than one");

          // nothing is wider than the page, every action inside its row
          const wide = await p.evaluate(() => { const v = document.querySelector("#view-usage"); return [v.scrollWidth, v.clientWidth, document.documentElement.scrollWidth, window.innerWidth]; });
          assert(wide[0] <= wide[1] + 1 && wide[2] <= wide[3] + 1, "no sideways scroll: " + wide);
          const out = await p.evaluate(() => [...document.querySelectorAll("#quotaHid .row.stat")].filter((r) => {
            const box = r.getBoundingClientRect(), x = r.querySelector(".us-unhide").getBoundingClientRect();
            return x.right > box.right + 0.5 || x.left < box.left - 0.5;
          }).map((r) => r.dataset.id));
          assert.deepEqual(out, [], "every row's Unhide stays inside its row");

          // opened again, the balance is still hidden
          await p.close();
          p = await open();
          assert.deepEqual(await cards(p), ["claude", "stepfun", "stepfun-2"]);
          assert.doesNotMatch(await card(p, "stepfun").textContent(), /42\.00/);
          assert.equal((await p.locator("#quotaHid .us-hidline").textContent()).trim(), w.n(1));

          // a whole card hidden too, then Show all has every card back
          await reader.inView(p, card(p, "claude"));
          await card(p, "claude").locator(".subscription-head").click({ button: "right" });
          assert.deepEqual(await items(p), [w.hide]);
          await p.locator(".row-menu .rm-item").click();
          await p.waitForFunction(() => !document.querySelector('#subscriptionUsage > [data-provider="claude"]'));
          await savedAs(["stepfun|balance", "claude|plan"]);
          await reader.click(p, p.locator("#quotaHid .us-hidline"));
          const all = p.locator("#quotaHid .us-showall");
          await reader.inView(p, all);
          await all.click();
          await p.waitForFunction(() => document.querySelector("#quotaHid").hidden);
          await savedAs([]);
          assert.deepEqual(await cards(p), ["claude", "stepfun", "stepfun-2"]);
          assert.match(await card(p, "stepfun").textContent(), /42\.00/);
          assert.deepEqual(errors, []);
          await ctx.close();
        });
      }
    }
  });
}
