// Run with Node's test runner and Playwright on the module path; see README.md.
// A plugin's usage read that failed says the plugin's own reason on the
// account's row, the Usage page's card and the tray's card, in every
// language, rather than only "Allowance unavailable" with the reason in a
// tooltip (aqiu817/magpie-sensenova#1; pluginError, plugin_usage.go).
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");

const assets = path.resolve(__dirname, "../assets");

function server(lang) {
  return async (route) => {
    const url = new URL(route.request().url());
    const json = (data) => route.fulfill({ json: data });
    if (url.pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = ${JSON.stringify({ lang, theme: "light", web: true })};` });
    if (url.pathname === "/wails/runtime.js") return route.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json({ agents: [], profiles: [], settings: { lang, theme: "light" } });
    if (url.pathname === "/api/providers") return json({ providers: [], presets: [], excluded: [], gateway: { running: true, window: true } });
    if (url.pathname === "/api/groups") return json({ groups: [] });
    if (url.pathname.startsWith("/api/")) return json({});
    const file = path.join(assets, url.pathname === "/" ? "index.html" : url.pathname);
    const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)];
    await route.fulfill({ body: await fs.readFile(file), contentType });
  };
}

// what the SenseNova plugin's usage hook returns when its console sign-in
// fails (bouncedToLogin, magpie-sensenova i18n.mjs at 1fcd5fc), as magpie
// passes it on: a reading of the plugin's, so pluginError is set
const why = "登录后又被踢回登录页，账号密码可能不对";
const quota = { provider: "sensenova", name: "SenseNova", plan: "Token Plan", user: "13800000000", windows: [], error: why, pluginError: true };

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  for (const lang of ["en", "zh", "zh-TW", "ja", "de"]) {
    test(`${engine}: a plugin's failed usage read says its reason on the card (${lang})`, async (t) => {
      const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
      t.after(() => browser.close());
      const page = await (await browser.newContext({ viewport: { width: 440, height: 760 } })).newPage();
      page.setDefaultTimeout(5000);
      const errors = [];
      page.on("pageerror", (e) => errors.push(e.message));
      await page.route("**/*", server(lang));
      await page.goto("http://magpie.test/?view=providers");
      await page.waitForFunction(() => typeof quotaError === "function");
      const generic = await page.evaluate(() => t("Allowance unavailable"));
      const want = await page.evaluate((w) => t("Allowance unavailable: {why}", { why: w }), why);
      assert.ok(want.includes(why), want);
      if (lang !== "en") assert.notEqual(want, `Allowance unavailable: ${why}`, `${lang} has its own words`);
      // the provider editor's account row, the Usage page's card and the
      // tray's card each say it, not only in their tooltip
      const said = await page.evaluate((q) => ({
        row: accountQuota({ [q.user]: q }, q.user).textContent,
        usage: quotaWindows(q).textContent,
        panel: panelQuotaCard(q).querySelector(".pq-sub.err").textContent,
      }), quota);
      for (const [where, text] of Object.entries(said)) assert.ok(text.includes(want), `${where}: ${text}`);
      // only the first line, and a long one cut short
      const long = await page.evaluate((w) => quotaError(w + "\nstack at …", { pluginError: true }), "x".repeat(400));
      assert.ok(!long.includes("stack at"), long);
      assert.ok(long.length < 220, long);
      // magpie's own errors keep their words: a plugin error it knows is
      // still said its way, and one that isn't the plugin's stays generic
      assert.equal(await page.evaluate(() => quotaError("your sign-in has expired", { pluginError: true })), await page.evaluate(() => t("Signed out — add this account again to use it")));
      assert.equal(await page.evaluate((w) => quotaError(w, { error: w }), why), generic);
      assert.equal(await page.evaluate((w) => quotaError(w), why), generic);
      assert.deepEqual(errors, []);
    });
  }
}
