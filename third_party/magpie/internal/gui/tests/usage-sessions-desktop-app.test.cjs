// Run with Node's test runner and Playwright on the module path; see README.md.
// Usage › Sessions: a session Claude Desktop's Code tab ran has its files in
// Claude Code's folder, so /api/sessions lists it under agent "claude", with
// the agent it ran in as "app" (CyberCaius on Discord: Desktop's sessions
// shown under Claude Code). The agent filter goes by the app: Claude
// Desktop lists it, Claude Code doesn't, and its row keeps Claude Code's
// key. English and Chinese, Chromium and WebKit.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");

const assets = path.resolve(__dirname, "../assets");
const iso = (d) => [d.getFullYear(), d.getMonth() + 1, d.getDate()].map((n) => String(n).padStart(2, "0")).join("-");
const today = iso(new Date());
const now = new Date().toISOString();
const tok = { input: 1000, output: 100, cache_read: 0, cache_write: 0 };
const sessions = [
  { agent: "claude", app: "claude-desktop", id: "s3p", name: "Claude Desktop", icon: "claude-color", title: "Desktop task", cwd: "/work/app", start: now, last: now, models: [{ model: "m1", ...tok }], ...tok },
  { agent: "claude", id: "sess1", name: "Claude Code", icon: "claude-color", title: "Terminal task", cwd: "/work/app", start: now, last: now, models: [{ model: "m2", ...tok }], ...tok },
];
const stats = {
  from: today, to: today, keys: ["claude:s3p", "claude:sess1"],
  agents: { "claude-desktop": "Claude Desktop", claude: "Claude Code" },
  days: [{
    date: today,
    usage: [
      { agent: "claude-desktop", cwd: "/work/app", model: "m1", ...tok, cost: 0.1, priced: true },
      { agent: "claude", cwd: "/work/app", model: "m2", ...tok, cost: 0.1, priced: true },
    ],
    active: [],
  }],
};

function serve(lang) {
  return async (route) => {
    const url = new URL(route.request().url());
    const json = (data) => route.fulfill({ json: data });
    if (url.pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"${lang}",theme:"light",web:false};` });
    if (url.pathname === "/wails/runtime.js") return route.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json({ agents: [], profiles: [], settings: { lang, theme: "light" } });
    if (url.pathname === "/api/sessions/progress") return json({ indexing: false });
    if (url.pathname === "/api/sessions") return json({ sessions, dirs: [] });
    if (url.pathname === "/api/sessions/stats") return json(stats);
    if (url.pathname === "/api/groups") return json({ groups: [] });
    if (url.pathname === "/api/usage/quotas") return json([]);
    if (url.pathname.startsWith("/api/")) return json({});
    const file = path.join(assets, url.pathname === "/" ? "index.html" : url.pathname);
    const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)];
    const body = await fs.readFile(file).catch(() => null);
    return body ? route.fulfill({ body, contentType }) : route.fulfill({ status: 404, body: "" });
  };
}

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  for (const lang of ["en", "zh"]) {
    test(`${engine} ${lang}: a Code tab session is listed under Claude Desktop`, async (t) => {
      const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
      t.after(() => browser.close());
      const page = await (await browser.newContext({ viewport: { width: 860, height: 700 }, reducedMotion: "reduce" })).newPage();
      page.setDefaultTimeout(8000);
      const errors = [];
      page.on("pageerror", (e) => errors.push(e.message));
      await page.route("**/*", serve(lang));
      await page.addInitScript(() => { localStorage.setItem("magpie.usageTab", "sessions"); localStorage.setItem("magpie.sessRange", "all"); localStorage.setItem("magpie.usageEvery", "0"); });
      await page.goto("http://magpie.test/?view=usage");
      const pick = page.locator("#sessAgent");
      await pick.waitFor({ state: "visible" });
      const listed = () => page.locator("#sessList .sess-item .who .name").allInnerTexts();
      await page.waitForFunction(() => document.querySelectorAll("#sessList .sess-item").length === 2);
      assert.deepEqual((await listed()).sort(), ["Desktop task", "Terminal task"]);

      const choose = async (name) => {
        await pick.click();
        const menu = page.locator(".pop.proto-menu.sess-menu");
        await menu.waitFor();
        await menu.locator(".pm-item", { hasText: name }).click();
        await menu.waitFor({ state: "detached" });
        await page.waitForFunction((n) => document.querySelector("#sessAgent span")?.textContent === n, name);
      };
      // the menu has each agent once: the session's app, not a second
      // Claude Code from its folder
      await pick.click();
      const names = await page.locator(".pop.proto-menu.sess-menu .pm-item .pm-name").allInnerTexts();
      assert.deepEqual(names.slice(1).sort(), ["Claude Code", "Claude Desktop"]);
      await page.keyboard.press("Escape");
      await page.locator(".pop.proto-menu.sess-menu").waitFor({ state: "detached" });

      await choose("Claude Desktop");
      await page.waitForFunction(() => document.querySelectorAll("#sessList .sess-item").length === 1);
      assert.deepEqual(await listed(), ["Desktop task"]);
      await choose("Claude Code");
      await page.waitForFunction(() => [...document.querySelectorAll("#sessList .sess-item .who .name")].map((e) => e.textContent).join() === "Terminal task");
      assert.deepEqual(errors, []);
    });
  }
}
