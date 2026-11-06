// Run with Node's test runner and Playwright on the module path; see README.md.
// What was hidden on Usage → Overview has a way back that can be found
// (yuu003348 on Discord: 观测/用量/概览 从概览中隐藏 后 无法取消隐藏). The
// menu that hid a card, or an agent, offers "Show {n} Hidden" while some
// are, which opens the line under the list with the keyboard on it, each
// with Unhide; the line itself is legible, not the page's faintest grey;
// and a hidden card a menu bar cell showed (usagePeek) offers "Show in
// Overview" in its menu, not Hide again. Every language, both engines, a
// wide window, a narrow one and magpie web on a phone; no backend, the API
// is faked.
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
  { provider: "stepfun", name: "StepFun", icon: "stepfun-color", plan: "Step Plan", windows: [win("5h", 30)] },
  { provider: "stepfun-2", name: "StepFun Work", icon: "stepfun-color", windows: [], balance: "¥7.50" },
];
const G = (id, name, tokens, icon) => ({ id, name, icon: icon || "generic", calls: 10, input: tokens - 100, output: 100, cache_read: 0, cache_write: 0, cost: 0 });
const AGENTS = [G("pi", "Pi", 900000, "pi"), G("curl", "curl", 40000)];
const USAGE = {
  calls: 20, errors: 0, input: 939800, output: 200, reasoning: 0, cache_read: 0, cache_write: 0, unpriced: 0, cost: 0, bucket: "day",
  series: [], path: "~/.config/magpie/usage.jsonl", agents: AGENTS, models: [{ ...G("m", "glm-5", 940000), model: "glm-5" }],
};

function server(lang, web, saved) {
  return async (route) => {
    const req = route.request();
    const url = new URL(req.url());
    const json = (data) => route.fulfill({ json: data });
    const settings = () => ({ lang, theme: "light", usageCardsHidden: saved.cards, usageAgentsHidden: saved.agents });
    if (url.pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"${lang}",theme:"light",web:${web}};` });
    if (url.pathname === "/wails/runtime.js") return route.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json({ agents: [], clients: [{ id: "pi", name: "Pi", icon: "pi" }, { id: "curl", name: "curl", icon: "curl" }], profiles: [], settings: settings(), fx: { rate: 7.2, at: new Date().toISOString() } });
    if (url.pathname === "/api/usage/arrange") {
      const body = JSON.parse(req.postData() || "{}");
      if (body.cardsHidden) saved.cards = body.cardsHidden;
      if (body.agentsHidden) saved.agents = body.agentsHidden;
      return json(settings());
    }
    if (url.pathname === "/api/settings") return json(settings());
    if (url.pathname === "/api/usage/quotas") return json(QUOTAS);
    if (url.pathname === "/api/usage") return json(USAGE);
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
  en: { hide: "Hide from Overview", agentHide: "Hide from this list", more: (n) => `Show ${n} Hidden`, show: "Show in Overview", n: (n) => `${n} hidden`, unhide: "Unhide" },
  zh: { hide: "从概览中隐藏", agentHide: "从列表中隐藏", more: (n) => `显示已隐藏的 ${n} 个`, show: "在概览中显示", n: (n) => `已隐藏 ${n} 个`, unhide: "取消隐藏" },
  "zh-TW": { hide: "從概覽中隱藏", agentHide: "從列表中隱藏", more: (n) => `顯示已隱藏的 ${n} 個`, show: "在概覽中顯示", n: (n) => `已隱藏 ${n} 個`, unhide: "取消隱藏" },
  ja: { hide: "概要から隠す", agentHide: "この一覧から隠す", more: (n) => `非表示の ${n} 件を表示`, show: "概要に表示", n: (n) => `${n} 件非表示`, unhide: "再表示" },
  de: { hide: "In der Übersicht ausblenden", agentHide: "In dieser Liste ausblenden", more: (n) => `${n} ausgeblendete anzeigen`, show: "In der Übersicht einblenden", n: (n) => `${n} ausgeblendet`, unhide: "Einblenden" },
};
const SIZES = [{ width: 1200, web: false }, { width: 560, web: false }, { width: 390, web: true }];

// WCAG's contrast of two "rgb(r, g, b)" colours
function contrast(a, b) {
  const lum = (c) => {
    const [r, g, bl] = c.match(/[\d.]+/g).slice(0, 3).map((x) => { const v = x / 255; return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4; });
    return 0.2126 * r + 0.7152 * g + 0.0722 * bl;
  };
  const [x, y] = [lum(a), lum(b)].sort((m, n) => n - m);
  return (x + 0.05) / (y + 0.05);
}

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  test(engine + ": what is hidden from the Overview has a way back from the menu that hid it", async (t) => {
    assert(["chromium", "webkit"].includes(engine), "BROWSER must be chromium or webkit");
    const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
    t.after(() => browser.close());
    for (const lang of Object.keys(L)) {
      for (const { width, web } of SIZES) {
        const w = L[lang];
        await t.test(`${lang} ${width}${web ? " web" : ""}`, async () => {
          const errors = [];
          const saved = { cards: ["stepfun-2|balance"], agents: ["curl"] };
          const ctx = await browser.newContext({ viewport: { width, height: 700 }, reducedMotion: "reduce", hasTouch: web, isMobile: web && engine === "chromium" });
          const open = async (query = "") => {
            const p = await ctx.newPage();
            p.setDefaultTimeout(5000);
            p.setDefaultNavigationTimeout(60000);
            p.on("pageerror", (e) => errors.push(e.message));
            await p.route("**/*", server(lang, web, saved));
            await p.goto("http://magpie.test/?view=usage" + query);
            // the first load of the whole app, on a busy machine
            await p.locator("#subscriptionUsage .subscription-card").first().waitFor({ timeout: 20000 });
            await p.locator("#usageAgents .row.stat").first().waitFor({ timeout: 20000 });
            return p;
          };
          const cards = (p) => p.locator("#subscriptionUsage > .subscription-card").evaluateAll((cs) => cs.map((c) => c.dataset.provider));
          const card = (p, id) => p.locator(`#subscriptionUsage > .subscription-card[data-provider="${id}"]`);
          const items = (p) => p.locator(".row-menu .rm-item").evaluateAll((bs) => bs.map((b) => b.textContent.trim()));
          const longPress = (loc) => loc.evaluate((c) => new Promise((done) => {
            const box = c.querySelector(".subscription-head").getBoundingClientRect();
            const at = { pointerType: "touch", isPrimary: true, pointerId: 7, bubbles: true, clientX: box.left + box.width / 2, clientY: box.top + box.height / 2 };
            c.querySelector(".subscription-head b").dispatchEvent(new PointerEvent("pointerdown", at));
            setTimeout(() => { c.querySelector(".subscription-head b").dispatchEvent(new PointerEvent("pointerup", at)); done(); }, 700);
          }));
          const cardMenu = async (p, id) => {
            await reader.inView(p, card(p, id));
            if (web) await longPress(card(p, id));
            else await card(p, id).locator(".subscription-head").click({ button: "right" });
            await p.locator(".row-menu").waitFor();
          };
          const until = async (f, want) => {
            for (const end = Date.now() + 5000; !isDeepStrictEqual(f(), want) && Date.now() < end;) await new Promise((ok) => setTimeout(ok, 20));
            assert.deepEqual(f(), want);
          };
          const inSight = (p, sel) => p.locator(sel).evaluate((e) => {
            const r = e.getBoundingClientRect(), v = document.querySelector("#view-usage").getBoundingClientRect();
            return r.top >= v.top - 1 && r.bottom <= v.bottom + 1;
          });

          let p = await open();
          assert.deepEqual(await cards(p), ["claude", "stepfun"]);

          // the line is legible: not the page's faintest grey
          const line = p.locator("#quotaHid .us-hidline");
          assert.equal((await line.textContent()).trim(), w.n(1));
          for (const sel of ["#quotaHid .us-hidline", "#usageAgentsHid .us-hidline"]) {
            const [fg, bg] = await p.locator(sel).evaluate((e) => {
              let b = e, c = "rgba(0, 0, 0, 0)";
              while (b && /rgba\(0, 0, 0, 0\)|transparent/.test(c)) { c = getComputedStyle(b).backgroundColor; b = b.parentElement; }
              return [getComputedStyle(e).color, c];
            });
            assert(contrast(fg, bg) >= 3, `${sel} reads ${fg} on ${bg}: ${contrast(fg, bg).toFixed(2)}:1`);
          }

          // the menu that hides a card shows the hidden ones
          await cardMenu(p, "claude");
          assert.deepEqual(await items(p), [w.hide, w.more(1)]);
          await p.locator(".row-menu .rm-item", { hasText: w.more(1) }).click();
          await p.waitForFunction(() => document.querySelector("#quotaHid .us-hidline").getAttribute("aria-expanded") === "true");
          assert.equal(await p.evaluate(() => document.activeElement?.matches("#quotaHid .us-hidline")), true, "the keyboard is on the line");
          assert.deepEqual(await p.locator("#quotaHid .row.stat").evaluateAll((rs) => rs.map((r) => r.dataset.id)), ["stepfun-2|balance"]);
          assert.equal(await inSight(p, "#quotaHid"), true, "the opened list is in sight");
          await reader.click(p, p.locator("#quotaHid .us-unhide"));
          await until(() => saved.cards, []);
          await p.waitForFunction(() => !!document.querySelector('#subscriptionUsage > [data-provider="stepfun-2"]'));
          assert.equal(await p.locator("#quotaHid").isHidden(), true);
          // nothing hidden, nothing to show
          await cardMenu(p, "claude");
          assert.deepEqual(await items(p), [w.hide]);
          await p.keyboard.press("Escape");

          // the Agents list's menu, the same
          const more = p.locator('#usageAgents .row.stat[data-id="pi"] .us-more');
          await reader.click(p, more);
          await p.locator(".row-menu").waitFor();
          assert.deepEqual(await items(p), [w.agentHide, w.more(1)]);
          await p.locator(".row-menu .rm-item", { hasText: w.more(1) }).click();
          await p.waitForFunction(() => document.querySelector("#usageAgentsHid .us-hidline").getAttribute("aria-expanded") === "true");
          assert.equal(await p.evaluate(() => document.activeElement?.matches("#usageAgentsHid .us-hidline")), true);
          assert.equal(await inSight(p, "#usageAgentsHid"), true);
          await reader.click(p, p.locator("#usageAgentsHid .us-unhide"));
          await until(() => saved.agents, []);
          await p.close();

          // a hidden card a menu bar cell asked for: shown, and its menu has it back
          saved.cards = ["stepfun-2|balance"];
          p = await open("&tab=usage&card=stepfun-2");
          await card(p, "stepfun-2").waitFor();
          assert.equal(await p.locator("#quotaHid").isHidden(), true, "the one hidden is the one shown");
          await cardMenu(p, "stepfun-2");
          assert.deepEqual(await items(p), [w.show]);
          await p.locator(".row-menu .rm-item").click();
          await until(() => saved.cards, []);
          assert.deepEqual(await cards(p), ["claude", "stepfun", "stepfun-2"]);

          const wide = await p.evaluate(() => { const v = document.querySelector("#view-usage"); return [v.scrollWidth, v.clientWidth, document.documentElement.scrollWidth, window.innerWidth]; });
          assert(wide[0] <= wide[1] + 1 && wide[2] <= wide[3] + 1, "no sideways scroll: " + wide);
          assert.deepEqual(errors, []);
          await ctx.close();
        });
      }
    }
  });
}
