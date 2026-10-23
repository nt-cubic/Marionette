// Run with Node's test runner and Playwright on the module path; see README.md.
// Codex's settings, on its opened row: background compaction (#2490): Off /
// On, and with it on the share of the window Codex starts at, typed as a
// percent (empty: Codex's own 65%). Each posts settings/codex-async-compact
// on its own, a value that isn't 0 to 100 is refused before anything is
// sent, and nothing scrolls the page. Every language, Chromium and WebKit;
// no backend, the API is faked here.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");
const { watchSettingsReads, agentRow, openAgentSettings } = require("./reader.cjs");

const assets = path.resolve(__dirname, "../assets");

function serve(lang, posts) {
  let settings = { theme: "light", lang, tray: "panel", currency: "usd", version: "0.1.900", dir: "~/.config/magpie", gateway: "http://127.0.0.1:3425", proxyNow: "none", proxySource: "none", visionModels: [], imageGenModels: [], titleModels: [], workbuddyCheckins: [], lanURLs: [], redactWords: [], trayUsageEvery: 3 };
  const models = ["deepseek-v4-flash", "deepseek-v4-pro"].map((id) => ({ id, name: id, on: true, context: 1000000 }));
  const provider = { id: "deepseek", name: "DeepSeek", icon: "generic", chat: "https://api.deepseek.example/v1", responses: "", anthropic: "", models, agents: [], key: { set: true, masked: "sk-…1234" }, ready: true, compacts: { "*": 500000, "deepseek-v4-flash": 272000 } };
  const providers = { providers: [provider], presets: [], excluded: [], gateway: { running: true, window: true } };
  return async (route) => {
    const req = route.request(), url = new URL(req.url());
    const json = (data) => route.fulfill({ json: data });
    if (url.pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:${JSON.stringify(lang)},theme:"light",web:true};` });
    if (url.pathname === "/wails/runtime.js") return route.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json({ agents: [agentRow("codex", "Codex")], profiles: [], settings: { lang, theme: "light" } });
    if (url.pathname === "/api/providers") return json(providers);
    if (url.pathname === "/api/groups") return json({ groups: [], models: [] });
    if (url.pathname.startsWith("/api/provider/")) {
      posts.push({ path: url.pathname, body: req.postDataJSON() });
      return json(providers);
    }
    if (url.pathname === "/api/settings/codex-async-compact") {
      const body = req.postDataJSON();
      posts.push({ path: url.pathname, body });
      settings = { ...settings, codexAsyncCompact: body.on, ...(body.percent !== undefined ? { codexAsyncCompactPercent: body.percent } : {}) };
      return json(settings);
    }
    if (url.pathname === "/api/settings") {
      if (req.method() === "POST") posts.push({ path: url.pathname, body: req.postDataJSON() });
      return json(settings);
    }
    if (url.pathname === "/api/plugins") return json({ plugins: [] });
    if (url.pathname === "/api/usage/quotas") return json([]);
    if (url.pathname.startsWith("/api/")) return json({});
    const file = path.join(assets, url.pathname === "/" ? "index.html" : url.pathname);
    try {
      await route.fulfill({ body: await fs.readFile(file), contentType: { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)] });
    } catch {
      await route.fulfill({ status: 404, body: "" });
    }
  };
}

const words = {
  en: { name: "Codex background compaction", field: "Background compaction at", bad: "Background compaction at: 150 is not a percent from 1 to 100", off: "Off", on: "On" },
  zh: { name: "Codex 后台压缩", field: "后台压缩阈值", bad: "后台压缩阈值：150 不是 1 到 100 的百分比", off: "关闭", on: "开启" },
  "zh-TW": { name: "Codex 背景壓縮", field: "背景壓縮閾值", bad: "背景壓縮閾值：150 不是 1 到 100 的百分比", off: "關閉", on: "開啟" },
  ja: { name: "Codex のバックグラウンド圧縮", field: "バックグラウンド圧縮のしきい値", bad: "バックグラウンド圧縮のしきい値：150 は 1〜100 のパーセントではありません", off: "オフ", on: "オン" },
  de: { name: "Codex-Hintergrundkomprimierung", field: "Hintergrundkomprimierung ab", bad: "Hintergrundkomprimierung ab: 150 ist kein Prozentwert von 1 bis 100", off: "Aus", on: "An" },
};

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  for (const lang of Object.keys(words)) {
    const w = words[lang];
    test(`${engine} ${lang}: Codex background compaction is turned on with a threshold`, async (t) => {
      const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: process.env.PLAYWRIGHT_CHANNEL || "chromium" }));
      t.after(() => browser.close());
      const page = await (await browser.newContext({ viewport: { width: 900, height: 720 }, reducedMotion: "reduce" })).newPage();
      page.setDefaultTimeout(5000);
      const errors = [], posts = [];
      page.on("pageerror", (e) => errors.push(e.message));
      await page.route("**/*", serve(lang, posts));
      await watchSettingsReads(page);
      await page.goto("http://magpie.test/?view=agents");
      await openAgentSettings(page);
      const row = page.locator("#codexAsyncRow");
      await row.getByText(w.name, { exact: true }).waitFor();
      assert.equal(await row.locator("select").count(), 0, "no native select");
      const num = row.locator("input.compact-num");
      assert(await num.isHidden(), "no threshold while off");
      const view = () => page.locator("#view-agents").evaluate((v) => v.scrollTop);
      const before = await view();
      await row.getByText(w.on, { exact: true }).click();
      await page.waitForFunction(() => document.querySelector("#codexAsyncRow input.compact-num")?.hidden === false);
      assert.deepEqual(posts.map((p) => [p.path, p.body]), [["/api/settings/codex-async-compact", { on: true }]]);
      assert.equal(await num.getAttribute("aria-label"), w.field);
      assert.equal(await num.getAttribute("placeholder"), "65%");
      await num.fill("150");
      await num.press("Enter");
      await page.getByText(w.bad).waitFor();
      assert.equal(posts.length, 1, "a bad percent is not sent");
      await num.fill("70%");
      await num.press("Enter");
      await page.waitForFunction(() => document.querySelector("#codexAsyncRow input.compact-num")?.value === "70%");
      assert.deepEqual(posts.at(-1), { path: "/api/settings/codex-async-compact", body: { on: true, percent: 70 } });
      await row.getByText(w.off, { exact: true }).click();
      await page.waitForFunction(() => document.querySelector("#codexAsyncRow input.compact-num")?.hidden);
      assert.deepEqual(posts.at(-1).body, { on: false });
      assert.equal(await view(), before, "the row scrolled the page");
      assert(!posts.some((p) => p.path === "/api/settings"), "set on its own, not with the other preferences");
      assert.deepEqual(errors, []);
    });
  }
}
