// OpenCode in the import picker (@hu_xiaomi on X): its tab carries
// OpenCode's own mark, OpenCode Go's key is picked to come in on its
// preset, and a sign-in (Claude Pro/Max) is listed, said why, and can't be
// picked. All APIs and keys are fixtures.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");
const assets = process.env.ASSET_DIR || path.resolve(__dirname, "../assets");
const engines = process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"];
const langs = ["en", "zh", "zh-TW", "ja", "de"];

const sources = () => [{
  id: "opencode", name: "OpenCode", path: "/fixture/.local/share/opencode/auth.json", found: true,
  items: [
    { ref: "opencode-go", from: "auth.json", fingerprint: "go", status: "new",
      provider: { id: "opencode-go", name: "OpenCode Go", icon: "opencode", preset: "opencode-go", chat: "https://opencode.ai/zen/go/v1", key: "sk-g…7c" } },
    { ref: "anthropic", from: "auth.json", fingerprint: "ant", skip: "it is a sign-in, not a key; add the subscription in magpie",
      provider: { name: "anthropic" } },
  ],
}];

for (const engine of engines) {
  for (const lang of langs) {
    for (const width of [900, 390]) {
      test(`${engine} ${lang} ${width}px: OpenCode's key is picked, its sign-in is not`, async (t) => {
        const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
        const page = await browser.newPage({ viewport: { width, height: 900 }, reducedMotion: "reduce" });
        page.setDefaultTimeout(7000);
        const errors = [], posts = [];
        page.on("pageerror", (e) => errors.push(e.stack || e.message));
        await page.route("**/*", async (route) => {
          const req = route.request(), url = new URL(req.url());
          const json = (data) => route.fulfill({ json: data });
          if (url.pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs={lang:"${lang}",theme:"light",web:true};` });
          if (url.pathname === "/api/state") return json({ agents: [], profiles: [], settings: { lang, theme: "light" } });
          if (url.pathname === "/api/providers") return json({ providers: [], presets: [], excluded: [], gateway: { running: true, window: true } });
          if (url.pathname === "/api/groups") return json({ groups: [], pools: [] });
          if (url.pathname === "/api/importapps/discovery") return json([]);
          if (url.pathname === "/api/importapps") {
            if (req.method() === "GET") return json(sources());
            posts.push(req.postDataJSON());
            return json({ added: ["OpenCode Go"], state: { providers: [], presets: [], excluded: [], gateway: { running: true, window: true } } });
          }
          if (url.pathname === "/api/gateway/trace") return new Promise(() => {});
          if (url.pathname.startsWith("/api/")) return json({});
          const file = path.join(assets, url.pathname === "/" ? "index.html" : url.pathname);
          const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)];
          try { await route.fulfill({ body: await fs.readFile(file), contentType }); } catch { await route.fulfill({ status: 404, body: "" }); }
        });
        t.after(async () => {
          await browser.close();
          assert.deepEqual(errors, []);
        });
        await page.goto("http://magpie.test/?view=providers");
        await page.waitForFunction(() => typeof openImportApps === "function");
        await page.evaluate(() => openImportApps());
        const picker = page.locator("#modal .importapps");
        await picker.locator(".approw").first().waitFor();
        const T = (s, v) => page.evaluate(([s, v]) => t(s, v), [s, v]);

        // the tab's mark is OpenCode's, and it loads
        const logo = picker.locator(".apptab .applogo");
        assert.match(await logo.getAttribute("src"), /icons\/opencode\.svg$/);
        await page.waitForFunction(() => document.querySelector("#modal .apptab .applogo").complete);
        assert((await logo.evaluate((img) => img.naturalWidth)) > 0, "OpenCode's mark loads");

        const go = picker.locator(".approw", { hasText: "OpenCode Go" });
        const ant = picker.locator(".approw", { hasText: "anthropic" });
        assert.equal(await go.locator('input[type="checkbox"]').isChecked(), true);
        assert.equal(await ant.locator('input[type="checkbox"]').isDisabled(), true);
        assert.equal(await ant.locator(".sub", { hasText: await T("it is a sign-in, not a key; add the subscription in magpie") }).count(), 1);
        assert.equal(await ant.locator(".apptag").textContent(), await T("Can't import"));
        if (lang !== "en") assert.notEqual(await T("it is a sign-in, not a key; add the subscription in magpie"), "it is a sign-in, not a key; add the subscription in magpie");

        const fits = await picker.evaluate((ed) => [...ed.querySelectorAll(".approw")].every((r) => r.scrollWidth <= r.clientWidth + 1) && ed.getBoundingClientRect().right <= innerWidth);
        assert(fits, "rows fit the window");

        await picker.locator(".bar button.primary").click();
        await page.waitForFunction(() => !importingApps);
        assert.deepEqual(posts, [{ picks: [{ source: "opencode", ref: "opencode-go", mode: "add" }] }]);
      });
    }
  }
}
