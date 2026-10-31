// Run with Node's test runner and Playwright on the module path; see README.md.
// Settings → Menu bar windows (settings.TrayWindows, StrikingRaw on X): for
// each ticked subscription with two or more windows the menu bar can show,
// a pill picks which of them its card shows, by kind ("5h", "7d"), every
// window unless some are picked. The pick is saved by provider, kept when
// another setting is saved, and All windows takes it back. Hidden on
// Windows, whose tray shows the tooltip alone, and with no card of two
// windows. Every language; narrow too. No backend, the API is faked here.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");
const { openPart } = require("./reader.cjs");

const assets = path.resolve(__dirname, "../assets");

const quotas = [
  { provider: "claude", name: "Claude Code", icon: "claude-color", user: "a@b.c", plan: "Pro",
    windows: [{ name: "5 hours", used: 42, trayKind: "5h" }, { name: "7 days", used: 18, trayKind: "7d" }, { name: "Opus", used: 90 }] },
  { provider: "copilot", name: "Copilot", icon: "githubcopilot", windows: [{ name: "Premium", used: 63, trayKind: "month" }] },
];

function server(lang, posts, over) {
  let cur = {
    theme: "light", lang, tray: "panel", quotaLeft: false, currency: "usd", dock: false, dockWindow: false, proxy: "",
    trayUsage: "claude|*", trayUsages: ["claude|*", "copilot", "usage:today"], trayUsageEvery: 3,
    version: "0.1.400", dir: "~/.config/magpie", gateway: "http://127.0.0.1:3425", proxyNow: "none", proxySource: "none",
    fx: { rate: 7.2, at: new Date().toISOString(), stale: false }, ...over,
  };
  return async (route) => {
    const req = route.request(), url = new URL(req.url());
    const json = (data) => route.fulfill({ json: data });
    if (url.pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"${lang}",theme:"light",web:false};` });
    if (url.pathname === "/wails/runtime.js") return route.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json({ agents: [], profiles: [], settings: { lang, theme: "light" }, fx: cur.fx });
    if (url.pathname === "/api/settings") {
      if (req.method() === "POST") { posts.push(req.postDataJSON()); cur = { ...cur, ...req.postDataJSON() }; }
      return json(cur);
    }
    if (url.pathname === "/api/usage/quotas") return json(quotas);
    if (url.pathname === "/api/groups") return json({ groups: [], models: [] });
    if (url.pathname.startsWith("/api/")) return json({});
    const file = path.join(assets, url.pathname === "/" ? "index.html" : url.pathname);
    const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)];
    await route.fulfill({ body: await fs.readFile(file), contentType });
  };
}

async function open(browser, lang, posts, { platform = "MacIntel", width = 900, over = {} } = {}) {
  const context = await browser.newContext({ viewport: { width, height: 560 }, reducedMotion: "reduce" });
  await context.addInitScript((p) => Object.defineProperty(Navigator.prototype, "platform", { get: () => p }), platform);
  const page = await context.newPage();
  page.setDefaultTimeout(5000);
  page.setDefaultNavigationTimeout(60000);
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.route("**/*", server(lang, posts, over));
  await page.goto("http://magpie.test/?view=usage");
  await page.locator("#prefs").click();
  await openPart(page, "usage");
  await page.locator("#trayUsagePick button").waitFor();
  return { context, page, errors };
}

const LANGS = [
  ["en", "Menu bar windows", "All windows", "5 hours", "7 days"],
  ["zh", "菜单栏额度窗口", "全部窗口", "5 小时", "7 天"],
  ["zh-TW", "選單列額度視窗", "全部視窗", "5 小時", "7 天"],
  ["ja", "メニューバーの期間", "すべての期間", "5 時間", "7 日"],
  ["de", "Zeitfenster in der Menüleiste", "Alle Zeitfenster", "5 Stunden", "7 Tage"],
];

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  test(engine + ": which windows a menu bar card shows", async (t) => {
    assert(["chromium", "webkit"].includes(engine), "BROWSER must be chromium or webkit");
    const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
    t.after(() => browser.close());

    for (const [lang, name, all, five, seven] of LANGS) {
      await t.test(lang, async () => {
        const posts = [];
        const { context, page, errors } = await open(browser, lang, posts);
        const row = page.locator("#trayWindowsRow");
        await row.waitFor();
        assert.equal(await row.locator(".name").textContent(), name);
        // one pill: Claude's, the card with two windows the menu bar can
        // show (Opus counts one model's use); Copilot has one, totals none
        const pills = row.locator("#trayWindowsPick button");
        await page.waitForFunction(() => document.querySelectorAll("#trayWindowsPick button").length === 1);
        assert.equal((await pills.first().textContent()).trim(), "Claude Code · " + all);
        assert.equal(await pills.first().evaluate((b) => getComputedStyle(b).borderLeftWidth), "0px");

        // the app's menu: All windows, then each window by its name
        await pills.first().click();
        const menu = page.locator(".proto-menu");
        await menu.waitFor();
        assert.equal(await page.locator("select").count(), 0);
        const items = menu.locator(".pm-item");
        assert.deepEqual((await items.locator(".pm-name").allTextContents()).map((s) => s.trim()), [all, five, seven]);
        assert.deepEqual(await items.evaluateAll((bs) => bs.map((b) => b.getAttribute("aria-checked"))), ["true", "false", "false"]);
        await items.nth(1).click();
        assert.equal(posts.length, 0, "nothing is saved till the menu closes");
        await page.keyboard.press("Escape");
        await page.waitForFunction((s) => document.querySelector("#trayWindowsPick button")?.textContent.trim() === s, "Claude Code · " + five);
        assert.equal(posts.length, 1);
        assert.deepEqual(posts[0].trayWindows, { claude: ["5h"] });
        assert.deepEqual(posts[0].trayUsages, ["claude|*", "copilot", "usage:today"], "the cards are kept");

        // another setting saved keeps the pick
        await page.locator("#trayPaceSegs button").nth(1).click();
        for (let i = 0; i < 100 && posts.length < 2; i++) await page.waitForTimeout(20);
        assert.equal(posts.length, 2);
        assert.equal(posts[1].trayPace, true);
        assert.deepEqual(posts.at(-1).trayWindows, { claude: ["5h"] });

        // both ticked is every window: nothing kept for it
        await page.locator("#trayWindowsPick button").click();
        await menu.waitFor();
        await menu.locator(".pm-item").nth(2).click();
        await page.keyboard.press("Escape");
        await page.waitForFunction((s) => document.querySelector("#trayWindowsPick button")?.textContent.trim() === s, "Claude Code · " + all);
        assert.deepEqual(posts.at(-1).trayWindows, {});
        assert.deepEqual(errors, []);
        await context.close();
      });
    }

    await t.test("All windows takes a pick back", async () => {
      const posts = [];
      const { context, page } = await open(browser, "en", posts, { over: { trayWindows: { claude: ["7d"], codex: ["5h"] } } });
      const pill = page.locator("#trayWindowsPick button");
      await page.waitForFunction(() => document.querySelector("#trayWindowsPick button")?.textContent.trim() === "Claude Code · 7 days");
      await pill.click();
      await page.locator(".proto-menu .pm-item").first().click();
      await page.waitForFunction(() => document.querySelector("#trayWindowsPick button")?.textContent.trim() === "Claude Code · All windows");
      // another subscription's pick, not ticked now, is left as it was
      assert.deepEqual(posts.at(-1).trayWindows, { codex: ["5h"] });
      await context.close();
    });

    await t.test("hidden where it changes nothing", async () => {
      for (const opt of [{ platform: "Win32" }, { over: { trayUsages: ["copilot", "usage:today"], trayUsage: "copilot" } }, { over: { trayUsages: [], trayUsage: "" } }]) {
        const { context, page } = await open(browser, "en", [], opt);
        await page.waitForTimeout(300);
        assert.equal(await page.locator("#trayWindowsRow").isVisible(), false, JSON.stringify(opt));
        await context.close();
      }
    });

    await t.test("narrow", async () => {
      for (const [lang] of LANGS) {
        const { context, page } = await open(browser, lang, [], { width: 360, over: { trayWindows: { claude: ["5h", "7d"] } } });
        const row = page.locator("#trayWindowsRow");
        await row.waitFor();
        const pill = row.locator("#trayWindowsPick button");
        await pill.waitFor();
        const [r, p] = [await row.boundingBox(), await pill.boundingBox()];
        assert(p.x >= r.x && p.x + p.width <= r.x + r.width + 0.5, `${lang}: the pill stays in its row: ${JSON.stringify([r, p])}`);
        assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true, lang + ": no sideways scroll");
        await context.close();
      }
    });
  });
}
